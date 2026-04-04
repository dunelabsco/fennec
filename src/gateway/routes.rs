use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing, Json, Router,
};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::auth;

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

async fn chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, String)> {
    let mut agent = state.agent.lock().await;
    match agent.turn(&req.message).await {
        Ok(response) => Ok(Json(ChatResponse { response })),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}
