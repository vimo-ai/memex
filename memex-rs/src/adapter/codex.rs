//! Codex CLI 数据适配器
//!
//! 解析 ~/.codex/history.jsonl (摘要) 和 ~/.codex/sessions/ (rollout 事件流)

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::{
    ConversationAdapter, MessageType, ParseResult, ParsedMessage, SessionMeta, Source,
};

/// Codex CLI 适配器
pub struct CodexAdapter {
    /// Codex 数据目录 (~/.codex)
    codex_path: PathBuf,
}

impl CodexAdapter {
    /// 创建适配器
    pub fn new(codex_path: PathBuf) -> Self {
        Self { codex_path }
    }

    /// history.jsonl 路径
    fn history_path(&self) -> PathBuf {
        self.codex_path.join("history.jsonl")
    }

    /// sessions 目录路径
    fn sessions_root(&self) -> PathBuf {
        self.codex_path.join("sessions")
    }

    /// 解析时间戳 (秒或毫秒) -> 本地时区 ISO 8601
    fn parse_timestamp(ts: Option<&serde_json::Value>) -> Option<String> {
        let ts = ts?;
        let num = if ts.is_number() {
            ts.as_f64()?
        } else if let Some(s) = ts.as_str() {
            s.parse::<f64>().ok()?
        } else {
            return None;
        };

        // Codex ts 可能是秒或毫秒
        let millis = if num > 1e12 { num as i64 } else { (num * 1000.0) as i64 };

        // 转换为本地时区 ISO 8601 格式
        use chrono::{Local, TimeZone};
        let datetime = Local.timestamp_millis_opt(millis).single()?;
        Some(datetime.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string())
    }

    /// 根据 ts 和 session_id 查找 rollout 文件
    fn resolve_session_path(&self, ts: Option<&str>, session_id: &str) -> Option<PathBuf> {
        let sessions_root = self.sessions_root();
        if !sessions_root.exists() {
            return None;
        }

        // 尝试从时间戳解析日期目录 (简化实现：直接递归搜索)
        let _ = ts; // 暂不使用时间戳优化查找

        // 递归搜索包含 session_id 的文件
        self.search_session_file(&sessions_root, session_id)
    }

    /// 递归搜索包含 session_id 的文件
    fn search_session_file(&self, dir: &Path, session_id: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(dir).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;

            if name.contains(session_id) {
                return Some(path);
            }

            // 目录则递归
            if path.is_dir() {
                if let Some(found) = self.search_session_file(&path, session_id) {
                    return Some(found);
                }
            }
        }

        None
    }

    /// 从 rollout 文件第一行提取 cwd
    fn extract_project_path(&self, session_path: &Path) -> Option<String> {
        let file = File::open(session_path).ok()?;
        let reader = BufReader::new(file);

        for line in reader.lines().take(5).flatten() {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&line) {
                let event_type = raw.get("type").or_else(|| {
                    raw.get("payload").and_then(|p| p.get("type"))
                });

                if event_type.and_then(|t| t.as_str()) == Some("session_meta") {
                    let payload = raw.get("payload").unwrap_or(&raw);
                    if let Some(cwd) = payload.get("cwd").and_then(|c| c.as_str()) {
                        return Some(cwd.to_string());
                    }
                }
            }
        }

        None
    }

    /// 解析 rollout 文件
    fn parse_rollout_file(&self, meta: &SessionMeta) -> Result<ParseResult> {
        let session_path = meta.session_path.as_ref()
            .context("缺少 session_path")?;

        let file = File::open(session_path)
            .with_context(|| format!("无法打开文件: {}", session_path))?;
        let reader = BufReader::new(file);

        let mut messages = Vec::new();
        let mut cwd = meta.cwd.clone();
        let mut model = meta.model.clone();
        let mut created_at = meta.created_at.clone();
        let mut updated_at = meta.updated_at.clone();
        let meta_bag = serde_json::Map::new();

        // 首条用户消息来自 history 摘要
        if let Some(history_meta) = &meta.meta {
            if let Some(text) = history_meta.get("historyText").and_then(|t| t.as_str()) {
                messages.push(ParsedMessage {
                    uuid: format!("{}:user:0", meta.id),
                    session_id: meta.id.clone(),
                    message_type: MessageType::User,
                    content: text.to_string(),
                    timestamp: created_at.clone(),
                    source: Source::Codex,
                    channel: Some("cli".to_string()),
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                });
            }
        }

        for line in reader.lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }

            let raw: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event = raw.get("payload").unwrap_or(&raw);
            let event_type = raw.get("type")
                .or_else(|| event.get("type"))
                .and_then(|t| t.as_str());

            match event_type {
                Some("session_meta") => {
                    let session_meta = event.get("session_meta").unwrap_or(event);
                    if cwd.is_none() {
                        cwd = session_meta.get("cwd").and_then(|c| c.as_str()).map(String::from);
                    }
                    if model.is_none() {
                        model = session_meta.get("model").and_then(|m| m.as_str()).map(String::from);
                    }
                    if created_at.is_none() {
                        created_at = Self::parse_timestamp(session_meta.get("ts"));
                    }
                }
                // event_msg 包含真正的用户消息和助手回复
                Some("event_msg") => {
                    let msg_type = event.get("type").and_then(|t| t.as_str());

                    match msg_type {
                        Some("user_message") => {
                            // 真正的用户消息
                            if let Some(text) = event.get("message").and_then(|m| m.as_str()) {
                                if !text.is_empty() {
                                    let ts = Self::parse_timestamp(raw.get("timestamp"));
                                    messages.push(ParsedMessage {
                                        uuid: Uuid::new_v4().to_string(),
                                        session_id: meta.id.clone(),
                                        message_type: MessageType::User,
                                        content: text.to_string(),
                                        timestamp: ts.clone(),
                                        source: Source::Codex,
                                        channel: Some("cli".to_string()),
                                        model: None,
                                        tool_call_id: None,
                                        tool_name: None,
                                        tool_args: None,
                                        raw: None,
                                    });
                                    if created_at.is_none() {
                                        created_at = ts;
                                    }
                                }
                            }
                        }
                        Some("agent_message") => {
                            // 真正的助手回复
                            if let Some(text) = event.get("message").and_then(|m| m.as_str()) {
                                if !text.is_empty() {
                                    let ts = Self::parse_timestamp(raw.get("timestamp"));
                                    updated_at = ts.clone().or(updated_at);
                                    messages.push(ParsedMessage {
                                        uuid: Uuid::new_v4().to_string(),
                                        session_id: meta.id.clone(),
                                        message_type: MessageType::Assistant,
                                        content: text.to_string(),
                                        timestamp: ts,
                                        source: Source::Codex,
                                        channel: Some("cli".to_string()),
                                        model: model.clone(),
                                        tool_call_id: None,
                                        tool_name: None,
                                        tool_args: None,
                                        raw: None,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // response_item 只处理 function_call (跳过 message 和 reasoning，它们是重复/中间状态)
                Some("response_item") => {
                    let item = event.get("response_item").unwrap_or(event);
                    let item_type = item.get("type")
                        .or_else(|| item.get("kind"))
                        .and_then(|t| t.as_str());

                    if matches!(item_type, Some("function_call") | Some("custom_tool_call")) {
                        let tool = item.get("tool_call")
                            .or_else(|| item.get("function_call"))
                            .or_else(|| item.get("custom_tool_call"))
                            .unwrap_or(item);

                        let tool_name = tool.get("name")
                            .or_else(|| tool.get("function_name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown");

                        let uuid = tool.get("id")
                            .and_then(|i| i.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| Uuid::new_v4().to_string());

                        let ts = Self::parse_timestamp(raw.get("timestamp"));
                        updated_at = ts.clone().or(updated_at);

                        messages.push(ParsedMessage {
                            uuid: uuid.clone(),
                            session_id: meta.id.clone(),
                            message_type: MessageType::Tool,
                            content: format!("Tool call: {}", tool_name),
                            timestamp: ts,
                            source: Source::Codex,
                            channel: Some("cli".to_string()),
                            model: model.clone(),
                            tool_call_id: Some(uuid),
                            tool_name: Some(tool_name.to_string()),
                            tool_args: tool.get("arguments").map(|a| a.to_string()),
                            raw: Some(tool.to_string()),
                        });
                    }
                    // 跳过 message 和 reasoning (它们是中间状态或与 event_msg 重复)
                }
                Some("tool_output") | Some("tool_call_output") => {
                    let tool_output = event.get("tool_output")
                        .or_else(|| event.get("tool_call_output"))
                        .or_else(|| event.get("output"))
                        .unwrap_or(event);

                    let content = if tool_output.is_string() {
                        tool_output.as_str().unwrap_or("").to_string()
                    } else {
                        tool_output.get("text")
                            .and_then(|t| t.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| tool_output.to_string())
                    };

                    let uuid = tool_output.get("id")
                        .and_then(|i| i.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| Uuid::new_v4().to_string());

                    let ts = Self::parse_timestamp(tool_output.get("ts"));
                    updated_at = ts.clone().or(updated_at);

                    messages.push(ParsedMessage {
                        uuid,
                        session_id: meta.id.clone(),
                        message_type: MessageType::Tool,
                        content,
                        timestamp: ts,
                        source: Source::Codex,
                        channel: Some("cli".to_string()),
                        model: None,
                        tool_call_id: tool_output.get("call_id")
                            .or_else(|| tool_output.get("tool_call_id"))
                            .and_then(|i| i.as_str())
                            .map(String::from),
                        tool_name: None,
                        tool_args: None,
                        raw: if tool_output.is_string() { None } else { Some(tool_output.to_string()) },
                    });
                }
                _ => {}
            }
        }

        Ok(ParseResult {
            messages,
            created_at,
            updated_at,
            cwd,
            model,
            meta: Some(serde_json::Value::Object(meta_bag)),
        })
    }
}

impl ConversationAdapter for CodexAdapter {
    fn source(&self) -> Source {
        Source::Codex
    }

    fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let mut results = Vec::new();

        let history_path = self.history_path();
        if !history_path.exists() {
            tracing::debug!("Codex history 文件不存在: {:?}", history_path);
            return Ok(results);
        }

        let file = File::open(&history_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }

            let entry: HistoryEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.session_id.is_empty() {
                continue;
            }

            // 解析时间戳
            let ts_str = entry.ts.map(|t| t.to_string());
            let created_at = Self::parse_timestamp(Some(&serde_json::Value::from(entry.ts.unwrap_or(0.0))));

            // 查找 rollout 文件
            let session_path = self.resolve_session_path(ts_str.as_deref(), &entry.session_id);

            // 获取文件元数据
            let (file_mtime, file_size) = session_path.as_ref()
                .and_then(|p| fs::metadata(p).ok())
                .map(|meta| {
                    let mtime = meta.modified().ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64);
                    (mtime, Some(meta.len()))
                })
                .unwrap_or((None, None));

            // 从 rollout 文件提取 cwd
            let project_path = session_path.as_ref()
                .and_then(|p| self.extract_project_path(p))
                .unwrap_or_else(|| self.codex_path.to_string_lossy().to_string());

            let mut meta_map = serde_json::Map::new();
            if let Some(text) = &entry.text {
                meta_map.insert("historyText".to_string(), serde_json::Value::String(text.clone()));
            }
            if let Some(ts) = entry.ts {
                meta_map.insert("historyTs".to_string(), serde_json::Value::from(ts));
            }

            results.push(SessionMeta {
                id: entry.session_id,
                source: Source::Codex,
                channel: Some("cli".to_string()),
                project_path,
                project_name: None,
                encoded_dir_name: None,
                session_path: session_path.map(|p| p.to_string_lossy().to_string()),
                file_mtime,
                file_size,
                cwd: None,
                model: None,
                meta: Some(serde_json::Value::Object(meta_map)),
                created_at: created_at.clone(),
                updated_at: created_at,
            });
        }

        Ok(results)
    }

    fn parse_session(&self, meta: &SessionMeta) -> Result<Option<ParseResult>> {
        if meta.session_path.is_none() {
            tracing::debug!("Codex 会话缺少 session_path: {}", meta.id);
            return Ok(None);
        }

        let result = self.parse_rollout_file(meta)?;
        Ok(Some(result))
    }
}

// ==================== History JSONL 数据结构 ====================

#[derive(Debug, Deserialize)]
struct HistoryEntry {
    session_id: String,
    ts: Option<f64>,
    text: Option<String>,
}
