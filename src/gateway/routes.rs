use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    middleware,
    routing, Json, Router,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tower_http::timeout::TimeoutLayer;

use super::auth;

/// Max inbound request body size. A /chat message is conversation text;
/// anything past 1 MiB is abuse, not a legitimate prompt. Without a cap,
/// a client could POST an arbitrarily large JSON body and axum would
/// buffer it before the handler ever ran — trivially DoS-able.
const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;

/// Per-request wall-clock cap. The agent's ReliableProvider deadline is
/// 60s per provider call (default), and a tool-heavy turn can chain up
/// to 15 iterations × per-call latency + tool execution. 10 minutes is
/// generous enough that no realistic agent turn gets cut off, while
/// bounding genuinely-stuck requests from holding resources forever.
///
/// Over-restriction lens: a typical chat completes in 2–30 s; even
/// long-thinking reasoning turns rarely exceed a couple of minutes.
/// Setting this lower (e.g. 60s) would surface as confusing "request
/// timed out" errors mid-thought, which we explicitly want to avoid.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Mutex<crate::agent::Agent>>,
    pub auth_token: Option<String>,
}

pub fn build_router(state: AppState) -> Router {
    // Public routes -- no auth required.
    let public = Router::new()
        .route("/health", routing::get(health))
        .route("/status", routing::get(status));

    // Protected routes -- auth middleware applied.
    let protected = Router::new()
        .route("/chat", routing::post(chat))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    public
        .merge(protected)
        // DefaultBodyLimit applies to every route in the merged router,
        // overriding axum's 2 MiB per-extractor default with a tighter,
        // explicit cap.
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        // Per-request wall-clock timeout. tower-http's TimeoutLayer
        // wraps the route service and returns 408 if the inner future
        // doesn't resolve within the deadline. The handler's mutex
        // release on cancellation is automatic — Rust drops the
        // MutexGuard when the future is cancelled.
        .layer(TimeoutLayer::new(REQUEST_TIMEOUT))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

async fn status() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running"
    }))
}

#[derive(serde::Deserialize)]
pub struct ChatRequest {
    pub message: String,
}

#[derive(serde::Serialize)]
pub struct ChatResponse {
    pub response: String,
}

#[derive(serde::Serialize)]
pub struct ErrorResponse {
    pub error: String,
    /// Short id the client can include when reporting the failure. The
    /// full error chain is logged server-side under the same id so the
    /// operator can correlate without the client receiving raw internal
    /// state (provider URLs, file paths, decrypt error chains).
    pub request_id: String,
}

async fn chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<ErrorResponse>)> {
    let mut agent = state.agent.lock().await;
    match agent.turn(&req.message).await {
        Ok(response) => Ok(Json(ChatResponse { response })),
        Err(e) => {
            // Log the full error chain server-side under a request_id.
            // The client gets a generic message + the same id so an
            // operator looking at logs can find the matching record.
            let request_id = uuid::Uuid::new_v4().simple().to_string();
            tracing::error!(request_id = %request_id, error = ?e, "agent turn failed");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal error processing chat request".to_string(),
                    request_id,
                }),
            ))
        }
    }
}
