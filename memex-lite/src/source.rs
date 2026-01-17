//! 数据源发现模块
//!
//! 复用 ai-cli-session-collector 的适配器，提供统一的数据源访问接口

use ai_cli_session_collector::{
    all_adapters, all_watch_configs, ConversationAdapter, ParseResult, SessionMeta, Source,
    WatchConfig,
};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// 数据源信息（用于显示）
#[derive(Debug, Clone)]
pub struct DataSourceInfo {
    /// CLI 类型
    pub source: Source,
    /// 名称
    pub name: String,
    /// 数据路径
    pub path: PathBuf,
    /// 是否存在
    pub exists: bool,
    /// 会话数量（如果存在）
    pub session_count: Option<usize>,
}

/// 数据源管理器
pub struct DataSourceManager {
    adapters: Vec<Arc<dyn ConversationAdapter>>,
}

impl DataSourceManager {
    /// 创建数据源管理器
    pub fn new() -> Self {
        Self {
            adapters: all_adapters(),
        }
    }

    /// 获取所有数据源信息
    pub fn list_sources(&self) -> Vec<DataSourceInfo> {
        self.adapters
            .iter()
            .map(|adapter| {
                let path = adapter.data_path().to_path_buf();
                let exists = path.exists();
                let session_count = if exists {
                    adapter.list_sessions().ok().map(|s| s.len())
                } else {
                    None
                };

                DataSourceInfo {
                    source: adapter.source(),
                    name: adapter.meta().name.to_string(),
                    path,
                    exists,
                    session_count,
                }
            })
            .collect()
    }

    /// 获取可用的数据源（路径存在的）
    #[allow(dead_code)]
    pub fn available_sources(&self) -> Vec<DataSourceInfo> {
        self.list_sources()
            .into_iter()
            .filter(|s| s.exists)
            .collect()
    }

    /// 获取所有监听配置
    #[allow(dead_code)]
    pub fn watch_configs(&self) -> Vec<WatchConfig> {
        all_watch_configs()
    }

    /// 列出所有会话（跨所有数据源）
    pub fn list_all_sessions(&self) -> Result<Vec<SessionMeta>> {
        let mut all_sessions = Vec::new();

        for adapter in &self.adapters {
            if adapter.data_path().exists()
                && let Ok(sessions) = adapter.list_sessions() {
                    all_sessions.extend(sessions);
                }
        }

        // 按文件修改时间排序（最新的在前）
        all_sessions.sort_by(|a, b| {
            b.file_mtime
                .unwrap_or(0)
                .cmp(&a.file_mtime.unwrap_or(0))
        });

        Ok(all_sessions)
    }

    /// 按数据源过滤会话
    #[allow(dead_code)]
    pub fn list_sessions_by_source(&self, source: Source) -> Result<Vec<SessionMeta>> {
        for adapter in &self.adapters {
            if adapter.source() == source && adapter.data_path().exists() {
                return adapter.list_sessions();
            }
        }
        Ok(Vec::new())
    }

    /// 解析会话
    pub fn parse_session(&self, meta: &SessionMeta) -> Result<Option<ParseResult>> {
        for adapter in &self.adapters {
            if adapter.source() == meta.source {
                return adapter.parse_session(meta);
            }
        }
        Ok(None)
    }

    /// 根据会话 ID 查找会话（支持前缀匹配，返回所有匹配结果）
    pub fn find_sessions(&self, session_id: &str) -> Vec<SessionMeta> {
        self.list_all_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.id == session_id || s.id.starts_with(session_id))
            .collect()
    }
}

impl Default for DataSourceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 解析 Source 字符串
pub fn parse_source(s: &str) -> Option<Source> {
    match s.to_lowercase().as_str() {
        "claude" | "claude-code" => Some(Source::Claude),
        "codex" | "codex-cli" => Some(Source::Codex),
        "opencode" => Some(Source::OpenCode),
        "gemini" | "gemini-cli" => Some(Source::Gemini),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source_claude() {
        assert_eq!(parse_source("claude"), Some(Source::Claude));
        assert_eq!(parse_source("Claude"), Some(Source::Claude));
        assert_eq!(parse_source("CLAUDE"), Some(Source::Claude));
        assert_eq!(parse_source("claude-code"), Some(Source::Claude));
    }

    #[test]
    fn test_parse_source_codex() {
        assert_eq!(parse_source("codex"), Some(Source::Codex));
        assert_eq!(parse_source("codex-cli"), Some(Source::Codex));
    }

    #[test]
    fn test_parse_source_opencode() {
        assert_eq!(parse_source("opencode"), Some(Source::OpenCode));
        assert_eq!(parse_source("OpenCode"), Some(Source::OpenCode));
    }

    #[test]
    fn test_parse_source_gemini() {
        assert_eq!(parse_source("gemini"), Some(Source::Gemini));
        assert_eq!(parse_source("gemini-cli"), Some(Source::Gemini));
    }

    #[test]
    fn test_parse_source_invalid() {
        assert_eq!(parse_source("unknown"), None);
        assert_eq!(parse_source(""), None);
        assert_eq!(parse_source("gpt"), None);
    }
}
