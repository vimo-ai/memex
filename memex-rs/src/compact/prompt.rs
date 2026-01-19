//! Compact Prompt 模板
//!
//! L1/L2/L3 各层的 LLM prompt 设计

use super::source::{Talk, ToolCall};

/// L1: Observation 生成 Prompt
pub struct L1Prompt;

impl L1Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"你是一个代码操作分析助手。你的任务是分析工具调用，生成结构化的 observation。

输出格式（JSON）：
{
  "type": "bugfix|feature|refactor|change|discovery|decision",
  "title": "短标题（< 20 字）",
  "subtitle": "一句话解释（可选）",
  "facts": ["具体事实1", "具体事实2"],
  "narrative": "完整上下文描述（可选，用于复杂操作）"
}

类型说明：
- bugfix: 修复 bug
- feature: 新功能
- refactor: 重构代码
- change: 一般修改
- discovery: 发现/理解代码
- decision: 做出技术决策

要求：
- 保持简洁，抓住核心
- facts 用于精确搜索，应包含关键词
- 如果操作很简单，narrative 可省略"#;

    /// 构建用户消息
    pub fn build_user_message(tool_call: &ToolCall, context: Option<&str>) -> String {
        let mut prompt = String::new();

        prompt.push_str(&format!("工具: {}\n", tool_call.name));

        if let Some(args) = &tool_call.arguments {
            // 截断过长的参数
            let args_display = if args.len() > 1000 {
                format!("{}...(truncated)", &args[..1000])
            } else {
                args.clone()
            };
            prompt.push_str(&format!("参数: {}\n", args_display));
        }

        if let Some(output) = &tool_call.output {
            // 截断过长的输出
            let output_display = if output.len() > 2000 {
                format!("{}...(truncated)", &output[..2000])
            } else {
                output.clone()
            };
            prompt.push_str(&format!("输出: {}\n", output_display));
        }

        if let Some(ctx) = context {
            prompt.push_str(&format!("\n上下文: {}\n", ctx));
        }

        prompt.push_str("\n请分析这个工具调用，生成 observation JSON：");
        prompt
    }
}

/// L2: Talk Summary 生成 Prompt
pub struct L2Prompt;

impl L2Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"你是一个对话摘要助手。你的任务是将一轮 AI 编程对话压缩为简洁的摘要。

输出格式（JSON）：
{
  "user_request": "用户请求的简要描述（< 50 字）",
  "summary": "这轮对话完成了什么（< 200 字）",
  "completed": "具体完成的事项（可选）",
  "files_involved": ["涉及的文件路径"]
}

要求：
- 保留关键决策和解决方案
- 去除重复和冗余
- 保持技术准确性
- files_involved 从工具调用中提取"#;

    /// 构建用户消息
    pub fn build_user_message(talk: &Talk) -> String {
        let mut prompt = String::new();

        prompt.push_str(&format!(
            "第 {} 轮对话\n\n",
            talk.prompt_number
        ));

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
            prompt.push_str("工具调用：\n");
            for tc in &talk.tool_calls {
                prompt.push_str(&format!("- {}", tc.name));
                if let Some(path) = tc.extract_file_path() {
                    prompt.push_str(&format!(" ({})", path));
                }
                prompt.push('\n');
            }
            prompt.push('\n');
        }

        prompt.push_str("请生成这轮对话的摘要 JSON：");
        prompt
    }
}

/// L3: Session Summary 生成 Prompt
pub struct L3Prompt;

impl L3Prompt {
    /// 系统提示
    pub const SYSTEM: &'static str = r#"你是一个会话摘要助手。你的任务是将多轮对话的摘要汇总为一个完整的会话总结。

输出格式（JSON）：
{
  "summary": "一句话总结整个会话（< 100 字）",
  "key_points": ["关键点1", "关键点2", ...],
  "files_involved": ["涉及的主要文件"],
  "technologies": ["涉及的技术/工具"]
}

要求：
- summary 要能让人快速理解这个会话做了什么
- key_points 3-7 条，覆盖主要工作内容
- 去除重复，合并相关内容
- 保持技术准确性"#;

    /// 从 L2 Talk Summaries 构建用户消息
    pub fn build_user_message(
        talk_summaries: &[(i32, String, Option<String>)], // (prompt_number, summary, completed)
        project_path: Option<&str>,
    ) -> String {
        let mut prompt = String::new();

        if let Some(path) = project_path {
            prompt.push_str(&format!("项目: {}\n\n", path));
        }

        prompt.push_str(&format!("共 {} 轮对话：\n\n", talk_summaries.len()));

        for (prompt_number, summary, completed) in talk_summaries {
            prompt.push_str(&format!("第 {} 轮: {}\n", prompt_number, summary));
            if let Some(comp) = completed {
                prompt.push_str(&format!("  完成: {}\n", comp));
            }
            prompt.push('\n');
        }

        prompt.push_str("请生成整个会话的总结 JSON：");
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
        assert!(prompt.contains("第 1 轮对话"));
        assert!(prompt.contains("Help me fix the bug"));
    }

    #[test]
    fn test_l3_prompt_build() {
        let summaries = vec![
            (1, "Fixed authentication bug".to_string(), Some("Added null check".to_string())),
            (2, "Added new API endpoint".to_string(), None),
        ];

        let prompt = L3Prompt::build_user_message(&summaries, Some("/project/path"));
        assert!(prompt.contains("项目: /project/path"));
        assert!(prompt.contains("共 2 轮对话"));
    }
}
