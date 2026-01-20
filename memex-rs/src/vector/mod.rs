//! 向量存储 - LanceDB 集成

#![allow(dead_code)] // 预留 API: delete

use anyhow::{Context, Result};
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder},
    Array, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, DistanceType, Table};
use std::path::Path;
use std::sync::Arc;

use crate::compact::VectorDistanceType;

/// 向量维度 (bge-m3 默认 1024)
const EMBEDDING_DIM: usize = 1024;

/// 向量存储
pub struct VectorStore {
    db: Connection,
    table: Option<Table>,
}

/// 向量记录
#[derive(Debug, Clone)]
pub struct VectorRecord {
    pub message_id: i64,
    pub chunk_index: i64,
    pub content: String,
    pub embedding: Vec<f32>,
}

/// 向量搜索结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct VectorSearchResult {
    pub message_id: i64,
    pub chunk_index: i64,
    pub content: String,
    pub distance: f32,
}

impl VectorStore {
    /// 打开向量存储
    pub async fn open(path: &Path) -> Result<Self> {
        // 确保目录存在
        std::fs::create_dir_all(path)?;

        let db = connect(path.to_str().unwrap())
            .execute()
            .await
            .context("无法连接 LanceDB")?;

        let mut store = Self { db, table: None };

        // Try to open existing table
        match store.db.open_table("embeddings").execute().await {
            Ok(table) => {
                store.table = Some(table);
                tracing::info!("LanceDB table opened");
            }
            Err(_) => {
                tracing::info!("LanceDB table not found, will create on first insert");
            }
        }

        Ok(store)
    }

    /// 创建表 Schema
    fn create_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("message_id", DataType::Int64, false),
            Field::new("chunk_index", DataType::Int64, false),
            Field::new("content", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    EMBEDDING_DIM as i32,
                ),
                false,
            ),
        ]))
    }

    /// 初始化表（如果不存在）
    pub async fn ensure_table(&mut self) -> Result<()> {
        if self.table.is_some() {
            return Ok(());
        }

        let schema = Self::create_schema();

        // 创建空的初始批次
        let batch = Self::create_empty_batch(schema.clone())?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        let table = self
            .db
            .create_table("embeddings", Box::new(batches))
            .execute()
            .await
            .context("创建 LanceDB 表失败")?;

        self.table = Some(table);
        tracing::info!("LanceDB table created");

        Ok(())
    }

    /// 创建空批次
    fn create_empty_batch(schema: Arc<Schema>) -> Result<RecordBatch> {
        let message_ids = Int64Array::from(Vec::<i64>::new());
        let chunk_indices = Int64Array::from(Vec::<i64>::new());
        let contents = StringArray::from(Vec::<String>::new());
        let vectors = Self::create_empty_vector_array();

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(message_ids),
                Arc::new(chunk_indices),
                Arc::new(contents),
                Arc::new(vectors),
            ],
        )
        .context("创建空 RecordBatch 失败")
    }

    /// 创建空的向量数组
    fn create_empty_vector_array() -> arrow_array::FixedSizeListArray {
        let values_builder = Float32Builder::new();
        let mut builder = FixedSizeListBuilder::new(values_builder, EMBEDDING_DIM as i32);
        builder.finish()
    }

    /// 创建向量数组
    fn create_vector_array(vectors: &[&Vec<f32>]) -> arrow_array::FixedSizeListArray {
        let values_builder = Float32Builder::with_capacity(vectors.len() * EMBEDDING_DIM);
        let mut builder = FixedSizeListBuilder::new(values_builder, EMBEDDING_DIM as i32);

        for vector in vectors {
            let values = builder.values();
            for &v in vector.iter() {
                values.append_value(v);
            }
            builder.append(true);
        }

        builder.finish()
    }

    /// 插入向量记录
    pub async fn insert(&mut self, records: &[VectorRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }

        self.ensure_table().await?;

        let table = self.table.as_ref().unwrap();
        let schema = Self::create_schema();

        let message_ids: Vec<i64> = records.iter().map(|r| r.message_id).collect();
        let chunk_indices: Vec<i64> = records.iter().map(|r| r.chunk_index).collect();
        let contents: Vec<&str> = records.iter().map(|r| r.content.as_str()).collect();

        // 构建向量数组
        let vectors =
            Self::create_vector_array(&records.iter().map(|r| &r.embedding).collect::<Vec<_>>());

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(message_ids)),
                Arc::new(Int64Array::from(chunk_indices)),
                Arc::new(StringArray::from(contents)),
                Arc::new(vectors),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        table
            .add(Box::new(batches))
            .execute()
            .await
            .context("插入向量失败")?;

        Ok(records.len())
    }

    /// 向量搜索
    ///
    /// - `distance_type`: 距离类型（默认 Cosine）
    pub async fn search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>> {
        self.search_with_distance_type(query_vector, limit, VectorDistanceType::Cosine)
            .await
    }

    /// 向量搜索（指定距离类型）
    pub async fn search_with_distance_type(
        &self,
        query_vector: &[f32],
        limit: usize,
        distance_type: VectorDistanceType,
    ) -> Result<Vec<VectorSearchResult>> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let lance_distance = match distance_type {
            VectorDistanceType::Cosine => DistanceType::Cosine,
            VectorDistanceType::Euclidean => DistanceType::L2,
            VectorDistanceType::Dot => DistanceType::Dot,
        };

        let results = table
            .vector_search(query_vector.to_vec())
            .context("向量搜索失败")?
            .distance_type(lance_distance)
            .limit(limit)
            .execute()
            .await
            .context("执行向量搜索失败")?;

        let mut search_results = Vec::new();

        use futures::TryStreamExt;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        for batch in batches {
            let message_ids = batch
                .column_by_name("message_id")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>());
            let chunk_indices = batch
                .column_by_name("chunk_index")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>());
            let contents = batch
                .column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let distances = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            if let (Some(ids), Some(indices), Some(conts), Some(dists)) =
                (message_ids, chunk_indices, contents, distances)
            {
                for i in 0..batch.num_rows() {
                    search_results.push(VectorSearchResult {
                        message_id: ids.value(i),
                        chunk_index: indices.value(i),
                        content: conts.value(i).to_string(),
                        distance: dists.value(i),
                    });
                }
            }
        }

        Ok(search_results)
    }

    /// 检查消息是否已索引（已弃用）
    ///
    /// 此方法效率低下（每次调用都查询 LanceDB），
    /// 已改用 SQLite 的 vector_indexed 字段来跟踪索引状态。
    #[deprecated(note = "使用 SQLite 的 vector_indexed 字段代替")]
    #[allow(dead_code)]
    pub async fn is_indexed(&self, message_id: i64) -> Result<bool> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(false),
        };

        use futures::TryStreamExt;

        let results = table
            .query()
            .only_if(format!("message_id = {}", message_id))
            .limit(1)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        Ok(batches.iter().any(|b| b.num_rows() > 0))
    }

    /// 获取已索引的 chunk 数量
    pub async fn count(&self) -> Result<usize> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(0),
        };

        // 使用 count_rows() 获取准确数量，避免 query() 的默认 limit
        Ok(table.count_rows(None).await?)
    }

    /// 删除消息的向量
    pub async fn delete(&mut self, message_id: i64) -> Result<()> {
        if let Some(table) = &self.table {
            table
                .delete(&format!("message_id = {}", message_id))
                .await
                .context("删除向量失败")?;
        }
        Ok(())
    }

    /// 获取所有已索引的 message_id（用于同步 SQLite 状态）
    pub async fn get_all_indexed_message_ids(&self) -> Result<Vec<i64>> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        use futures::TryStreamExt;
        use std::collections::HashSet;

        let results = table
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "message_id".to_string()
            ]))
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut ids = HashSet::new();
        for batch in batches {
            if let Some(col) = batch
                .column_by_name("message_id")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            {
                for i in 0..col.len() {
                    ids.insert(col.value(i));
                }
            }
        }

        Ok(ids.into_iter().collect())
    }

    /// 压缩数据库（合并小文件、清理旧版本）
    ///
    /// 执行两个操作：
    /// 1. compact_files: 合并小文件片段
    /// 2. cleanup_old_versions: 清理 7 天前的旧版本
    pub async fn compact(&self) -> Result<()> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(()),
        };

        use lancedb::table::OptimizeAction;

        // 1. Merge small file fragments
        tracing::info!("🔧 Starting LanceDB data file compaction...");
        table
            .optimize(OptimizeAction::Compact {
                options: Default::default(),
                remap_options: None,
            })
            .await
            .context("compact_files failed")?;

        // 2. Cleanup all old versions (memex doesn't need version history)
        tracing::info!("🧹 Cleaning up old versions...");
        table
            .optimize(OptimizeAction::Prune {
                older_than: Some(chrono::TimeDelta::zero()), // Keep no old versions
                delete_unverified: Some(true),
                error_if_tagged_old_versions: None,
            })
            .await
            .context("cleanup failed")?;

        tracing::info!("✅ LanceDB compaction done");
        Ok(())
    }
}
