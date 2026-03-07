//! One-time cleanup: compact_vectors LanceDB version pruning
//!
//! cargo run --example compact_vector_cleanup

use anyhow::Result;
use memex::compact::CompactVectorStore;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,memex=debug")
        .init();

    let path = dirs::home_dir()
        .unwrap()
        .join(".vimo/db/lancedb/compact");

    println!("Opening compact vector store: {:?}", path);

    let store = CompactVectorStore::open(&path).await?;

    let count = store.count().await?;
    println!("Records in compact_vectors: {}", count);

    println!("Starting compaction (this may take a while)...");
    store.compact().await?;

    println!("Done! Check disk usage with: du -sh ~/.vimo/db/lancedb/compact/");
    Ok(())
}
