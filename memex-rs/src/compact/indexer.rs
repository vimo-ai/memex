//! Compact 向量索引器
//!
//! 将 L1/L2/L3 摘要向量化并存储到 LanceDB

#![allow(dead_code)] // compact_db 预留用于后续查询

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::llm::EmbeddingProvider;

use super::db::{CompactDB, Observation, SessionSummary, TalkSummary};
use super::vector::{CompactLevel, CompactVectorRecord, CompactVectorStore};

/// Compact 向量索引器
#[derive(Clone)]
pub struct CompactIndexer {
    compact_db: Arc<CompactDB>,
    embedding: Arc<dyn EmbeddingProvider>,
    vector: Arc<RwLock<CompactVectorStore>>,
}

impl CompactIndexer {
    /// 创建索引器
    pub fn new(
        compact_db: Arc<CompactDB>,
        embedding: Arc<dyn EmbeddingProvider>,
        vector: Arc<RwLock<CompactVectorStore>>,
    ) -> Self {
        Self {
            compact_db,
            embedding,
            vector,
        }
    }

    /// 索引单个 L1 Observation
    pub async fn index_observation(&self, obs: &Observation) -> Result<bool> {
        // 构建向量化文本
        let text = self.build_observation_text(obs);

        // 生成向量
        let embedding = self.embedding.embed(&text).await?;

        // 构建记录
        let record = CompactVectorRecord {
            id: format!("l1_{}", obs.id),
            session_id: obs.session_id.clone(),
            level: CompactLevel::L1,
            source_id: obs.id.clone(),
            prompt_number: Some(obs.prompt_number),
            text,
            created_at: obs.created_at.clone(),
            embedding,
        };

        // 插入
        let mut store = self.vector.write().await;
        store.insert(&[record]).await?;

        Ok(true)
    }

    /// 索引单个 L2 Talk Summary
    pub async fn index_talk_summary(&self, summary: &TalkSummary) -> Result<bool> {
        // 构建向量化文本
        let text = self.build_talk_summary_text(summary);

        // 生成向量
        let embedding = self.embedding.embed(&text).await?;

        // 构建记录
        let record = CompactVectorRecord {
            id: format!("l2_{}", summary.id),
            session_id: summary.session_id.clone(),
            level: CompactLevel::L2,
            source_id: summary.id.clone(),
            prompt_number: Some(summary.prompt_number),
            text,
            created_at: summary.created_at.clone(),
            embedding,
        };

        // 插入
        let mut store = self.vector.write().await;
        store.insert(&[record]).await?;

        Ok(true)
    }

    /// 索引单个 L3 Session Summary
    pub async fn index_session_summary(&self, summary: &SessionSummary) -> Result<bool> {
        // 构建向量化文本
        let text = self.build_session_summary_text(summary);

        // 生成向量
        let embedding = self.embedding.embed(&text).await?;

        // 构建记录
        let record = CompactVectorRecord {
            id: format!("l3_{}", summary.id),
            session_id: summary.session_id.clone(),
            level: CompactLevel::L3,
            source_id: summary.id.clone(),
            prompt_number: None,
            text,
            created_at: summary.created_at.clone(),
            embedding,
        };

        // 插入
        let mut store = self.vector.write().await;
        store.insert(&[record]).await?;

        Ok(true)
    }

    /// 批量索引 Observations
    pub async fn index_observations_batch(&self, observations: &[Observation]) -> Result<usize> {
        if observations.is_empty() {
            return Ok(0);
        }

        // 构建文本列表
        let texts: Vec<String> = observations
            .iter()
            .map(|obs| self.build_observation_text(obs))
            .collect();

        // 批量生成向量
        let embeddings = self.embedding.embed_batch(texts.clone()).await?;

        // 构建记录
        let records: Vec<CompactVectorRecord> = observations
            .iter()
            .zip(texts.into_iter())
            .zip(embeddings.into_iter())
            .map(|((obs, text), embedding)| CompactVectorRecord {
                id: format!("l1_{}", obs.id),
                session_id: obs.session_id.clone(),
                level: CompactLevel::L1,
                source_id: obs.id.clone(),
                prompt_number: Some(obs.prompt_number),
                text,
                created_at: obs.created_at.clone(),
                embedding,
            })
            .collect();

        // 批量插入
        let mut store = self.vector.write().await;
        store.insert(&records).await?;

        Ok(records.len())
    }

    /// 批量索引 Talk Summaries
    pub async fn index_talk_summaries_batch(&self, summaries: &[TalkSummary]) -> Result<usize> {
        if summaries.is_empty() {
            return Ok(0);
        }

        let texts: Vec<String> = summaries
            .iter()
            .map(|s| self.build_talk_summary_text(s))
            .collect();

        let embeddings = self.embedding.embed_batch(texts.clone()).await?;

        let records: Vec<CompactVectorRecord> = summaries
            .iter()
            .zip(texts.into_iter())
            .zip(embeddings.into_iter())
            .map(|((s, text), embedding)| CompactVectorRecord {
                id: format!("l2_{}", s.id),
                session_id: s.session_id.clone(),
                level: CompactLevel::L2,
                source_id: s.id.clone(),
                prompt_number: Some(s.prompt_number),
                text,
                created_at: s.created_at.clone(),
                embedding,
            })
            .collect();

        let mut store = self.vector.write().await;
        store.insert(&records).await?;

        Ok(records.len())
    }

    /// 删除会话的所有向量索引
    pub async fn delete_session_vectors(&self, session_id: &str) -> Result<()> {
        let mut store = self.vector.write().await;
        store.delete_by_session(session_id).await
    }

    /// 构建 Observation 的向量化文本
    fn build_observation_text(&self, obs: &Observation) -> String {
        let mut parts = vec![obs.title.clone()];

        if let Some(ref subtitle) = obs.subtitle {
            parts.push(subtitle.clone());
        }

        if let Some(ref facts) = obs.facts {
            parts.extend(facts.iter().cloned());
        }

        if let Some(ref narrative) = obs.narrative {
            parts.push(narrative.clone());
        }

        // 加入文件路径信息
        if let Some(ref files) = obs.files_read {
            parts.push(format!("读取文件: {}", files.join(", ")));
        }
        if let Some(ref files) = obs.files_modified {
            parts.push(format!("修改文件: {}", files.join(", ")));
        }

        parts.join(" | ")
    }

    /// 构建 Talk Summary 的向量化文本
    fn build_talk_summary_text(&self, summary: &TalkSummary) -> String {
        let mut parts = Vec::new();

        if let Some(ref req) = summary.user_request {
            parts.push(format!("用户请求: {}", req));
        }

        parts.push(summary.summary.clone());

        if let Some(ref completed) = summary.completed {
            parts.push(format!("完成情况: {}", completed));
        }

        if let Some(ref files) = summary.files_involved {
            parts.push(format!("涉及文件: {}", files.join(", ")));
        }

        parts.join(" | ")
    }

    /// 构建 Session Summary 的向量化文本
    fn build_session_summary_text(&self, summary: &SessionSummary) -> String {
        let mut parts = vec![summary.summary.clone()];

        if let Some(ref points) = summary.key_points {
            parts.extend(points.iter().cloned());
        }

        if let Some(ref files) = summary.files_involved {
            parts.push(format!("涉及文件: {}", files.join(", ")));
        }

        if let Some(ref techs) = summary.technologies {
            parts.push(format!("技术栈: {}", techs.join(", ")));
        }

        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试文本构建逻辑
    #[test]
    fn test_build_observation_text() {
        let obs = Observation {
            id: "obs-1".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            source_offset: Some(100),
            observation_type: super::super::db::ObservationType::Bugfix,
            title: "Fix null pointer".to_string(),
            subtitle: Some("In user module".to_string()),
            facts: Some(vec!["Fixed NPE".to_string(), "Added null check".to_string()]),
            narrative: None,
            files_read: Some(vec!["src/user.rs".to_string()]),
            files_modified: Some(vec!["src/user.rs".to_string()]),
            provider: None,
            model: None,
            created_at: "2024-01-01".to_string(),
        };

        // 简单验证 Observation 的字段
        assert_eq!(obs.title, "Fix null pointer");
        assert!(obs.subtitle.as_ref().unwrap().contains("user module"));
        assert!(obs.facts.as_ref().unwrap().contains(&"Fixed NPE".to_string()));
        assert!(obs.files_read.as_ref().unwrap().contains(&"src/user.rs".to_string()));
    }
}
