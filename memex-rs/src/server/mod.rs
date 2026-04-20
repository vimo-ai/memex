//! Server 模式 - 接收客户端 push，提供跨人搜索
//!
//! server 不跑 LLM、不跑 embedding，纯存储 + 索引 + 查询。
//! 算力由各客户端本地完成后推送结果。

mod ingest;

pub use ingest::{create_sync_router, IngestState};
