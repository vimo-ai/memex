//! Compact 向量存储 - LanceDB 集成
//!
//! 存储 L1/L2/L3 摘要的向量索引，与原文向量表分离

use anyhow::{Context, Result};
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder, Int32Builder},
    Array, Float32Array, Int32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, DistanceType, Table};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use super::config::VectorDistanceType;

/// 向量维度 (bge-m3 默认 1024)
const EMBEDDING_DIM: usize = 1024;

/// Compact 层级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactLevel {
    /// L1: Observations
    L1,
    /// L2: Talk Summaries
    L2,
    /// L3: Session Summaries
    L3,
}

impl CompactLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompactLevel::L1 => "l1",
            CompactLevel::L2 => "l2",
            CompactLevel::L3 => "l3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "l1" => Some(CompactLevel::L1),
            "l2" => Some(CompactLevel::L2),
            "l3" => Some(CompactLevel::L3),
            _ => None,
        }
    }
}

/// Compact 向量记录
#[derive(Debug, Clone)]
pub struct CompactVectorRecord {
    /// 唯一 ID
    pub id: String,
    /// 会话 ID
    pub session_id: String,
    /// 层级 (l1/l2/l3)
    pub level: CompactLevel,
    /// 源记录 ID（observation/talk_summary/session_summary 的 id）
    pub source_id: String,
    /// prompt 编号（L1/L2 有，L3 为 None）
    pub prompt_number: Option<i32>,
    /// 向量化的文本内容
    pub text: String,
    /// 创建时间 (RFC3339)
    pub created_at: String,
    /// 向量
    pub embedding: Vec<f32>,
}

/// Compact 向量搜索结果
#[derive(Debug, Clone, Serialize)]
pub struct CompactVectorSearchResult {
    /// 唯一 ID
    pub id: String,
    /// 会话 ID
    pub session_id: String,
    /// 层级
    pub level: String,
    /// 源记录 ID
    pub source_id: String,
    /// prompt 编号
    pub prompt_number: Option<i32>,
    /// 文本内容
    pub text: String,
    /// 创建时间 (RFC3339)，老数据可能为 None
    pub created_at: Option<String>,
    /// 距离（越小越相似）
    pub distance: f32,
}

/// Compact 向量存储
pub struct CompactVectorStore {
    db: Connection,
    table: Option<Table>,
}

impl CompactVectorStore {
    /// 打开向量存储
    pub async fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;

        let db = connect(path.to_str().unwrap())
            .execute()
            .await
            .context("无法连接 LanceDB (compact)")?;

        let mut store = Self { db, table: None };

        // 尝试打开已存在的表
        match store.db.open_table("compact_vectors").execute().await {
            Ok(table) => {
                store.table = Some(table);
                tracing::info!("LanceDB compact_vectors table opened");
            }
            Err(_) => {
                tracing::info!(
                    "LanceDB compact_vectors table not found, will create on first insert"
                );
            }
        }

        Ok(store)
    }

    /// 创建表 Schema
    fn create_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("session_id", DataType::Utf8, false),
            Field::new("level", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, false),
            Field::new("prompt_number", DataType::Int32, true), // nullable
            Field::new("text", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
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
        let batch = Self::create_empty_batch(schema.clone())?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        let table = self
            .db
            .create_table("compact_vectors", Box::new(batches))
            .execute()
            .await
            .context("创建 compact_vectors 表失败")?;

        self.table = Some(table);
        tracing::info!("LanceDB compact_vectors table created");

        Ok(())
    }

    /// 创建空批次
    fn create_empty_batch(schema: Arc<Schema>) -> Result<RecordBatch> {
        let ids = StringArray::from(Vec::<String>::new());
        let session_ids = StringArray::from(Vec::<String>::new());
        let levels = StringArray::from(Vec::<String>::new());
        let source_ids = StringArray::from(Vec::<String>::new());
        let prompt_numbers = Int32Array::from(Vec::<Option<i32>>::new());
        let texts = StringArray::from(Vec::<String>::new());
        let created_ats = StringArray::from(Vec::<String>::new());
        let vectors = Self::create_empty_vector_array();

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(ids),
                Arc::new(session_ids),
                Arc::new(levels),
                Arc::new(source_ids),
                Arc::new(prompt_numbers),
                Arc::new(texts),
                Arc::new(created_ats),
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
    pub async fn insert(&mut self, records: &[CompactVectorRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }

        self.ensure_table().await?;

        let table = self.table.as_ref().unwrap();
        let schema = Self::create_schema();

        // 构建各列数据
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        let session_ids: Vec<&str> = records.iter().map(|r| r.session_id.as_str()).collect();
        let levels: Vec<&str> = records.iter().map(|r| r.level.as_str()).collect();
        let source_ids: Vec<&str> = records.iter().map(|r| r.source_id.as_str()).collect();
        let prompt_numbers: Vec<Option<i32>> = records.iter().map(|r| r.prompt_number).collect();
        let texts: Vec<&str> = records.iter().map(|r| r.text.as_str()).collect();
        let created_ats: Vec<&str> = records.iter().map(|r| r.created_at.as_str()).collect();

        // 构建向量数组
        let vectors =
            Self::create_vector_array(&records.iter().map(|r| &r.embedding).collect::<Vec<_>>());

        // 构建 prompt_number 数组（nullable）
        let mut prompt_builder = Int32Builder::new();
        for pn in &prompt_numbers {
            match pn {
                Some(n) => prompt_builder.append_value(*n),
                None => prompt_builder.append_null(),
            }
        }

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(session_ids)),
                Arc::new(StringArray::from(levels)),
                Arc::new(StringArray::from(source_ids)),
                Arc::new(prompt_builder.finish()),
                Arc::new(StringArray::from(texts)),
                Arc::new(StringArray::from(created_ats)),
                Arc::new(vectors),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        table
            .add(Box::new(batches))
            .execute()
            .await
            .context("插入 compact 向量失败")?;

        Ok(records.len())
    }

    /// 向量搜索
    ///
    /// - `level`: None 表示搜索所有层级，Some 表示只搜索指定层级
    pub async fn search(
        &self,
        query_vector: &[f32],
        level: Option<CompactLevel>,
        limit: usize,
    ) -> Result<Vec<CompactVectorSearchResult>> {
        self.search_with_distance_type(query_vector, level, limit, VectorDistanceType::Cosine)
            .await
    }

    /// 向量搜索（指定距离类型）
    ///
    /// - `level`: None 表示搜索所有层级，Some 表示只搜索指定层级
    /// - `distance_type`: 距离类型（默认 Cosine）
    pub async fn search_with_distance_type(
        &self,
        query_vector: &[f32],
        level: Option<CompactLevel>,
        limit: usize,
        distance_type: VectorDistanceType,
    ) -> Result<Vec<CompactVectorSearchResult>> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let lance_distance = match distance_type {
            VectorDistanceType::Cosine => DistanceType::Cosine,
            VectorDistanceType::Euclidean => DistanceType::L2,
            VectorDistanceType::Dot => DistanceType::Dot,
        };

        // 构建查询
        let mut query = table
            .vector_search(query_vector.to_vec())
            .context("向量搜索失败")?
            .distance_type(lance_distance);

        // 添加层级过滤
        if let Some(l) = level {
            query = query.only_if(format!("level = '{}'", l.as_str()));
        }

        let results = query
            .limit(limit)
            .execute()
            .await
            .context("执行向量搜索失败")?;

        let mut search_results = Vec::new();

        use futures::TryStreamExt;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        for batch in batches {
            let ids = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let session_ids = batch
                .column_by_name("session_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let levels = batch
                .column_by_name("level")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let source_ids = batch
                .column_by_name("source_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let prompt_numbers = batch
                .column_by_name("prompt_number")
                .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
            let texts = batch
                .column_by_name("text")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            // created_at 是可选的，兼容老数据
            let created_ats = batch
                .column_by_name("created_at")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let distances = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            // 核心字段必须存在，created_at 可选
            if let (
                Some(ids),
                Some(sids),
                Some(lvls),
                Some(src_ids),
                Some(pns),
                Some(txts),
                Some(dists),
            ) = (
                ids,
                session_ids,
                levels,
                source_ids,
                prompt_numbers,
                texts,
                distances,
            ) {
                for i in 0..batch.num_rows() {
                    search_results.push(CompactVectorSearchResult {
                        id: ids.value(i).to_string(),
                        session_id: sids.value(i).to_string(),
                        level: lvls.value(i).to_string(),
                        source_id: src_ids.value(i).to_string(),
                        prompt_number: if pns.is_null(i) {
                            None
                        } else {
                            Some(pns.value(i))
                        },
                        text: txts.value(i).to_string(),
                        created_at: created_ats.map(|cats| cats.value(i).to_string()),
                        distance: dists.value(i),
                    });
                }
            }
        }

        Ok(search_results)
    }

    /// 删除指定源记录的向量
    pub async fn delete_by_source_id(&mut self, source_id: &str) -> Result<()> {
        if let Some(table) = &self.table {
            table
                .delete(&format!("source_id = '{}'", source_id))
                .await
                .context("删除 compact 向量失败")?;
        }
        Ok(())
    }

    /// 删除指定会话的所有向量
    pub async fn delete_by_session(&mut self, session_id: &str) -> Result<()> {
        if let Some(table) = &self.table {
            table
                .delete(&format!("session_id = '{}'", session_id))
                .await
                .context("删除会话 compact 向量失败")?;
        }
        Ok(())
    }

    /// 获取记录数量
    pub async fn count(&self) -> Result<usize> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(0),
        };

        Ok(table.count_rows(None).await?)
    }

    /// 按层级统计数量
    pub async fn count_by_level(&self, level: CompactLevel) -> Result<usize> {
        let table = match &self.table {
            Some(t) => t,
            None => return Ok(0),
        };

        Ok(table
            .count_rows(Some(format!("level = '{}'", level.as_str())))
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_compact_vector_store() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_compact_vectors");

        let mut store = CompactVectorStore::open(&db_path).await.unwrap();

        // 插入测试数据
        let records = vec![
            CompactVectorRecord {
                id: "vec-1".to_string(),
                session_id: "session-1".to_string(),
                level: CompactLevel::L1,
                source_id: "obs-1".to_string(),
                prompt_number: Some(1),
                text: "Fixed a bug in user authentication".to_string(),
                created_at: "2024-01-01T10:00:00Z".to_string(),
                embedding: vec![0.1; EMBEDDING_DIM],
            },
            CompactVectorRecord {
                id: "vec-2".to_string(),
                session_id: "session-1".to_string(),
                level: CompactLevel::L2,
                source_id: "talk-1".to_string(),
                prompt_number: Some(1),
                text: "Implemented login flow with OAuth".to_string(),
                created_at: "2024-01-01T11:00:00Z".to_string(),
                embedding: vec![0.2; EMBEDDING_DIM],
            },
            CompactVectorRecord {
                id: "vec-3".to_string(),
                session_id: "session-1".to_string(),
                level: CompactLevel::L3,
                source_id: "summary-1".to_string(),
                prompt_number: None,
                text: "Complete authentication system implementation".to_string(),
                created_at: "2024-01-01T12:00:00Z".to_string(),
                embedding: vec![0.3; EMBEDDING_DIM],
            },
        ];

        let inserted = store.insert(&records).await.unwrap();
        assert_eq!(inserted, 3);

        // 测试总数量
        let count = store.count().await.unwrap();
        assert_eq!(count, 3);

        // 测试按层级统计
        let l1_count = store.count_by_level(CompactLevel::L1).await.unwrap();
        assert_eq!(l1_count, 1);

        let l2_count = store.count_by_level(CompactLevel::L2).await.unwrap();
        assert_eq!(l2_count, 1);

        // 测试搜索（所有层级）
        let query = vec![0.15; EMBEDDING_DIM];
        let results = store.search(&query, None, 10).await.unwrap();
        assert_eq!(results.len(), 3);

        // 测试搜索（只搜 L1）
        let results = store
            .search(&query, Some(CompactLevel::L1), 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].level, "l1");

        // 测试 created_at 正确返回
        assert_eq!(
            results[0].created_at,
            Some("2024-01-01T10:00:00Z".to_string())
        );

        // 测试搜索所有层级时 created_at 都正确
        let results = store.search(&query, None, 10).await.unwrap();
        let created_ats: Vec<Option<String>> =
            results.iter().map(|r| r.created_at.clone()).collect();
        assert!(created_ats.contains(&Some("2024-01-01T10:00:00Z".to_string()))); // L1
        assert!(created_ats.contains(&Some("2024-01-01T11:00:00Z".to_string()))); // L2
        assert!(created_ats.contains(&Some("2024-01-01T12:00:00Z".to_string())));
        // L3
    }

    #[test]
    fn test_compact_level() {
        assert_eq!(CompactLevel::L1.as_str(), "l1");
        assert_eq!(CompactLevel::parse("L2"), Some(CompactLevel::L2));
        assert_eq!(CompactLevel::parse("invalid"), None);
    }
}
