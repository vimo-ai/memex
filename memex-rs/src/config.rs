//! 配置模块

use std::path::PathBuf;

/// 应用配置
#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP 服务端口
    pub port: u16,
    /// 数据目录
    pub data_dir: PathBuf,
    /// Claude Code 项目目录
    pub claude_projects_path: PathBuf,
    /// Codex CLI 数据目录
    pub codex_path: PathBuf,
    /// Ollama API 地址
    pub ollama_api: String,
    /// Embedding 模型 (用于语义搜索，核心功能)
    pub embedding_model: String,
    /// Chat 模型 (用于 AI 问答，可选功能)
    pub chat_model: String,
    /// 是否启用 AI 问答功能 (可选，需要 Chat 模型)
    pub enable_ai_chat: bool,
}

impl Config {
    /// 从环境变量加载配置
    pub fn from_env() -> Self {
        let home = dirs::home_dir().unwrap_or_default();

        Self {
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(10013),

            data_dir: std::env::var("MEMEX_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("memex-data")),

            claude_projects_path: std::env::var("CLAUDE_PROJECTS_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".claude/projects")),

            codex_path: std::env::var("CODEX_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".codex")),

            ollama_api: std::env::var("OLLAMA_API")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),

            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "bge-m3".to_string()),

            chat_model: std::env::var("CHAT_MODEL")
                .unwrap_or_else(|_| "qwen3:8b".to_string()),

            // AI 问答功能默认关闭，需要显式启用
            enable_ai_chat: std::env::var("ENABLE_AI_CHAT")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        }
    }

    /// 数据库路径
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("memex.db")
    }

    /// LanceDB 路径
    pub fn lancedb_path(&self) -> PathBuf {
        self.data_dir.join("lancedb")
    }

    /// 备份目录
    pub fn backup_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_env()
    }
}
