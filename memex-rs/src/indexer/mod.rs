//! 向量索引服务 - 将消息内容向量化并存储到 LanceDB

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::db::Database;
use crate::embedding::{Chunker, OllamaClient};
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
    db: Database,
    ollama: Arc<OllamaClient>,
    vector: Arc<RwLock<VectorStore>>,
    chunker: Chunker,
    batch_size: usize,
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
        db: Database,
        ollama: Arc<OllamaClient>,
        vector: Arc<RwLock<VectorStore>>,
    ) -> Self {
        Self {
            db,
            ollama,
            vector,
            chunker: Chunker::default(),
            batch_size: 10,
        }
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
        let sessions = self.db.get_sessions(None, 10000)?;

        for session in sessions {
            match self.index_session(&session.id).await {
                Ok(session_result) => {
                    result.total_messages += session_result.total_messages;
                    result.indexed_messages += session_result.indexed_messages;
                    result.indexed_chunks += session_result.indexed_chunks;
                    result.skipped += session_result.skipped;
                }
                Err(e) => {
                    result.errors.push(format!("会话 {} 索引失败: {}", session.id, e));
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

    /// 索引单个会话
    pub async fn index_session(&self, session_id: &str) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        let messages = self.db.get_messages(session_id)?;
        result.total_messages = messages.len();

        let mut records = Vec::new();

        for message in messages {
            // 检查是否已索引
            let vector_store = self.vector.read().await;
            if vector_store.is_indexed(message.id).await? {
                result.skipped += 1;
                continue;
            }
            drop(vector_store);

            // 分片
            let chunks = self.chunker.chunk(&message.content);

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
                    }
                }

                // 批量插入
                if records.len() >= self.batch_size {
                    let mut vector_store = self.vector.write().await;
                    match vector_store.insert(&records).await {
                        Ok(n) => {
                            result.indexed_chunks += n;
                        }
                        Err(e) => {
                            result.errors.push(format!("批量插入失败: {}", e));
                        }
                    }
                    records.clear();
                }
            }

            result.indexed_messages += 1;
        }

        // 插入剩余记录
        if !records.is_empty() {
            let mut vector_store = self.vector.write().await;
            match vector_store.insert(&records).await {
                Ok(n) => {
                    result.indexed_chunks += n;
                }
                Err(e) => {
                    result.errors.push(format!("最终批量插入失败: {}", e));
                }
            }
        }

        Ok(result)
    }

    /// 按消息 ID 列表索引（用于实时索引）
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
        let messages = self.db.get_messages_by_ids(message_ids)?;
        let mut records = Vec::new();

        for message in messages {
            // 检查是否已索引
            let vector_store = self.vector.read().await;
            if vector_store.is_indexed(message.id).await? {
                result.skipped += 1;
                continue;
            }
            drop(vector_store);

            // 分片
            let chunks = self.chunker.chunk(&message.content);

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
                        result.errors.push(format!(
                            "消息 {} 块 {} embedding 失败: {}",
                            message.id, chunk.index, e
                        ));
                    }
                }

                // 批量插入
                if records.len() >= self.batch_size {
                    let mut vector_store = self.vector.write().await;
                    match vector_store.insert(&records).await {
                        Ok(n) => {
                            result.indexed_chunks += n;
                        }
                        Err(e) => {
                            result.errors.push(format!("批量插入失败: {}", e));
                        }
                    }
                    records.clear();
                }
            }

            result.indexed_messages += 1;
        }

        // 插入剩余记录
        if !records.is_empty() {
            let mut vector_store = self.vector.write().await;
            match vector_store.insert(&records).await {
                Ok(n) => {
                    result.indexed_chunks += n;
                }
                Err(e) => {
                    result.errors.push(format!("最终批量插入失败: {}", e));
                }
            }
        }

        tracing::debug!(
            "实时索引完成: {} 消息, {} 块",
            result.indexed_messages,
            result.indexed_chunks
        );

        Ok(result)
    }

    /// 索引指定数量的消息（用于增量索引）
    pub async fn index_batch(&self, limit: usize) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            errors: vec![],
        };

        // 获取最近的会话
        let sessions = self.db.get_sessions(None, 100)?;

        let mut indexed = 0;
        for session in sessions {
            if indexed >= limit {
                break;
            }

            let messages = self.db.get_messages(&session.id)?;

            for message in messages {
                if indexed >= limit {
                    break;
                }

                // 检查是否已索引
                let vector_store = self.vector.read().await;
                if vector_store.is_indexed(message.id).await? {
                    continue;
                }
                drop(vector_store);

                result.total_messages += 1;

                // 分片并索引
                let chunks = self.chunker.chunk(&message.content);
                let mut records = Vec::new();

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
                        }
                    }
                }

                if !records.is_empty() {
                    let mut vector_store = self.vector.write().await;
                    match vector_store.insert(&records).await {
                        Ok(n) => {
                            result.indexed_chunks += n;
                            result.indexed_messages += 1;
                        }
                        Err(e) => {
                            result.errors.push(format!("插入失败: {}", e));
                        }
                    }
                }

                indexed += 1;
            }
        }

        Ok(result)
    }
}
