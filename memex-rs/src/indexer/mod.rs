//! 向量索引服务 - 将消息内容向量化并存储到 LanceDB

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::embedding::{Chunker, OllamaClient};
use crate::shared_adapter::SharedDbAdapter;
use crate::vector::{VectorRecord, VectorStore};

// ==================== 索引队列 ====================

/// 索引队列 - 异步处理新消息的向量索引
#[derive(Clone)]
pub struct IndexQueue {
    tx: mpsc::Sender<Vec<i64>>,
}

impl IndexQueue {
    /// 创建索引队列并启动后台处理任务
    pub fn new(indexer: VectorIndexer) -> Self {
        let (tx, rx) = mpsc::channel::<Vec<i64>>(100);

        // 启动后台消费任务
        tokio::spawn(Self::process_queue(rx, indexer));

        Self { tx }
    }

    /// 发送消息 ID 到队列
    pub async fn enqueue(&self, message_ids: Vec<i64>) {
        if message_ids.is_empty() {
            return;
        }

        if let Err(e) = self.tx.send(message_ids).await {
            tracing::error!("发送索引任务失败: {}", e);
        }
    }

    /// 后台处理队列
    async fn process_queue(mut rx: mpsc::Receiver<Vec<i64>>, indexer: VectorIndexer) {
        tracing::info!("🔄 索引队列已启动");

        while let Some(ids) = rx.recv().await {
            tracing::debug!("📥 收到 {} 条消息待索引", ids.len());

            if let Err(e) = indexer.index_by_ids(&ids).await {
                tracing::error!("❌ 索引失败: {}", e);
            }
        }

        tracing::warn!("⚠️ 索引队列已关闭");
    }
}

// ==================== 向量索引器 ====================

/// 向量索引服务
#[derive(Clone)]
pub struct VectorIndexer {
    db: Arc<SharedDbAdapter>,
    ollama: Arc<OllamaClient>,
    vector: Arc<RwLock<VectorStore>>,
    chunker: Chunker,
}

/// 索引结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexResult {
    pub total_messages: usize,
    pub indexed_messages: usize,
    pub indexed_chunks: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl VectorIndexer {
    /// 创建索引服务
    pub fn new(
        db: Arc<SharedDbAdapter>,
        ollama: Arc<OllamaClient>,
        vector: Arc<RwLock<VectorStore>>,
    ) -> Self {
        Self {
            db,
            ollama,
            vector,
            chunker: Chunker::default(),
        }
    }

    /// 同步 LanceDB 已索引状态到 SQLite
    ///
    /// 用于迁移后首次同步：将 LanceDB 中已存在的 message_id
    /// 标记为已索引（vector_indexed = 1）
    pub async fn sync_indexed_status(&self) -> Result<usize> {
        let vector_store = self.vector.read().await;
        let indexed_ids = vector_store.get_all_indexed_message_ids().await?;
        drop(vector_store);

        if indexed_ids.is_empty() {
            return Ok(0);
        }

        tracing::info!("同步索引状态: 找到 {} 条已索引消息", indexed_ids.len());

        // 批量标记（每次 1000 条，避免 SQL 语句过长）
        let mut total_marked = 0;
        for chunk in indexed_ids.chunks(1000) {
            match self.db.mark_messages_indexed(chunk).await {
                Ok(n) => total_marked += n,
                Err(e) => tracing::error!("标记已索引失败: {}", e),
            }
        }

        tracing::info!("同步完成: 标记 {} 条消息为已索引", total_marked);
        Ok(total_marked)
    }

    /// 索引所有未索引的消息
    pub async fn index_all(&self) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        // 获取所有会话
        let sessions = self.db.get_sessions(None, 10000).await?;

        for session in sessions {
            match self.index_session(&session.session_id).await {
                Ok(session_result) => {
                    result.total_messages += session_result.total_messages;
                    result.indexed_messages += session_result.indexed_messages;
                    result.indexed_chunks += session_result.indexed_chunks;
                    result.skipped += session_result.skipped;
                }
                Err(e) => {
                    result.errors.push(format!("会话 {} 索引失败: {}", session.session_id, e));
                }
            }
        }

        if result.indexed_messages > 0 {
            tracing::info!(
                "📊 索引: {} 消息, {} chunks",
                result.indexed_messages,
                result.indexed_chunks
            );
        }

        Ok(result)
    }

    /// 索引单个会话（已弃用，请使用 index_batch）
    ///
    /// 注意：此方法会重新索引会话中所有 assistant 消息，
    /// 包括已索引的消息（会在 LanceDB 中创建重复记录）。
    /// 推荐使用 index_batch 进行增量索引。
    #[deprecated(note = "请使用 index_batch 进行增量索引")]
    pub async fn index_session(&self, session_id: &str) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        let messages = self.db.get_messages(session_id).await?;
        result.total_messages = messages.len();

        let mut indexed_ids = Vec::new();

        for message in messages {
            // 只处理 assistant 类型
            if message.r#type.to_string() != "assistant" {
                result.skipped += 1;
                continue;
            }

            // 分片
            let chunks = self.chunker.chunk(&message.content_text);
            let mut records = Vec::new();
            let mut chunk_success = true;

            for chunk in chunks {
                // 生成 embedding
                match self.ollama.embed(&chunk.content).await {
                    Ok(embedding) => {
                        records.push(VectorRecord {
                            message_id: message.id,
                            chunk_index: chunk.index as i64,
                            content: chunk.content,
                            embedding,
                        });
                    }
                    Err(e) => {
                        result.errors.push(format!("消息 {} 块 {} embedding 失败: {}",
                            message.id, chunk.index, e));
                        chunk_success = false;
                        break;
                    }
                }
            }

            // 只有所有 chunk 成功才插入
            if chunk_success && !records.is_empty() {
                let mut vector_store = self.vector.write().await;
                match vector_store.insert(&records).await {
                    Ok(n) => {
                        result.indexed_chunks += n;
                        result.indexed_messages += 1;
                        indexed_ids.push(message.id);
                    }
                    Err(e) => {
                        result.errors.push(format!("插入失败: {}", e));
                    }
                }
            }
        }

        // 批量标记已索引
        if !indexed_ids.is_empty() {
            if let Err(e) = self.db.mark_messages_indexed(&indexed_ids).await {
                tracing::error!("标记已索引失败: {}", e);
            }
        }

        Ok(result)
    }

    /// 按消息 ID 列表索引（用于实时索引新消息）
    ///
    /// 这些消息是刚插入的，所以不需要检查是否已索引
    pub async fn index_by_ids(&self, message_ids: &[i64]) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: message_ids.len(),
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        if message_ids.is_empty() {
            return Ok(result);
        }

        // 按 ID 获取消息
        let messages = self.db.get_messages_by_ids(message_ids).await?;
        let mut indexed_ids = Vec::new();

        for message in &messages {
            // 只处理 assistant 类型的消息
            if message.r#type.to_string() != "assistant" {
                result.skipped += 1;
                continue;
            }

            // 分片
            let chunks = self.chunker.chunk(&message.content_text);
            let mut records = Vec::new();
            let mut chunk_success = true;

            for chunk in chunks {
                // 生成 embedding
                match self.ollama.embed(&chunk.content).await {
                    Ok(embedding) => {
                        records.push(VectorRecord {
                            message_id: message.id,
                            chunk_index: chunk.index as i64,
                            content: chunk.content.clone(),
                            embedding,
                        });
                    }
                    Err(e) => {
                        result.errors.push(format!(
                            "消息 {} 块 {} embedding 失败: {}",
                            message.id, chunk.index, e
                        ));
                        chunk_success = false;
                        break;
                    }
                }
            }

            // 只有所有 chunk 成功才插入向量库并标记
            if chunk_success && !records.is_empty() {
                let mut vector_store = self.vector.write().await;
                match vector_store.insert(&records).await {
                    Ok(n) => {
                        result.indexed_chunks += n;
                        result.indexed_messages += 1;
                        indexed_ids.push(message.id);
                    }
                    Err(e) => {
                        result.errors.push(format!("插入失败: {}", e));
                    }
                }
            }
        }

        // 批量标记已索引
        if !indexed_ids.is_empty() {
            if let Err(e) = self.db.mark_messages_indexed(&indexed_ids).await {
                tracing::error!("标记已索引失败: {}", e);
            }
        }

        if result.indexed_messages > 0 {
            tracing::debug!(
                "实时索引完成: {} 消息, {} 块",
                result.indexed_messages,
                result.indexed_chunks
            );
        }

        Ok(result)
    }

    /// 索引指定数量的消息（用于增量索引）
    ///
    /// 优化版本：直接从 SQLite 查询未索引的消息，避免遍历所有消息
    pub async fn index_batch(&self, limit: usize) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        // 直接获取未索引的消息（高效！只查询 vector_indexed = 0 的记录）
        let messages = self.db.get_unindexed_messages(limit).await?;
        result.total_messages = messages.len();

        if messages.is_empty() {
            return Ok(result);
        }

        tracing::debug!("增量索引: 找到 {} 条未索引消息", messages.len());

        let mut indexed_ids = Vec::new();

        for message in messages {
            // 分片并索引
            let chunks = self.chunker.chunk(&message.content_text);
            let mut records = Vec::new();
            let mut chunk_success = true;

            for chunk in chunks {
                match self.ollama.embed(&chunk.content).await {
                    Ok(embedding) => {
                        records.push(VectorRecord {
                            message_id: message.id,
                            chunk_index: chunk.index as i64,
                            content: chunk.content,
                            embedding,
                        });
                    }
                    Err(e) => {
                        result.errors.push(format!("Embedding 失败: {}", e));
                        chunk_success = false;
                        break;
                    }
                }
            }

            // 只有所有 chunk 成功才插入向量库并标记
            if chunk_success && !records.is_empty() {
                let mut vector_store = self.vector.write().await;
                match vector_store.insert(&records).await {
                    Ok(n) => {
                        result.indexed_chunks += n;
                        result.indexed_messages += 1;
                        indexed_ids.push(message.id);
                    }
                    Err(e) => {
                        result.errors.push(format!("插入失败: {}", e));
                    }
                }
            }
        }

        // 批量标记已索引
        if !indexed_ids.is_empty() {
            if let Err(e) = self.db.mark_messages_indexed(&indexed_ids).await {
                tracing::error!("标记已索引失败: {}", e);
            }
        }

        if result.indexed_messages > 0 {
            tracing::info!(
                "增量索引完成: {} 消息, {} chunks",
                result.indexed_messages,
                result.indexed_chunks
            );
        }

        Ok(result)
    }
}
