//! Generic HTTP webhook channel.
//!
//! Receives POSTs from external systems (CI, monitoring, GitHub /
//! GitLab events, custom integrations), validates HMAC signatures,
//! renders a prompt template against the payload, and forwards the
//! result to the agent loop as a synthetic inbound message.
//!
//! The channel is **inbound-only**: `Channel::send` is a no-op (a
//! webhook source is the originator of the message, not a recipient
//! the agent talks back to). Replies the agent generates flow out
//! through whichever channel the user has configured for the
//! relevant chat, exactly the same way as for telegram/discord/etc.
//!
//! Per-route configuration lives in `[channels.webhook.routes.<name>]`
//! (see [`crate::config::WebhookRouteEntry`]). The HTTP path
//! `POST /webhook/<name>` looks up the route, validates its HMAC,
//! checks the event allowlist, renders the route's prompt template
//! against the JSON payload, and synthesizes an inbound message
//! that arrives at the bus tagged `channel = "webhook:<route>"`.
//!
//! HMAC formats supported (in priority order):
//!   - GitHub: `X-Hub-Signature-256: sha256=<hex>` over the raw
//!     request body
//!   - GitLab: `X-Gitlab-Token: <secret>` (plain shared secret, not
//!     a HMAC, but the upstream behavior is the same — match against
//!     the route's secret)
//!   - Generic: `X-Webhook-Signature: sha256=<hex>` over the raw body
//!
//! Idempotency: the channel keeps a per-process LRU keyed by
//! `(route_name, body_sha256)`. If a duplicate (route, body) arrives
//! within the configured TTL, the second POST returns 200 OK
//! without re-firing the agent. Webhooks retry aggressively on
//! transient failure; without this guard a single event would run
//! the agent multiple times.
//!
//! Rate limit: a fixed-window counter per route. Default 30 req/min.
//! Bursts past the limit get 429 with `Retry-After`.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::bus::InboundMessage;
use crate::config::{WebhookChannelEntry, WebhookRouteEntry};

use super::traits::{Channel, SendMessage};

type HmacSha256 = Hmac<Sha256>;

/// Sentinel value a route can set in its `secret` field to skip
/// signature checks entirely. Only safe on a trusted network.
pub const INSECURE_NO_AUTH: &str = "INSECURE_NO_AUTH";

/// Default fallback when a route doesn't set `prompt`. Just relays
/// the payload as JSON.
pub const DEFAULT_PROMPT: &str = "Webhook event {event} on route {route}: {payload}";

/// The webhook channel, owning its config snapshot and the per-
/// process state (idempotency cache + rate-limiter buckets).
pub struct WebhookChannel {
    config: WebhookChannelEntry,
    state: Arc<WebhookState>,
}

impl WebhookChannel {
    /// Build a webhook channel from config. Returns `None` when the
    /// channel is disabled, so callers can silently skip
    /// registration. When enabled but misconfigured (no routes), a
    /// warning is logged but the channel is still constructed —
    /// the listener will simply 404 every request.
    pub fn from_config(config: &WebhookChannelEntry) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        if config.routes.is_empty() {
            tracing::warn!(
                "webhook channel enabled but no routes configured; \
                 every request will return 404"
            );
        }
        for (name, route) in &config.routes {
            if route.secret.is_empty() && config.secret.is_empty() {
                tracing::warn!(
                    "webhook route {:?} has no secret and no global fallback; \
                     requests to it will return 401 unless the route's secret \
                     is set or the global secret is",
                    name
                );
            }
            if route.secret == INSECURE_NO_AUTH {
                tracing::warn!(
                    "webhook route {:?} uses INSECURE_NO_AUTH; signature \
                     checks are disabled — only safe on a trusted network",
                    name
                );
            }
        }
        Some(Self {
            config: config.clone(),
            state: Arc::new(WebhookState::new(config)),
        })
    }
}

/// Shared per-process state, behind an `Arc` so axum handlers can
/// see the same idempotency cache and rate-limiter buckets.
struct WebhookState {
    /// `(route, body_hash)` -> first-seen timestamp. Pruned on each
    /// request when the front of the queue is older than the TTL.
    seen: Mutex<IdempotencyCache>,
    /// Per-route fixed-window counters. Window length is one minute.
    rate: Mutex<RateLimiter>,
    /// Idempotency TTL (snapshotted from config at construction).
    idempotency_ttl: Duration,
    /// Per-route rate limit (snapshotted from config).
    rate_limit_per_minute: u32,
    /// Global secret fallback (snapshotted).
    global_secret: String,
    /// Routes (snapshotted).
    routes: HashMap<String, WebhookRouteEntry>,
}

impl WebhookState {
    fn new(config: &WebhookChannelEntry) -> Self {
        Self {
            seen: Mutex::new(IdempotencyCache::default()),
            rate: Mutex::new(RateLimiter::default()),
            idempotency_ttl: Duration::from_secs(config.idempotency_ttl_secs),
            rate_limit_per_minute: config.rate_limit_per_minute,
            global_secret: config.secret.clone(),
            routes: config.routes.clone(),
        }
    }
}

/// Idempotency cache: bounded queue of `(timestamp, key)` plus a
/// hash set for O(1) lookup. The queue is checked from the front
/// on every insert and entries older than the TTL are dropped.
#[derive(Default)]
struct IdempotencyCache {
    queue: VecDeque<(Instant, String)>,
    keys: std::collections::HashSet<String>,
}

impl IdempotencyCache {
    /// Returns `true` if this is a duplicate within the TTL window.
    /// Otherwise records the key and returns `false`.
    fn check_and_record(&mut self, key: String, ttl: Duration) -> bool {
        // Prune expired entries.
        let now = Instant::now();
        while let Some((ts, _)) = self.queue.front() {
            if now.duration_since(*ts) > ttl {
                if let Some((_, k)) = self.queue.pop_front() {
                    self.keys.remove(&k);
                }
            } else {
                break;
            }
        }
        if self.keys.contains(&key) {
            return true;
        }
        self.queue.push_back((now, key.clone()));
        self.keys.insert(key);
        false
    }
}

/// Rate limiter: per-route fixed-window counter. Each window is one
/// minute long. When `count` exceeds `limit`, requests get 429.
#[derive(Default)]
struct RateLimiter {
    /// `route -> (window_start, count_in_window)`
    buckets: HashMap<String, (Instant, u32)>,
}

impl RateLimiter {
    /// Returns `true` if this request would exceed the limit.
    /// Otherwise records the request and returns `false`.
    fn check_and_record(&mut self, route: &str, limit: u32) -> bool {
        let now = Instant::now();
        let entry = self.buckets.entry(route.to_string()).or_insert((now, 0));
        // Roll the window if we're more than 60s past its start.
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        if entry.1 >= limit {
            return true;
        }
        entry.1 += 1;
        false
    }
}

#[async_trait]
impl Channel for WebhookChannel {
    fn name(&self) -> &str {
        "webhook"
    }

    /// No-op: webhooks are inbound only.
    async fn send(&self, _message: &SendMessage) -> Result<()> {
        anyhow::bail!(
            "webhook channel is inbound-only; route replies through \
             another channel (telegram/discord/slack/email) instead"
        )
    }

    async fn listen(&self, tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .with_context(|| {
                format!(
                    "invalid webhook listen address {}:{}",
                    self.config.host, self.config.port
                )
            })?;

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .with_context(|| format!("binding webhook server to {}", addr))?;
        tracing::info!(addr = %addr, "webhook channel listening");

        let app_state = WebhookHandlerState {
            state: Arc::clone(&self.state),
            tx,
        };
        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/webhook/{route_name}", post(webhook_handler))
            .with_state(app_state);

        axum::serve(listener, app)
            .await
            .context("webhook server crashed")?;

        Ok(())
    }
}

#[derive(Clone)]
struct WebhookHandlerState {
    state: Arc<WebhookState>,
    tx: mpsc::Sender<InboundMessage>,
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn webhook_handler(
    State(app): State<WebhookHandlerState>,
    Path(route_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let route = match app.state.routes.get(&route_name) {
        Some(r) => r.clone(),
        None => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "route not found"})))
                .into_response();
        }
    };

    // Rate limit before doing any expensive work.
    let rate_limited = {
        let mut rate = app.state.rate.lock();
        rate.check_and_record(&route_name, app.state.rate_limit_per_minute)
    };
    if rate_limited {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, "60")],
            Json(serde_json::json!({"error": "rate limit exceeded"})),
        )
            .into_response();
    }

    // Resolve secret: route's own > global > none. INSECURE_NO_AUTH
    // sentinel skips the check entirely.
    let secret = if !route.secret.is_empty() {
        route.secret.clone()
    } else {
        app.state.global_secret.clone()
    };

    if secret == INSECURE_NO_AUTH {
        // Skip the signature check.
    } else if secret.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "no secret configured for this route"})),
        )
            .into_response();
    } else if !verify_signature(&secret, &body, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "signature verification failed"})),
        )
            .into_response();
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("invalid JSON body: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Event allowlist check.
    let event = extract_event_type(&headers, &payload);
    if !route.events.is_empty() && !route.events.iter().any(|e| e == &event) {
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "ignored": true,
                "reason": format!("event {:?} not in allowlist", event)
            })),
        )
            .into_response();
    }

    // Idempotency: hash the body and check the per-route cache.
    if app.state.idempotency_ttl > Duration::ZERO {
        let key = idempotency_key(&route_name, &body);
        let duplicate = {
            let mut seen = app.state.seen.lock();
            seen.check_and_record(key, app.state.idempotency_ttl)
        };
        if duplicate {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "deduped": true,
                })),
            )
                .into_response();
        }
    }

    let prompt = render_prompt(
        if route.prompt.is_empty() {
            DEFAULT_PROMPT
        } else {
            &route.prompt
        },
        &payload,
        &event,
        &route_name,
    );

    let inbound = InboundMessage {
        id: uuid::Uuid::new_v4().to_string(),
        sender: format!("webhook-{}", event),
        content: prompt,
        channel: format!("webhook:{}", route_name),
        chat_id: route_name.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        reply_to: None,
        metadata: {
            let mut m = HashMap::new();
            m.insert("event".into(), event.clone());
            m.insert("route".into(), route_name.clone());
            m
        },
    };

    if let Err(e) = app.tx.send(inbound).await {
        tracing::error!(error = %e, "failed to forward webhook payload to bus");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "could not forward to agent"})),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"ok": true, "event": event})),
    )
        .into_response()
}

/// Verify a webhook signature against the body using the supplied
/// secret. Tries (in order) GitHub-style `X-Hub-Signature-256`,
/// GitLab-style `X-Gitlab-Token` (plain shared-secret comparison,
/// not a HMAC), then generic `X-Webhook-Signature`.
pub fn verify_signature(secret: &str, body: &[u8], headers: &HeaderMap) -> bool {
    // GitHub: HMAC-SHA256 of body, hex-encoded, prefixed `sha256=`.
    if let Some(sig) = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
    {
        let hex = sig.strip_prefix("sha256=").unwrap_or(sig);
        return verify_hmac(secret, body, hex);
    }
    // GitLab: token-only header. The whole point of GitLab's
    // webhook signature is "shared secret in a header", and we
    // honor that even though it's weaker than HMAC-of-body.
    if let Some(token) = headers.get("X-Gitlab-Token").and_then(|v| v.to_str().ok()) {
        return constant_time_eq(secret.as_bytes(), token.as_bytes());
    }
    // Generic: HMAC-SHA256 of body. Same shape as GitHub but
    // differently-named header.
    if let Some(sig) = headers
        .get("X-Webhook-Signature")
        .and_then(|v| v.to_str().ok())
    {
        let hex = sig.strip_prefix("sha256=").unwrap_or(sig);
        return verify_hmac(secret, body, hex);
    }
    false
}

fn verify_hmac(secret: &str, body: &[u8], expected_hex: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let computed = mac.finalize().into_bytes();
    let expected = match hex::decode(expected_hex.trim()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    constant_time_eq(&computed, &expected)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Read the event type from the request: GitHub's
/// `X-GitHub-Event`, GitLab's `X-Gitlab-Event`, generic's
/// `X-Webhook-Event`, then a top-level `event_type` field in the
/// payload, falling back to "webhook".
pub fn extract_event_type(headers: &HeaderMap, payload: &Value) -> String {
    for header in ["X-GitHub-Event", "X-Gitlab-Event", "X-Webhook-Event"] {
        if let Some(v) = headers.get(header).and_then(|v| v.to_str().ok()) {
            return v.to_string();
        }
    }
    if let Some(v) = payload.get("event_type").and_then(|v| v.as_str()) {
        return v.to_string();
    }
    "webhook".into()
}

/// Compute the idempotency cache key for a request.
fn idempotency_key(route: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(body);
    format!("{}:{}", route, hex::encode(h.finalize()))
}

/// Render the prompt template with dot-notation lookups against
/// `payload`. The two synthetic placeholders `{event}` and
/// `{route}` resolve from the supplied values; everything else is
/// looked up against the JSON payload using
/// `path1.path2.path3` notation.
///
/// Missing keys render as the empty string with a debug log so a
/// typo in a template doesn't silently swallow a value.
pub fn render_prompt(template: &str, payload: &Value, event: &str, route: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        // Read until the matching '}'.
        let mut placeholder = String::new();
        let mut closed = false;
        while let Some(next) = chars.next() {
            if next == '}' {
                closed = true;
                break;
            }
            placeholder.push(next);
        }
        if !closed {
            // Unclosed brace: emit verbatim and stop trying.
            out.push('{');
            out.push_str(&placeholder);
            break;
        }
        let resolved = resolve_placeholder(&placeholder, payload, event, route);
        out.push_str(&resolved);
    }
    out
}

fn resolve_placeholder(name: &str, payload: &Value, event: &str, route: &str) -> String {
    match name.trim() {
        "event" => event.to_string(),
        "route" => route.to_string(),
        "payload" => payload.to_string(),
        other => match dot_lookup(payload, other) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => {
                tracing::debug!(
                    placeholder = name,
                    "webhook prompt template: placeholder did not resolve"
                );
                String::new()
            }
        },
    }
}

fn dot_lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = root;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn config(routes: HashMap<String, WebhookRouteEntry>) -> WebhookChannelEntry {
        WebhookChannelEntry {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 0,
            secret: String::new(),
            idempotency_ttl_secs: 60,
            rate_limit_per_minute: 30,
            routes,
        }
    }

    // -- HMAC verification ------------------------------------------

    fn github_signature(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn github_signature_verifies() {
        let body = b"{\"hello\":\"world\"}";
        let secret = "test-secret";
        let sig = github_signature(secret, body);
        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(verify_signature(secret, body, &headers));
    }

    #[test]
    fn github_signature_rejects_wrong_secret() {
        let body = b"{}";
        let sig = github_signature("right", body);
        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("wrong", body, &headers));
    }

    #[test]
    fn github_signature_rejects_tampered_body() {
        let sig = github_signature("k", b"original");
        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("k", b"tampered", &headers));
    }

    #[test]
    fn gitlab_token_verifies_when_equal() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Gitlab-Token", HeaderValue::from_static("shared-secret"));
        assert!(verify_signature("shared-secret", b"any body", &headers));
    }

    #[test]
    fn gitlab_token_rejects_when_unequal() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Gitlab-Token", HeaderValue::from_static("wrong"));
        assert!(!verify_signature("right", b"any", &headers));
    }

    #[test]
    fn generic_signature_verifies() {
        let body = b"{}";
        let sig = github_signature("k", body); // same algorithm
        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Signature", HeaderValue::from_str(&sig).unwrap());
        assert!(verify_signature("k", body, &headers));
    }

    #[test]
    fn no_signature_header_fails() {
        let headers = HeaderMap::new();
        assert!(!verify_signature("k", b"body", &headers));
    }

    // -- prompt template --------------------------------------------

    #[test]
    fn template_resolves_event_and_route() {
        let payload = serde_json::json!({});
        let out = render_prompt("event={event} route={route}", &payload, "push", "ci");
        assert_eq!(out, "event=push route=ci");
    }

    #[test]
    fn template_resolves_dot_path() {
        let payload = serde_json::json!({
            "pull_request": { "title": "Add feature" }
        });
        let out = render_prompt(
            "PR: {pull_request.title}",
            &payload,
            "pull_request",
            "ci",
        );
        assert_eq!(out, "PR: Add feature");
    }

    #[test]
    fn template_missing_key_renders_empty() {
        let payload = serde_json::json!({});
        let out = render_prompt("X={missing.key} done", &payload, "e", "r");
        assert_eq!(out, "X= done");
    }

    #[test]
    fn template_unclosed_brace_emits_verbatim() {
        let payload = serde_json::json!({});
        let out = render_prompt("hello {oops", &payload, "e", "r");
        assert_eq!(out, "hello {oops");
    }

    #[test]
    fn template_payload_placeholder_serializes() {
        let payload = serde_json::json!({"x": 1});
        let out = render_prompt("p={payload}", &payload, "e", "r");
        assert!(out.contains("\"x\""));
    }

    #[test]
    fn template_handles_nested_objects() {
        let payload = serde_json::json!({
            "a": { "b": { "c": "deep" } }
        });
        assert_eq!(
            render_prompt("{a.b.c}", &payload, "e", "r"),
            "deep"
        );
    }

    // -- event extraction -------------------------------------------

    #[test]
    fn event_from_github_header() {
        let mut h = HeaderMap::new();
        h.insert("X-GitHub-Event", HeaderValue::from_static("push"));
        let p = serde_json::json!({});
        assert_eq!(extract_event_type(&h, &p), "push");
    }

    #[test]
    fn event_from_gitlab_header() {
        let mut h = HeaderMap::new();
        h.insert("X-Gitlab-Event", HeaderValue::from_static("Merge Request Hook"));
        let p = serde_json::json!({});
        assert_eq!(extract_event_type(&h, &p), "Merge Request Hook");
    }

    #[test]
    fn event_from_payload_field() {
        let h = HeaderMap::new();
        let p = serde_json::json!({"event_type": "deploy"});
        assert_eq!(extract_event_type(&h, &p), "deploy");
    }

    #[test]
    fn event_falls_back_to_webhook() {
        let h = HeaderMap::new();
        let p = serde_json::json!({});
        assert_eq!(extract_event_type(&h, &p), "webhook");
    }

    // -- idempotency -----------------------------------------------

    #[test]
    fn idempotency_first_call_records() {
        let mut cache = IdempotencyCache::default();
        assert!(!cache.check_and_record("k1".into(), Duration::from_secs(60)));
    }

    #[test]
    fn idempotency_duplicate_within_ttl_is_blocked() {
        let mut cache = IdempotencyCache::default();
        cache.check_and_record("k1".into(), Duration::from_secs(60));
        assert!(cache.check_and_record("k1".into(), Duration::from_secs(60)));
    }

    #[test]
    fn idempotency_expires_after_ttl() {
        let mut cache = IdempotencyCache::default();
        cache.check_and_record("k1".into(), Duration::from_nanos(1));
        std::thread::sleep(Duration::from_millis(5));
        // Record again — should NOT be flagged duplicate because
        // the previous entry was pruned.
        assert!(!cache.check_and_record("k1".into(), Duration::from_nanos(1)));
    }

    #[test]
    fn idempotency_distinct_keys_independent() {
        let mut cache = IdempotencyCache::default();
        assert!(!cache.check_and_record("a".into(), Duration::from_secs(60)));
        assert!(!cache.check_and_record("b".into(), Duration::from_secs(60)));
        assert!(cache.check_and_record("a".into(), Duration::from_secs(60)));
        assert!(cache.check_and_record("b".into(), Duration::from_secs(60)));
    }

    // -- rate limit -------------------------------------------------

    #[test]
    fn rate_limit_under_quota_passes() {
        let mut r = RateLimiter::default();
        for _ in 0..30 {
            assert!(!r.check_and_record("route", 30));
        }
    }

    #[test]
    fn rate_limit_over_quota_blocks() {
        let mut r = RateLimiter::default();
        for _ in 0..30 {
            r.check_and_record("route", 30);
        }
        assert!(r.check_and_record("route", 30));
    }

    #[test]
    fn rate_limit_per_route_isolated() {
        let mut r = RateLimiter::default();
        for _ in 0..30 {
            r.check_and_record("route_a", 30);
        }
        // route_a is full; route_b is fresh.
        assert!(r.check_and_record("route_a", 30));
        assert!(!r.check_and_record("route_b", 30));
    }

    // -- channel construction ---------------------------------------

    #[test]
    fn from_config_disabled_returns_none() {
        let mut cfg = config(HashMap::new());
        cfg.enabled = false;
        assert!(WebhookChannel::from_config(&cfg).is_none());
    }

    #[test]
    fn from_config_enabled_returns_some_even_without_routes() {
        let cfg = config(HashMap::new());
        let ch = WebhookChannel::from_config(&cfg);
        assert!(ch.is_some());
    }

    #[test]
    fn channel_send_is_no_op_error() {
        let cfg = config(HashMap::new());
        let ch = WebhookChannel::from_config(&cfg).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(ch.send(&SendMessage::new("hi", "x")));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("inbound-only"));
    }

    #[test]
    fn channel_name_is_webhook() {
        let cfg = config(HashMap::new());
        let ch = WebhookChannel::from_config(&cfg).unwrap();
        assert_eq!(ch.name(), "webhook");
    }

    // -- end-to-end via axum router --------------------------------

    /// Build the same axum router the channel uses, but bypass the
    /// real listener so we can call handlers directly.
    fn test_router(state: WebhookHandlerState) -> Router {
        Router::new()
            .route("/health", get(health_handler))
            .route("/webhook/{route_name}", post(webhook_handler))
            .with_state(state)
    }

    #[tokio::test]
    async fn router_returns_404_for_unknown_route() {
        use tower::ServiceExt;

        let cfg = config(HashMap::new());
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx: mpsc::channel(1).0,
        };
        let app = test_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/missing")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_accepts_valid_github_webhook() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: "ssss".into(),
                events: vec![],
                prompt: "got {event} on {route}".into(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let cfg = config(routes);
        let (tx, mut rx) = mpsc::channel(8);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx,
        };

        let body = b"{\"x\":1}".to_vec();
        let sig = github_signature("ssss", &body);
        let app = test_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .header("X-GitHub-Event", "push")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.channel, "webhook:ci");
        assert!(inbound.content.contains("got push"));
    }

    #[tokio::test]
    async fn router_rejects_bad_signature() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: "right".into(),
                events: vec![],
                prompt: String::new(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let cfg = config(routes);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx: mpsc::channel(1).0,
        };

        let body = b"{}".to_vec();
        let sig = github_signature("wrong", &body);
        let app = test_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn router_filters_events_when_allowlist_set() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: "k".into(),
                events: vec!["push".into()],
                prompt: "x".into(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let cfg = config(routes);
        let (tx, mut rx) = mpsc::channel(8);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx,
        };

        let body = b"{}".to_vec();
        let sig = github_signature("k", &body);
        let app = test_router(state);
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .header("X-GitHub-Event", "issue_comment") // not in allowlist
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(rx.try_recv().is_err()); // no message forwarded
    }

    #[tokio::test]
    async fn router_dedupes_identical_body_within_ttl() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: "k".into(),
                events: vec![],
                prompt: "x".into(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let cfg = config(routes);
        let (tx, mut rx) = mpsc::channel(8);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx,
        };

        let body = b"{\"y\":2}".to_vec();
        let sig = github_signature("k", &body);
        let app = test_router(state);

        let r1 = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .body(axum::body::Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);

        let r2 = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);

        // First call forwarded, second was deduped.
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn router_rate_limits() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: "k".into(),
                events: vec![],
                prompt: "x".into(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let mut cfg = config(routes);
        cfg.rate_limit_per_minute = 2;
        let (tx, _rx) = mpsc::channel(8);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx,
        };

        // Three identical-but-different-body requests so idempotency
        // doesn't fire.
        let app = test_router(state);
        for i in 0..2 {
            let body = format!("{{\"i\":{}}}", i).into_bytes();
            let sig = github_signature("k", &body);
            let r = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/webhook/ci")
                        .header("X-Hub-Signature-256", &sig)
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        let body = b"{\"i\":99}".to_vec();
        let sig = github_signature("k", &body);
        let r = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .header("X-Hub-Signature-256", &sig)
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn insecure_no_auth_skips_signature_check() {
        use tower::ServiceExt;

        let mut routes = HashMap::new();
        routes.insert(
            "ci".into(),
            WebhookRouteEntry {
                secret: INSECURE_NO_AUTH.into(),
                events: vec![],
                prompt: "x".into(),
                skills: vec![],
                deliver: "log".into(),
                deliver_extra: HashMap::new(),
            },
        );
        let cfg = config(routes);
        let (tx, mut rx) = mpsc::channel(8);
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx,
        };

        let app = test_router(state);
        let r = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhook/ci")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        use tower::ServiceExt;

        let cfg = config(HashMap::new());
        let state = WebhookHandlerState {
            state: Arc::new(WebhookState::new(&cfg)),
            tx: mpsc::channel(1).0,
        };
        let app = test_router(state);
        let r = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
}
