//! Compact Prompt 模板
//!
//! L1/L2/L3 各层的 LLM prompt 设计

use super::source::{Talk, ToolCall};

/// L1: Observation 生成 Prompt
pub struct L1Prompt;

impl L1Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"You are a code operation analyst. Analyze tool calls and generate structured observations.

Output format (JSON):
{
  "type": "bugfix|feature|refactor|change|discovery|decision",
  "title": "Short title (< 20 words)",
  "subtitle": "One-line explanation (optional)",
  "facts": ["Specific fact 1", "Specific fact 2"],
  "narrative": "Full context description (optional, for complex operations)"
}

Type definitions:
- bugfix: Bug fix
- feature: New feature
- refactor: Code refactoring
- change: General modification
- discovery: Code discovery/understanding
- decision: Technical decision

Requirements:
- Keep it concise, capture the core
- facts should contain searchable keywords
- narrative can be omitted for simple operations
- Output language: Match the language of the input content"#;

    /// 构建用户消息
    pub fn build_user_message(tool_call: &ToolCall, context: Option<&str>) -> String {
        let mut prompt = String::new();

        prompt.push_str(&format!("Tool: {}\n", tool_call.name));

        if let Some(args) = &tool_call.arguments {
            // 截断过长的参数
            let args_display = if args.len() > 1000 {
                format!("{}...(truncated)", &args[..1000])
            } else {
                args.clone()
            };
            prompt.push_str(&format!("Arguments: {}\n", args_display));
        }

        if let Some(output) = &tool_call.output {
            // 截断过长的输出
            let output_display = if output.len() > 2000 {
                format!("{}...(truncated)", &output[..2000])
            } else {
                output.clone()
            };
            prompt.push_str(&format!("Output: {}\n", output_display));
        }

        if let Some(ctx) = context {
            prompt.push_str(&format!("\nContext: {}\n", ctx));
        }

        prompt
    }
}

/// L2: Talk Summary 生成 Prompt
pub struct L2Prompt;

impl L2Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"You are a conversation summarizer. Compress one round of AI coding conversation into a concise summary.

Output format (JSON):
{
  "user_request": "Brief description of user request (< 50 words)",
  "summary": "What was accomplished in this round (< 200 words)",
  "completed": "Specific completed items (optional)",
  "files_involved": ["File paths involved"]
}

Requirements:
- Preserve key decisions and solutions
- Remove duplicates and redundancy
- Maintain technical accuracy
- Extract files_involved from tool calls
- Output language: Match the language of the input content"#;

    /// 构建用户消息
    pub fn build_user_message(talk: &Talk) -> String {
        let mut prompt = String::new();

        // 用户请求
        prompt.push_str(&format!(
            "[User]: {}\n\n",
            truncate(&talk.user_message.content, 2000)
        ));

        // Assistant 回复
        for msg in &talk.assistant_messages {
            prompt.push_str(&format!(
                "[Assistant]: {}\n\n",
                truncate(&msg.content, 2000)
            ));
        }

        // 工具调用摘要
        if !talk.tool_calls.is_empty() {
            prompt.push_str("Tool calls:\n");
            for tc in &talk.tool_calls {
                prompt.push_str(&format!("- {}", tc.name));
                if let Some(path) = tc.extract_file_path() {
                    prompt.push_str(&format!(" ({})", path));
                }
                prompt.push('\n');
            }
        }

        prompt
    }
}

/// L3: Session Summary 生成 Prompt
pub struct L3Prompt;

impl L3Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"You are a session summarizer. Consolidate multiple conversation round summaries into a complete session summary.

Output format (JSON):
{
  "summary": "One-sentence summary of the entire session (< 100 words)",
  "key_points": ["Key point 1", "Key point 2", ...],
  "files_involved": ["Main files involved"],
  "technologies": ["Technologies/tools involved"]
}

Requirements:
- summary should quickly convey what this session accomplished
- key_points: 3-7 items covering main work
- Remove duplicates, merge related content
- Maintain technical accuracy
- Output language: Match the language of the input content"#;

    /// 从 L2 Talk Summaries 构建用户消息
    pub fn build_user_message(
        talk_summaries: &[(i32, String, Option<String>)], // (prompt_number, summary, completed)
        project_path: Option<&str>,
    ) -> String {
        let mut prompt = String::new();

        if let Some(path) = project_path {
            prompt.push_str(&format!("Project: {}\n\n", path));
        }

        for (_prompt_number, summary, completed) in talk_summaries {
            prompt.push_str(&format!("- {}", summary));
            if let Some(comp) = completed {
                prompt.push_str(&format!(". {}", comp));
            }
            prompt.push('\n');
        }

        prompt
    }
}

/// 截断字符串
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // 找到 UTF-8 安全的截断点
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// L1 Observation 输出结构
#[derive(Debug, Clone, serde::Deserialize)]
pub struct L1Output {
    #[serde(rename = "type")]
    pub observation_type: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Option<Vec<String>>,
    pub narrative: Option<String>,
}

/// L2 Talk Summary 输出结构
#[derive(Debug, Clone, serde::Deserialize)]
pub struct L2Output {
    pub user_request: Option<String>,
    pub summary: String,
    /// 完成事项（LLM 可能返回 string 或 array，用 Value 兼容）
    #[serde(default)]
    pub completed: Option<serde_json::Value>,
    pub files_involved: Option<Vec<String>>,
}

impl L2Output {
    /// 获取 completed 字符串（兼容 string/array 格式）
    pub fn completed_str(&self) -> Option<String> {
        match &self.completed {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
                let items: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if items.is_empty() {
                    None
                } else {
                    Some(items.join("; "))
                }
            }
            _ => None,
        }
    }
}

/// L3 Session Summary 输出结构
#[derive(Debug, Clone, serde::Deserialize)]
pub struct L3Output {
    pub summary: String,
    pub key_points: Option<Vec<String>>,
    pub files_involved: Option<Vec<String>>,
    pub technologies: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact::source::{Message, MessageRole};

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello");
        // UTF-8 安全
        assert_eq!(truncate("你好世界", 6), "你好"); // 每个中文 3 bytes
    }

    #[test]
    fn test_l1_prompt_build() {
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Read".to_string(),
            arguments: Some(r#"{"file_path": "/src/main.rs"}"#.to_string()),
            output: Some("fn main() { }".to_string()),
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };

        let prompt = L1Prompt::build_user_message(&tc, None);
        assert!(prompt.contains("Read"));
        assert!(prompt.contains("file_path"));
    }

    #[test]
    fn test_l2_prompt_build() {
        let talk = Talk {
            prompt_number: 1,
            user_message: Message {
                id: 1,
                uuid: "1".to_string(),
                role: MessageRole::User,
                content: "Help me fix the bug".to_string(),
                timestamp: 0,
                sequence: 0,
                prompt_number: 1,
                tool_name: None,
                tool_args: None,
            },
            assistant_messages: vec![Message {
                id: 2,
                uuid: "2".to_string(),
                role: MessageRole::Assistant,
                content: "I found the issue...".to_string(),
                timestamp: 0,
                sequence: 1,
                prompt_number: 1,
                tool_name: None,
                tool_args: None,
            }],
            tool_calls: vec![],
        };

        let prompt = L2Prompt::build_user_message(&talk);
        assert!(prompt.contains("[User]: Help me fix the bug"));
        assert!(prompt.contains("[Assistant]: I found the issue"));
    }

    #[test]
    fn test_l3_prompt_build() {
        let summaries = vec![
            (1, "Fixed authentication bug".to_string(), Some("Added null check".to_string())),
            (2, "Added new API endpoint".to_string(), None),
        ];

        let prompt = L3Prompt::build_user_message(&summaries, Some("/project/path"));
        assert!(prompt.contains("Project: /project/path"));
        assert!(prompt.contains("- Fixed authentication bug. Added null check"));
        assert!(prompt.contains("- Added new API endpoint"));
    }
}
