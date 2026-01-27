//! 输出格式化模块

use crate::search::SearchResult;
use crate::source::DataSourceInfo;
use ai_cli_session_collector::{MessageType, ParsedMessage, SessionMeta};
use colored::*;

/// 输出格式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Text,
    Json,
    Markdown,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" | "t" => Ok(OutputFormat::Text),
            "json" | "j" => Ok(OutputFormat::Json),
            "markdown" | "md" | "m" => Ok(OutputFormat::Markdown),
            _ => Err(format!("Unknown format: {}", s)),
        }
    }
}

/// 格式化数据源信息
pub fn format_sources(sources: &[DataSourceInfo], format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(
            &sources
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "source": s.source.to_string(),
                        "name": s.name,
                        "path": s.path.to_string_lossy(),
                        "exists": s.exists,
                        "session_count": s.session_count,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default(),

        OutputFormat::Text | OutputFormat::Markdown => {
            let mut output = String::new();
            output.push_str(&format!(
                "{}\n",
                "Available Data Sources".bold().underline()
            ));
            output.push('\n');

            for source in sources {
                let status = if source.exists {
                    "✓".green()
                } else {
                    "✗".red()
                };

                output.push_str(&format!(
                    "{} {} {}\n",
                    status,
                    source.name.bold(),
                    format!("({})", source.source).dimmed()
                ));
                output.push_str(&format!(
                    "  Path: {}\n",
                    source.path.to_string_lossy().dimmed()
                ));

                if let Some(count) = source.session_count {
                    output.push_str(&format!("  Sessions: {}\n", count.to_string().cyan()));
                }
                output.push('\n');
            }

            output
        }
    }
}

/// 格式化会话列表
pub fn format_sessions(sessions: &[SessionMeta], format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(sessions).unwrap_or_default(),

        OutputFormat::Text | OutputFormat::Markdown => {
            let mut output = String::new();
            output.push_str(&format!(
                "{} ({})\n\n",
                "Sessions".bold().underline(),
                sessions.len()
            ));

            for session in sessions {
                // 会话 ID (截取前 8 位)
                let short_id = if session.id.len() > 8 {
                    &session.id[..8]
                } else {
                    &session.id
                };

                // 源标识
                let source_tag = format!("[{}]", session.source).cyan();

                // 时间
                let time = session
                    .updated_at
                    .as_ref()
                    .or(session.created_at.as_ref())
                    .map(|t| format_timestamp(t))
                    .unwrap_or_else(|| "Unknown".to_string());

                // 项目名
                let project = session
                    .project_name.as_deref()
                    .unwrap_or("Unknown");

                output.push_str(&format!(
                    "{} {} {} {}\n",
                    source_tag,
                    short_id.yellow(),
                    project.bold(),
                    time.dimmed()
                ));

                // 消息数
                if let Some(count) = session.message_count {
                    output.push_str(&format!("  {} messages\n", count));
                }

                output.push('\n');
            }

            output
        }
    }
}

/// 格式化搜索结果
pub fn format_search_results(results: &[SearchResult], format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(results).unwrap_or_default(),

        OutputFormat::Text | OutputFormat::Markdown => {
            if results.is_empty() {
                return "No matches found.\n".to_string();
            }

            let mut output = String::new();

            // 按源分组（使用 BTreeMap，因为 Source 没有实现 Hash）
            let mut by_source: std::collections::BTreeMap<String, Vec<&SearchResult>> =
                std::collections::BTreeMap::new();
            for result in results {
                by_source
                    .entry(result.source.to_string())
                    .or_default()
                    .push(result);
            }

            for (source, group) in &by_source {
                output.push_str(&format!(
                    "{} ({} matches)\n",
                    format!("[{}]", source).cyan().bold(),
                    group.len()
                ));
                output.push_str(&"─".repeat(50));
                output.push('\n');

                // 按会话分组
                let mut by_session: std::collections::HashMap<&str, Vec<&&SearchResult>> =
                    std::collections::HashMap::new();
                for result in group {
                    by_session
                        .entry(&result.session_id)
                        .or_default()
                        .push(result);
                }

                for (session_id, session_results) in &by_session {
                    let first = session_results[0];
                    let short_id = if session_id.len() > 8 {
                        &session_id[..8]
                    } else {
                        session_id
                    };

                    output.push_str(&format!(
                        "\n{}: {} ({})\n",
                        "Session".dimmed(),
                        short_id.yellow(),
                        first
                            .project_name
                            .as_ref()
                            .unwrap_or(&first.project_path)
                            .bold()
                    ));

                    if let Some(ts) = &first.timestamp {
                        output.push_str(&format!("{}: {}\n", "Time".dimmed(), format_timestamp(ts)));
                    }

                    for result in session_results {
                        output.push('\n');

                        // 上下文（前）
                        for ctx in &result.context_before {
                            let role = format_role(ctx.message_type);
                            output.push_str(&format!(
                                "  {} {}\n",
                                role.dimmed(),
                                ctx.content.dimmed()
                            ));
                        }

                        // 匹配的消息
                        let role = format_role(result.message_type);
                        output.push_str(&format!("  {} {}\n", role.bold(), result.content));
                        output.push_str(&format!(
                            "         {} at message #{}\n",
                            "↑ match".green(),
                            result.message_index
                        ));

                        // 上下文（后）
                        for ctx in &result.context_after {
                            let role = format_role(ctx.message_type);
                            output.push_str(&format!(
                                "  {} {}\n",
                                role.dimmed(),
                                ctx.content.dimmed()
                            ));
                        }
                    }
                }

                output.push('\n');
            }

            output.push_str(&format!(
                "\nFound {} matches across {} source(s)\n",
                results.len(),
                by_source.len()
            ));

            output
        }
    }
}

/// 格式化会话详情
pub fn format_session_detail(
    session: &SessionMeta,
    messages: &[ParsedMessage],
    format: OutputFormat,
    user_only: bool,
    assistant_only: bool,
) -> String {
    match format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(&serde_json::json!({
                "session": session,
                "messages": messages,
            }))
            .unwrap_or_default()
        }

        OutputFormat::Markdown => {
            let mut output = String::new();

            // 标题
            output.push_str(&format!("# Session: {}\n\n", session.id));

            // 元信息
            output.push_str(&format!(
                "- **Source**: {}\n",
                session.source
            ));
            if let Some(project) = &session.project_name {
                output.push_str(&format!("- **Project**: {}\n", project));
            }
            output.push_str(&format!("- **Path**: {}\n", session.project_path));
            if let Some(created) = &session.created_at {
                output.push_str(&format!("- **Created**: {}\n", created));
            }
            if let Some(updated) = &session.updated_at {
                output.push_str(&format!("- **Updated**: {}\n", updated));
            }
            output.push_str("\n---\n\n");

            // 消息
            for msg in messages {
                if user_only && msg.message_type != MessageType::User {
                    continue;
                }
                if assistant_only && msg.message_type != MessageType::Assistant {
                    continue;
                }
                if msg.message_type == MessageType::Tool {
                    continue;
                }

                let role = match msg.message_type {
                    MessageType::User => "**User**",
                    MessageType::Assistant => "**Assistant**",
                    MessageType::Tool | MessageType::System => continue,
                };

                output.push_str(&format!("## {}\n\n", role));
                output.push_str(&msg.content.text);
                output.push_str("\n\n");
            }

            output
        }

        OutputFormat::Text => {
            let mut output = String::new();

            // 标题
            output.push_str(&format!(
                "{}: {}\n",
                "Session".bold().underline(),
                session.id.yellow()
            ));

            // 元信息
            output.push_str(&format!(
                "{}: {}\n",
                "Source".dimmed(),
                session.source
            ));
            if let Some(project) = &session.project_name {
                output.push_str(&format!("{}: {}\n", "Project".dimmed(), project.bold()));
            }
            output.push_str(&format!(
                "{}: {}\n",
                "Path".dimmed(),
                session.project_path
            ));
            if let Some(created) = &session.created_at {
                output.push_str(&format!(
                    "{}: {}\n",
                    "Created".dimmed(),
                    format_timestamp(created)
                ));
            }
            if let Some(updated) = &session.updated_at {
                output.push_str(&format!(
                    "{}: {}\n",
                    "Updated".dimmed(),
                    format_timestamp(updated)
                ));
            }

            output.push_str(&"\n─".repeat(40));
            output.push_str("\n\n");

            // 消息
            for (idx, msg) in messages.iter().enumerate() {
                if user_only && msg.message_type != MessageType::User {
                    continue;
                }
                if assistant_only && msg.message_type != MessageType::Assistant {
                    continue;
                }
                if msg.message_type == MessageType::Tool {
                    continue;
                }

                let role = format_role(msg.message_type);
                output.push_str(&format!("{} #{}\n", role.bold(), idx));
                output.push_str(&msg.content.text);
                output.push_str("\n\n");
            }

            output
        }
    }
}

/// 格式化角色标签
fn format_role(msg_type: MessageType) -> ColoredString {
    match msg_type {
        MessageType::User => "[User]".blue(),
        MessageType::Assistant => "[Assistant]".green(),
        MessageType::Tool => "[Tool]".yellow(),
        MessageType::System => "[System]".dimmed(),
    }
}

/// 格式化时间戳
fn format_timestamp(ts: &str) -> String {
    // 尝试解析 ISO 8601 格式，简化显示
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        dt.format("%Y-%m-%d %H:%M").to_string()
    } else {
        // 回退：直接返回原始字符串的前 16 个字符
        ts.chars().take(16).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parse() {
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert_eq!("TEXT".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert_eq!("t".parse::<OutputFormat>().unwrap(), OutputFormat::Text);

        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("j".parse::<OutputFormat>().unwrap(), OutputFormat::Json);

        assert_eq!("markdown".parse::<OutputFormat>().unwrap(), OutputFormat::Markdown);
        assert_eq!("md".parse::<OutputFormat>().unwrap(), OutputFormat::Markdown);
        assert_eq!("m".parse::<OutputFormat>().unwrap(), OutputFormat::Markdown);
    }

    #[test]
    fn test_output_format_invalid() {
        assert!("xyz".parse::<OutputFormat>().is_err());
        assert!("".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_format_timestamp_rfc3339() {
        let ts = "2026-01-15T14:30:00+08:00";
        let result = format_timestamp(ts);
        assert_eq!(result, "2026-01-15 14:30");
    }

    #[test]
    fn test_format_timestamp_fallback() {
        let ts = "invalid-timestamp-string";
        let result = format_timestamp(ts);
        assert_eq!(result, "invalid-timestam");  // 前 16 个字符
    }
}
