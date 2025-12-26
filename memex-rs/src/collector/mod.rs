//! 采集服务 - 使用 Adapter 架构扫描和收集多种 CLI 会话

use anyhow::Result;

use crate::adapter::AdapterRegistry;
use crate::config::Config;
use crate::db::Database;

/// 采集服务
#[derive(Clone)]
pub struct Collector {
    registry: AdapterRegistry,
    db: Database,
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
    pub fn new(config: Config, db: Database) -> Self {
        let registry = AdapterRegistry::from_config(&config);
        Self { registry, db }
    }

    /// 执行全量采集
    pub fn collect_all(&self) -> Result<CollectResult> {
        let mut result = CollectResult::default();

        // 遍历所有适配器
        for adapter in self.registry.adapters() {
            let source = adapter.source();
            tracing::info!("开始扫描: {:?}", source);

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

                // 插入消息
                match self.db.insert_messages_v2(&meta.id, &parse_result.messages) {
                    Ok((inserted, new_ids)) => {
                        if inserted > 0 {
                            result.sessions_scanned += 1;
                            result.messages_inserted += inserted;
                            result.new_message_ids.extend(new_ids);
                            tracing::debug!("会话 {} 插入 {} 条消息", meta.id, inserted);
                        }
                    }
                    Err(e) => {
                        result.errors.push(format!("插入消息失败: {}", e));
                    }
                }
            }

            result.projects_scanned += 1;
        }

        tracing::info!(
            "采集完成: {} 项目, {} 会话, {} 消息",
            result.projects_scanned,
            result.sessions_scanned,
            result.messages_inserted
        );

        Ok(result)
    }

    /// 增量采集
    pub fn collect_incremental(&self) -> Result<CollectResult> {
        // 目前实现与全量相同
        self.collect_all()
    }
}

/// 从路径提取项目名
fn extract_project_name(path: &str) -> &str {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
}
