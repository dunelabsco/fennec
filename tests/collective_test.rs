use fennec::collective::mock::MockCollective;
use fennec::collective::traits::*;
use fennec::memory::experience::{Attempt, Experience, ExperienceContext};

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

// ---------------------------------------------------------------------------
// MockCollective tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_publish_and_search_roundtrip() {
    let mock = MockCollective::new();
    let exp = make_experience("exp-1", "Fix Rust lifetime error", Some("Use Arc"));

    let id = mock.publish(&exp).await.unwrap();
    assert_eq!(id, "exp-1");

    let results = mock.search("lifetime", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "exp-1");
    assert_eq!(results[0].goal, "Fix Rust lifetime error");
    assert_eq!(results[0].solution.as_deref(), Some("Use Arc"));
    assert_eq!(results[0].gotchas, vec!["watch out for edge cases"]);
}

#[tokio::test]
async fn mock_search_no_results() {
    let mock = MockCollective::new();
    let exp = make_experience("exp-1", "Fix Rust lifetime error", Some("Use Arc"));
    mock.publish(&exp).await.unwrap();

    let results = mock.search("python decorator", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn mock_search_case_insensitive() {
    let mock = MockCollective::new();
    let exp = make_experience("exp-1", "Fix Rust Lifetime Error", Some("Use Arc"));
    mock.publish(&exp).await.unwrap();

    let results = mock.search("rust lifetime", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn mock_get_experience_by_id() {
    let mock = MockCollective::new();
    let exp = make_experience("exp-42", "Set up CI pipeline", Some("Use GitHub Actions"));
    mock.publish(&exp).await.unwrap();

    let found = mock.get_experience("exp-42").await.unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.id, "exp-42");
    assert_eq!(found.goal, "Set up CI pipeline");

    let missing = mock.get_experience("nonexistent").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn mock_with_preloaded_experiences() {
    let exps = vec![
        make_experience("a", "Deploy to AWS", Some("Use ECS")),
        make_experience("b", "Deploy to GCP", Some("Use Cloud Run")),
    ];
    let mock = MockCollective::with_experiences(exps);

    let results = mock.search("deploy", 10).await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn mock_search_respects_limit() {
    let exps = vec![
        make_experience("a", "Deploy alpha", None),
        make_experience("b", "Deploy beta", None),
        make_experience("c", "Deploy gamma", None),
    ];
    let mock = MockCollective::with_experiences(exps);

    let results = mock.search("deploy", 2).await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn mock_report_outcome_succeeds() {
    let mock = MockCollective::new();
    let report = OutcomeReport {
        success: true,
        execution_time_ms: Some(150),
        error_message: None,
        context_notes: Some("worked perfectly".to_string()),
    };
    // Should not error even though experience doesn't exist.
    mock.report_outcome("exp-1", &report).await.unwrap();
}

#[tokio::test]
async fn mock_health_check() {
    let mock = MockCollective::new();
    assert!(mock.health_check().await);
}

#[tokio::test]
async fn mock_name() {
    let mock = MockCollective::new();
    assert_eq!(mock.name(), "mock");
}

// ---------------------------------------------------------------------------
// PlurumlClient serialization / deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn plurum_search_request_serializes_correctly() {
    #[derive(serde::Serialize)]
    struct SearchRequest {
        query: String,
        match_count: usize,
    }

    let req = SearchRequest {
        query: "lifetime error".to_string(),
        match_count: 5,
    };
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["query"], "lifetime error");
    assert_eq!(json["match_count"], 5);
}

#[test]
fn plurum_search_response_deserializes_with_trust_score() {
    let json = r#"[{
        "id": "exp-1",
        "goal": "Fix lifetime",
        "solution": "Use Arc",
        "gotchas": ["careful"],
        "trust_score": 0.85,
        "relevance_score": 0.9,
        "outcome_reports": {"success": 3, "failure": 1}
    }]"#;

    #[derive(serde::Deserialize)]
    struct SearchResult {
        id: String,
        goal: String,
        solution: Option<String>,
        gotchas: Vec<String>,
        trust_score: Option<f64>,
        quality_score: Option<f64>,
        relevance_score: Option<f64>,
        outcome_reports: Option<OutcomeReportsWire>,
    }

    #[derive(serde::Deserialize)]
    struct OutcomeReportsWire {
        success: u32,
        failure: u32,
    }

    let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "exp-1");
    assert_eq!(results[0].trust_score, Some(0.85));
    assert!(results[0].quality_score.is_none());
    let reports = results[0].outcome_reports.as_ref().unwrap();
    assert_eq!(reports.success, 3);
    assert_eq!(reports.failure, 1);
}

#[test]
fn plurum_search_response_deserializes_with_quality_score() {
    let json = r#"[{
        "id": "exp-2",
        "goal": "Fix borrow",
        "solution": null,
        "gotchas": [],
        "quality_score": 0.6,
        "relevance_score": 0.5
    }]"#;

    #[derive(serde::Deserialize)]
    struct SearchResult {
        id: String,
        trust_score: Option<f64>,
        quality_score: Option<f64>,
    }

    let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
    assert!(results[0].trust_score.is_none());
    assert_eq!(results[0].quality_score, Some(0.6));
}

#[test]
fn plurum_experience_response_with_attempts() {
    let json = r#"{
        "id": "exp-10",
        "goal": "Setup CI",
        "context": {
            "tools_used": ["cargo"],
            "environment": "linux",
            "constraints": "none"
        },
        "attempts": [{
            "action": "tried make",
            "outcome": "failed",
            "dead_end": true,
            "insight": "make is not the way"
        }],
        "solution": "Use cargo",
        "gotchas": [],
        "tags": ["ci"],
        "confidence": 0.9,
        "created_at": "2026-01-01T00:00:00Z"
    }"#;

    #[derive(serde::Deserialize)]
    struct PlurumlExp {
        id: String,
        attempts: Option<Vec<serde_json::Value>>,
        dead_ends: Option<Vec<String>>,
    }

    let exp: PlurumlExp = serde_json::from_str(json).unwrap();
    assert_eq!(exp.id, "exp-10");
    assert!(exp.attempts.is_some());
    assert_eq!(exp.attempts.unwrap().len(), 1);
    assert!(exp.dead_ends.is_none());
}

#[test]
fn plurum_experience_response_with_legacy_fields() {
    let json = r#"{
        "id": "exp-11",
        "goal": "Debug crash",
        "dead_ends": ["tried restart", "tried cache clear"],
        "breakthroughs": ["found race condition"],
        "solution": "Add mutex",
        "gotchas": [],
        "tags": [],
        "confidence": 0.7,
        "created_at": "2026-01-01T00:00:00Z"
    }"#;

    #[derive(serde::Deserialize)]
    struct PlurumlExp {
        id: String,
        attempts: Option<Vec<serde_json::Value>>,
        dead_ends: Option<Vec<String>>,
        breakthroughs: Option<Vec<String>>,
    }

    let exp: PlurumlExp = serde_json::from_str(json).unwrap();
    assert!(exp.attempts.is_none());
    assert_eq!(exp.dead_ends.as_ref().unwrap().len(), 2);
    assert_eq!(exp.breakthroughs.as_ref().unwrap().len(), 1);
}

#[test]
fn plurum_publish_request_format() {
    #[derive(serde::Serialize)]
    struct PublishRequest {
        id: String,
        goal: String,
        context: PublishContext,
        attempts: Vec<PublishAttempt>,
        solution: Option<String>,
        gotchas: Vec<String>,
        tags: Vec<String>,
        confidence: f32,
        session_id: Option<String>,
        created_at: String,
    }

    #[derive(serde::Serialize)]
    struct PublishContext {
        tools_used: Vec<String>,
        environment: String,
        constraints: String,
    }

    #[derive(serde::Serialize)]
    struct PublishAttempt {
        action: String,
        outcome: String,
        dead_end: bool,
        insight: String,
    }

    let req = PublishRequest {
        id: "exp-1".to_string(),
        goal: "Fix bug".to_string(),
        context: PublishContext {
            tools_used: vec!["cargo".to_string()],
            environment: "linux".to_string(),
            constraints: "".to_string(),
        },
        attempts: vec![PublishAttempt {
            action: "debug".to_string(),
            outcome: "found it".to_string(),
            dead_end: false,
            insight: "check logs".to_string(),
        }],
        solution: Some("patch applied".to_string()),
        gotchas: vec![],
        tags: vec!["rust".to_string()],
        confidence: 0.9,
        session_id: None,
        created_at: "2026-04-03T00:00:00Z".to_string(),
    };

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["id"], "exp-1");
    assert_eq!(json["goal"], "Fix bug");
    assert_eq!(json["context"]["tools_used"][0], "cargo");
    assert_eq!(json["attempts"][0]["action"], "debug");
    let conf = json["confidence"].as_f64().unwrap();
    assert!((conf - 0.9).abs() < 0.001, "confidence should be ~0.9, got {}", conf);
    assert!(json["session_id"].is_null());
}

#[test]
fn plurum_outcome_report_serializes_correctly() {
    #[derive(serde::Serialize)]
    struct OutcomeReportRequest {
        success: bool,
        execution_time_ms: Option<u64>,
        error_message: Option<String>,
        context_notes: Option<String>,
    }

    let req = OutcomeReportRequest {
        success: false,
        execution_time_ms: Some(2500),
        error_message: Some("timeout".to_string()),
        context_notes: None,
    };

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["success"], false);
    assert_eq!(json["execution_time_ms"], 2500);
    assert_eq!(json["error_message"], "timeout");
    assert!(json["context_notes"].is_null());
}
