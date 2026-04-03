use std::sync::Arc;

use fennec::memory::embedding::NoopEmbedding;
use fennec::memory::snapshot::{export_snapshot, hydrate_from_snapshot};
use fennec::memory::sqlite::SqliteMemory;
use fennec::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use tempfile::TempDir;

fn make_db(dir: &TempDir, name: &str) -> SqliteMemory {
    let db_path = dir.path().join(name);
    let embedder = Arc::new(NoopEmbedding::new(1536));
    SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory")
}

fn make_entry(key: &str, content: &str) -> MemoryEntry {
    MemoryEntry {
        key: key.to_string(),
        content: content.to_string(),
        category: MemoryCategory::Core,
        ..MemoryEntry::default()
    }
}

#[tokio::test]
async fn export_creates_valid_file() {
    let dir = TempDir::new().expect("tempdir");
    let mem = make_db(&dir, "export.db");

    mem.store(make_entry("user-name", "Alice"))
        .await
        .expect("store");
    mem.store(make_entry("fav-language", "Rust"))
        .await
        .expect("store");

    let snap_path = dir.path().join("snapshot.md");
    let count = export_snapshot(&mem, &snap_path).await.expect("export");
    assert_eq!(count, 2);

    let content = tokio::fs::read_to_string(&snap_path)
        .await
        .expect("read snapshot");
    assert!(content.starts_with("# Fennec Memory Snapshot"));
    assert!(content.contains("## user-name"));
    assert!(content.contains("Alice"));
    assert!(content.contains("## fav-language"));
    assert!(content.contains("Rust"));
}

#[tokio::test]
async fn hydrate_restores_entries() {
    let dir = TempDir::new().expect("tempdir");
    let mem = make_db(&dir, "hydrate.db");

    let snap_content = "\
# Fennec Memory Snapshot
Generated: 2026-01-01T00:00:00+00:00

## greeting
Hello world

## project
Fennec AI Agent
";
    let snap_path = dir.path().join("snap.md");
    tokio::fs::write(&snap_path, snap_content)
        .await
        .expect("write snap");

    let count = hydrate_from_snapshot(&mem, &snap_path)
        .await
        .expect("hydrate");
    assert_eq!(count, 2);

    let greeting = mem.get("greeting").await.expect("get").expect("exists");
    assert_eq!(greeting.content, "Hello world");
    assert_eq!(greeting.category, MemoryCategory::Core);

    let project = mem.get("project").await.expect("get").expect("exists");
    assert_eq!(project.content, "Fennec AI Agent");
}

#[tokio::test]
async fn roundtrip_export_then_hydrate() {
    let dir = TempDir::new().expect("tempdir");

    // Populate DB 1
    let mem1 = make_db(&dir, "src.db");
    mem1.store(make_entry("color", "blue")).await.expect("store");
    mem1.store(make_entry("editor", "neovim")).await.expect("store");

    // Export
    let snap_path = dir.path().join("roundtrip.md");
    let exported = export_snapshot(&mem1, &snap_path).await.expect("export");
    assert_eq!(exported, 2);

    // Hydrate into fresh DB
    let mem2 = make_db(&dir, "dst.db");
    let hydrated = hydrate_from_snapshot(&mem2, &snap_path)
        .await
        .expect("hydrate");
    assert_eq!(hydrated, 2);

    // Verify
    let color = mem2.get("color").await.expect("get").expect("exists");
    assert_eq!(color.content, "blue");

    let editor = mem2.get("editor").await.expect("get").expect("exists");
    assert_eq!(editor.content, "neovim");
}
