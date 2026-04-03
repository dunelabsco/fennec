use std::sync::Arc;

use fennec::memory::experience::{Attempt, Experience, ExperienceContext};
use fennec::memory::embedding::NoopEmbedding;
use fennec::memory::sqlite::SqliteMemory;
use tempfile::TempDir;

fn make_db() -> (TempDir, SqliteMemory) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let embedder = Arc::new(NoopEmbedding::new(1536));
    let mem = SqliteMemory::new(db_path, 0.7, 0.3, 10_000, embedder).expect("new sqlite memory");
    (dir, mem)
}

fn make_experience(id: &str, goal: &str, solution: Option<&str>) -> Experience {
    Experience {
        id: id.to_string(),
        goal: goal.to_string(),
        context: ExperienceContext {
            tools_used: vec!["shell".to_string(), "read_file".to_string()],
            environment: "linux".to_string(),
            constraints: "no sudo".to_string(),
        },
        attempts: vec![
            Attempt {
                action: "tried rm".to_string(),
                outcome: "permission denied".to_string(),
                dead_end: true,
                insight: "need elevated permissions".to_string(),
            },
        ],
        solution: solution.map(|s| s.to_string()),
        gotchas: vec!["watch out for symlinks".to_string()],
        tags: vec!["filesystem".to_string(), "permissions".to_string()],
        confidence: 0.85,
        session_id: Some("sess-1".to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[tokio::test]
async fn store_and_search_experience() {
    let (_dir, mem) = make_db();

    let exp = make_experience("exp-1", "Delete temporary build files safely", Some("use find with -delete flag"));
    mem.store_experience(&exp).await.expect("store experience");

    let results = mem.search_experiences("build files", 10).await.expect("search");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "exp-1");
    assert_eq!(results[0].goal, "Delete temporary build files safely");
    assert_eq!(results[0].solution.as_deref(), Some("use find with -delete flag"));
    assert_eq!(results[0].context.tools_used, vec!["shell", "read_file"]);
    assert_eq!(results[0].attempts.len(), 1);
    assert!(results[0].attempts[0].dead_end);
    assert_eq!(results[0].gotchas.len(), 1);
    assert_eq!(results[0].tags.len(), 2);
    assert!((results[0].confidence - 0.85).abs() < 0.01);
}

#[tokio::test]
async fn list_experiences_with_limit() {
    let (_dir, mem) = make_db();

    for i in 0..5 {
        let exp = make_experience(
            &format!("exp-{i}"),
            &format!("Goal number {i}"),
            Some(&format!("Solution {i}")),
        );
        mem.store_experience(&exp).await.expect("store");
    }

    let all = mem.list_experiences(100).await.expect("list all");
    assert_eq!(all.len(), 5);

    let limited = mem.list_experiences(3).await.expect("list limited");
    assert_eq!(limited.len(), 3);
}

#[tokio::test]
async fn experience_with_no_solution() {
    let (_dir, mem) = make_db();

    let exp = make_experience("exp-ns", "Unsolved mystery problem", None);
    mem.store_experience(&exp).await.expect("store");

    let results = mem.list_experiences(10).await.expect("list");
    assert_eq!(results.len(), 1);
    assert!(results[0].solution.is_none());
    assert_eq!(results[0].goal, "Unsolved mystery problem");
}
