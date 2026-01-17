//! Grep 搜索模块
//!
//! 直接在 JSONL 文件中搜索，无需数据库

use crate::source::DataSourceManager;
use crate::utils::truncate_str;
use ai_cli_session_collector::{MessageType, ParsedMessage, SessionMeta, Source};
use anyhow::Result;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::path::PathBuf;

/// 搜索选项
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// 搜索查询（正则表达式）
    pub query: String,
    /// 是否忽略大小写
    pub case_insensitive: bool,
    /// 最大结果数
    pub max_results: usize,
    /// CLI 过滤
    pub source_filter: Option<Vec<Source>>,
    /// 项目过滤
    pub project_filter: Option<String>,
    /// 上下文行数
    pub context_lines: usize,
    /// 开始时间（毫秒时间戳）
    pub start_time: Option<u64>,
    /// 结束时间（毫秒时间戳）
    pub end_time: Option<u64>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            case_insensitive: false,
            max_results: 20,
            source_filter: None,
            project_filter: None,
            context_lines: 2,
            start_time: None,
            end_time: None,
        }
    }
}

/// 搜索结果
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// CLI 类型
    pub source: Source,
    /// 会话 ID
    pub session_id: String,
    /// 项目路径
    pub project_path: String,
    /// 项目名称
    pub project_name: Option<String>,
    /// 文件路径
    pub file_path: PathBuf,
    /// 消息索引
    pub message_index: usize,
    /// 消息类型
    pub message_type: MessageType,
    /// 匹配的内容（摘要）
    pub content: String,
    /// 完整内容
    pub full_content: String,
    /// 时间戳
    pub timestamp: Option<String>,
    /// 上下文（前几条消息）
    pub context_before: Vec<ContextMessage>,
    /// 上下文（后几条消息）
    pub context_after: Vec<ContextMessage>,
}

/// 上下文消息
#[derive(Debug, Clone, Serialize)]
pub struct ContextMessage {
    pub message_type: MessageType,
    pub content: String,
}

/// 搜索器
pub struct Searcher {
    manager: DataSourceManager,
}

impl Searcher {
    /// 创建搜索器
    pub fn new() -> Self {
        Self {
            manager: DataSourceManager::new(),
        }
    }

    /// 执行搜索
    pub fn search(&self, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        // 构建正则表达式
        let regex = RegexBuilder::new(&options.query)
            .case_insensitive(options.case_insensitive)
            .build()?;

        let mut results = Vec::new();

        // 获取所有会话
        let sessions = self.manager.list_all_sessions()?;

        for session in sessions {
            // 源过滤
            if let Some(ref filters) = options.source_filter {
                if !filters.contains(&session.source) {
                    continue;
                }
            }

            // 项目过滤
            if let Some(ref project) = options.project_filter {
                let project_match = session.project_path.contains(project)
                    || session
                        .project_name
                        .as_ref()
                        .map(|n| n.contains(project))
                        .unwrap_or(false);
                if !project_match {
                    continue;
                }
            }

            // 时间过滤（使用文件修改时间）
            if let Some(mtime) = session.file_mtime {
                if let Some(start) = options.start_time {
                    if mtime < start {
                        continue;
                    }
                }
                if let Some(end) = options.end_time {
                    if mtime > end {
                        continue;
                    }
                }
            }

            // 解析并搜索会话
            if let Some(found) =
                self.search_session(&session, &regex, options.context_lines)?
            {
                results.extend(found);

                if results.len() >= options.max_results {
                    results.truncate(options.max_results);
                    break;
                }
            }
        }

        Ok(results)
    }

    /// 在单个会话中搜索
    fn search_session(
        &self,
        session: &SessionMeta,
        regex: &Regex,
        context_lines: usize,
    ) -> Result<Option<Vec<SearchResult>>> {
        let parse_result = match self.manager.parse_session(session)? {
            Some(r) => r,
            None => return Ok(None),
        };

        let messages = &parse_result.messages;
        let mut results = Vec::new();

        for (idx, msg) in messages.iter().enumerate() {
            // 只搜索 user 和 assistant 消息
            if msg.message_type == MessageType::Tool {
                continue;
            }

            // 在内容中搜索
            let content = &msg.content.text;
            if regex.is_match(content) {
                // 收集上下文
                let context_before = self.collect_context(messages, idx, context_lines, true);
                let context_after = self.collect_context(messages, idx, context_lines, false);

                // 截取匹配位置附近的内容作为摘要
                let summary = self.extract_summary(content, regex, 100);

                results.push(SearchResult {
                    source: session.source,
                    session_id: session.id.clone(),
                    project_path: session.project_path.clone(),
                    project_name: session.project_name.clone(),
                    file_path: session
                        .session_path
                        .as_ref()
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                    message_index: idx,
                    message_type: msg.message_type,
                    content: summary,
                    full_content: content.clone(),
                    timestamp: msg.timestamp.clone(),
                    context_before,
                    context_after,
                });
            }
        }

        if results.is_empty() {
            Ok(None)
        } else {
            Ok(Some(results))
        }
    }

    /// 收集上下文消息
    fn collect_context(
        &self,
        messages: &[ParsedMessage],
        current_idx: usize,
        count: usize,
        before: bool,
    ) -> Vec<ContextMessage> {
        let mut context = Vec::new();

        if before {
            // 向前收集
            let start = current_idx.saturating_sub(count);
            for i in start..current_idx {
                if messages[i].message_type != MessageType::Tool {
                    context.push(ContextMessage {
                        message_type: messages[i].message_type,
                        content: truncate_str(&messages[i].content.text.replace('\n', " "), 200),
                    });
                }
            }
        } else {
            // 向后收集
            let end = (current_idx + 1 + count).min(messages.len());
            for i in (current_idx + 1)..end {
                if messages[i].message_type != MessageType::Tool {
                    context.push(ContextMessage {
                        message_type: messages[i].message_type,
                        content: truncate_str(&messages[i].content.text.replace('\n', " "), 200),
                    });
                }
            }
        }

        context
    }

    /// 提取匹配位置附近的摘要（UTF-8 安全）
    fn extract_summary(&self, content: &str, regex: &Regex, max_chars: usize) -> String {
        if let Some(m) = regex.find(content) {
            // 收集字符边界
            let char_indices: Vec<(usize, char)> = content.char_indices().collect();
            let total_chars = char_indices.len();

            // 找到匹配位置对应的字符索引
            let match_start_char = char_indices
                .iter()
                .position(|(byte_idx, _)| *byte_idx >= m.start())
                .unwrap_or(0);
            let match_end_char = char_indices
                .iter()
                .position(|(byte_idx, _)| *byte_idx >= m.end())
                .unwrap_or(total_chars);

            // 计算截取范围（字符数）
            let half = max_chars / 2;
            let start_char = match_start_char.saturating_sub(half);
            let end_char = (match_end_char + half).min(total_chars);

            // 转换回字节索引
            let start_byte = char_indices.get(start_char).map(|(i, _)| *i).unwrap_or(0);
            let end_byte = char_indices
                .get(end_char)
                .map(|(i, _)| *i)
                .unwrap_or(content.len());

            let mut summary = String::new();
            if start_char > 0 {
                summary.push_str("...");
            }
            summary.push_str(&content[start_byte..end_byte]);
            if end_char < total_chars {
                summary.push_str("...");
            }

            // 清理换行
            summary.replace('\n', " ").trim().to_string()
        } else {
            truncate_str(&content.replace('\n', " "), max_chars)
        }
    }
}

impl Default for Searcher {
    fn default() -> Self {
        Self::new()
    }
}
