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
//! Signature formats supported (in priority order, per a fresh
//! check of GitHub and GitLab live docs):
//!
//!   1. **Standard Webhooks** (GitLab's recommended new mode, also
//!      Svix and others): three headers `webhook-id`,
//!      `webhook-timestamp`, `webhook-signature: v1,<base64>`. The
//!      signed message is `{id}.{timestamp}.{body}`. The secret is
//!      `whsec_<base64>`; we strip the prefix and base64-decode to
//!      get the raw HMAC key. Multiple space-delimited signatures
//!      are accepted (key rotation). The `webhook-timestamp` must
//!      be within `STANDARD_WEBHOOKS_TOLERANCE_SECS` of now.
//!   2. **GitHub**: `X-Hub-Signature-256: sha256=<hex>` —
//!      HMAC-SHA256 over the raw request body, secret used directly.
//!   3. **GitLab legacy "Secret Token"**: `X-Gitlab-Token: <token>`,
//!      plain shared-secret comparison. Weaker than the above but
//!      still GitLab's default for older webhooks.
//!   4. **Generic**: `X-Webhook-Signature: sha256=<hex>` —
//!      GitHub-shaped HMAC for non-GitHub sources.
//!
//! GitHub's deprecated `X-Hub-Signature` (HMAC-SHA1) is intentionally
//! not supported. SHA-1 is cryptographically broken and GitHub
//! itself recommends `-256` only; accepting `-1` would weaken the
//! security posture for negligible compatibility gain (every active
//! GitHub webhook sends both headers; we just ignore the SHA-1 one).
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
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
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

/// Tolerance window for `webhook-timestamp` in the Standard Webhooks
/// signature scheme. Requests whose timestamp is more than this many
/// seconds away from local clock are rejected as replay attempts.
/// 5 minutes matches the typical recommendation from the spec.
pub const STANDARD_WEBHOOKS_TOLERANCE_SECS: i64 = 300;

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
/// secret. Priority order:
///
///   1. Standard Webhooks (GitLab signing-token, Svix, etc.) when
///      all three `webhook-*` headers are present.
///   2. GitHub `X-Hub-Signature-256`.
///   3. GitLab legacy `X-Gitlab-Token` (plain shared-secret
///      comparison, kept for backward compatibility with older
///      GitLab "Secret Token" webhooks).
///   4. Generic `X-Webhook-Signature` (HMAC-SHA256 same as GitHub).
pub fn verify_signature(secret: &str, body: &[u8], headers: &HeaderMap) -> bool {
    // 1. Standard Webhooks: detected by presence of `webhook-signature`.
    if headers.get("webhook-signature").is_some() {
        return verify_standard_webhooks(secret, body, headers);
    }
    // 2. GitHub: HMAC-SHA256 of body, hex-encoded, prefixed `sha256=`.
    if let Some(sig) = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
    {
        let hex = sig.strip_prefix("sha256=").unwrap_or(sig);
        return verify_hmac_hex(secret.as_bytes(), body, hex);
    }
    // 3. GitLab legacy: token-only header.
    if let Some(token) = headers.get("X-Gitlab-Token").and_then(|v| v.to_str().ok()) {
        return constant_time_eq(secret.as_bytes(), token.as_bytes());
    }
    // 4. Generic: HMAC-SHA256 of body, GitHub-shaped header.
    if let Some(sig) = headers
        .get("X-Webhook-Signature")
        .and_then(|v| v.to_str().ok())
    {
        let hex = sig.strip_prefix("sha256=").unwrap_or(sig);
        return verify_hmac_hex(secret.as_bytes(), body, hex);
    }
    false
}

/// Verify a Standard Webhooks signature. Required headers:
/// `webhook-id`, `webhook-timestamp`, `webhook-signature`. Returns
/// `false` on any malformed input — the caller can't tell parsing
/// failure from signature failure, but for an inbound HTTP request
/// either should produce 401.
///
/// `secret` is the user's webhook secret as configured. Per the
/// spec, the "real" key is `secret` with the `whsec_` prefix
/// stripped and the remainder base64-decoded. If `secret` doesn't
/// have the prefix we treat it as already-raw key bytes; this
/// keeps configurations friendly for users who don't know the
/// prefix convention.
fn verify_standard_webhooks(secret: &str, body: &[u8], headers: &HeaderMap) -> bool {
    let webhook_id = match headers.get("webhook-id").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return false,
    };
    let webhook_ts = match headers.get("webhook-timestamp").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return false,
    };
    let signature_header = match headers.get("webhook-signature").and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return false,
    };

    // Replay protection: timestamp must be within tolerance.
    let ts: i64 = match webhook_ts.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > STANDARD_WEBHOOKS_TOLERANCE_SECS {
        tracing::warn!(
            webhook_timestamp = ts,
            now = now,
            tolerance = STANDARD_WEBHOOKS_TOLERANCE_SECS,
            "Standard Webhooks: timestamp outside tolerance window — possible replay"
        );
        return false;
    }

    // Derive raw HMAC key. Spec: `whsec_<base64>`.
    let key_bytes: Vec<u8> = match secret.strip_prefix("whsec_") {
        Some(b64) => match B64.decode(b64.trim()) {
            Ok(b) => b,
            Err(_) => return false,
        },
        None => secret.as_bytes().to_vec(),
    };

    // Build signed message: "{id}.{ts}.{body}".
    let mut signed =
        Vec::with_capacity(webhook_id.len() + webhook_ts.len() + body.len() + 2);
    signed.extend_from_slice(webhook_id.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(webhook_ts.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);

    let mut mac = match HmacSha256::new_from_slice(&key_bytes) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(&signed);
    let computed = mac.finalize().into_bytes();

    // Multiple signatures may appear space-delimited (key rotation):
    //   webhook-signature: v1,sigA v1,sigB v1a,otherSig
    // Any one matching is enough. Unknown version prefixes (`v1a,`,
    // future versions) are skipped, not rejected.
    for entry in signature_header.split_whitespace() {
        let Some(b64_sig) = entry.strip_prefix("v1,") else {
            continue;
        };
        let Ok(sig_bytes) = B64.decode(b64_sig) else {
            continue;
        };
        if constant_time_eq(&computed, &sig_bytes) {
            return true;
        }
    }
    false
}

fn verify_hmac_hex(key: &[u8], body: &[u8], expected_hex: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(key) {
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

    // -- Standard Webhooks signing-token format --------------------

    /// Helper: produce a Standard-Webhooks-shaped signature header
    /// for the given key + id + timestamp + body.
    fn standard_signature(key_bytes: &[u8], id: &str, ts: &str, body: &[u8]) -> String {
        let mut signed = Vec::new();
        signed.extend_from_slice(id.as_bytes());
        signed.push(b'.');
        signed.extend_from_slice(ts.as_bytes());
        signed.push(b'.');
        signed.extend_from_slice(body);
        let mut mac = HmacSha256::new_from_slice(key_bytes).unwrap();
        mac.update(&signed);
        format!("v1,{}", B64.encode(mac.finalize().into_bytes()))
    }

    fn now_ts() -> String {
        chrono::Utc::now().timestamp().to_string()
    }

    #[test]
    fn standard_webhooks_verifies_with_raw_key() {
        let key = b"raw-secret-bytes";
        let id = "msg_1";
        let ts = now_ts();
        let body = b"{\"x\":1}";
        let sig = standard_signature(key, id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        // Pass the secret as-is (no whsec_ prefix); the verifier
        // treats it as raw bytes.
        assert!(verify_signature(
            std::str::from_utf8(key).unwrap(),
            body,
            &headers
        ));
    }

    #[test]
    fn standard_webhooks_verifies_with_whsec_prefix() {
        // Realistic shape: secret is whsec_<base64-of-key>.
        let raw_key: Vec<u8> = (0..32u8).collect(); // 32 bytes of test key
        let secret = format!("whsec_{}", B64.encode(&raw_key));
        let id = "msg_2";
        let ts = now_ts();
        let body = b"hello";
        let sig = standard_signature(&raw_key, id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(verify_signature(&secret, body, &headers));
    }

    #[test]
    fn standard_webhooks_rejects_old_timestamp() {
        let key = b"k";
        let id = "msg_old";
        // 10 minutes in the past — well outside the 5-minute tolerance.
        let ts = (chrono::Utc::now().timestamp() - 600).to_string();
        let body = b"old";
        let sig = standard_signature(key, id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("k", body, &headers));
    }

    #[test]
    fn standard_webhooks_rejects_future_timestamp() {
        let key = b"k";
        let id = "msg_future";
        let ts = (chrono::Utc::now().timestamp() + 600).to_string();
        let body = b"future";
        let sig = standard_signature(key, id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("k", body, &headers));
    }

    #[test]
    fn standard_webhooks_rejects_wrong_key() {
        let id = "msg_wk";
        let ts = now_ts();
        let body = b"x";
        let sig = standard_signature(b"right", id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("wrong", body, &headers));
    }

    #[test]
    fn standard_webhooks_rejects_tampered_body() {
        let key = b"k";
        let id = "msg_tb";
        let ts = now_ts();
        let sig = standard_signature(key, id, &ts, b"original");
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("k", b"tampered", &headers));
    }

    #[test]
    fn standard_webhooks_rejects_tampered_id() {
        let key = b"k";
        let id = "msg_real";
        let ts = now_ts();
        let body = b"x";
        let sig = standard_signature(key, id, &ts, body);
        let mut headers = HeaderMap::new();
        // Lying about the id mid-flight invalidates the signature.
        headers.insert("webhook-id", HeaderValue::from_str("msg_fake").unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        assert!(!verify_signature("k", body, &headers));
    }

    #[test]
    fn standard_webhooks_accepts_one_of_multiple_signatures() {
        let key_old = b"old-key";
        let key_new = b"new-key";
        let id = "msg_rot";
        let ts = now_ts();
        let body = b"rotation-test";
        // Two signatures, space-delimited (key rotation).
        let sig_old = standard_signature(key_old, id, &ts, body);
        let sig_new = standard_signature(key_new, id, &ts, body);
        let combined = format!("{} {}", sig_old, sig_new);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&combined).unwrap());
        // Verifying with the new key works.
        assert!(verify_signature(
            std::str::from_utf8(key_new).unwrap(),
            body,
            &headers
        ));
        // Verifying with the old key also still works (during a
        // rotation window the source emits both).
        assert!(verify_signature(
            std::str::from_utf8(key_old).unwrap(),
            body,
            &headers
        ));
    }

    #[test]
    fn standard_webhooks_skips_unknown_version_prefixes() {
        let key = b"k";
        let id = "msg_uv";
        let ts = now_ts();
        let body = b"x";
        let sig = standard_signature(key, id, &ts, body);
        // Mix in a future version prefix the verifier doesn't know.
        let combined = format!("v2,unknown-base64-content {}", sig);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&combined).unwrap());
        assert!(verify_signature("k", body, &headers));
    }

    #[test]
    fn standard_webhooks_missing_id_or_timestamp_fails() {
        let mut headers = HeaderMap::new();
        headers.insert("webhook-signature", HeaderValue::from_static("v1,xx"));
        assert!(!verify_signature("k", b"x", &headers));
        // With only id but no timestamp.
        let mut headers = HeaderMap::new();
        headers.insert("webhook-signature", HeaderValue::from_static("v1,xx"));
        headers.insert("webhook-id", HeaderValue::from_static("a"));
        assert!(!verify_signature("k", b"x", &headers));
    }

    #[test]
    fn standard_webhooks_takes_priority_over_github_header() {
        // A request that somehow carries BOTH `webhook-signature` and
        // `X-Hub-Signature-256` should be dispatched to the Standard
        // Webhooks path (newer + stronger). With a valid `webhook-*`
        // bundle and a garbage GitHub header, verification still passes.
        let key = b"k";
        let id = "msg_pri";
        let ts = now_ts();
        let body = b"hello";
        let sig = standard_signature(key, id, &ts, body);
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert("webhook-timestamp", HeaderValue::from_str(&ts).unwrap());
        headers.insert("webhook-signature", HeaderValue::from_str(&sig).unwrap());
        headers.insert(
            "X-Hub-Signature-256",
            HeaderValue::from_static("sha256=garbage"),
        );
        assert!(verify_signature("k", body, &headers));
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
