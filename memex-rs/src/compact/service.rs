//! CompactService - 核心压缩服务
//!
//! 从 messages 表读取数据，调用 LLM 生成摘要，存储结果

use anyhow::Result;
use std::sync::Arc;

use crate::llm::{ChatMessage, ChatProvider, ChatRole};
use crate::shared_adapter::SharedDbAdapter;

use super::config::CompactConfig;
use super::db::{CompactDB, Observation, ObservationType, SessionSummary, TalkSummary};
use super::indexer::CompactIndexer;
use super::prompt::{L1Output, L1Prompt, L2Output, L2Prompt, L3Output, L3Prompt};
use super::source::{build_parsed_session, ParsedSession, Talk, ToolCall, ToolCategory};

/// Compact 处理结果
#[derive(Debug, Default)]
pub struct CompactResult {
    /// 生成的 L1 Observations 数量
    pub observations_count: usize,
    /// 生成的 L2 Talk Summaries 数量
    pub talk_summaries_count: usize,
    /// 是否生成了 L3 Session Summary
    pub session_summary_generated: bool,
    /// 跳过的工具调用数量（剪枝）
    pub tool_calls_pruned: usize,
    /// 合并的工具调用数量
    pub tool_calls_merged: usize,
}

/// CompactService - 核心压缩服务
pub struct CompactService {
    /// 共享数据库（读取 messages）
    shared_db: Arc<SharedDbAdapter>,
    /// LLM Provider
    chat_provider: Arc<dyn ChatProvider>,
    /// Compact 数据库（存储压缩结果）
    compact_db: Arc<CompactDB>,
    /// 配置
    config: CompactConfig,
    /// 向量索引器（可选，用于将摘要向量化）
    indexer: Option<CompactIndexer>,
}

impl CompactService {
    /// 创建 CompactService
    pub fn new(
        shared_db: Arc<SharedDbAdapter>,
        chat_provider: Arc<dyn ChatProvider>,
        compact_db: Arc<CompactDB>,
        config: CompactConfig,
    ) -> Self {
        // 确保配置一致性
        let mut config = config;
        config.validate();

        Self {
            shared_db,
            chat_provider,
            compact_db,
            config,
            indexer: None,
        }
    }

    /// 设置向量索引器
    pub fn with_indexer(mut self, indexer: CompactIndexer) -> Self {
        self.indexer = Some(indexer);
        self
    }

    /// 处理会话（完整流程）
    pub async fn process_session(&self, session_id: &str) -> Result<CompactResult> {
        // 获取处理进度
        let progress = self.compact_db.get_progress(session_id).await?;

        // 从 messages 表读取数据
        let db_messages = self.shared_db.get_messages(session_id).await?;

        if db_messages.is_empty() {
            tracing::debug!("会话 {} 无数据", session_id);
            return Ok(CompactResult::default());
        }

        // 增量处理：只处理新消息
        let new_messages: Vec<_> = db_messages
            .into_iter()
            .filter(|m| m.sequence > progress.last_processed_offset)
            .collect();

        if new_messages.is_empty() {
            return Ok(CompactResult::default());
        }

        // 构建 ParsedSession（使用上次处理的 prompt_number 作为起点）
        let session = build_parsed_session(session_id, new_messages, progress.last_processed_prompt);

        let mut result = CompactResult::default();

        // L1: Observations
        if self.config.l1_observations {
            let (obs_count, pruned, merged) = self.generate_l1_observations(&session).await?;
            result.observations_count = obs_count;
            result.tool_calls_pruned = pruned;
            result.tool_calls_merged = merged;
        }

        // L2: Talk Summaries
        if self.config.l2_talk_summary {
            result.talk_summaries_count = self.generate_l2_talk_summaries(&session).await?;
        }

        // 更新处理进度
        // - offset: 总是更新到最新消息（避免重复读取）
        // - prompt: 只更新到最后一个完整 Talk（有 assistant 回复的）
        let new_offset = session
            .messages
            .last()
            .map(|m| m.sequence)
            .unwrap_or(progress.last_processed_offset);

        // 找到最后一个完整的 Talk
        let last_complete_prompt = session
            .talks
            .iter()
            .filter(|t| !t.assistant_messages.is_empty())
            .map(|t| t.prompt_number)
            .max()
            .unwrap_or(progress.last_processed_prompt);

        self.compact_db
            .update_progress(
                session_id,
                &super::db::ProcessingProgress {
                    last_processed_offset: new_offset,
                    last_processed_prompt: last_complete_prompt,
                },
            )
            .await?;

        Ok(result)
    }

    /// 生成 L1 Observations
    ///
    /// 返回 (生成数量, 剪枝数量, 合并数量)
    async fn generate_l1_observations(
        &self,
        session: &ParsedSession,
    ) -> Result<(usize, usize, usize)> {
        let mut generated = 0;
        let mut merged = 0;

        // 剪枝和合并
        let tool_calls = self.prune_and_merge_tool_calls(&session.tool_calls);
        let pruned = session.tool_calls.len() - tool_calls.len();

        for tc in &tool_calls {
            // 检查是否为合并后的工具调用
            if tc.id.starts_with("merged_") {
                merged += 1;
            }

            // 调用 LLM 生成 observation
            match self
                .generate_single_observation(tc, &session.session_id)
                .await
            {
                Ok(obs) => {
                    let inserted = self.compact_db.insert_observation(&obs).await?;
                    if inserted {
                        generated += 1;
                        // 向量索引（如果配置了 indexer）
                        if let Some(ref indexer) = self.indexer {
                            if let Err(e) = indexer.index_observation(&obs).await {
                                tracing::warn!("向量索引 observation 失败: {}", e);
                            }
                        }
                    } else {
                        tracing::debug!(
                            "Observation 已存在，跳过 (session={}, offset={:?})",
                            session.session_id,
                            obs.source_offset
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "生成 observation 失败 (session={}, tool={}): {}",
                        session.session_id,
                        tc.name,
                        e
                    );
                }
            }
        }

        Ok((generated, pruned, merged))
    }

    /// 剪枝和合并工具调用
    fn prune_and_merge_tool_calls(&self, tool_calls: &[ToolCall]) -> Vec<ToolCall> {
        let mut result = Vec::new();
        let mut consecutive_group: Vec<&ToolCall> = Vec::new();
        let mut last_category: Option<ToolCategory> = None;
        let mut last_prompt: Option<i32> = None;

        for tc in tool_calls {
            // 剪枝：跳过空输出
            if self.config.l1.prune_empty && tc.is_empty_output() {
                continue;
            }

            let category = tc.tool_category();

            // 合并：连续同类操作（必须在同一个 prompt 内）
            if self.config.l1.merge_consecutive {
                let same_prompt = last_prompt == Some(tc.prompt_number);
                if Some(category) == last_category && category != ToolCategory::Other && same_prompt {
                    consecutive_group.push(tc);

                    // 达到阈值，合并
                    if consecutive_group.len() >= self.config.l1.merge_threshold {
                        if let Some(merged) = self.merge_tool_calls(&consecutive_group) {
                            result.push(merged);
                        }
                        consecutive_group.clear();
                    }
                } else {
                    // 不同类别，先处理之前的组
                    if consecutive_group.len() >= self.config.l1.merge_threshold {
                        if let Some(merged) = self.merge_tool_calls(&consecutive_group) {
                            result.push(merged);
                        }
                    } else {
                        // 不够合并阈值，逐个添加
                        for item in &consecutive_group {
                            result.push((*item).clone());
                        }
                    }
                    consecutive_group.clear();
                    consecutive_group.push(tc);
                }
                last_category = Some(category);
                last_prompt = Some(tc.prompt_number);
            } else {
                result.push(tc.clone());
            }
        }

        // 处理最后一组
        if !consecutive_group.is_empty() {
            if consecutive_group.len() >= self.config.l1.merge_threshold {
                if let Some(merged) = self.merge_tool_calls(&consecutive_group) {
                    result.push(merged);
                }
            } else {
                for item in &consecutive_group {
                    result.push((*item).clone());
                }
            }
        }

        result
    }

    /// 合并同类工具调用
    fn merge_tool_calls(&self, calls: &[&ToolCall]) -> Option<ToolCall> {
        if calls.is_empty() {
            return None;
        }

        let first = calls[0];
        let category = first.tool_category();

        // 收集所有文件路径
        let files: Vec<String> = calls
            .iter()
            .filter_map(|tc| tc.extract_file_path())
            .collect();

        // 合并参数（简化版：列出所有文件）
        let merged_args = serde_json::json!({
            "merged_count": calls.len(),
            "category": format!("{:?}", category),
            "files": files,
        });

        Some(ToolCall {
            id: format!("merged_{}_{}", first.id, calls.len()),
            name: format!("{}(x{})", first.name, calls.len()),
            arguments: Some(merged_args.to_string()),
            output: Some(format!("Merged {} {} operations", calls.len(), first.name)),
            timestamp: first.timestamp,
            sequence: first.sequence,
            prompt_number: first.prompt_number,
        })
    }

    /// 生成单个 Observation
    async fn generate_single_observation(
        &self,
        tc: &ToolCall,
        session_id: &str,
    ) -> Result<Observation> {
        let messages = vec![
            ChatMessage {
                role: ChatRole::System,
                content: L1Prompt::SYSTEM.to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: L1Prompt::build_user_message(tc, None),
            },
        ];

        let response = self.chat_provider.chat(&messages).await?;

        // 解析 JSON 响应
        let output: L1Output = parse_json_response(&response.content)?;

        let now = chrono::Utc::now().to_rfc3339();

        // 提取文件路径（支持单文件和合并后的多文件）
        let files = tc.extract_file_paths();
        let category = tc.tool_category();

        Ok(Observation {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            prompt_number: tc.prompt_number,
            source_offset: Some(tc.sequence),
            observation_type: ObservationType::from_str(&output.observation_type)
                .unwrap_or(ObservationType::Change),
            title: output.title,
            subtitle: output.subtitle,
            facts: output.facts,
            narrative: output.narrative,
            files_read: if category.is_read() && !files.is_empty() {
                Some(files.clone())
            } else {
                None
            },
            files_modified: if category.is_write() && !files.is_empty() {
                Some(files)
            } else {
                None
            },
            provider: Some(self.chat_provider.name().to_string()),
            model: None,
            created_at: now,
        })
    }

    /// 生成 L2 Talk Summaries
    ///
    /// 只为"完整"的对话轮次生成摘要（有 assistant 回复）
    async fn generate_l2_talk_summaries(&self, session: &ParsedSession) -> Result<usize> {
        let mut generated = 0;

        for talk in &session.talks {
            // 跳过不完整的对话（没有 assistant 回复）
            if talk.assistant_messages.is_empty() {
                tracing::debug!(
                    "跳过不完整的对话 (session={}, prompt={}): 没有 assistant 回复",
                    session.session_id,
                    talk.prompt_number
                );
                continue;
            }

            match self
                .generate_single_talk_summary(talk, &session.session_id)
                .await
            {
                Ok(summary) => {
                    self.compact_db.upsert_talk_summary(&summary).await?;
                    generated += 1;
                    // 向量索引（如果配置了 indexer）
                    if let Some(ref indexer) = self.indexer {
                        if let Err(e) = indexer.index_talk_summary(&summary).await {
                            tracing::warn!("向量索引 talk summary 失败: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "生成 talk summary 失败 (session={}, prompt={}): {}",
                        session.session_id,
                        talk.prompt_number,
                        e
                    );
                }
            }
        }

        Ok(generated)
    }

    /// 生成单个 Talk Summary
    async fn generate_single_talk_summary(
        &self,
        talk: &Talk,
        session_id: &str,
    ) -> Result<TalkSummary> {
        let messages = vec![
            ChatMessage {
                role: ChatRole::System,
                content: L2Prompt::SYSTEM.to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: L2Prompt::build_user_message(talk),
            },
        ];

        let response = self.chat_provider.chat(&messages).await?;

        // 解析 JSON 响应
        let output: L2Output = parse_json_response(&response.content)?;

        let now = chrono::Utc::now().to_rfc3339();

        // 先提取字段（避免借用问题）
        let completed = output.completed_str();

        // 从工具调用提取文件（如果 LLM 没返回）
        let files_involved = output.files_involved.or_else(|| {
            let files = talk.files_involved();
            if files.is_empty() {
                None
            } else {
                Some(files)
            }
        });

        Ok(TalkSummary {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            prompt_number: talk.prompt_number,
            user_request: output.user_request,
            summary: output.summary,
            completed,
            files_involved,
            provider: Some(self.chat_provider.name().to_string()),
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: now,
        })
    }

    /// 生成 L3 Session Summary
    ///
    /// 依赖 L2，从已生成的 Talk Summaries 汇总
    pub async fn generate_l3_session_summary(&self, session_id: &str) -> Result<bool> {
        if !self.config.l3_session_summary {
            return Ok(false);
        }

        // 获取已有的 L2 summaries
        let talk_summaries = self.compact_db.get_talk_summaries(session_id).await?;

        if talk_summaries.is_empty() {
            tracing::debug!("会话 {} 没有 talk summaries，跳过 L3 生成", session_id);
            return Ok(false);
        }

        // 构建 L3 输入
        let summaries: Vec<(i32, String, Option<String>)> = talk_summaries
            .iter()
            .map(|s| (s.prompt_number, s.summary.clone(), s.completed.clone()))
            .collect();

        // 获取项目路径
        let project_path: Option<String> = None; // TODO: 从 session 表获取

        let messages = vec![
            ChatMessage {
                role: ChatRole::System,
                content: L3Prompt::SYSTEM.to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: L3Prompt::build_user_message(&summaries, project_path.as_deref()),
            },
        ];

        let response = self.chat_provider.chat(&messages).await?;

        // 解析 JSON 响应
        let output: L3Output = parse_json_response(&response.content)?;

        let now = chrono::Utc::now().to_rfc3339();

        // 合并所有 talk summaries 的文件
        let files_involved = output.files_involved.or_else(|| {
            let files: Vec<String> = talk_summaries
                .iter()
                .filter_map(|s| s.files_involved.clone())
                .flatten()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            if files.is_empty() {
                None
            } else {
                Some(files)
            }
        });

        let session_summary = SessionSummary {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            summary: output.summary,
            key_points: output.key_points,
            files_involved,
            technologies: output.technologies,
            provider: Some(self.chat_provider.name().to_string()),
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: now.clone(),
            updated_at: now,
        };

        self.compact_db
            .upsert_session_summary(&session_summary)
            .await?;

        // 向量索引（如果配置了 indexer）
        if let Some(ref indexer) = self.indexer {
            if let Err(e) = indexer.index_session_summary(&session_summary).await {
                tracing::warn!("向量索引 session summary 失败: {}", e);
            }
        }

        Ok(true)
    }

    /// 检查会话是否需要 compact
    pub async fn needs_compact(&self, session_id: &str) -> Result<bool> {
        let progress = self.compact_db.get_progress(session_id).await?;

        // 获取最新消息序号
        let messages = self.shared_db.get_messages(session_id).await?;
        if let Some(last_msg) = messages.last() {
            return Ok(last_msg.sequence > progress.last_processed_offset);
        }

        Ok(false)
    }
}

/// 解析 JSON 响应（支持 markdown code block 包裹，自动去除 think 标签）
fn parse_json_response<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
    // 去除 qwen3 等模型的 <think>...</think> 标签
    let stripped = strip_think_tags(response);
    let response = stripped.trim();

    // 尝试直接解析
    if let Ok(parsed) = serde_json::from_str(response) {
        return Ok(parsed);
    }

    // 尝试从 markdown code block 提取
    let json_str = if response.starts_with("```json") {
        response
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .map(|s| s.trim())
            .unwrap_or(response)
    } else if response.starts_with("```") {
        response
            .strip_prefix("```")
            .and_then(|s| s.strip_suffix("```"))
            .map(|s| s.trim())
            .unwrap_or(response)
    } else {
        response
    };

    // 尝试找到 JSON 对象
    if let Some(start) = json_str.find('{') {
        if let Some(end) = json_str.rfind('}') {
            let json_part = &json_str[start..=end];
            if let Ok(parsed) = serde_json::from_str(json_part) {
                return Ok(parsed);
            }
        }
    }

    anyhow::bail!("无法解析 LLM 响应为 JSON: {}", response)
}

/// 去除 <think>...</think> 标签（qwen3 等模型的 thinking 输出）
fn strip_think_tags(s: &str) -> String {
    // 简单实现：找到 </think> 后的内容
    if let Some(end_pos) = s.find("</think>") {
        s[end_pos + 8..].to_string()
    } else if s.contains("<think>") {
        // 有开始标签但没结束标签，可能是截断了，返回原文
        s.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_response_direct() {
        let json = r#"{"type": "bugfix", "title": "Fix null pointer"}"#;
        let result: L1Output = parse_json_response(json).unwrap();
        assert_eq!(result.observation_type, "bugfix");
        assert_eq!(result.title, "Fix null pointer");
    }

    #[test]
    fn test_parse_json_response_markdown() {
        let json = r#"```json
{"type": "feature", "title": "Add new API"}
```"#;
        let result: L1Output = parse_json_response(json).unwrap();
        assert_eq!(result.observation_type, "feature");
    }

    #[test]
    fn test_parse_json_response_with_text() {
        let json = r#"Here is the observation:
{"type": "change", "title": "Update config"}
That's it."#;
        let result: L1Output = parse_json_response(json).unwrap();
        assert_eq!(result.observation_type, "change");
    }

    #[test]
    fn test_prune_empty_output() {
        let tc = ToolCall {
            id: "1".to_string(),
            name: "Read".to_string(),
            arguments: None,
            output: Some("   ".to_string()), // 空白输出
            timestamp: 0,
            sequence: 0,
            prompt_number: 1,
        };
        assert!(tc.is_empty_output());
    }
}
