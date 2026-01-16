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
    pub fn collect_by_path(&self, path: &str) -> Result<CollectResult> {
        use claude_session_db::{
            db::{MessageInput, SessionInput},
            ClaudeAdapter, MessageType,
        };

        const BUFFER_MS: i64 = 30 * 60 * 1000; // 30 分钟提前量

        let mut result = CollectResult::default();

        // 解析会话
        let session = match ClaudeAdapter::parse_session_from_path(path) {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(result),
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to parse session: {}", e));
                return Ok(result);
            }
        };

        let encoded_dir_name = extract_encoded_dir_name(path);

        // 获取数据库中该会话的最新消息时间戳
        let latest_ts = self
            .blocking_get_session_latest_timestamp(&session.session_id)
            .unwrap_or(None);
        let cutoff_ts = latest_ts.map(|ts| ts - BUFFER_MS).unwrap_or(0);

        // 获取或创建项目
        let project_id = match self.blocking_get_or_create_project(
            &session.project_name,
            &session.project_path,
            "claude",
            encoded_dir_name.as_deref(),
        ) {
            Ok(id) => id,
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to create project: {}", e));
                return Ok(result);
            }
        };

        // 创建/更新会话
        let session_input = SessionInput {
            session_id: session.session_id.clone(),
            project_id,
            cwd: Some(session.project_path.clone()),
            model: None,
            channel: Some("cli".to_string()),
            message_count: Some(session.messages.len() as i64),
            file_mtime: None,
            file_size: None,
            meta: None,
        };
        if let Err(e) = self.blocking_upsert_session(&session_input) {
            result
                .errors
                .push(format!("Failed to create session: {}", e));
            return Ok(result);
        }

        // 转换消息格式，过滤掉旧消息（时间戳增量采集）
        let messages: Vec<MessageInput> = session
            .messages
            .iter()
            .filter(|msg| msg.timestamp > cutoff_ts) // 只保留比 cutoff 更新的消息
            .map(|msg| MessageInput {
                uuid: msg.uuid.clone(),
                r#type: if msg.role == "user" || msg.role == "human" {
                    MessageType::User
                } else if msg.role == "tool" {
                    MessageType::Tool
                } else {
                    MessageType::Assistant
                },
                content_text: msg.content.text.clone(),
                content_full: msg.content.full.clone(),
                timestamp: msg.timestamp,
                sequence: msg.sequence,
                source: Some("claude".to_string()),
                channel: Some("cli".to_string()),
                model: None,
                tool_call_id: None,
                tool_name: None,
                tool_args: None,
                raw: msg.raw.clone(),
                approval_status: None,
                approval_resolved_at: None,
            })
            .collect();

        if messages.is_empty() {
            result.projects_scanned = 1;
            return Ok(result);
        }

        // 插入消息（ON CONFLICT DO NOTHING 保证不重复）
        match self.blocking_insert_messages(&session.session_id, &messages) {
            Ok(inserted) => {
                result.sessions_scanned = 1;
                result.messages_inserted = inserted;
                if inserted > 0 {
                    tracing::info!(
                        "📥 Incremental indexing: session {} inserted {} messages",
                        session.session_id,
                        inserted
                    );
                }
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to insert messages: {}", e));
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
