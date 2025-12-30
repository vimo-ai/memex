//! 采集服务 - 使用 Adapter 架构扫描和收集多种 CLI 会话

#![allow(dead_code)] // 预留 API: collect_incremental

use anyhow::Result;
use std::sync::Arc;

use crate::adapter::AdapterRegistry;
use crate::config::Config;
use crate::db::Database;

use crate::shared_adapter::SharedDbAdapter;

/// 采集服务
#[derive(Clone)]
pub struct Collector {
    registry: AdapterRegistry,
    db: Database,
    shared_db: Option<Arc<SharedDbAdapter>>,
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
    pub fn new(config: Config, db: Database, shared_db: Option<Arc<SharedDbAdapter>>) -> Self {
        let registry = AdapterRegistry::from_config(&config);
        Self { registry, db, shared_db }
    }

    /// 执行全量采集
    pub fn collect_all(&self) -> Result<CollectResult> {
        let mut result = CollectResult::default();

        // 遍历所有适配器
        for adapter in self.registry.adapters() {
            let source = adapter.source();

            // 列出所有会话
            let sessions = match adapter.list_sessions() {
                Ok(s) => s,
                Err(e) => {
                    let err_msg = format!("{:?} 列出会话失败: {}", source, e);
                    tracing::warn!("{}", err_msg);
                    result.errors.push(err_msg);
                    continue;
                }
            };

            for meta in sessions {
                // 获取或创建项目
                let project_name = meta.project_name.as_deref()
                    .unwrap_or_else(|| extract_project_name(&meta.project_path));
                let source_str = source.to_string();
                let project_id = match self.db.get_or_create_project(project_name, &meta.project_path, &source_str) {
                    Ok(id) => id,
                    Err(e) => {
                        result.errors.push(format!("创建项目失败: {}", e));
                        continue;
                    }
                };

                // 检查会话是否已存在且消息数量相同
                let existing_count = if self.db.session_exists(&meta.id).unwrap_or(false) {
                    self.db.get_session_message_count(&meta.id).unwrap_or(0)
                } else {
                    0
                };

                // 解析会话
                let parse_result = match adapter.parse_session(&meta) {
                    Ok(Some(r)) => r,
                    Ok(None) => continue,
                    Err(e) => {
                        let err_msg = format!("解析会话 {} 失败: {}", meta.id, e);
                        tracing::debug!("{}", err_msg);
                        result.errors.push(err_msg);
                        continue;
                    }
                };

                // 如果消息数量相同，跳过
                if existing_count as usize == parse_result.messages.len() {
                    continue;
                }

                // 创建会话
                if let Err(e) = self.db.create_session_v2(
                    &meta.id,
                    project_id,
                    parse_result.cwd.as_deref(),
                    parse_result.model.as_deref(),
                    &source.to_string(),
                    meta.channel.as_deref(),
                ) {
                    result.errors.push(format!("创建会话失败: {}", e));
                    continue;
                }

                // 插入消息到 memex db
                match self.db.insert_messages_v2(&meta.id, &parse_result.messages) {
                    Ok((inserted, new_ids)) => {
                        if inserted > 0 {
                            result.sessions_scanned += 1;
                            result.messages_inserted += inserted;
                            result.new_message_ids.extend(new_ids);
                            tracing::debug!("会话 {} 插入 {} 条消息", meta.id, inserted);

                            // 同步写入共享数据库
                            self.sync_to_shared_db(
                                project_name,
                                &meta.project_path,
                                &source_str,
                                &meta.id,
                                &parse_result.messages,
                            );
                        }
                    }
                    Err(e) => {
                        result.errors.push(format!("插入消息失败: {}", e));
                    }
                }
            }

            result.projects_scanned += 1;
        }

        // 只在有新消息时打印
        if result.messages_inserted > 0 {
            tracing::info!(
                "📥 采集: {} 会话, {} 新消息",
                result.sessions_scanned,
                result.messages_inserted
            );
        }

        Ok(result)
    }

    /// 增量采集
    pub fn collect_incremental(&self) -> Result<CollectResult> {
        // 目前实现与全量相同
        self.collect_all()
    }

    /// 按路径采集单个会话（精确索引，替代 file watcher）
    /// 接受 JSONL 文件路径，解析并更新数据库
    pub fn collect_by_path(&self, path: &str) -> Result<CollectResult> {
        use ai_cli_session_collector::ClaudeAdapter;

        let mut result = CollectResult::default();

        // 直接调用 ai-cli-session-collector 的核心实现
        let session = match ClaudeAdapter::parse_session_from_path(path) {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(result),
            Err(e) => {
                result.errors.push(format!("解析会话失败: {}", e));
                return Ok(result);
            }
        };

        // 获取或创建项目（使用正确的 project_path，已从 cwd 读取）
        let project_id = self.db.get_or_create_project(
            &session.project_name,
            &session.project_path,
            "claude",
        )?;

        // 创建/更新会话
        self.db.create_session_v2(
            &session.session_id,
            project_id,
            Some(&session.project_path),  // cwd 就是 project_path
            None,  // model 在 IndexableSession 中不可用
            "claude",
            Some("code"),
        )?;

        // 插入消息（使用新的 insert_indexable_messages 方法）
        match self.db.insert_indexable_messages(&session.session_id, &session.messages) {
            Ok((inserted, new_ids)) => {
                result.sessions_scanned = 1;
                result.messages_inserted = inserted;
                result.new_message_ids = new_ids;
                tracing::info!("📥 精确索引: 会话 {} 插入 {} 条消息", session.session_id, inserted);

                // 同步写入共享数据库
                if inserted > 0 {
                    self.sync_indexable_to_shared_db(
                        &session.project_name,
                        &session.project_path,
                        &session.session_id,
                        &session.messages,
                    );
                }
            }
            Err(e) => {
                result.errors.push(format!("插入消息失败: {}", e));
            }
        }

        result.projects_scanned = 1;
        Ok(result)
    }

    /// 同步数据到共享数据库（仅在 Writer 模式下）
    fn sync_to_shared_db(
        &self,
        project_name: &str,
        project_path: &str,
        source: &str,
        session_id: &str,
        messages: &[crate::adapter::ParsedMessage],
    ) {
        use claude_session_db::db::MessageInput;
        use claude_session_db::MessageRole;

        let Some(shared_db) = &self.shared_db else {
            return;
        };

        // 预先转换消息格式（在闭包外）
        let shared_messages: Vec<MessageInput> = messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                // 解析时间戳：Option<String> -> i64
                let timestamp = msg.timestamp
                    .as_ref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });

                MessageInput {
                    uuid: msg.uuid.clone(),
                    role: match msg.message_type.to_string().as_str() {
                        "human" => MessageRole::Human,
                        "assistant" => MessageRole::Assistant,
                        _ => MessageRole::Human,
                    },
                    content: msg.content.clone(),
                    timestamp,
                    sequence: i as i64,
                }
            })
            .collect();

        // 使用 block_in_place 允许在 tokio runtime 内执行阻塞操作
        let shared_db = shared_db.clone();
        let project_name = project_name.to_string();
        let project_path = project_path.to_string();
        let source = source.to_string();
        let session_id = session_id.to_string();

        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();

            // 检查是否是 Writer
            let is_writer = rt.block_on(shared_db.is_writer());
            if !is_writer {
                tracing::debug!("[SharedDB] 非 Writer，跳过写入");
                return;
            }

            // 获取或创建项目
            let shared_project_id = match rt.block_on(shared_db.get_or_create_project(&project_name, &project_path, &source)) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("[SharedDB] 创建项目失败: {}", e);
                    return;
                }
            };

            // 创建会话
            if let Err(e) = rt.block_on(shared_db.upsert_session(&session_id, shared_project_id)) {
                tracing::warn!("[SharedDB] 创建会话失败: {}", e);
                return;
            }

            // 插入消息
            match rt.block_on(shared_db.insert_messages(&session_id, &shared_messages)) {
                Ok(inserted) => {
                    if inserted > 0 {
                        tracing::debug!("[SharedDB] 同步 {} 条消息到共享数据库", inserted);
                    }
                }
                Err(e) => {
                    tracing::warn!("[SharedDB] 插入消息失败: {}", e);
                }
            }
        });
    }

    /// 同步 IndexableMessage 到共享数据库
    fn sync_indexable_to_shared_db(
        &self,
        project_name: &str,
        project_path: &str,
        session_id: &str,
        messages: &[ai_cli_session_collector::IndexableMessage],
    ) {
        use claude_session_db::db::MessageInput;
        use claude_session_db::MessageRole;

        let Some(shared_db) = &self.shared_db else {
            return;
        };

        // 转换消息格式
        let shared_messages: Vec<MessageInput> = messages
            .iter()
            .map(|msg| {
                MessageInput {
                    uuid: msg.uuid.clone(),
                    role: if msg.role == "user" || msg.role == "human" {
                        MessageRole::Human
                    } else {
                        MessageRole::Assistant
                    },
                    content: msg.content.clone(),
                    timestamp: msg.timestamp,
                    sequence: msg.sequence,
                }
            })
            .collect();

        let shared_db = shared_db.clone();
        let project_name = project_name.to_string();
        let project_path = project_path.to_string();
        let session_id = session_id.to_string();

        tokio::task::block_in_place(move || {
            let rt = tokio::runtime::Handle::current();

            let is_writer = rt.block_on(shared_db.is_writer());
            if !is_writer {
                tracing::debug!("[SharedDB] 非 Writer，跳过写入");
                return;
            }

            let shared_project_id = match rt.block_on(shared_db.get_or_create_project(&project_name, &project_path, "claude")) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("[SharedDB] 创建项目失败: {}", e);
                    return;
                }
            };

            if let Err(e) = rt.block_on(shared_db.upsert_session(&session_id, shared_project_id)) {
                tracing::warn!("[SharedDB] 创建会话失败: {}", e);
                return;
            }

            match rt.block_on(shared_db.insert_messages(&session_id, &shared_messages)) {
                Ok(inserted) => {
                    if inserted > 0 {
                        tracing::debug!("[SharedDB] 同步 {} 条消息到共享数据库", inserted);
                    }
                }
                Err(e) => {
                    tracing::warn!("[SharedDB] 插入消息失败: {}", e);
                }
            }
        });
    }
}

/// 从路径提取项目名
fn extract_project_name(path: &str) -> &str {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
}
