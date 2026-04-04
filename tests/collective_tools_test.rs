use std::sync::Arc;

use serde_json::json;

use fennec::collective::cache::CollectiveCache;
use fennec::collective::mock::MockCollective;
use fennec::collective::search::CollectiveSearch;
use fennec::collective::traits::CollectiveLayer;
use fennec::memory::experience::{Attempt, Experience, ExperienceContext};
use fennec::tools::collective_tools::{CollectiveReportTool, CollectiveSearchTool};
use fennec::tools::traits::Tool;

fn make_experience(id: &str, goal: &str, solution: Option<&str>) -> Experience {
    Experience {
        id: id.to_string(),
        goal: goal.to_string(),
        context: ExperienceContext {
            tools_used: vec!["cargo".to_string()],
            environment: "test".to_string(),
            constraints: "none".to_string(),
        },
        attempts: vec![Attempt {
            action: "tried something".to_string(),
            outcome: "it worked".to_string(),
            dead_end: false,
            insight: "keep trying".to_string(),
        }],
        solution: solution.map(|s| s.to_string()),
        gotchas: vec!["watch out for edge cases".to_string()],
        tags: vec!["rust".to_string()],
        confidence: 0.8,
        session_id: None,
        created_at: "2026-04-03T00:00:00Z".to_string(),
    }
}

/// Helper: create a CollectiveSearch backed by a temp SQLite DB and a mock remote.
fn make_search_with_mock(
    mock: MockCollective,
) -> (Arc<CollectiveSearch>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let embedder: Arc<dyn fennec::memory::embedding::EmbeddingProvider> =
        Arc::new(fennec::memory::embedding::NoopEmbedding::new(1536));
    let memory = Arc::new(
        fennec::memory::sqlite::SqliteMemory::new(db_path, 0.7, 0.3, 100, embedder).unwrap(),
    );
    let cache = CollectiveCache::new(memory.clone());
    let remote: Arc<dyn CollectiveLayer> = Arc::new(mock);
    let search = CollectiveSearch::new(memory, cache, Some(remote), None);
    (Arc::new(search), tmp)
}

// ---------------------------------------------------------------------------
// CollectiveSearchTool tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_tool_returns_formatted_results() {
    let exps = vec![
        make_experience("exp-1", "Fix Rust lifetime error", Some("Use Arc")),
        make_experience("exp-2", "Fix Rust borrow checker issue", Some("Use clone")),
    ];
    let mock = MockCollective::with_experiences(exps);
    let (search, _tmp) = make_search_with_mock(mock);
    let tool = CollectiveSearchTool::new(search);

    let result = tool.execute(json!({"query": "Rust"})).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("Found 2 experiences"));
    assert!(result.output.contains("lifetime"));
    assert!(result.output.contains("borrow"));
    assert!(result.output.contains("Trust:"));
    assert!(result.output.contains("Relevance:"));
}

#[tokio::test]
async fn search_tool_no_results() {
    let mock = MockCollective::new();
    let (search, _tmp) = make_search_with_mock(mock);
    let tool = CollectiveSearchTool::new(search);

    let result = tool.execute(json!({"query": "python"})).await.unwrap();
    assert!(result.success);
    assert!(
        result.output.contains("No relevant experiences"),
        "Expected no-results message, got: {}",
        result.output
    );
}

#[tokio::test]
async fn search_tool_empty_query_returns_error() {
    let mock = MockCollective::new();
    let (search, _tmp) = make_search_with_mock(mock);
    let tool = CollectiveSearchTool::new(search);

    let result = tool.execute(json!({"query": ""})).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.error, Some("No query provided".to_string()));
}

#[tokio::test]
async fn search_tool_missing_query_returns_error() {
    let mock = MockCollective::new();
    let (search, _tmp) = make_search_with_mock(mock);
    let tool = CollectiveSearchTool::new(search);

    let result = tool.execute(json!({})).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.error, Some("No query provided".to_string()));
}

#[tokio::test]
async fn search_tool_metadata() {
    let mock = MockCollective::new();
    let (search, _tmp) = make_search_with_mock(mock);
    let tool = CollectiveSearchTool::new(search);

    assert_eq!(tool.name(), "collective_search");
    assert!(tool.is_read_only());
    let spec = tool.spec();
    assert_eq!(spec.name, "collective_search");
    assert!(spec.parameters["properties"]["query"].is_object());
}

// ---------------------------------------------------------------------------
// CollectiveReportTool tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn report_tool_success() {
    let mock: Arc<dyn CollectiveLayer> = Arc::new(MockCollective::new());
    let tool = CollectiveReportTool::new(mock);

    let result = tool
        .execute(json!({
            "experience_id": "exp-1",
            "success": true,
            "notes": "Worked great"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("exp-1"));
    assert!(result.output.contains("success"));
}

#[tokio::test]
async fn report_tool_failure() {
    let mock: Arc<dyn CollectiveLayer> = Arc::new(MockCollective::new());
    let tool = CollectiveReportTool::new(mock);

    let result = tool
        .execute(json!({
            "experience_id": "exp-2",
            "success": false,
            "notes": "Did not work"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("exp-2"));
    assert!(result.output.contains("failure"));
}

#[tokio::test]
async fn report_tool_missing_experience_id() {
    let mock: Arc<dyn CollectiveLayer> = Arc::new(MockCollective::new());
    let tool = CollectiveReportTool::new(mock);

    let result = tool
        .execute(json!({"success": true}))
        .await
        .unwrap();

    assert!(!result.success);
    assert_eq!(result.error, Some("No experience_id provided".to_string()));
}

#[tokio::test]
async fn report_tool_metadata() {
    let mock: Arc<dyn CollectiveLayer> = Arc::new(MockCollective::new());
    let tool = CollectiveReportTool::new(mock);

    assert_eq!(tool.name(), "collective_report");
    assert!(!tool.is_read_only());
    let spec = tool.spec();
    assert_eq!(spec.name, "collective_report");
    assert!(spec.parameters["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "experience_id"));
}
