//! JSONL 解析器 - 解析 Claude Code 会话文件

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    User,
    Assistant,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::User => write!(f, "user"),
            MessageType::Assistant => write!(f, "assistant"),
        }
    }
}

/// 解析后的消息
#[derive(Debug, Clone, Serialize)]
pub struct ParsedMessage {
    pub uuid: String,
    pub r#type: MessageType,
    pub content: String,
    pub timestamp: Option<String>,
}

/// 会话解析结果
#[derive(Debug)]
pub struct ParseSessionResult {
    pub messages: Vec<ParsedMessage>,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

/// JSONL 消息条目（原始格式）
#[derive(Debug, Deserialize)]
struct JsonlEntry {
    uuid: Option<String>,
    #[serde(rename = "type")]
    entry_type: Option<String>,
    message: Option<MessageContent>,
    timestamp: Option<String>,

    // 会话元数据
    cwd: Option<String>,
    model: Option<String>,

    // User 消息标记
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<serde_json::Value>,
    #[serde(rename = "isVisibleInTranscriptOnly")]
    is_visible_in_transcript_only: Option<bool>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
}

/// 消息内容
#[derive(Debug, Deserialize)]
struct MessageContent {
    id: Option<String>,
    role: Option<String>,
    content: Option<ContentValue>,
}

/// 内容值（字符串或内容块数组）
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ContentValue {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// 内容块
#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    text: Option<String>,
}

/// 解析 JSONL 会话文件
pub fn parse_session_file(path: &Path) -> Result<ParseSessionResult> {
    let file = File::open(path)
        .with_context(|| format!("无法打开文件: {:?}", path))?;
    let reader = BufReader::new(file);

    let mut messages = Vec::new();
    let mut cwd = None;
    let mut model = None;

    for (line_num, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("读取第 {} 行失败", line_num + 1))?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<JsonlEntry>(&line) {
            Ok(entry) => {
                // 提取会话元数据
                if cwd.is_none() && entry.cwd.is_some() {
                    cwd = entry.cwd.clone();
                }
                if model.is_none() && entry.model.is_some() {
                    model = entry.model.clone();
                }

                // 转换消息
                if let Some(msg) = convert_entry(&entry) {
                    messages.push(msg);
                }
            }
            Err(e) => {
                tracing::debug!("第 {} 行解析失败: {}", line_num + 1, e);
            }
        }
    }

    Ok(ParseSessionResult {
        messages,
        cwd,
        model,
    })
}

/// 转换单条消息
fn convert_entry(entry: &JsonlEntry) -> Option<ParsedMessage> {
    let entry_type = entry.entry_type.as_deref()?;

    // 跳过 summary 类型
    if entry_type == "summary" {
        return None;
    }

    // 确定消息类型
    let msg_type = get_message_type(entry)?;

    // 提取内容
    let content = extract_content(entry)?;
    if content.is_empty() {
        return None;
    }

    // 获取 UUID
    let uuid = entry.uuid.clone()
        .or_else(|| entry.message.as_ref()?.id.clone())?;

    Some(ParsedMessage {
        uuid,
        r#type: msg_type,
        content,
        timestamp: entry.timestamp.clone(),
    })
}

/// 获取消息类型
fn get_message_type(entry: &JsonlEntry) -> Option<MessageType> {
    let entry_type = entry.entry_type.as_deref()?;

    match entry_type {
        "user" => {
            // 过滤不需要显示的 user 消息
            if !should_display_user_message(entry) {
                return None;
            }
            Some(MessageType::User)
        }
        "assistant" => Some(MessageType::Assistant),
        "message" => {
            // 旧格式：通过 role 判断
            let role = entry.message.as_ref()?.role.as_deref()?;
            match role {
                "assistant" => Some(MessageType::Assistant),
                "user" => {
                    if !should_display_user_message(entry) {
                        return None;
                    }
                    Some(MessageType::User)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// 判断 User 消息是否应该显示
fn should_display_user_message(entry: &JsonlEntry) -> bool {
    // 工具执行结果 - 不显示
    if entry.tool_use_result.is_some() {
        return false;
    }

    // 检查 content 中是否包含 tool_result
    if has_tool_result_in_content(entry) {
        return false;
    }

    // 仅 Transcript 可见 - 不显示
    if entry.is_visible_in_transcript_only == Some(true) {
        return false;
    }

    // 压缩摘要 - 不显示
    if entry.is_compact_summary == Some(true) {
        return false;
    }

    // 元数据消息 - 不显示
    if entry.is_meta == Some(true) {
        return false;
    }

    true
}

/// 检查内容中是否包含 tool_result
fn has_tool_result_in_content(entry: &JsonlEntry) -> bool {
    if let Some(message) = &entry.message {
        if let Some(ContentValue::Blocks(blocks)) = &message.content {
            for block in blocks {
                if block.block_type.as_deref() == Some("tool_result") {
                    return true;
                }
            }
        }
    }
    false
}

/// 提取消息内容
fn extract_content(entry: &JsonlEntry) -> Option<String> {
    let message = entry.message.as_ref()?;
    let content = message.content.as_ref()?;

    match content {
        ContentValue::Text(text) => {
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        ContentValue::Blocks(blocks) => {
            let text_parts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| {
                    if b.block_type.as_deref() == Some("text") {
                        b.text.as_deref()
                    } else {
                        None
                    }
                })
                .collect();

            if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            }
        }
    }
}

/// 解码 Claude Code 目录名为真实路径
/// Claude Code 使用 `-` 替换 `/` 的编码方式
/// @example -Users-xxx-project → /Users/xxx/project
pub fn decode_project_path(encoded: &str) -> String {
    if encoded.starts_with('-') {
        format!("/{}", &encoded[1..].replace('-', "/"))
    } else {
        encoded.replace('-', "/")
    }
}

/// 从解码后的路径提取项目名
pub fn extract_project_name(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_project_path() {
        let encoded = "%2FUsers%2Ftest%2Fprojects%2Fmy-app";
        let decoded = decode_project_path(encoded);
        assert_eq!(decoded, "/Users/test/projects/my-app");
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(extract_project_name("/Users/test/my-app"), "my-app");
        assert_eq!(extract_project_name("/foo/bar/baz"), "baz");
    }
}
