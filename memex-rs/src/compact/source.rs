//! Compact 数据结构
//!
//! 消息、工具调用、对话轮次等核心类型

use ai_cli_session_db::{Message as DbMessage, MessageType};
use serde::{Deserialize, Serialize};

/// 消息角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
    /// 未知角色（避免数据误分类）
    Unknown,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
            Self::Unknown => "unknown",
        }
    }
}

impl From<MessageType> for MessageRole {
    fn from(mt: MessageType) -> Self {
        match mt {
            MessageType::User => Self::User,
            MessageType::Assistant => Self::Assistant,
            MessageType::Tool => Self::Tool,
        }
    }
}

/// 统一的消息结构（用于 compact 内部处理）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息 ID（数据库主键）
    pub id: i64,
    /// 消息 UUID
    pub uuid: String,
    /// 消息角色
    pub role: MessageRole,
    /// 消息内容（纯文本）
    pub content: String,
    /// 时间戳（毫秒）
    pub timestamp: i64,
    /// 序号
    pub sequence: i64,
    /// 所属的 prompt_number（从 user message 顺序推断）
    pub prompt_number: i32,
    /// 工具名称（如果是工具调用）
    pub tool_name: Option<String>,
    /// 工具参数
    pub tool_args: Option<String>,
}

impl Message {
    /// 从数据库 Message 转换
    pub fn from_db(msg: &DbMessage, prompt_number: i32) -> Self {
        Self {
            id: msg.id,
            uuid: msg.uuid.clone(),
            role: msg.r#type.into(),
            content: msg.content_text.clone(),
            timestamp: msg.timestamp,
            sequence: msg.sequence,
            prompt_number,
            tool_name: msg.tool_name.clone(),
            tool_args: msg.tool_args.clone(),
        }
    }
}

/// 工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 工具调用 ID
    pub id: String,
    /// 工具名称
    pub name: String,
    /// 工具参数（JSON 字符串）
    pub arguments: Option<String>,
    /// 工具输出
    pub output: Option<String>,
    /// 时间戳（毫秒）
    pub timestamp: i64,
    /// 消息序号
    pub sequence: i64,
    /// 所属的 prompt_number
    pub prompt_number: i32,
}

impl ToolCall {
    /// 是否为空输出（用于 L1 剪枝）
    pub fn is_empty_output(&self) -> bool {
        self.output
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
    }

    /// 从参数中提取所有文件路径（支持单文件和合并后的多文件）
    pub fn extract_file_paths(&self) -> Vec<String> {
        let Some(args) = self.arguments.as_ref() else {
            return vec![];
        };

        let Ok(json) = serde_json::from_str::<serde_json::Value>(args) else {
            return vec![];
        };

        // 1. 先检查 files 数组（合并后的 ToolCall）
        if let Some(files) = json.get("files").and_then(|v| v.as_array()) {
            let paths: Vec<String> = files
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !paths.is_empty() {
                return paths;
            }
        }

        // 2. 单文件字段
        for field in ["file_path", "path", "file", "filename"] {
            if let Some(path) = json.get(field).and_then(|v| v.as_str()) {
                return vec![path.to_string()];
            }
        }

        vec![]
    }

    /// 从参数中提取文件路径（单个，向后兼容）
    pub fn extract_file_path(&self) -> Option<String> {
        self.extract_file_paths().into_iter().next()
    }

    /// 判断工具类型
    pub fn tool_category(&self) -> ToolCategory {
        match self.name.as_str() {
            "Read" | "read_file" => ToolCategory::Read,
            "Write" | "write_file" | "write_to_file" => ToolCategory::Write,
            "Edit" | "edit_file" | "str_replace_editor" => ToolCategory::Edit,
            "Bash" | "execute_bash" | "run_terminal_cmd" => ToolCategory::Bash,
            "Glob" | "list_files" | "find_files" => ToolCategory::Glob,
            "Grep" | "search_files" | "codebase_search" => ToolCategory::Grep,
            "TodoWrite" | "todo" => ToolCategory::Todo,
            "Task" | "dispatch_agent" => ToolCategory::Task,
            _ => ToolCategory::Other,
        }
    }
}

/// 工具分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    Read,
    Write,
    Edit,
    Bash,
    Glob,
    Grep,
    Todo,
    Task,
    Other,
}

impl ToolCategory {
    /// 是否为读取操作
    pub fn is_read(&self) -> bool {
        matches!(self, Self::Read | Self::Glob | Self::Grep)
    }

    /// 是否为写入操作
    pub fn is_write(&self) -> bool {
        matches!(self, Self::Write | Self::Edit)
    }
}

/// 一轮对话（user prompt + assistant response）
#[derive(Debug, Clone)]
pub struct Talk {
    /// 第几轮对话（从 1 开始）
    pub prompt_number: i32,
    /// 用户消息
    pub user_message: Message,
    /// Assistant 回复的消息列表
    pub assistant_messages: Vec<Message>,
    /// 本轮的工具调用
    pub tool_calls: Vec<ToolCall>,
}

impl Talk {
    /// 获取用户请求的内容
    pub fn user_request(&self) -> &str {
        &self.user_message.content
    }

    /// 获取 assistant 回复的纯文本（合并所有消息）
    pub fn assistant_response(&self) -> String {
        self.assistant_messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// 获取本轮涉及的文件
    pub fn files_involved(&self) -> Vec<String> {
        let mut files = Vec::new();
        for tc in &self.tool_calls {
            if let Some(path) = tc.extract_file_path() {
                if !files.contains(&path) {
                    files.push(path);
                }
            }
        }
        files
    }

    /// 是否包含代码修改
    pub fn has_code_changes(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|tc| tc.tool_category().is_write())
    }
}

/// 解析后的会话数据
#[derive(Debug, Clone)]
pub struct ParsedSession {
    /// 会话 ID
    pub session_id: String,
    /// 所有消息
    pub messages: Vec<Message>,
    /// 所有工具调用
    pub tool_calls: Vec<ToolCall>,
    /// 按轮次组织的对话
    pub talks: Vec<Talk>,
    /// 最大 prompt_number
    pub max_prompt_number: i32,
}

/// 从数据库消息列表构建 ParsedSession
///
/// # Arguments
/// * `session_id` - 会话 ID
/// * `db_messages` - 数据库消息列表（必须按 sequence 排序）
/// * `from_prompt` - 起始 prompt_number（增量处理时使用，传入上次处理的最大 prompt_number）
pub fn build_parsed_session(
    session_id: &str,
    db_messages: Vec<DbMessage>,
    from_prompt: i32,
) -> ParsedSession {
    use std::collections::HashMap;

    let mut messages = Vec::new();
    let mut current_prompt = from_prompt;

    // 临时存储：tool_call_id -> (ToolCall, index)
    // 用于关联工具调用请求和输出
    let mut tool_call_map: HashMap<String, ToolCall> = HashMap::new();

    // 第一遍：转换消息，计算 prompt_number，收集工具调用
    for msg in &db_messages {
        let role: MessageRole = msg.r#type.into();
        if role == MessageRole::User {
            current_prompt += 1;
        }

        let message = Message::from_db(msg, current_prompt);

        // 处理工具调用
        if let Some(ref tool_call_id) = msg.tool_call_id {
            if let Some(ref tool_name) = msg.tool_name {
                // 这是工具调用请求（assistant 消息或 tool 消息带 tool_name）
                if !tool_call_map.contains_key(tool_call_id) {
                    tool_call_map.insert(
                        tool_call_id.clone(),
                        ToolCall {
                            id: tool_call_id.clone(),
                            name: tool_name.clone(),
                            arguments: msg.tool_args.clone(),
                            output: None,
                            timestamp: msg.timestamp,
                            sequence: msg.sequence,
                            prompt_number: current_prompt,
                        },
                    );
                }
                // 如果是 Tool 类型且有 tool_name，内容可能是输出
                if msg.r#type == MessageType::Tool && !msg.content_text.is_empty() {
                    if let Some(tc) = tool_call_map.get_mut(tool_call_id) {
                        tc.output = Some(msg.content_text.clone());
                    }
                }
            } else {
                // 这是工具输出（tool 消息，没有 tool_name，但有 tool_call_id）
                // 关联到对应的工具调用
                if let Some(tc) = tool_call_map.get_mut(tool_call_id) {
                    if tc.output.is_none()
                        || tc.output.as_ref().map(|s| s.is_empty()).unwrap_or(true)
                    {
                        tc.output = Some(msg.content_text.clone());
                    }
                }
            }
        } else if let Some(ref tool_name) = msg.tool_name {
            // 没有 tool_call_id 但有 tool_name（旧格式或内部工具）
            let id = msg.uuid.clone();
            tool_call_map.insert(
                id.clone(),
                ToolCall {
                    id,
                    name: tool_name.clone(),
                    arguments: msg.tool_args.clone(),
                    output: if msg.r#type == MessageType::Tool {
                        Some(msg.content_text.clone())
                    } else {
                        None
                    },
                    timestamp: msg.timestamp,
                    sequence: msg.sequence,
                    prompt_number: current_prompt,
                },
            );
        }

        messages.push(message);
    }

    // 收集工具调用并按 sequence 排序
    let mut tool_calls: Vec<ToolCall> = tool_call_map.into_values().collect();
    tool_calls.sort_by_key(|tc| tc.sequence);

    let max_prompt_number = current_prompt;

    // 组织成 talks
    let talks = organize_talks(&messages, &tool_calls);

    ParsedSession {
        session_id: session_id.to_string(),
        messages,
        tool_calls,
        talks,
        max_prompt_number,
    }
}

/// 将消息和工具调用组织成对话轮次
fn organize_talks(messages: &[Message], tool_calls: &[ToolCall]) -> Vec<Talk> {
    let mut talks = Vec::new();
    let mut current_talk: Option<Talk> = None;

    for msg in messages {
        match msg.role {
            MessageRole::User => {
                // 保存上一轮对话
                if let Some(talk) = current_talk.take() {
                    talks.push(talk);
                }

                // 开始新一轮对话
                let prompt_number = msg.prompt_number;
                let talk_tool_calls: Vec<ToolCall> = tool_calls
                    .iter()
                    .filter(|tc| tc.prompt_number == prompt_number)
                    .cloned()
                    .collect();

                current_talk = Some(Talk {
                    prompt_number,
                    user_message: msg.clone(),
                    assistant_messages: Vec::new(),
                    tool_calls: talk_tool_calls,
                });
            }
            MessageRole::Assistant | MessageRole::Tool => {
                // 添加到当前对话
                if let Some(ref mut talk) = current_talk {
                    if msg.prompt_number == talk.prompt_number {
                        talk.assistant_messages.push(msg.clone());
                    }
                }
            }
            MessageRole::System | MessageRole::Unknown => {
                // 系统消息和未知角色消息不属于任何对话轮次
            }
        }
    }

    // 保存最后一轮对话
    if let Some(talk) = current_talk {
        talks.push(talk);
    }

    talks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_category() {
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Read".to_string(),
            arguments: None,
            output: None,
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert_eq!(tc.tool_category(), ToolCategory::Read);
        assert!(tc.tool_category().is_read());
        assert!(!tc.tool_category().is_write());
    }

    #[test]
    fn test_extract_file_path() {
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Read".to_string(),
            arguments: Some(r#"{"file_path": "/src/main.rs"}"#.to_string()),
            output: None,
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert_eq!(tc.extract_file_path(), Some("/src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_file_paths_single() {
        // 单文件场景
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Read".to_string(),
            arguments: Some(r#"{"file_path": "/src/main.rs"}"#.to_string()),
            output: None,
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert_eq!(tc.extract_file_paths(), vec!["/src/main.rs".to_string()]);
    }

    #[test]
    fn test_extract_file_paths_merged() {
        // 合并后的多文件场景（修复 Codex review 指出的 bug）
        let tc = ToolCall {
            id: "merged_1_3".to_string(),
            name: "Read(x3)".to_string(),
            arguments: Some(
                r#"{"merged_count": 3, "category": "Read", "files": ["/src/a.rs", "/src/b.rs", "/src/c.rs"]}"#
                    .to_string(),
            ),
            output: Some("Merged 3 Read operations".to_string()),
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert_eq!(
            tc.extract_file_paths(),
            vec![
                "/src/a.rs".to_string(),
                "/src/b.rs".to_string(),
                "/src/c.rs".to_string()
            ]
        );
        // extract_file_path 应该返回第一个
        assert_eq!(tc.extract_file_path(), Some("/src/a.rs".to_string()));
    }

    #[test]
    fn test_extract_file_paths_empty() {
        // 无参数场景
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Bash".to_string(),
            arguments: Some(r#"{"command": "ls -la"}"#.to_string()),
            output: None,
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert!(tc.extract_file_paths().is_empty());
        assert_eq!(tc.extract_file_path(), None);
    }

    #[test]
    fn test_organize_talks() {
        let messages = vec![
            Message {
                id: 1,
                uuid: "1".to_string(),
                role: MessageRole::User,
                content: "Hello".to_string(),
                timestamp: 1000,
                sequence: 0,
                prompt_number: 1,
                tool_name: None,
                tool_args: None,
            },
            Message {
                id: 2,
                uuid: "2".to_string(),
                role: MessageRole::Assistant,
                content: "Hi there".to_string(),
                timestamp: 1001,
                sequence: 1,
                prompt_number: 1,
                tool_name: None,
                tool_args: None,
            },
            Message {
                id: 3,
                uuid: "3".to_string(),
                role: MessageRole::User,
                content: "Help me".to_string(),
                timestamp: 2000,
                sequence: 2,
                prompt_number: 2,
                tool_name: None,
                tool_args: None,
            },
            Message {
                id: 4,
                uuid: "4".to_string(),
                role: MessageRole::Assistant,
                content: "Sure".to_string(),
                timestamp: 2001,
                sequence: 3,
                prompt_number: 2,
                tool_name: None,
                tool_args: None,
            },
        ];

        let talks = organize_talks(&messages, &[]);

        assert_eq!(talks.len(), 2);
        assert_eq!(talks[0].prompt_number, 1);
        assert_eq!(talks[0].user_message.content, "Hello");
        assert_eq!(talks[0].assistant_messages.len(), 1);
        assert_eq!(talks[1].prompt_number, 2);
        assert_eq!(talks[1].user_message.content, "Help me");
    }
}
