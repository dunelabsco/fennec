use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use fennec::providers::traits::{ChatRequest, ChatResponse, Provider, UsageInfo};
use fennec::providers::ReliableProvider;

// ---------------------------------------------------------------------------
// Mock Providers
// ---------------------------------------------------------------------------

/// A mock provider that succeeds with a configured response.
struct SuccessProvider {
    provider_name: String,
    call_count: AtomicUsize,
}

impl SuccessProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_string(),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for SuccessProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn chat(&self, _request: ChatRequest<'_>) -> Result<ChatResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(ChatResponse {
            content: Some(format!("response from {}", self.provider_name)),
            tool_calls: vec![],
            usage: Some(UsageInfo {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
            }),
        })
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        100_000
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

/// A mock provider that always fails with a specific error.
struct FailProvider {
    provider_name: String,
    error_msg: String,
    call_count: AtomicUsize,
}

impl FailProvider {
    fn new(name: &str, error_msg: &str) -> Self {
        Self {
            provider_name: name.to_string(),
            error_msg: error_msg.to_string(),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for FailProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn chat(&self, _request: ChatRequest<'_>) -> Result<ChatResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("{}", self.error_msg)
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        100_000
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

/// A mock provider that fails N times then succeeds.
struct FlakeyProvider {
    provider_name: String,
    failures_remaining: Mutex<usize>,
    call_count: AtomicUsize,
}

impl FlakeyProvider {
    fn new(name: &str, fail_count: usize) -> Self {
        Self {
            provider_name: name.to_string(),
            failures_remaining: Mutex::new(fail_count),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for FlakeyProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn chat(&self, _request: ChatRequest<'_>) -> Result<ChatResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut remaining = self.failures_remaining.lock();
        if *remaining > 0 {
            *remaining -= 1;
            anyhow::bail!("temporary error from {}", self.provider_name)
        } else {
            Ok(ChatResponse {
                content: Some(format!("recovered response from {}", self.provider_name)),
                tool_calls: vec![],
                usage: None,
            })
        }
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        50_000
    }

    async fn chat_stream(&self, request: ChatRequest<'_>) -> anyhow::Result<tokio::sync::mpsc::Receiver<fennec::providers::traits::StreamEvent>> {
        fennec::providers::traits::default_chat_stream(self, request).await
    }
}

fn make_request() -> ChatRequest<'static> {
    ChatRequest {
        system: None,
        messages: &[],
        tools: None,
        max_tokens: 1024,
        temperature: 0.7,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn primary_succeeds() {
    let primary = SuccessProvider::new("primary");
    let fallback = SuccessProvider::new("fallback");

    let reliable = ReliableProvider::new(
        vec![Box::new(primary), Box::new(fallback)],
        Some(3),
    );

    let result = reliable.chat(make_request()).await.unwrap();
    assert_eq!(result.content.as_deref(), Some("response from primary"));
}

#[tokio::test]
async fn primary_fails_then_fallback_succeeds() {
    let primary = FailProvider::new("primary", "server error");
    let fallback = SuccessProvider::new("fallback");

    let reliable = ReliableProvider::new(
        vec![Box::new(primary), Box::new(fallback)],
        Some(1), // Only 1 retry per provider to speed up test.
    );

    let result = reliable.chat(make_request()).await.unwrap();
    assert_eq!(result.content.as_deref(), Some("response from fallback"));
}

#[tokio::test]
async fn all_providers_fail_returns_error() {
    let primary = FailProvider::new("primary", "error 1");
    let fallback = FailProvider::new("fallback", "error 2");

    let reliable = ReliableProvider::new(
        vec![Box::new(primary), Box::new(fallback)],
        Some(1),
    );

    let result = reliable.chat(make_request()).await;
    assert!(result.is_err());
    // Should contain the last error message.
    let err = result.unwrap_err().to_string();
    assert!(err.contains("error 2"), "expected last error, got: {err}");
}

#[tokio::test]
async fn rate_limit_skips_to_fallback() {
    let primary = FailProvider::new("primary", "HTTP 429 rate limit exceeded");
    let fallback = SuccessProvider::new("fallback");

    let reliable = ReliableProvider::new(
        vec![Box::new(primary), Box::new(fallback)],
        Some(3),
    );

    let result = reliable.chat(make_request()).await.unwrap();
    // Should have skipped to the fallback after rate limit on primary.
    assert_eq!(result.content.as_deref(), Some("response from fallback"));
}

#[tokio::test]
async fn flakey_provider_retries_and_succeeds() {
    let flakey = FlakeyProvider::new("flakey", 1); // fail once, then succeed

    let reliable = ReliableProvider::new(
        vec![Box::new(flakey)],
        Some(3),
    );

    let result = reliable.chat(make_request()).await.unwrap();
    assert_eq!(
        result.content.as_deref(),
        Some("recovered response from flakey")
    );
}

#[tokio::test]
async fn no_providers_returns_error() {
    let reliable = ReliableProvider::new(vec![], Some(3));
    let result = reliable.chat(make_request()).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("no providers available"),
        "expected 'no providers available', got: {err}"
    );
}

#[tokio::test]
async fn reliable_delegates_supports_tool_calling() {
    let provider = SuccessProvider::new("test");
    let reliable = ReliableProvider::new(vec![Box::new(provider)], None);
    assert!(reliable.supports_tool_calling());
}

#[tokio::test]
async fn reliable_delegates_context_window() {
    let provider = SuccessProvider::new("test");
    let reliable = ReliableProvider::new(vec![Box::new(provider)], None);
    assert_eq!(reliable.context_window(), 100_000);
}

#[tokio::test]
async fn reliable_name() {
    let provider = SuccessProvider::new("test");
    let reliable = ReliableProvider::new(vec![Box::new(provider)], None);
    assert_eq!(reliable.name(), "reliable");
}
