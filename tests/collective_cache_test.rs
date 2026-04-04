use std::sync::Arc;

use fennec::collective::cache::CollectiveCache;
use fennec::collective::traits::{CollectiveSearchResult, OutcomeReports};
use fennec::memory::embedding::NoopEmbedding;
use fennec::memory::sqlite::SqliteMemory;
use tempfile::TempDir;

fn make_db() -> (TempDir, Arc<SqliteMemory>) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");
    (dir, Arc::new(mem))
}

fn make_result(id: &str, goal: &str, solution: Option<&str>) -> CollectiveSearchResult {
    CollectiveSearchResult {
        id: id.to_string(),
        goal: goal.to_string(),
        solution: solution.map(|s| s.to_string()),
        gotchas: vec!["watch out for edge cases".to_string()],
        trust_score: 0.7,
        relevance_score: 0.8,
        outcome_reports: OutcomeReports {
            success: 5,
            failure: 1,
        },
    }
}

#[tokio::test]
async fn cache_result_and_search_finds_it() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result = make_result("remote-1", "Fix Rust borrow checker error", Some("Use clone"));
    cache.cache_result(&result, "plurum").await.unwrap();

    let found = cache.search_cache("borrow checker", 10).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "remote-1");
    assert_eq!(found[0].goal, "Fix Rust borrow checker error");
    assert_eq!(found[0].solution.as_deref(), Some("Use clone"));
    assert_eq!(found[0].gotchas, vec!["watch out for edge cases"]);
    assert!((found[0].trust_score - 0.7).abs() < 0.01);
    assert_eq!(found[0].outcome_reports.success, 5);
    assert_eq!(found[0].outcome_reports.failure, 1);
}

#[tokio::test]
async fn cache_deduplicates_by_original_id() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result1 = CollectiveSearchResult {
        id: "remote-1".to_string(),
        goal: "Fix Rust lifetime error".to_string(),
        solution: Some("Use Arc".to_string()),
        gotchas: vec![],
        trust_score: 0.5,
        relevance_score: 0.6,
        outcome_reports: OutcomeReports {
            success: 1,
            failure: 0,
        },
    };
    cache.cache_result(&result1, "plurum").await.unwrap();

    // Cache again with same original_id but updated fields
    let result2 = CollectiveSearchResult {
        id: "remote-1".to_string(),
        goal: "Fix Rust lifetime error".to_string(),
        solution: Some("Use Rc instead".to_string()),
        gotchas: vec!["only for single-threaded".to_string()],
        trust_score: 0.9,
        relevance_score: 0.85,
        outcome_reports: OutcomeReports {
            success: 10,
            failure: 0,
        },
    };
    cache.cache_result(&result2, "plurum").await.unwrap();

    let found = cache.search_cache("lifetime", 10).await.unwrap();
    assert_eq!(found.len(), 1, "should have exactly one entry after dedup");
    assert_eq!(found[0].solution.as_deref(), Some("Use Rc instead"));
    assert!((found[0].trust_score - 0.9).abs() < 0.01);
    assert_eq!(found[0].outcome_reports.success, 10);
}

#[tokio::test]
async fn mark_used_updates_timestamp() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result = make_result("remote-2", "Deploy to production safely", Some("Use blue-green"));
    cache.cache_result(&result, "plurum").await.unwrap();

    // Initially last_used is NULL
    {
        let conn = mem.connection();
        let conn = conn.lock();
        let last_used: Option<String> = conn
            .query_row(
                "SELECT last_used FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["remote-2"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_none(), "last_used should be NULL initially");
    }

    cache.mark_used("remote-2").await.unwrap();

    // Now last_used should be set
    {
        let conn = mem.connection();
        let conn = conn.lock();
        let last_used: Option<String> = conn
            .query_row(
                "SELECT last_used FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["remote-2"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_some(), "last_used should be set after mark_used");
    }
}

#[tokio::test]
async fn decay_reduces_trust_score() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result = make_result("remote-3", "Handle database migrations", Some("Use alembic"));
    cache.cache_result(&result, "plurum").await.unwrap();

    // The entry has no last_used, so decay with 0 days should affect it
    let affected = cache.decay_stale(0).await.unwrap();
    assert_eq!(affected, 1);

    let found = cache.search_cache("database migrations", 10).await.unwrap();
    assert_eq!(found.len(), 1);
    // Original trust was 0.7, after one decay: 0.7 * 0.9 = 0.63
    assert!(
        (found[0].trust_score - 0.63).abs() < 0.01,
        "trust_score should be ~0.63, got {}",
        found[0].trust_score
    );

    // Decay again
    cache.decay_stale(0).await.unwrap();
    let found = cache.search_cache("database migrations", 10).await.unwrap();
    // 0.63 * 0.9 = 0.567
    assert!(
        (found[0].trust_score - 0.567).abs() < 0.01,
        "trust_score should be ~0.567, got {}",
        found[0].trust_score
    );
}

#[tokio::test]
async fn evict_removes_low_trust_entries() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let low_trust = CollectiveSearchResult {
        id: "remote-low".to_string(),
        goal: "Fix obscure webpack error".to_string(),
        solution: Some("Delete node_modules".to_string()),
        gotchas: vec![],
        trust_score: 0.1,
        relevance_score: 0.5,
        outcome_reports: OutcomeReports {
            success: 0,
            failure: 3,
        },
    };
    cache.cache_result(&low_trust, "plurum").await.unwrap();

    let high_trust = CollectiveSearchResult {
        id: "remote-high".to_string(),
        goal: "Fix webpack chunking configuration".to_string(),
        solution: Some("Use splitChunks".to_string()),
        gotchas: vec![],
        trust_score: 0.9,
        relevance_score: 0.8,
        outcome_reports: OutcomeReports {
            success: 20,
            failure: 0,
        },
    };
    cache.cache_result(&high_trust, "plurum").await.unwrap();

    let evicted = cache.evict_low_trust(0.3).await.unwrap();
    assert_eq!(evicted, 1, "should evict exactly one low-trust entry");

    let found = cache.search_cache("webpack", 10).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "remote-high");
}

#[tokio::test]
async fn search_empty_query_returns_nothing() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result = make_result("remote-4", "Something interesting", Some("Do stuff"));
    cache.cache_result(&result, "plurum").await.unwrap();

    let found = cache.search_cache("", 10).await.unwrap();
    assert!(found.is_empty());
}

#[tokio::test]
async fn search_no_match_returns_empty() {
    let (_dir, mem) = make_db();
    let cache = CollectiveCache::new(Arc::clone(&mem));

    let result = make_result("remote-5", "Fix Rust borrow checker", Some("Use clone"));
    cache.cache_result(&result, "plurum").await.unwrap();

    let found = cache.search_cache("python decorator", 10).await.unwrap();
    assert!(found.is_empty());
}
