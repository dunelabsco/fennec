use std::sync::Arc;
use tokio::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use fennec::gateway::routes::{AppState, ChatRequest, ChatResponse};

#[tokio::test]
async fn test_health_endpoint() {
    let state = build_test_state(None);
    let app = fennec::gateway::routes::build_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_status_endpoint() {
    let state = build_test_state(None);
    let app = fennec::gateway::routes::build_router(state);

    let req = Request::builder()
        .uri("/status")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "running");
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn test_chat_requires_auth_when_configured() {
    let state = build_test_state(Some("secret_token".to_string()));
    let app = fennec::gateway::routes::build_router(state);

    // No auth header -- should get 401.
    let req = Request::builder()
        .method("POST")
        .uri("/chat")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_wrong_token_rejected() {
    let state = build_test_state(Some("correct_token".to_string()));
    let app = fennec::gateway::routes::build_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/chat")
        .header("content-type", "application/json")
        .header("authorization", "Bearer wrong_token")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_health_no_auth_needed_even_when_token_set() {
    let state = build_test_state(Some("secret".to_string()));
    let app = fennec::gateway::routes::build_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_chat_request_response_serde() {
    // Verify the request/response types serialize/deserialize correctly.
    let req_json = r#"{"message":"hello"}"#;
    let req: ChatRequest = serde_json::from_str(req_json).unwrap();
    assert_eq!(req.message, "hello");

    let resp = ChatResponse {
        response: "world".to_string(),
    };
    let resp_json = serde_json::to_string(&resp).unwrap();
    assert!(resp_json.contains("world"));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_test_state(auth_token: Option<String>) -> AppState {
    use fennec::agent::AgentBuilder;

    let provider = StubProvider;
    let memory = Arc::new(StubMemory);

    let agent = AgentBuilder::new()
        .provider(Arc::new(provider) as Arc<dyn fennec::providers::traits::Provider>)
        .memory(memory)
        .build()
        .expect("build stub agent");

    AppState {
        agent: Arc::new(Mutex::new(agent)),
        auth_token,
    }
}

// Minimal stub provider -- never actually called in these tests.
struct StubProvider;

#[async_trait::async_trait]
impl fennec::providers::traits::Provider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }

    async fn chat(
        &self,
        _request: fennec::providers::traits::ChatRequest<'_>,
    ) -> anyhow::Result<fennec::providers::traits::ChatResponse> {
        anyhow::bail!("stub provider: not implemented")
    }

    fn supports_tool_calling(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        4096
    }

    async fn chat_stream(
        &self,
        request: fennec::providers::traits::ChatRequest<'_>,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

// Minimal stub memory -- never actually called in these tests.
struct StubMemory;

#[async_trait::async_trait]
impl fennec::memory::traits::Memory for StubMemory {
    fn name(&self) -> &str {
        "stub"
    }

    async fn store(
        &self,
        _entry: fennec::memory::traits::MemoryEntry,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
    ) -> anyhow::Result<Vec<fennec::memory::traits::MemoryEntry>> {
        Ok(vec![])
    }

    async fn get(
        &self,
        _key: &str,
    ) -> anyhow::Result<Option<fennec::memory::traits::MemoryEntry>> {
        Ok(None)
    }

    async fn list(
        &self,
        _category: Option<&fennec::memory::traits::MemoryCategory>,
        _limit: usize,
    ) -> anyhow::Result<Vec<fennec::memory::traits::MemoryEntry>> {
        Ok(vec![])
    }

    async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn count(
        &self,
        _category: Option<&fennec::memory::traits::MemoryCategory>,
    ) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
