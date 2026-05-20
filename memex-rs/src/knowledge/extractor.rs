use std::sync::Arc;

use ai_cli_session_db::MessageType;
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::db_reader::DbReader;
use crate::llm::{ChatProvider, ChatProviderExt};

use super::prompt;
use super::store::KnowledgeNode;

pub struct Extractor {
    db: Arc<DbReader>,
}

impl Extractor {
    pub fn new(db: Arc<DbReader>) -> Self {
        Self { db }
    }

    /// Build conversation digest chunks from L0 messages.
    /// Keeps only User + Assistant text, skips tool/system/command messages.
    /// Splits into chunks that fit within the model's context window.
    pub async fn extract_conversation_digest(
        &self,
        session_id: &str,
        max_chunk_chars: usize,
    ) -> Result<Vec<String>> {
        let messages = self.db.get_messages(session_id).await?;

        let mut lines = Vec::new();
        for msg in &messages {
            match msg.r#type {
                MessageType::User => {
                    let text = msg.content_text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(ref raw) = msg.raw {
                        if raw.contains("<command-name>") || raw.contains("<system-reminder>") {
                            continue;
                        }
                    }
                    lines.push(format!("USER: {text}"));
                }
                MessageType::Assistant => {
                    let text = msg.content_text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(ref raw) = msg.raw {
                        if raw.contains("\"tool_result\"") || raw.contains("\"tool_use\"") {
                            if text.len() < 20 {
                                continue;
                            }
                        }
                    }
                    lines.push(format!("ASSISTANT: {text}"));
                }
                MessageType::Tool | MessageType::System => {
                    continue;
                }
            }
        }

        if lines.is_empty() {
            bail!("no extractable messages in session {session_id}");
        }

        let sid = &session_id[..8.min(session_id.len())];
        let total_chars: usize = lines.iter().map(|l| l.len() + 2).sum();

        if total_chars <= max_chunk_chars {
            debug!("session {sid} digest: {} messages → {} lines, single chunk", messages.len(), lines.len());
            return Ok(vec![lines.join("\n\n")]);
        }

        // Split into chunks at line boundaries
        let mut chunks = Vec::new();
        let mut current = String::new();
        for line in &lines {
            let addition = if current.is_empty() { line.len() } else { line.len() + 2 };
            if !current.is_empty() && current.len() + addition > max_chunk_chars {
                chunks.push(current);
                current = line.clone();
            } else {
                if !current.is_empty() {
                    current.push_str("\n\n");
                }
                current.push_str(line);
            }
        }
        if !current.is_empty() {
            chunks.push(current);
        }

        debug!(
            "session {sid} digest: {} messages → {} lines → {} chunks (total {total_chars} chars)",
            messages.len(), lines.len(), chunks.len()
        );

        Ok(chunks)
    }

    /// Two-pass LLM extraction: analysis → structured JSON → KnowledgeNodes.
    /// For long sessions, processes each chunk independently and merges results.
    pub async fn extract_nodes(
        &self,
        chat: &dyn ChatProvider,
        session_id: &str,
        project_id: i64,
        day: &str,
        chunks: &[String],
        project_desc: &str,
        pipeline_version: &str,
    ) -> Result<Vec<KnowledgeNode>> {
        let sid = &session_id[..8.min(session_id.len())];
        let total_chunks = chunks.len();
        let mut all_nodes = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let chunk_label = if total_chunks > 1 {
                format!(" (chunk {}/{})", i + 1, total_chunks)
            } else {
                String::new()
            };

            // Pass 1: free-form analysis
            let pass1_prompt = prompt::format_pass1(chunk, project_desc, i + 1, total_chunks);
            let analysis = chat
                .chat_with_system(prompt::SYSTEM_PROMPT, &pass1_prompt)
                .await?;

            debug!("pass1{chunk_label} done, {} chars", analysis.content.len());

            // Pass 2: structured extraction
            let pass2_prompt = prompt::format_pass2(&analysis.content);
            let structured = chat
                .chat_with_system(prompt::SYSTEM_PROMPT, &pass2_prompt)
                .await?;

            let parsed: Pass2Output = match prompt::try_parse_json(&structured.content) {
                Some(v) => v,
                None => {
                    warn!("pass2{chunk_label} JSON parse failed, retrying");
                    let retry = chat
                        .chat_with_system(prompt::SYSTEM_PROMPT, &pass2_prompt)
                        .await?;
                    match prompt::try_parse_json(&retry.content) {
                        Some(v) => v,
                        None => {
                            warn!("pass2{chunk_label} JSON parse failed after retry, skipping chunk");
                            continue;
                        }
                    }
                }
            };

            let nodes: Vec<KnowledgeNode> = parsed
                .knowledge_nodes
                .into_iter()
                .filter(|n| !n.topic.is_empty() && !n.conclusion.is_empty())
                .map(|n| {
                    let id = node_id(session_id, &n.topic, &n.conclusion);
                    KnowledgeNode {
                        id,
                        cluster_id: String::new(),
                        project_id,
                        session_id: session_id.to_string(),
                        day: day.to_string(),
                        topic: n.topic,
                        conclusion: n.conclusion,
                        node_type: n.r#type,
                        confidence: n.confidence,
                        pipeline_version: Some(pipeline_version.to_string()),
                    }
                })
                .collect();

            debug!("session {sid}{chunk_label} → {} nodes", nodes.len());
            all_nodes.extend(nodes);
        }

        debug!("session {sid} total → {} nodes from {total_chunks} chunk(s)", all_nodes.len());
        Ok(all_nodes)
    }
}

fn node_id(session_id: &str, topic: &str, conclusion: &str) -> String {
    let conclusion_prefix: String = conclusion.chars().take(50).collect();
    let raw = format!("{session_id}|{topic}|{conclusion_prefix}");
    let hash = Sha256::digest(raw.as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(serde::Deserialize)]
struct Pass2Output {
    #[allow(dead_code)]
    summary: Option<String>,
    #[allow(dead_code)]
    keywords: Option<Vec<String>>,
    knowledge_nodes: Vec<RawNode>,
    #[allow(dead_code)]
    density: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawNode {
    topic: String,
    conclusion: String,
    r#type: String,
    confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_deterministic() {
        let id1 = node_id("sess-001", "TLS", "rustls works better than native-tls");
        let id2 = node_id("sess-001", "TLS", "rustls works better than native-tls");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn node_id_differs_on_different_input() {
        let id1 = node_id("sess-001", "TLS", "conclusion A");
        let id2 = node_id("sess-001", "TLS", "conclusion B");
        assert_ne!(id1, id2);
    }

    #[test]
    fn node_id_truncates_conclusion() {
        let long_conclusion = "a".repeat(200);
        let id1 = node_id("s", "t", &long_conclusion);
        let id2 = node_id("s", "t", &format!("{}extra", "a".repeat(200)));
        // both truncate to 50 chars of 'a', so same id
        assert_eq!(id1, id2);
    }
}
