//! 采集服务 - 使用 Adapter 架构扫描和收集多种 CLI 会话
//!
//! 统一使用 SharedDbAdapter (claude-session-db) 作为数据层

use anyhow::Result;
use std::sync::Arc;

use crate::adapter::AdapterRegistry;
use crate::config::Config;
use crate::shared_adapter::SharedDbAdapter;

/// 采集服务
#[derive(Clone)]
pub struct Collector {
    registry: AdapterRegistry,
    db: Arc<SharedDbAdapter>,
}

/// 采集结果
#[derive(Debug, Default, Clone)]
pub struct CollectResult {
    pub projects_scanned: usize,
    pub sessions_scanned: usize,
    pub messages_inserted: usize,
    pub new_message_ids: Vec<i64>,
    pub errors: Vec<String>,
}

impl Collector {
    /// 创建采集服务
    pub fn new(_config: Config, db: Arc<SharedDbAdapter>) -> Self {
        // 使用适配器自注册机制
        let registry = AdapterRegistry::new();
        Self { registry, db }
    }

    /// 执行全量采集
    pub fn collect_all(&self) -> Result<CollectResult> {
        use claude_session_db::db::{MessageInput, SessionInput};

        const BUFFER_MS: i64 = 30 * 60 * 1000; // 30 分钟提前量

        let mut result = CollectResult::default();

        // 遍历所有适配器
        for adapter in self.registry.adapters() {
            let source = adapter.source();

            // 列出所有会话
            let sessions = match adapter.list_sessions() {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = format!("{:?} failed to list sessions: {}", source, e);
                    tracing::warn!("{}", err_msg);
                    result.errors.push(err_msg);
                    continue;
                }
            };

            for meta in sessions {
                // 获取或创建项目
                let project_name = meta
                    .project_name
                    .as_deref()
                    .unwrap_or_else(|| extract_project_name(&meta.project_path));
                let source_str = source.to_string();

                let project_id = match self.blocking_get_or_create_project(
                    project_name,
                    &meta.project_path,
                    &source_str,
                    meta.encoded_dir_name.as_deref(),
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        result
                            .errors
                            .push(format!("Failed to create project: {}", e));
                        continue;
                    }
                };

                // 获取数据库中该会话的最新消息时间戳（时间戳增量采集）
                let latest_ts = self
                    .blocking_get_session_latest_timestamp(&meta.id)
                    .unwrap_or(None);
                let cutoff_ts = latest_ts.map(|ts| ts - BUFFER_MS).unwrap_or(0);

                // 解析会话
                let parse_result = match adapter.parse_session(&meta) {
                    Ok(Some(r)) => r,
                    Ok(None) => continue,
                    Err(e) => {
                        let err_msg = format!("Failed to parse session {}: {}", meta.id, e);
                        tracing::debug!("{}", err_msg);
                        result.errors.push(err_msg);
                        continue;
                    }
                };

                // 创建会话
                let session_input = SessionInput {
                    session_id: meta.id.clone(),
                    project_id,
                    cwd: parse_result.cwd.clone(),
                    model: parse_result.model.clone(),
                    channel: meta.channel.clone(),
                    message_count: Some(parse_result.messages.len() as i64),
                    file_mtime: None,
                    file_size: None,
                    meta: None,
                };
                if let Err(e) = self.blocking_upsert_session(&session_input) {
                    result
                        .errors
                        .push(format!("Failed to create session: {}", e));
                    continue;
                }

                // 转换并插入消息（时间戳增量过滤）
                let messages: Vec<MessageInput> = parse_result
                    .messages
                    .iter()
                    .enumerate()
                    .filter_map(|(i, msg)| {
                        let timestamp = msg
                            .timestamp
                            .as_ref()
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or_else(|| {
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis() as i64)
                                    .unwrap_or(0)
                            });

                        // 只保留比 cutoff_ts 更新的消息
                        if timestamp <= cutoff_ts {
                            return None;
                        }

                        Some(MessageInput {
                            uuid: msg.uuid.clone(),
                            r#type: msg.message_type,
                            content_text: msg.content.text.clone(),
                            content_full: msg.content.full.clone(),
                            timestamp,
                            sequence: i as i64,
                            source: Some(msg.source.to_string()),
                            channel: msg.channel.clone(),
                            model: msg.model.clone(),
                            tool_call_id: msg.tool_call_id.clone(),
                            tool_name: msg.tool_name.clone(),
                            tool_args: msg.tool_args.clone(),
                            raw: msg.raw.clone(),
                            approval_status: None,
                            approval_resolved_at: None,
                        })
                    })
                    .collect();

                // 如果没有新消息，跳过
                if messages.is_empty() {
                    continue;
                }

                match self.blocking_insert_messages(&meta.id, &messages) {
                    Ok(inserted) => {
                        if inserted > 0 {
                            result.sessions_scanned += 1;
                            result.messages_inserted += inserted;
                            tracing::debug!("Session {} inserted {} messages", meta.id, inserted);
                        }
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("Failed to insert messages: {}", e));
                    }
                }
            }

            result.projects_scanned += 1;
        }

        // Only print when there are new messages
        if result.messages_inserted > 0 {
            tracing::info!(
                "📥 Collection: {} sessions, {} new messages",
                result.sessions_scanned,
                result.messages_inserted
            );
        }

        Ok(result)
    }

    /// 增量采集
    pub fn collect_incremental(&self) -> Result<CollectResult> {
        self.collect_all()
    }

    /// 按路径采集单个会话（精确索引，替代 file watcher）
    /// 使用时间戳增量采集：只采集比数据库中最新消息更新的消息（提前量 30 分钟）
    ///
    /// 支持多数据源：根据文件路径自动选择正确的 Adapter
    pub fn collect_by_path(&self, path: &str) -> Result<CollectResult> {
        use claude_session_db::db::{MessageInput, SessionInput};
        use std::path::Path;

        const BUFFER_MS: i64 = 30 * 60 * 1000; // 30 分钟提前量

        let mut result = CollectResult::default();
        let file_path = Path::new(path);

        // 根据路径找到对应的 adapter
        let adapter = match self.registry.adapters().iter().find(|a| a.should_handle(file_path)) {
            Some(a) => a.clone(),
            None => {
                tracing::debug!("No adapter found for path: {}", path);
                return Ok(result);
            }
        };

        let source = adapter.source();
        let source_str = source.to_string();

        // 列出会话元数据（找到匹配此路径的会话）
        let sessions = match adapter.list_sessions() {
            Ok(s) => s,
            Err(e) => {
                result.errors.push(format!("Failed to list sessions: {}", e));
                return Ok(result);
            }
        };

        // 找到对应的会话元数据
        let meta = match sessions.iter().find(|m| m.session_path.as_deref() == Some(path)) {
            Some(m) => m,
            None => {
                tracing::debug!("Session meta not found for path: {}", path);
                return Ok(result);
            }
        };

        // 解析会话
        let parse_result = match adapter.parse_session(meta) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(result),
            Err(e) => {
                result.errors.push(format!("Failed to parse session {}: {}", meta.id, e));
                return Ok(result);
            }
        };

        let encoded_dir_name = extract_encoded_dir_name(path);

        // 获取数据库中该会话的最新消息时间戳
        let latest_ts = self
            .blocking_get_session_latest_timestamp(&meta.id)
            .unwrap_or(None);
        let cutoff_ts = latest_ts.map(|ts| ts - BUFFER_MS).unwrap_or(0);

        // 获取或创建项目
        let project_name = meta
            .project_name
            .as_deref()
            .unwrap_or_else(|| extract_project_name(&meta.project_path));

        let project_id = match self.blocking_get_or_create_project(
            project_name,
            &meta.project_path,
            &source_str,
            encoded_dir_name.as_deref(),
        ) {
            Ok(id) => id,
            Err(e) => {
                result.errors.push(format!("Failed to create project: {}", e));
                return Ok(result);
            }
        };

        // 创建/更新会话
        let session_input = SessionInput {
            session_id: meta.id.clone(),
            project_id,
            cwd: parse_result.cwd.clone(),
            model: parse_result.model.clone(),
            channel: meta.channel.clone(),
            message_count: Some(parse_result.messages.len() as i64),
            file_mtime: None,
            file_size: None,
            meta: None,
        };
        if let Err(e) = self.blocking_upsert_session(&session_input) {
            result.errors.push(format!("Failed to create session: {}", e));
            return Ok(result);
        }

        // 转换消息格式，过滤掉旧消息（时间戳增量采集）
        let messages: Vec<MessageInput> = parse_result
            .messages
            .iter()
            .enumerate()
            .filter_map(|(i, msg)| {
                let timestamp = msg
                    .timestamp
                    .as_ref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });

                // 只保留比 cutoff_ts 更新的消息
                if timestamp <= cutoff_ts {
                    return None;
                }

                Some(MessageInput {
                    uuid: msg.uuid.clone(),
                    r#type: msg.message_type,
                    content_text: msg.content.text.clone(),
                    content_full: msg.content.full.clone(),
                    timestamp,
                    sequence: i as i64,
                    source: Some(msg.source.to_string()),
                    channel: msg.channel.clone(),
                    model: msg.model.clone(),
                    tool_call_id: msg.tool_call_id.clone(),
                    tool_name: msg.tool_name.clone(),
                    tool_args: msg.tool_args.clone(),
                    raw: msg.raw.clone(),
                    approval_status: None,
                    approval_resolved_at: None,
                })
            })
            .collect();

        if messages.is_empty() {
            result.projects_scanned = 1;
            return Ok(result);
        }

        // 插入消息（ON CONFLICT DO NOTHING 保证不重复）
        match self.blocking_insert_messages(&meta.id, &messages) {
            Ok(inserted) => {
                result.sessions_scanned = 1;
                result.messages_inserted = inserted;
                if inserted > 0 {
                    tracing::info!(
                        "📥 Incremental indexing [{}]: session {} inserted {} messages",
                        source_str,
                        meta.id,
                        inserted
                    );
                }
            }
            Err(e) => {
                result.errors.push(format!("Failed to insert messages: {}", e));
            }
        }

        result.projects_scanned = 1;
        Ok(result)
    }

    // ==================== 阻塞式 API 包装 ====================

    fn blocking_get_or_create_project(
        &self,
        name: &str,
        path: &str,
        source: &str,
        encoded_dir_name: Option<&str>,
    ) -> Result<i64> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(self.db.get_or_create_project_with_encoded(
                name,
                path,
                source,
                encoded_dir_name,
            ))
        })
    }

    fn blocking_get_session_latest_timestamp(&self, session_id: &str) -> Result<Option<i64>> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(self.db.get_session_latest_timestamp(session_id))
        })
    }

    fn blocking_upsert_session(&self, input: &claude_session_db::db::SessionInput) -> Result<()> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(self.db.upsert_session(input))
        })
    }

    fn blocking_insert_messages(
        &self,
        session_id: &str,
        messages: &[claude_session_db::db::MessageInput],
    ) -> Result<usize> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(self.db.insert_messages(session_id, messages))
        })
    }
}

/// 从 JSONL 文件路径提取 encoded_dir_name
fn extract_encoded_dir_name(path: &str) -> Option<String> {
    use std::path::Path;
    let path = Path::new(path);
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// 从路径提取项目名
fn extract_project_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 测试 AdapterRegistry 的 adapter 选择逻辑
    ///
    /// 验证 `should_handle` 方法能正确根据路径选择 adapter
    #[test]
    fn test_adapter_selection_by_path() {
        let registry = AdapterRegistry::new();
        let adapters = registry.adapters();

        // 找到 Claude adapter 并用它的实际 data_path 构造测试路径
        let claude_adapter = adapters.iter().find(|a| a.source().to_string() == "claude");
        assert!(claude_adapter.is_some(), "Claude adapter should exist");
        let claude_adapter = claude_adapter.unwrap();

        // 用 adapter 的 data_path 构造合法路径
        let claude_path = claude_adapter
            .data_path()
            .join("-Users-test-myproject")
            .join("session-123.jsonl");
        assert!(
            claude_adapter.should_handle(&claude_path),
            "Claude adapter should handle path under its data_path"
        );

        // 找到 OpenCode adapter（如果存在）
        if let Some(opencode_adapter) =
            adapters.iter().find(|a| a.source().to_string() == "opencode")
        {
            let opencode_path = opencode_adapter
                .data_path()
                .join("proj1")
                .join("ses_123.json");
            assert!(
                opencode_adapter.should_handle(&opencode_path),
                "OpenCode adapter should handle path under its data_path"
            );
        }

        // 未知路径不应该被任何 adapter 处理
        let unknown_path = Path::new("/some/random/path/file.txt");
        let unknown_adapter = adapters.iter().find(|a| a.should_handle(unknown_path));
        assert!(
            unknown_adapter.is_none(),
            "Unknown path should not be handled by any adapter"
        );
    }

    /// 测试 Claude 和 OpenCode 路径不会互相混淆
    #[test]
    fn test_adapter_no_cross_handling() {
        let registry = AdapterRegistry::new();
        let adapters = registry.adapters();

        // Claude adapter 不应该处理 OpenCode 路径
        let opencode_path = Path::new("/Users/test/.local/share/opencode/storage/session/proj1/ses_123.json");
        for adapter in adapters.iter() {
            if adapter.source().to_string() == "claude" {
                assert!(
                    !adapter.should_handle(opencode_path),
                    "Claude adapter should NOT handle OpenCode paths"
                );
            }
        }

        // OpenCode adapter 不应该处理 Claude 路径
        let claude_path = Path::new("/Users/test/.claude/projects/-Users-test-myproject/session-123.jsonl");
        for adapter in adapters.iter() {
            if adapter.source().to_string() == "opencode" {
                assert!(
                    !adapter.should_handle(claude_path),
                    "OpenCode adapter should NOT handle Claude paths"
                );
            }
        }
    }

    /// 测试 extract_encoded_dir_name 函数
    #[test]
    fn test_extract_encoded_dir_name() {
        // Claude 路径
        let path = "/Users/test/.claude/projects/-Users-test-myproject/session-123.jsonl";
        let encoded = extract_encoded_dir_name(path);
        assert_eq!(encoded, Some("-Users-test-myproject".to_string()));

        // OpenCode 路径
        let path = "/Users/test/.local/share/opencode/storage/session/proj1/ses_123.json";
        let encoded = extract_encoded_dir_name(path);
        assert_eq!(encoded, Some("proj1".to_string()));

        // 根目录文件
        let path = "/file.txt";
        let encoded = extract_encoded_dir_name(path);
        assert_eq!(encoded, None);
    }

    /// 测试 extract_project_name 函数
    #[test]
    fn test_extract_project_name() {
        assert_eq!(extract_project_name("/Users/test/myproject"), "myproject");
        assert_eq!(extract_project_name("/Users/test/my-project/"), "my-project");
        assert_eq!(extract_project_name("simple"), "simple");
        assert_eq!(extract_project_name(""), "");
    }
}
