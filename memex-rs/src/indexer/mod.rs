//! 向量索引服务 - 将消息内容向量化并存储到 LanceDB

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
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
            tracing::error!("Failed to send index task: {}", e);
        }
    }

    /// Background queue processor
    async fn process_queue(mut rx: mpsc::Receiver<Vec<i64>>, indexer: VectorIndexer) {
        tracing::info!("🔄 Index queue started");

        while let Some(ids) = rx.recv().await {
            tracing::debug!("📥 Received {} messages to index", ids.len());

            if let Err(e) = indexer.index_by_ids(&ids).await {
                tracing::error!("❌ Indexing failed: {}", e);
            }
        }

        tracing::warn!("⚠️ Index queue closed");
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
    /// 后台全量索引任务是否正在运行
    running: Arc<AtomicBool>,
}

/// 索引结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexResult {
    pub total_messages: usize,
    pub indexed_messages: usize,
    pub indexed_chunks: usize,
    pub skipped: usize,
    pub failed: usize, // 新增：标记为失败的消息数量
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
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 检查后台全量索引任务是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 设置运行状态
    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
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

        tracing::info!(
            "Syncing index status: found {} indexed messages",
            indexed_ids.len()
        );

        // Batch mark (1000 per batch, avoid SQL too long)
        let mut total_marked = 0;
        for chunk in indexed_ids.chunks(1000) {
            match self.db.mark_messages_indexed(chunk).await {
                Ok(n) => total_marked += n,
                Err(e) => tracing::error!("Failed to mark as indexed: {}", e),
            }
        }

        tracing::info!("Sync complete: marked {} messages as indexed", total_marked);
        Ok(total_marked)
    }

    /// 索引所有未索引的消息
    pub async fn index_all(&self) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            failed: 0,
            errors: vec![],
        };

        // 获取所有会话
        let sessions = self.db.get_sessions(None, 10000).await?;

        for session in sessions {
            #[allow(deprecated)]
            match self.index_session(&session.session_id).await {
                Ok(session_result) => {
                    result.total_messages += session_result.total_messages;
                    result.indexed_messages += session_result.indexed_messages;
                    result.indexed_chunks += session_result.indexed_chunks;
                    result.skipped += session_result.skipped;
                    result.failed += session_result.failed;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Session {} indexing failed: {}",
                        session.session_id, e
                    ));
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
            failed: 0,
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
                        result.errors.push(format!(
                            "Message {} chunk {} embedding failed: {}",
                            message.id, chunk.index, e
                        ));
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
                        result.errors.push(format!("Insert failed: {}", e));
                    }
                }
            }
        }

        // Batch mark as indexed
        if !indexed_ids.is_empty() {
            if let Err(e) = self.db.mark_messages_indexed(&indexed_ids).await {
                tracing::error!("Failed to mark as indexed: {}", e);
            }
        }

        Ok(result)
    }

    /// 按消息 ID 列表索引（用于实时索引新消息）
    ///
    /// 并发 embedding + 批量 insert
    pub async fn index_by_ids(&self, message_ids: &[i64]) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: message_ids.len(),
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            failed: 0,
            errors: vec![],
        };

        if message_ids.is_empty() {
            return Ok(result);
        }

        // 按 ID 获取消息
        let messages = self.db.get_messages_by_ids(message_ids).await?;

        // 收集需要索引的 assistant 消息
        struct ChunkInfo {
            message_id: i64,
            chunk_index: i64,
            content: String,
        }

        let mut all_chunks: Vec<ChunkInfo> = Vec::new();
        let mut assistant_ids: Vec<i64> = Vec::new();

        for message in &messages {
            // 只处理 assistant 类型的消息
            if message.r#type.to_string() != "assistant" {
                result.skipped += 1;
                continue;
            }

            assistant_ids.push(message.id);
            let chunks = self.chunker.chunk(&message.content_text);
            for chunk in chunks {
                all_chunks.push(ChunkInfo {
                    message_id: message.id,
                    chunk_index: chunk.index as i64,
                    content: chunk.content,
                });
            }
        }

        if all_chunks.is_empty() {
            return Ok(result);
        }

        // 并发 embedding
        let texts: Vec<String> = all_chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = match self.ollama.embed_batch(texts).await {
            Ok(embs) => embs,
            Err(e) => {
                result.errors.push(format!("Batch embedding failed: {}", e));
                result.failed = assistant_ids.len();
                if let Err(e) = self.db.mark_messages_index_failed(&assistant_ids).await {
                    tracing::error!("Failed to mark index failed status: {}", e);
                }
                return Ok(result);
            }
        };

        // 组装 VectorRecord
        let all_records: Vec<VectorRecord> = all_chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| VectorRecord {
                message_id: chunk.message_id,
                chunk_index: chunk.chunk_index,
                content: chunk.content,
                embedding,
            })
            .collect();

        result.indexed_chunks = all_records.len();
        result.indexed_messages = assistant_ids.len();

        // Batch insert
        if !all_records.is_empty() {
            let mut vector_store = self.vector.write().await;
            if let Err(e) = vector_store.insert(&all_records).await {
                tracing::error!("Batch insert failed: {}", e);
                result.failed = result.indexed_messages;
                result.indexed_messages = 0;
                result.indexed_chunks = 0;
                if let Err(e) = self.db.mark_messages_index_failed(&assistant_ids).await {
                    tracing::error!("Failed to mark index failed status: {}", e);
                }
                return Ok(result);
            }
        }

        // Mark as indexed
        if let Err(e) = self.db.mark_messages_indexed(&assistant_ids).await {
            tracing::error!("Failed to mark as indexed: {}", e);
        }

        if result.indexed_messages > 0 {
            tracing::debug!(
                "Real-time indexing done: {} messages, {} chunks (concurrent embedding)",
                result.indexed_messages,
                result.indexed_chunks
            );
        }

        Ok(result)
    }

    /// 索引指定数量的消息（用于增量索引）
    ///
    /// 优化版本：
    /// 1. 直接从 SQLite 查询未索引的消息
    /// 2. 并发调用 Ollama embedding（10 个同时）
    /// 3. 批量 insert 到 LanceDB（减少版本数）
    pub async fn index_batch(&self, limit: usize) -> Result<IndexResult> {
        let mut result = IndexResult {
            total_messages: 0,
            indexed_messages: 0,
            indexed_chunks: 0,
            skipped: 0,
            failed: 0,
            errors: vec![],
        };

        // 直接获取未索引的消息
        let messages = self.db.get_unindexed_messages(limit).await?;
        result.total_messages = messages.len();

        if messages.is_empty() {
            return Ok(result);
        }

        tracing::debug!(
            "Incremental indexing: found {} unindexed messages",
            messages.len()
        );

        // 1. 收集所有 chunks（带 message_id 信息）
        struct ChunkInfo {
            message_id: i64,
            chunk_index: i64,
            content: String,
        }

        let mut all_chunks: Vec<ChunkInfo> = Vec::new();
        let mut message_chunk_counts: std::collections::HashMap<i64, usize> =
            std::collections::HashMap::new();

        for message in &messages {
            let chunks = self.chunker.chunk(&message.content_text);
            message_chunk_counts.insert(message.id, chunks.len());
            for chunk in chunks {
                all_chunks.push(ChunkInfo {
                    message_id: message.id,
                    chunk_index: chunk.index as i64,
                    content: chunk.content,
                });
            }
        }

        if all_chunks.is_empty() {
            return Ok(result);
        }

        tracing::debug!("Concurrent embedding: {} chunks", all_chunks.len());

        // 2. Concurrent embedding
        let texts: Vec<String> = all_chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = match self.ollama.embed_batch(texts).await {
            Ok(embs) => embs,
            Err(e) => {
                // All failed
                result.errors.push(format!("Batch embedding failed: {}", e));
                let failed_ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
                result.failed = failed_ids.len();
                if let Err(e) = self.db.mark_messages_index_failed(&failed_ids).await {
                    tracing::error!("Failed to mark index failed status: {}", e);
                }
                return Ok(result);
            }
        };

        // 3. 组装 VectorRecord
        let all_records: Vec<VectorRecord> = all_chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| VectorRecord {
                message_id: chunk.message_id,
                chunk_index: chunk.chunk_index,
                content: chunk.content,
                embedding,
            })
            .collect();

        result.indexed_chunks = all_records.len();
        result.indexed_messages = messages.len();
        let indexed_ids: Vec<i64> = messages.iter().map(|m| m.id).collect();

        // 4. Batch insert
        if !all_records.is_empty() {
            let mut vector_store = self.vector.write().await;
            if let Err(e) = vector_store.insert(&all_records).await {
                tracing::error!("Batch insert failed: {}", e);
                // Insert failed, mark all as failed
                result.failed = result.indexed_messages;
                result.indexed_messages = 0;
                result.indexed_chunks = 0;
                if let Err(e) = self.db.mark_messages_index_failed(&indexed_ids).await {
                    tracing::error!("Failed to mark index failed status: {}", e);
                }
                return Ok(result);
            }
        }

        // 5. Mark as indexed
        if let Err(e) = self.db.mark_messages_indexed(&indexed_ids).await {
            tracing::error!("Failed to mark as indexed: {}", e);
        }

        if result.indexed_messages > 0 {
            tracing::info!(
                "Incremental indexing done: {} messages, {} chunks (concurrent embedding + single insert)",
                result.indexed_messages,
                result.indexed_chunks
            );
        }

        Ok(result)
    }

    /// 索引所有待处理的消息（用于定时任务清空增量）
    ///
    /// 与 index_batch 的区别：
    /// - index_batch(100): 固定索引 100 条
    /// - index_pending(3000): 索引所有待处理的消息，但最多 3000 条（防止 Ollama 过载）
    ///
    /// 适用场景：
    /// - 定时任务每小时清空增量
    /// - 新用户首次启动时快速追赶历史数据
    pub async fn index_pending(&self, max_limit: usize) -> Result<IndexResult> {
        let pending_count = self.db.count_unindexed_messages().await? as usize;

        if pending_count == 0 {
            return Ok(IndexResult {
                total_messages: 0,
                indexed_messages: 0,
                indexed_chunks: 0,
                skipped: 0,
                failed: 0,
                errors: vec![],
            });
        }

        // 实际处理数量 = min(待处理数量, 上限)
        let actual_limit = pending_count.min(max_limit);

        if pending_count > max_limit {
            tracing::info!(
                "📊 Pending {} messages, processing {} (max {}), rest in next hour",
                pending_count,
                actual_limit,
                max_limit
            );
        } else {
            tracing::info!("📊 Pending {} messages, will process all", pending_count);
        }

        self.index_batch(actual_limit).await
    }

    /// 获取索引状态统计
    pub async fn get_index_stats(&self) -> Result<IndexStats> {
        let pending = self.db.count_unindexed_messages().await? as usize;
        let failed = self.db.count_failed_indexed_messages().await? as usize;

        let vector_count = {
            let vector_store = self.vector.read().await;
            vector_store.count().await.unwrap_or(0)
        };

        Ok(IndexStats {
            pending,
            failed,
            indexed: vector_count,
        })
    }

    /// 压缩向量数据库（合并文件、清理旧版本）
    pub async fn compact(&self) -> Result<()> {
        let vector_store = self.vector.read().await;
        vector_store.compact().await
    }
}

/// 索引状态统计
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStats {
    /// 待索引数量（vector_indexed = 0）
    pub pending: usize,
    /// 索引失败数量（vector_indexed = -1）
    pub failed: usize,
    /// 已索引数量（LanceDB 中的记录数）
    pub indexed: usize,
}
