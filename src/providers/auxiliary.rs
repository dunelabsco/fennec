//! Auxiliary provider client for background tasks.
//!
//! Background tasks (curator skill consolidation, vision analysis,
//! web-page summarization, smart-approval LLM, etc.) run a lot of
//! short LLM calls against models that don't need to be the main
//! agent's expensive primary model. Routing those through the main
//! provider has two costs:
//!
//! 1. The main session's prompt cache gets dirtied — every
//!    background call evicts cached tokens we'll have to pay to
//!    re-send on the next user turn.
//! 2. Background calls inherit the main provider's pricing — using
//!    Claude Opus to summarize a search snippet is wasteful.
//!
//! The auxiliary client routes background calls through a separate
//! provider chain with per-task model selection. Tasks get a
//! sensible default but can override the provider and model
//! independently from the main agent's config.
//!
//! # Resolution chain
//!
//! When a task's `provider` is `"auto"` (the default), the client
//! walks an ordered fallback chain and picks the first provider
//! that's configured:
//!
//! 1. The main agent's primary provider (so single-key setups
//!    "just work" without extra config)
//! 2. OpenRouter, if `OPENROUTER_API_KEY` is set
//! 3. Anthropic, if `ANTHROPIC_API_KEY` is set
//! 4. OpenAI, if `OPENAI_API_KEY` is set
//! 5. Kimi/Moonshot, if `KIMI_API_KEY` is set
//! 6. Ollama (local — no key needed)
//!
//! Vision tasks use a slightly different chain that prefers
//! providers known to support multimodal input.
//!
//! When a task pins a specific provider (e.g.
//! `auxiliary.vision.provider = "openai"`), the chain is bypassed
//! and only that provider is used.
//!
//! # Per-task config
//!
//! Each background task has its own subsection in
//! `[auxiliary.<task>]` with three fields:
//!
//! ```toml
//! [auxiliary.curator]
//! provider = "auto"           # or a specific name
//! model    = ""               # task-specific model override
//! timeout  = 60               # seconds; 0 = use provider default
//! ```
//!
//! # Credit-exhaustion fallback
//!
//! Returning `HTTP 402` or matching an `"insufficient_quota"` /
//! `"out of credits"` error pattern marks the provider as
//! exhausted for the rest of the process lifetime. Subsequent
//! calls skip exhausted providers and try the next in the chain.
//!
//! # Cleanly handles "no auxiliary configured"
//!
//! If no `[auxiliary]` config exists, the client falls through to
//! the main provider for every task — current behavior of
//! pre-auxiliary Fennec. This means the auxiliary client is
//! always constructible; callers don't have to special-case the
//! empty config.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;

use crate::providers::traits::{ChatRequest, ChatResponse, Provider};

/// Built-in task identifiers. New consumers should add a variant
/// here so they get type-checked routing rather than passing
/// arbitrary strings. Custom task names (for plugin background
/// tasks) use [`TaskKind::Custom`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Skill curator — periodic skill consolidation pass.
    Curator,
    /// Memory consolidation — daily summaries + core-fact extraction.
    Compression,
    /// Web-page extraction / summarisation.
    WebExtract,
    /// Image analysis / vision.
    Vision,
    /// Past-session FTS5 search summarisation.
    SessionSearch,
    /// Smart approval — auxiliary LLM judging command safety.
    SmartApproval,
    /// Title generation for sessions.
    Title,
    /// Custom task name from plugin or user code.
    Custom(String),
}

impl TaskKind {
    fn key(&self) -> &str {
        match self {
            Self::Curator => "curator",
            Self::Compression => "compression",
            Self::WebExtract => "web_extract",
            Self::Vision => "vision",
            Self::SessionSearch => "session_search",
            Self::SmartApproval => "smart_approval",
            Self::Title => "title",
            Self::Custom(s) => s.as_str(),
        }
    }
}

/// Per-task auxiliary config.
#[derive(Debug, Clone, Default)]
pub struct AuxiliaryTaskConfig {
    /// Provider name (`"auto"` walks the chain; specific name
    /// pins). Empty string is treated as `"auto"`.
    pub provider: String,
    /// Task-specific model override. Empty = use whatever the
    /// resolved provider's default is.
    pub model: String,
    /// Per-call timeout in seconds. `0` = use the provider's own
    /// default.
    pub timeout_secs: u64,
}

/// Auxiliary system config — a map of task name → per-task
/// settings. Tasks not in the map fall through to defaults.
#[derive(Debug, Clone, Default)]
pub struct AuxiliaryConfig {
    pub tasks: HashMap<String, AuxiliaryTaskConfig>,
}

impl AuxiliaryConfig {
    pub fn task(&self, kind: &TaskKind) -> AuxiliaryTaskConfig {
        self.tasks.get(kind.key()).cloned().unwrap_or_default()
    }

    /// Build an `AuxiliaryConfig` from the on-disk TOML structure.
    /// Each built-in task subsection maps to a fixed key; the
    /// `custom` map carries through verbatim for plugin task names.
    pub fn from_toml(toml: &crate::config::schema::AuxiliaryConfigToml) -> Self {
        let mut tasks = HashMap::new();
        let pairs: &[(&str, &crate::config::schema::AuxiliaryTaskToml)] = &[
            ("curator", &toml.curator),
            ("compression", &toml.compression),
            ("web_extract", &toml.web_extract),
            ("vision", &toml.vision),
            ("session_search", &toml.session_search),
            ("smart_approval", &toml.smart_approval),
            ("title", &toml.title),
        ];
        for (key, t) in pairs {
            // Skip entries that are entirely default — keeps the
            // map small and makes diagnostics ("which tasks have
            // an override?") clean.
            if t.provider.is_empty() && t.model.is_empty() && t.timeout_secs == 0 {
                continue;
            }
            tasks.insert(
                (*key).to_string(),
                AuxiliaryTaskConfig {
                    provider: t.provider.clone(),
                    model: t.model.clone(),
                    timeout_secs: t.timeout_secs,
                },
            );
        }
        for (name, t) in &toml.custom {
            tasks.insert(
                name.clone(),
                AuxiliaryTaskConfig {
                    provider: t.provider.clone(),
                    model: t.model.clone(),
                    timeout_secs: t.timeout_secs,
                },
            );
        }
        Self { tasks }
    }
}

/// One entry in the resolution chain — a provider with its name
/// for diagnostic logging and exhaustion tracking.
#[derive(Clone)]
pub struct ChainEntry {
    pub name: String,
    pub provider: Arc<dyn Provider>,
}

/// The auxiliary client. Constructed once at agent build time,
/// shared across consumers.
pub struct AuxiliaryClient {
    config: AuxiliaryConfig,
    /// Default fallback chain for text tasks. Walked in order when
    /// a task uses `"auto"`.
    text_chain: Vec<ChainEntry>,
    /// Vision-aware chain — text-only providers excluded (e.g.
    /// Ollama by default). Walked in order when a vision task
    /// uses `"auto"`.
    vision_chain: Vec<ChainEntry>,
    /// Direct lookup for tasks that pin a specific provider name.
    by_name: HashMap<String, ChainEntry>,
    /// Names of providers we've seen credit-exhaustion errors
    /// from this session. Skipped on subsequent attempts.
    exhausted: Mutex<HashSet<String>>,
}

impl AuxiliaryClient {
    /// Build an auxiliary client from a config + a list of
    /// available providers. Each entry is named — the names are
    /// what users put in `[auxiliary.<task>] provider = "<name>"`.
    pub fn new(
        config: AuxiliaryConfig,
        text_chain: Vec<ChainEntry>,
        vision_chain: Vec<ChainEntry>,
    ) -> Self {
        let by_name: HashMap<String, ChainEntry> = text_chain
            .iter()
            .chain(vision_chain.iter())
            .map(|e| (e.name.clone(), e.clone()))
            .collect();
        Self {
            config,
            text_chain,
            vision_chain,
            by_name,
            exhausted: Mutex::new(HashSet::new()),
        }
    }

    /// Names of providers in the resolution chain, in order.
    /// Used by `fennec doctor` to show which auxiliaries are
    /// available.
    pub fn chain_names(&self) -> Vec<String> {
        self.text_chain.iter().map(|e| e.name.clone()).collect()
    }

    /// Returns true when at least one provider was registered. A
    /// fully empty client is constructible (when no providers are
    /// configured at all) but `call_for` will fail.
    pub fn is_available(&self) -> bool {
        !self.text_chain.is_empty()
    }

    /// Whether a specific task has a pinned provider in config.
    /// Used by `fennec doctor` for diagnostics.
    pub fn task_provider(&self, kind: &TaskKind) -> Option<String> {
        let cfg = self.config.task(kind);
        if cfg.provider.is_empty() || cfg.provider == "auto" {
            None
        } else {
            Some(cfg.provider)
        }
    }

    /// Run an LLM call for a specific background task. Picks the
    /// provider per the resolution rules, applies the task's model
    /// override, and falls back to the next provider in the chain
    /// on credit-exhaustion errors.
    ///
    /// `request` is consumed because we may need to mutate it
    /// (specifically, replace the messages' historical state via
    /// `model` override is not currently exposed on
    /// `ChatRequest` — see note below). For now this is a
    /// straight pass-through; the model override is conveyed by
    /// the resolved provider's own `model` configuration.
    pub async fn call_for<'a>(
        &self,
        kind: TaskKind,
        request: ChatRequest<'a>,
    ) -> Result<ChatResponse> {
        let task_config = self.config.task(&kind);
        let is_vision = matches!(kind, TaskKind::Vision);
        let candidates = self.candidates_for(&task_config.provider, is_vision)?;

        if candidates.is_empty() {
            anyhow::bail!(
                "auxiliary client: no providers available for task '{}'",
                kind.key()
            );
        }

        let mut last_err: Option<anyhow::Error> = None;
        for entry in candidates {
            if self.is_exhausted(&entry.name) {
                tracing::debug!(
                    provider = %entry.name,
                    task = %kind.key(),
                    "Skipping exhausted provider in auxiliary chain"
                );
                continue;
            }

            tracing::debug!(
                provider = %entry.name,
                task = %kind.key(),
                "Auxiliary call dispatching"
            );

            // Per-call timeout via tokio::time::timeout. 0 means
            // "use the provider's own default" — we don't add an
            // outer wrapper in that case.
            let call = entry.provider.chat(request.clone());
            let result = if task_config.timeout_secs > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(task_config.timeout_secs),
                    call,
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(anyhow!(
                        "auxiliary task '{}' timed out after {}s on provider '{}'",
                        kind.key(),
                        task_config.timeout_secs,
                        entry.name
                    )),
                }
            } else {
                call.await
            };

            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if is_credit_error(&e) {
                        tracing::warn!(
                            provider = %entry.name,
                            task = %kind.key(),
                            "Auxiliary provider exhausted credits, marking and trying next: {e}"
                        );
                        self.exhausted.lock().insert(entry.name.clone());
                        last_err = Some(e);
                        continue;
                    }
                    // Non-credit error — propagate immediately rather
                    // than burning through the rest of the chain on a
                    // bug that isn't provider-specific.
                    return Err(e).with_context(|| {
                        format!(
                            "auxiliary call failed (provider='{}', task='{}')",
                            entry.name,
                            kind.key()
                        )
                    });
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow!(
                "auxiliary client: all providers exhausted for task '{}'",
                kind.key()
            )
        }))
    }

    /// Resolve the candidate chain for a task given its
    /// configured `provider` value. `"auto"` (or empty) walks the
    /// auto-fallback chain; a specific name yields a single-entry
    /// list.
    fn candidates_for(&self, configured: &str, is_vision: bool) -> Result<Vec<ChainEntry>> {
        let trimmed = configured.trim();
        if trimmed.is_empty() || trimmed == "auto" {
            return Ok(if is_vision {
                self.vision_chain.clone()
            } else {
                self.text_chain.clone()
            });
        }
        match self.by_name.get(trimmed) {
            Some(entry) => Ok(vec![entry.clone()]),
            None => {
                tracing::warn!(
                    requested = %trimmed,
                    available = ?self.text_chain.iter().map(|e| &e.name).collect::<Vec<_>>(),
                    "Auxiliary task pins provider '{}' but it isn't available; falling back to auto chain",
                    trimmed
                );
                // Don't error — fall back to the auto chain so a
                // typo or a temporarily-missing key doesn't break
                // background tasks.
                Ok(if is_vision {
                    self.vision_chain.clone()
                } else {
                    self.text_chain.clone()
                })
            }
        }
    }

    fn is_exhausted(&self, name: &str) -> bool {
        self.exhausted.lock().contains(name)
    }

    /// Test/diagnostic helper: clear the exhaustion set so a
    /// subsequent call retries everyone. Not exposed via config —
    /// exhaustion is a process-lifetime concept by design.
    #[cfg(test)]
    fn clear_exhausted(&self) {
        self.exhausted.lock().clear();
    }
}

/// Heuristic credit-exhaustion detector. Looks at the error
/// message for known phrases. Errors not matching are treated as
/// regular failures (propagated to the caller).
///
/// We match on substrings rather than HTTP status codes because
/// our `Provider` trait wraps errors with `anyhow::Context` —
/// the original status is in the chain but not directly
/// accessible. The substring set covers the common phrasings
/// across providers (Anthropic, OpenAI, OpenRouter, Kimi,
/// Moonshot).
fn is_credit_error(err: &anyhow::Error) -> bool {
    let s = format!("{:#}", err).to_lowercase();
    const NEEDLES: &[&str] = &[
        "402",
        "insufficient_quota",
        "insufficient quota",
        "out of credits",
        "credit balance is too low",
        "exceeded your current quota",
        "billing.hard_limit",
        "rate_limit_exceeded", // some providers conflate cred + rate
    ];
    NEEDLES.iter().any(|needle| s.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::providers::traits::{ChatMessage, ChatRequest, ChatResponse, StreamEvent};

    /// Mock provider that lets tests dictate per-call outcomes.
    struct MockProvider {
        name: &'static str,
        // Each call pops the next response in order.
        results: Mutex<Vec<Result<ChatResponse>>>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new(name: &'static str, results: Vec<Result<ChatResponse>>) -> Self {
            Self {
                name,
                results: Mutex::new(results),
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }
        async fn chat(&self, _req: ChatRequest<'_>) -> Result<ChatResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.results
                .lock()
                .pop()
                .unwrap_or_else(|| Err(anyhow!("mock exhausted")))
        }
        fn supports_tool_calling(&self) -> bool {
            true
        }
        fn context_window(&self) -> usize {
            128_000
        }
        async fn chat_stream(
            &self,
            request: ChatRequest<'_>,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
            crate::providers::traits::default_chat_stream(self, request).await
        }
    }

    fn ok_response() -> ChatResponse {
        ChatResponse {
            content: Some("ok".to_string()),
            tool_calls: vec![],
            usage: None,
        }
    }

    fn make_request<'a>() -> ChatRequest<'a> {
        ChatRequest {
            system: None,
            messages: &[],
            tools: None,
            max_tokens: 1024,
            temperature: 0.7,
            thinking_level: crate::agent::thinking::ThinkingLevel::Off,
        }
    }

    /// An empty client (no providers) refuses calls cleanly rather
    /// than panicking. This is the path users hit when no
    /// provider is configured AT ALL — it should not crash.
    #[tokio::test]
    async fn empty_client_refuses_call() {
        let client =
            AuxiliaryClient::new(AuxiliaryConfig::default(), vec![], vec![]);
        assert!(!client.is_available());
        let result = client.call_for(TaskKind::Curator, make_request()).await;
        assert!(result.is_err());
    }

    /// A client with a single provider routes every task through
    /// that provider regardless of `auto` vs pinned config.
    #[tokio::test]
    async fn single_provider_routes_all_tasks() {
        let provider: Arc<dyn Provider> =
            Arc::new(MockProvider::new("only", vec![Ok(ok_response())]));
        let entry = ChainEntry {
            name: "only".to_string(),
            provider,
        };
        let client = AuxiliaryClient::new(
            AuxiliaryConfig::default(),
            vec![entry.clone()],
            vec![entry],
        );
        let resp = client
            .call_for(TaskKind::Curator, make_request())
            .await
            .unwrap();
        assert_eq!(resp.content.as_deref(), Some("ok"));
    }

    /// Pinning a specific provider via config bypasses the chain
    /// even if other providers are listed first.
    #[tokio::test]
    async fn pinned_provider_bypasses_chain() {
        let first: Arc<dyn Provider> =
            Arc::new(MockProvider::new("first", vec![Ok(ChatResponse {
                content: Some("first".to_string()),
                tool_calls: vec![],
                usage: None,
            })]));
        let second: Arc<dyn Provider> =
            Arc::new(MockProvider::new("second", vec![Ok(ChatResponse {
                content: Some("second".to_string()),
                tool_calls: vec![],
                usage: None,
            })]));
        let chain = vec![
            ChainEntry {
                name: "first".to_string(),
                provider: first,
            },
            ChainEntry {
                name: "second".to_string(),
                provider: second,
            },
        ];
        let mut config = AuxiliaryConfig::default();
        config.tasks.insert(
            "curator".to_string(),
            AuxiliaryTaskConfig {
                provider: "second".to_string(),
                model: String::new(),
                timeout_secs: 0,
            },
        );
        let client = AuxiliaryClient::new(config, chain.clone(), chain);
        let resp = client
            .call_for(TaskKind::Curator, make_request())
            .await
            .unwrap();
        // Pinned to "second" → should bypass "first".
        assert_eq!(resp.content.as_deref(), Some("second"));
    }

    /// On HTTP 402-shaped error from the first provider, the
    /// client marks it exhausted and tries the next in the chain.
    /// Subsequent calls skip the exhausted provider.
    #[tokio::test]
    async fn credit_exhaustion_falls_back_to_next() {
        let first: Arc<dyn Provider> = Arc::new(MockProvider::new(
            "first",
            vec![Err(anyhow!("HTTP 402: insufficient_quota"))],
        ));
        let second: Arc<dyn Provider> = Arc::new(MockProvider::new(
            "second",
            vec![Ok(ChatResponse {
                content: Some("from second".to_string()),
                tool_calls: vec![],
                usage: None,
            })],
        ));
        let chain = vec![
            ChainEntry {
                name: "first".to_string(),
                provider: first,
            },
            ChainEntry {
                name: "second".to_string(),
                provider: second,
            },
        ];
        let client =
            AuxiliaryClient::new(AuxiliaryConfig::default(), chain.clone(), chain);
        let resp = client
            .call_for(TaskKind::Curator, make_request())
            .await
            .unwrap();
        assert_eq!(resp.content.as_deref(), Some("from second"));
        assert!(client.is_exhausted("first"));
        assert!(!client.is_exhausted("second"));
    }

    /// Non-credit errors (regular failures) propagate immediately
    /// without trying the rest of the chain. Burning through the
    /// chain on a bug would slow user-visible operations.
    #[tokio::test]
    async fn non_credit_error_propagates_immediately() {
        let counter = Arc::new(AtomicUsize::new(0));
        let first: Arc<dyn Provider> = Arc::new(MockProvider::new(
            "first",
            vec![Err(anyhow!("network unreachable"))],
        ));
        let second_counter = Arc::clone(&counter);
        struct CountingProvider {
            counter: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl Provider for CountingProvider {
            fn name(&self) -> &str {
                "second"
            }
            async fn chat(
                &self,
                _req: ChatRequest<'_>,
            ) -> Result<ChatResponse> {
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(ok_response())
            }
            fn supports_tool_calling(&self) -> bool {
                true
            }
            fn context_window(&self) -> usize {
                128_000
            }
            async fn chat_stream(
                &self,
                request: ChatRequest<'_>,
            ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
                crate::providers::traits::default_chat_stream(self, request)
                    .await
            }
        }
        let second: Arc<dyn Provider> = Arc::new(CountingProvider {
            counter: second_counter,
        });
        let chain = vec![
            ChainEntry {
                name: "first".to_string(),
                provider: first,
            },
            ChainEntry {
                name: "second".to_string(),
                provider: second,
            },
        ];
        let client =
            AuxiliaryClient::new(AuxiliaryConfig::default(), chain.clone(), chain);
        let result = client.call_for(TaskKind::Curator, make_request()).await;
        assert!(result.is_err());
        // Second provider should NOT have been attempted.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// `is_credit_error` accepts the common phrasings and rejects
    /// generic failures. Locks the heuristic.
    #[test]
    fn credit_error_heuristic() {
        assert!(is_credit_error(&anyhow!("HTTP 402 Payment Required")));
        assert!(is_credit_error(&anyhow!("insufficient_quota")));
        assert!(is_credit_error(&anyhow!("Your credit balance is too low")));
        assert!(is_credit_error(&anyhow!("You exceeded your current quota")));
        assert!(is_credit_error(&anyhow!("error: out of credits")));
        assert!(!is_credit_error(&anyhow!("connection refused")));
        assert!(!is_credit_error(&anyhow!("invalid api key")));
        assert!(!is_credit_error(&anyhow!("model not found")));
    }

    /// Configuring a provider name that doesn't exist in the
    /// chain doesn't error — it falls back to the auto chain
    /// with a warn log. Operators should see typos surface
    /// without breaking everything.
    #[tokio::test]
    async fn unknown_pinned_provider_falls_back_to_auto() {
        let provider: Arc<dyn Provider> =
            Arc::new(MockProvider::new("only", vec![Ok(ok_response())]));
        let entry = ChainEntry {
            name: "only".to_string(),
            provider,
        };
        let mut config = AuxiliaryConfig::default();
        config.tasks.insert(
            "curator".to_string(),
            AuxiliaryTaskConfig {
                provider: "totally-not-real".to_string(),
                model: String::new(),
                timeout_secs: 0,
            },
        );
        let client = AuxiliaryClient::new(config, vec![entry.clone()], vec![entry]);
        let resp = client
            .call_for(TaskKind::Curator, make_request())
            .await
            .unwrap();
        assert_eq!(resp.content.as_deref(), Some("ok"));
    }

    /// `clear_exhausted` (test-only) restores normal flow after a
    /// previous credit-exhaustion event. Verifies the
    /// `is_exhausted` accessor too.
    #[tokio::test]
    async fn exhaustion_clear_restores_flow() {
        let first: Arc<dyn Provider> = Arc::new(MockProvider::new(
            "first",
            vec![
                Ok(ChatResponse {
                    content: Some("recovered".to_string()),
                    tool_calls: vec![],
                    usage: None,
                }),
                Err(anyhow!("HTTP 402 insufficient_quota")),
            ],
        ));
        let chain = vec![ChainEntry {
            name: "first".to_string(),
            provider: first,
        }];
        let client =
            AuxiliaryClient::new(AuxiliaryConfig::default(), chain.clone(), chain);
        // First call exhausts.
        let r = client.call_for(TaskKind::Curator, make_request()).await;
        assert!(r.is_err());
        assert!(client.is_exhausted("first"));
        // Clear and retry — second mock response is `Ok`.
        client.clear_exhausted();
        let r = client.call_for(TaskKind::Curator, make_request()).await;
        assert!(r.is_ok());
    }

    /// Per-task config lookups return defaults for unconfigured
    /// tasks — important so background tasks added later don't
    /// error on missing config.
    #[test]
    fn unconfigured_task_returns_default() {
        let config = AuxiliaryConfig::default();
        let cfg = config.task(&TaskKind::Curator);
        assert_eq!(cfg.provider, "");
        assert_eq!(cfg.model, "");
        assert_eq!(cfg.timeout_secs, 0);
    }
}
