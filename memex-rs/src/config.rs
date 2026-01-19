//! 配置模块
//!
//! 支持从配置文件 `~/.vimo/memex/config.json` 加载配置
//! 优先级：环境变量 > 配置文件 > 默认值

use serde::Deserialize;
use std::path::PathBuf;

use crate::compact::CompactConfig;

/// 配置文件结构（用于反序列化 JSON）
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    compact: CompactConfig,
    server: ServerConfig,
    ollama: OllamaConfig,
    features: FeaturesConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ServerConfig {
    port: Option<u16>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: None }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OllamaConfig {
    api: Option<String>,
    embedding_model: Option<String>,
    chat_model: Option<String>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            api: None,
            embedding_model: None,
            chat_model: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct FeaturesConfig {
    enable_ai_chat: Option<bool>,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            enable_ai_chat: None,
        }
    }
}

/// 应用配置
#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP 服务端口
    pub port: u16,
    /// 数据目录 (数据库、LanceDB、备份等)
    pub data_dir: PathBuf,
    /// Web 静态文件目录 (前端 dist)
    pub web_dir: PathBuf,
    /// Claude Code 项目目录 (用于 Archive 服务)
    pub claude_projects_path: PathBuf,
    /// Ollama API 地址
    pub ollama_api: String,
    /// Embedding 模型 (用于语义搜索，核心功能)
    pub embedding_model: String,
    /// Chat 模型 (用于 AI 问答，可选功能)
    pub chat_model: String,
    /// 是否启用 AI 问答功能 (可选，需要 Chat 模型)
    pub enable_ai_chat: bool,
    /// Compact 配置
    pub compact: CompactConfig,
}

impl Config {
    /// 配置文件路径
    fn config_file_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        let vimo_root = std::env::var("VIMO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".vimo"));
        vimo_root.join("memex/config.json")
    }

    /// 从配置文件加载
    fn load_from_file() -> FileConfig {
        let path = Self::config_file_path();
        if !path.exists() {
            return FileConfig::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(config) => {
                    tracing::info!("📄 Loaded config from {:?}", path);
                    config
                }
                Err(e) => {
                    tracing::warn!("⚠️ Failed to parse config file {:?}: {}", path, e);
                    FileConfig::default()
                }
            },
            Err(e) => {
                tracing::warn!("⚠️ Failed to read config file {:?}: {}", path, e);
                FileConfig::default()
            }
        }
    }

    /// 加载配置（优先级：环境变量 > 配置文件 > 默认值）
    pub fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let vimo_root = std::env::var("VIMO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".vimo"));

        // 先加载配置文件
        let file_config = Self::load_from_file();

        Self {
            // 环境变量 > 配置文件 > 默认值
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .or(file_config.server.port)
                .unwrap_or(10013),

            // 数据目录：~/.vimo/db (vimo 多项目共享)
            data_dir: std::env::var("MEMEX_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| vimo_root.join("db")),

            // Web 静态文件目录：~/.vimo/memex/web (memex 独立)
            web_dir: std::env::var("MEMEX_WEB_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| vimo_root.join("memex/web")),

            claude_projects_path: std::env::var("CLAUDE_PROJECTS_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".claude/projects")),

            ollama_api: std::env::var("OLLAMA_API")
                .ok()
                .or(file_config.ollama.api)
                .unwrap_or_else(|| "http://localhost:11434".to_string()),

            embedding_model: std::env::var("EMBEDDING_MODEL")
                .ok()
                .or(file_config.ollama.embedding_model)
                .unwrap_or_else(|| "bge-m3".to_string()),

            chat_model: std::env::var("CHAT_MODEL")
                .ok()
                .or(file_config.ollama.chat_model)
                .unwrap_or_else(|| "qwen3:0.6b".to_string()),

            // AI 问答功能默认关闭，需要显式启用
            enable_ai_chat: std::env::var("ENABLE_AI_CHAT")
                .ok()
                .map(|v| v == "true" || v == "1")
                .or(file_config.features.enable_ai_chat)
                .unwrap_or(false),

            // Compact 配置（环境变量可覆盖 enabled）
            compact: {
                let mut compact = file_config.compact;
                if let Ok(v) = std::env::var("COMPACT_ENABLED") {
                    compact.enabled = v == "true" || v == "1";
                }
                compact
            },
        }
    }

    /// 从环境变量加载配置（兼容旧代码）
    #[deprecated(note = "Use Config::load() instead")]
    pub fn from_env() -> Self {
        Self::load()
    }

    /// 数据库路径 (使用共享数据库，不受 MEMEX_DATA_DIR 影响)
    pub fn db_path(&self) -> PathBuf {
        // 统一使用共享数据库路径 ~/.vimo/db/ai-cli-session.db
        let home = dirs::home_dir().unwrap_or_default();
        let vimo_root = std::env::var("VIMO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".vimo"));
        vimo_root.join("db/ai-cli-session.db")
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
        Self::load()
    }
}
