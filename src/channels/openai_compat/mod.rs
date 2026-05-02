//! OpenAI-compatible HTTP API channel.
//!
//! Exposes Fennec's primary LLM provider as an
//! `/v1/chat/completions` endpoint speaking the OpenAI wire
//! format. Any OpenAI-compatible client (Open WebUI, raycast
//! extensions, custom scripts, Cursor's "OpenAI-compatible"
//! mode) can point at this server's base URL and use Fennec
//! as if it were OpenAI itself.
//!
//! E-2-1 ships a passthrough — requests forward to Fennec's
//! configured primary LLM provider without running through the
//! agent loop. Tools, memory, and skills aren't surfaced through
//! this endpoint yet; that wiring lands in E-2-2.
//!
//! Endpoints:
//!
//!   POST /v1/chat/completions  OpenAI Chat Completions
//!                              (non-streaming + streaming SSE)
//!   GET  /v1/models            list a single model named
//!                              `[channels.openai_compat] model_name`
//!   GET  /v1/capabilities      Hermes-style advertise endpoint
//!   GET  /health               liveness probe
//!   GET  /health/detailed      includes provider name + flags
//!
//! Auth: `Authorization: Bearer <api_key>` when
//! `[channels.openai_compat] api_key` is non-empty. Empty key
//! means no auth — only safe with `host = 127.0.0.1`. A startup
//! warning is logged in that case.
//!
//! Outbound is a no-op: the API responds to its own HTTP
//! requests; it does not push messages to a recipient through
//! this channel.

pub mod auth;
pub mod handlers;
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::{
    Router,
    routing::{get, post},
};
use tokio::sync::mpsc;

use crate::bus::InboundMessage;
use crate::config::OpenAiCompatChannelEntry;
use crate::providers::traits::Provider;

use super::traits::{Channel, SendMessage};

pub use handlers::ServerState;

/// The OpenAI-compatible channel. Owns the bind address, the
/// shared server state (provider + auth key + model name), and
/// the optional CORS allow-list.
pub struct OpenAiCompatChannel {
    config: OpenAiCompatChannelEntry,
    state: ServerState,
}

impl OpenAiCompatChannel {
    /// Construct from config + provider. Returns `None` when the
    /// channel is disabled. Logs a warning when `api_key` is empty
    /// (auth-disabled mode) so operators don't accidentally expose
    /// an unauthenticated endpoint to the network.
    pub fn from_config(
        config: &OpenAiCompatChannelEntry,
        provider: Arc<dyn Provider>,
    ) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        if config.api_key.is_empty() {
            tracing::warn!(
                host = %config.host,
                port = config.port,
                "openai_compat: api_key is empty — endpoint accepts unauthenticated \
                 requests. This is only safe when host = 127.0.0.1."
            );
        }
        if config.model_name.is_empty() {
            tracing::warn!("openai_compat: model_name is empty; clients won't have a model id to send");
        }
        let state = ServerState {
            provider,
            api_key: Arc::new(config.api_key.clone()),
            model_name: Arc::new(config.model_name.clone()),
        };
        Some(Self {
            config: config.clone(),
            state,
        })
    }

    /// Build the axum router. Public for testing — production code
    /// goes through `listen()`.
    pub fn router(&self) -> Router {
        let state = self.state.clone();
        let app = Router::new()
            .route("/health", get(handlers::health))
            .route("/health/detailed", get(handlers::health_detailed))
            .route("/v1/models", get(handlers::list_models))
            .route("/v1/capabilities", get(handlers::capabilities))
            .route(
                "/v1/chat/completions",
                post(handlers::chat_completions),
            )
            .with_state(state);

        if !self.config.cors_origins.is_empty() {
            apply_cors(app, &self.config.cors_origins)
        } else {
            app
        }
    }
}

#[async_trait]
impl Channel for OpenAiCompatChannel {
    fn name(&self) -> &str {
        "openai_compat"
    }

    /// No-op: the API responds to its own HTTP requests. Calling
    /// `send` on this channel is a configuration mistake — the
    /// caller probably wanted a real messaging channel.
    async fn send(&self, _message: &SendMessage) -> Result<()> {
        anyhow::bail!(
            "openai_compat channel is request-response only; route messages \
             through a messaging channel (telegram/discord/slack/email) instead"
        )
    }

    async fn listen(&self, _tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .with_context(|| {
                format!(
                    "invalid openai_compat listen address {}:{}",
                    self.config.host, self.config.port
                )
            })?;

        let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| {
            format!("binding openai_compat HTTP server to {}", addr)
        })?;
        tracing::info!(addr = %addr, "openai_compat HTTP server listening");

        let app = self.router();
        axum::serve(listener, app)
            .await
            .context("openai_compat server crashed")?;
        Ok(())
    }
}

/// Apply a CORS layer for the configured allow-origin list.
/// `*` permits any origin; otherwise the value is a comma-separated
/// list of origins.
fn apply_cors(router: Router, origins: &str) -> Router {
    use tower_http::cors::{Any, CorsLayer};
    let layer = if origins.trim() == "*" {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let parsed: Vec<_> = origins
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        if parsed.is_empty() {
            return router;
        }
        CorsLayer::new()
            .allow_origin(parsed)
            .allow_methods(Any)
            .allow_headers(Any)
    };
    router.layer(layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::traits::{
        ChatRequest, ChatResponse, StreamEvent, ToolCall, UsageInfo,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use std::sync::Mutex;
    use tower::ServiceExt;

    /// Minimal in-memory provider so handlers can exercise the
    /// passthrough path without a real LLM. Records the last request
    /// and emits a configurable response.
    struct MockProvider {
        next_response: Mutex<ChatResponse>,
        stream_events: Mutex<Vec<StreamEvent>>,
        last_messages: Mutex<Vec<crate::providers::traits::ChatMessage>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                next_response: Mutex::new(ChatResponse {
                    content: Some("hello".into()),
                    tool_calls: vec![],
                    usage: Some(UsageInfo {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                    }),
                }),
                stream_events: Mutex::new(vec![
                    StreamEvent::Delta("Hel".into()),
                    StreamEvent::Delta("lo".into()),
                    StreamEvent::Done,
                ]),
                last_messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
            *self.last_messages.lock().unwrap() = request.messages.to_vec();
            Ok(self.next_response.lock().unwrap().clone())
        }
        fn supports_tool_calling(&self) -> bool {
            true
        }
        fn context_window(&self) -> usize {
            128_000
        }
        fn supports_streaming(&self) -> bool {
            true
        }
        async fn chat_stream(
            &self,
            request: ChatRequest<'_>,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            *self.last_messages.lock().unwrap() = request.messages.to_vec();
            let events = self.stream_events.lock().unwrap().clone();
            let (tx, rx) = mpsc::channel(32);
            tokio::spawn(async move {
                for ev in events {
                    let _ = tx.send(ev).await;
                }
            });
            Ok(rx)
        }
    }

    fn config(api_key: &str) -> OpenAiCompatChannelEntry {
        OpenAiCompatChannelEntry {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 0,
            api_key: api_key.into(),
            model_name: "fennec-agent".into(),
            cors_origins: String::new(),
        }
    }

    fn make_channel(api_key: &str) -> (OpenAiCompatChannel, Arc<MockProvider>) {
        let provider = Arc::new(MockProvider::new());
        let cfg = config(api_key);
        let ch = OpenAiCompatChannel::from_config(
            &cfg,
            Arc::clone(&provider) as Arc<dyn Provider>,
        )
        .expect("enabled config returns Some");
        (ch, provider)
    }

    // -- construction ----------------------------------------------

    #[test]
    fn from_config_disabled_returns_none() {
        let mut cfg = config("k");
        cfg.enabled = false;
        let p: Arc<dyn Provider> = Arc::new(MockProvider::new());
        assert!(OpenAiCompatChannel::from_config(&cfg, p).is_none());
    }

    #[tokio::test]
    async fn channel_send_is_no_op_error() {
        let (ch, _) = make_channel("k");
        let r = ch.send(&SendMessage::new("hi", "x")).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("request-response only"));
    }

    #[test]
    fn channel_name_is_openai_compat() {
        let (ch, _) = make_channel("k");
        assert_eq!(ch.name(), "openai_compat");
    }

    // -- /health ---------------------------------------------------

    #[tokio::test]
    async fn health_returns_ok() {
        let (ch, _) = make_channel("");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_detailed_includes_provider() {
        let (ch, _) = make_channel("");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/health/detailed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["provider"], "mock");
        assert_eq!(v["model"], "fennec-agent");
    }

    // -- auth ------------------------------------------------------

    #[tokio::test]
    async fn no_auth_when_api_key_empty() {
        let (ch, _) = make_channel("");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_bearer_returns_401() {
        let (ch, _) = make_channel("expected-key");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_bearer_passes() {
        let (ch, _) = make_channel("expected-key");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", "Bearer expected-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_bearer_returns_401() {
        let (ch, _) = make_channel("right");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    // -- /v1/models -----------------------------------------------

    #[tokio::test]
    async fn models_returns_configured_model() {
        let (ch, _) = make_channel("");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"][0]["id"], "fennec-agent");
        assert_eq!(v["data"][0]["object"], "model");
        assert_eq!(v["data"][0]["owned_by"], "fennec");
    }

    // -- /v1/capabilities -----------------------------------------

    #[tokio::test]
    async fn capabilities_advertises_endpoints() {
        let (ch, _) = make_channel("");
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .uri("/v1/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["server"], "fennec");
        let endpoints = v["endpoints"].as_array().unwrap();
        let paths: Vec<&str> = endpoints.iter().filter_map(|e| e.as_str()).collect();
        assert!(paths.contains(&"/v1/chat/completions"));
        assert!(paths.contains(&"/v1/models"));
    }

    // -- /v1/chat/completions non-streaming -----------------------

    #[tokio::test]
    async fn chat_completions_non_streaming_returns_assistant_message() {
        let (ch, _) = make_channel("");
        let body = json!({
            "model": "fennec-agent",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": false
        });
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["choices"][0]["message"]["content"], "hello");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 10);
        assert_eq!(v["usage"]["completion_tokens"], 5);
        assert_eq!(v["usage"]["total_tokens"], 15);
    }

    #[tokio::test]
    async fn chat_completions_passes_messages_to_provider() {
        let (ch, provider) = make_channel("");
        let body = json!({
            "model": "fennec-agent",
            "messages": [
                {"role": "system", "content": "you are helpful"},
                {"role": "user", "content": "hello"}
            ]
        });
        let _ = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let seen = provider.last_messages.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].role, "system");
        assert_eq!(seen[1].role, "user");
        assert_eq!(seen[1].content.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn chat_completions_flattens_multipart_text_content() {
        let (ch, provider) = make_channel("");
        let body = json!({
            "model": "fennec-agent",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {"type": "image_url", "image_url": {"url": "https://x"}},
                    {"type": "text", "text": "please"}
                ]
            }]
        });
        let _ = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let seen = provider.last_messages.lock().unwrap().clone();
        // Image part dropped; text parts joined with newline.
        assert_eq!(seen[0].content.as_deref(), Some("describe\nplease"));
    }

    #[tokio::test]
    async fn chat_completions_empty_messages_400() {
        let (ch, _) = make_channel("");
        let body = json!({"model": "x", "messages": []});
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_completions_missing_messages_400() {
        let (ch, _) = make_channel("");
        let body = json!({"model": "x"});
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        // axum returns 422 (Unprocessable Entity) for serde rejection.
        assert!(matches!(
            r.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
    }

    #[tokio::test]
    async fn chat_completions_tool_call_response_finish_reason() {
        let (ch, provider) = make_channel("");
        // Configure provider to emit a tool call.
        *provider.next_response.lock().unwrap() = ChatResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "tc1".into(),
                name: "do_thing".into(),
                arguments: json!({"x": 1}),
            }],
            usage: None,
        };
        let body = json!({"model": "x", "messages": [{"role": "user", "content": "x"}]});
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        let tcs = v["choices"][0]["message"]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs[0]["id"], "tc1");
        assert_eq!(tcs[0]["type"], "function");
        assert_eq!(tcs[0]["function"]["name"], "do_thing");
    }

    // -- /v1/chat/completions streaming ---------------------------

    #[tokio::test]
    async fn chat_completions_streaming_emits_sse_chunks() {
        let (ch, _) = make_channel("");
        let body = json!({
            "model": "fennec-agent",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        });
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();

        // Should contain the role announcement, two content deltas,
        // a finish-reason chunk, and the [DONE] terminator.
        let data_lines: Vec<&str> = body
            .split("\n\n")
            .filter(|s| s.starts_with("data: "))
            .collect();
        assert!(data_lines.len() >= 4, "got {} chunks: {:?}", data_lines.len(), data_lines);
        assert!(body.ends_with("data: [DONE]\n\n"));

        // Parse first non-DONE chunks as JSON.
        let json_chunks: Vec<Value> = data_lines
            .iter()
            .filter(|l| !l.contains("[DONE]"))
            .filter_map(|l| serde_json::from_str(l.trim_start_matches("data: ")).ok())
            .collect();
        assert_eq!(json_chunks[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(json_chunks[1]["choices"][0]["delta"]["content"], "Hel");
        assert_eq!(json_chunks[2]["choices"][0]["delta"]["content"], "lo");
        // Final chunk has finish_reason.
        let last_json = json_chunks.last().unwrap();
        assert_eq!(last_json["choices"][0]["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn chat_completions_streaming_with_include_usage() {
        let (ch, _) = make_channel("");
        let body = json!({
            "model": "fennec-agent",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        // The usage chunk has empty choices and a populated usage.
        let json_chunks: Vec<Value> = body
            .split("\n\n")
            .filter(|s| s.starts_with("data: ") && !s.contains("[DONE]"))
            .filter_map(|s| serde_json::from_str(s.trim_start_matches("data: ")).ok())
            .collect();
        let usage_chunk = json_chunks
            .iter()
            .find(|c| c.get("choices").map(|a| a.as_array().map(|x| x.is_empty()).unwrap_or(false)).unwrap_or(false))
            .expect("a chunk with empty choices should carry usage when include_usage=true");
        assert!(usage_chunk["usage"].is_object());
    }

    #[tokio::test]
    async fn chat_completions_provider_error_returns_502() {
        // A provider that errors on chat() should produce 502.
        struct FailProvider;
        #[async_trait]
        impl Provider for FailProvider {
            fn name(&self) -> &str {
                "fail"
            }
            async fn chat(&self, _r: ChatRequest<'_>) -> Result<ChatResponse> {
                anyhow::bail!("upstream down")
            }
            fn supports_tool_calling(&self) -> bool {
                false
            }
            fn context_window(&self) -> usize {
                0
            }
            async fn chat_stream(
                &self,
                _r: ChatRequest<'_>,
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                anyhow::bail!("upstream down")
            }
        }
        let cfg = config("");
        let provider: Arc<dyn Provider> = Arc::new(FailProvider);
        let ch = OpenAiCompatChannel::from_config(&cfg, provider).unwrap();

        let body = json!({"model": "x", "messages": [{"role": "user", "content": "x"}]});
        let r = ch
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "upstream_error");
    }
}
