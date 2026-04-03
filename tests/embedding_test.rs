use std::sync::Arc;

use fennec::memory::embedding::{EmbeddingProvider, NoopEmbedding};
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use tempfile::TempDir;

fn make_entry(key: &str, content: &str, category: MemoryCategory) -> MemoryEntry {
    MemoryEntry {
        key: key.to_string(),
        content: content.to_string(),
        category,
        ..MemoryEntry::default()
    }
}

#[tokio::test]
async fn noop_embedding_returns_correct_dimensions() {
    let noop = NoopEmbedding::new(1536);
    assert_eq!(noop.name(), "noop");
    assert_eq!(noop.dimensions(), 1536);

    let vec = noop.embed("hello world").await.expect("embed");
    assert_eq!(vec.len(), 1536);
    assert!(vec.iter().all(|&v| v == 0.0));
}

#[tokio::test]
async fn noop_embedding_custom_dimensions() {
    let noop = NoopEmbedding::new(768);
    assert_eq!(noop.dimensions(), 768);

    let vec = noop.embed("test").await.expect("embed");
    assert_eq!(vec.len(), 768);
}

#[tokio::test]
async fn noop_embed_batch() {
    let noop = NoopEmbedding::new(384);
    let results = noop.embed_batch(&["one", "two", "three"]).await.expect("embed_batch");
    assert_eq!(results.len(), 3);
    for vec in &results {
        assert_eq!(vec.len(), 384);
        assert!(vec.iter().all(|&v| v == 0.0));
    }
}

#[tokio::test]
async fn sqlite_with_noop_embedding_keyword_recall() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");

    // Store entries.
    mem.store(make_entry(
        "rust-info",
        "Rust is a systems programming language focused on safety",
        MemoryCategory::Core,
    ))
    .await
    .expect("store");

    mem.store(make_entry(
        "python-info",
        "Python is a high-level scripting language",
        MemoryCategory::Core,
    ))
    .await
    .expect("store");

    // Keyword recall should still work with noop embedder.
    let results = mem.recall("Rust programming", 10).await.expect("recall");
    assert!(!results.is_empty());
    assert!(results.iter().any(|e| e.key == "rust-info"));
}

#[tokio::test]
async fn sqlite_with_noop_embedding_store_and_get() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");

    let entry = make_entry("key1", "some content", MemoryCategory::Daily);
    mem.store(entry).await.expect("store");

    let retrieved = mem.get("key1").await.expect("get");
    assert!(retrieved.is_some());
    let r = retrieved.unwrap();
    assert_eq!(r.key, "key1");
    assert_eq!(r.content, "some content");
}
