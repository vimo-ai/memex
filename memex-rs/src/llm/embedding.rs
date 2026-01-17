//! Embedding Provider trait
//!
//! 定义文本向量化能力

use anyhow::Result;
use async_trait::async_trait;

/// Embedding Provider trait
///
/// 提供文本向量化能力，用于语义搜索、RAG 等场景
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider 名称
    fn name(&self) -> &str;

    /// 获取当前使用的模型名称
    fn model(&self) -> &str;

    /// 生成单个文本的 embedding
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// 批量生成 embedding
    ///
    /// 默认实现：顺序调用 embed()
    /// Provider 可以覆盖此方法提供更高效的并发实现
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(&text).await?);
        }
        Ok(results)
    }

    /// 检查 embedding 服务是否可用
    async fn is_available(&self) -> bool;
}
