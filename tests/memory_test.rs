use std::sync::Arc;

use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::embedding::NoopEmbedding;
use tempfile::TempDir;

fn make_db() -> (TempDir, SqliteMemory) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");
    (dir, mem)
}

fn make_entry(key: &str, content: &str, category: MemoryCategory) -> MemoryEntry {
    MemoryEntry {
        key: key.to_string(),
        content: content.to_string(),
        category,
        ..MemoryEntry::default()
    }
}

#[tokio::test]
async fn store_and_get() {
    let (_dir, mem) = make_db();
    let entry = make_entry("greeting", "Hello world", MemoryCategory::Core);
    mem.store(entry).await.expect("store");

    let retrieved = mem.get("greeting").await.expect("get");
    assert!(retrieved.is_some());
    let r = retrieved.unwrap();
    assert_eq!(r.key, "greeting");
    assert_eq!(r.content, "Hello world");
    assert_eq!(r.category, MemoryCategory::Core);
}

#[tokio::test]
async fn store_and_recall_by_keyword() {
    let (_dir, mem) = make_db();
    mem.store(make_entry("rust-lang", "Rust is a systems programming language", MemoryCategory::Core))
        .await
        .expect("store");
    mem.store(make_entry("python-lang", "Python is a scripting language", MemoryCategory::Core))
        .await
        .expect("store");

    let results = mem.recall("Rust programming", 10).await.expect("recall");
    assert!(!results.is_empty());
    // The Rust entry should be among the results.
    assert!(results.iter().any(|e| e.key == "rust-lang"));
}

#[tokio::test]
async fn forget() {
    let (_dir, mem) = make_db();
    mem.store(make_entry("temp", "temporary note", MemoryCategory::Daily))
        .await
        .expect("store");

    let deleted = mem.forget("temp").await.expect("forget");
    assert!(deleted);

    let gone = mem.get("temp").await.expect("get");
    assert!(gone.is_none());

    // Forgetting a non-existent key returns false.
    let deleted_again = mem.forget("temp").await.expect("forget again");
    assert!(!deleted_again);
}

#[tokio::test]
async fn count() {
    let (_dir, mem) = make_db();
    assert_eq!(mem.count(None).await.expect("count"), 0);

    mem.store(make_entry("a", "alpha", MemoryCategory::Core))
        .await
        .expect("store");
    mem.store(make_entry("b", "beta", MemoryCategory::Daily))
        .await
        .expect("store");
    mem.store(make_entry("c", "gamma", MemoryCategory::Core))
        .await
        .expect("store");

    assert_eq!(mem.count(None).await.expect("count all"), 3);
    assert_eq!(mem.count(Some(&MemoryCategory::Core)).await.expect("count core"), 2);
    assert_eq!(mem.count(Some(&MemoryCategory::Daily)).await.expect("count daily"), 1);
    assert_eq!(
        mem.count(Some(&MemoryCategory::Conversation)).await.expect("count convo"),
        0
    );
}

#[tokio::test]
async fn list_by_category() {
    let (_dir, mem) = make_db();
    mem.store(make_entry("a", "alpha", MemoryCategory::Core))
        .await
        .expect("store");
    mem.store(make_entry("b", "beta", MemoryCategory::Daily))
        .await
        .expect("store");
    mem.store(make_entry("c", "gamma", MemoryCategory::Core))
        .await
        .expect("store");

    let all = mem.list(None, 100).await.expect("list all");
    assert_eq!(all.len(), 3);

    let core = mem.list(Some(&MemoryCategory::Core), 100).await.expect("list core");
    assert_eq!(core.len(), 2);
    assert!(core.iter().all(|e| e.category == MemoryCategory::Core));

    let daily = mem.list(Some(&MemoryCategory::Daily), 100).await.expect("list daily");
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].key, "b");
}

#[tokio::test]
async fn upsert_on_duplicate_key() {
    let (_dir, mem) = make_db();
    mem.store(make_entry("ukey", "original content", MemoryCategory::Core))
        .await
        .expect("store");

    // Store again with same key but different content.
    mem.store(make_entry("ukey", "updated content", MemoryCategory::Daily))
        .await
        .expect("upsert");

    let entry = mem.get("ukey").await.expect("get").expect("entry exists");
    assert_eq!(entry.content, "updated content");
    assert_eq!(entry.category, MemoryCategory::Daily);

    // Should still be a single entry.
    assert_eq!(mem.count(None).await.expect("count"), 1);
}

#[tokio::test]
async fn health_check() {
    let (_dir, mem) = make_db();
    mem.health_check().await.expect("health_check should succeed");
}
