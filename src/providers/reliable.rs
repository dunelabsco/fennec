use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use super::traits::{ChatRequest, ChatResponse, Provider, StreamEvent};

/// Default cooldown duration for rate-limited providers (60 seconds).
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);

/// A provider wrapper that adds automatic failover and retry logic.
///
/// `ReliableProvider` holds a prioritized list of providers and tries each
/// one in order. On rate limits it applies a cooldown; on other errors it
/// retries with exponential backoff before moving to the next provider.
pub struct ReliableProvider {
    providers: Vec<Box<dyn Provider>>,
    max_retries: usize,
    cooldowns: Arc<Mutex<HashMap<usize, Instant>>>,
}

impl ReliableProvider {
    /// Create a new reliable provider wrapper.
    ///
    /// - `providers`: Prioritized list of providers to try in order.
    /// - `max_retries`: Maximum retry attempts per provider (default 3).
    pub fn new(providers: Vec<Box<dyn Provider>>, max_retries: Option<usize>) -> Self {
        Self {
            providers,
            max_retries: max_retries.unwrap_or(3),
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check whether a provider index is currently on cooldown.
    fn is_on_cooldown(&self, index: usize) -> bool {
        let cooldowns = self.cooldowns.lock();
        if let Some(until) = cooldowns.get(&index) {
            Instant::now() < *until
        } else {
            false
        }
    }

    /// Mark a provider as rate-limited, putting it on cooldown.
    fn mark_rate_limited(&self, index: usize) {
        let mut cooldowns = self.cooldowns.lock();
        cooldowns.insert(index, Instant::now() + RATE_LIMIT_COOLDOWN);
    }

    /// Check if an error looks like a rate limit (429).
    fn is_rate_limit_error(err: &anyhow::Error) -> bool {
        let msg = err.to_string().to_lowercase();
        msg.contains("429") || msg.contains("rate limit")
    }
}

#[async_trait]
impl Provider for ReliableProvider {
    fn name(&self) -> &str {
        "reliable"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        let mut last_error: Option<anyhow::Error> = None;

        for (index, provider) in self.providers.iter().enumerate() {
            // Skip providers on cooldown.
            if self.is_on_cooldown(index) {
                tracing::debug!(
                    "Skipping provider {} (index {}): on cooldown",
                    provider.name(),
                    index
                );
                continue;
            }

            // Retry loop for this provider.
            for attempt in 0..self.max_retries {
                // Build a new request referencing the same data.
                let req = ChatRequest {
                    system: request.system,
                    messages: request.messages,
                    tools: request.tools,
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                    thinking_level: request.thinking_level,
                };

                match provider.chat(req).await {
                    Ok(response) => return Ok(response),
                    Err(err) => {
                        if Self::is_rate_limit_error(&err) {
                            tracing::warn!(
                                "Provider {} rate limited, applying cooldown",
                                provider.name()
                            );
                            self.mark_rate_limited(index);
                            last_error = Some(err);
                            break; // Move to next provider.
                        }

                        tracing::warn!(
                            "Provider {} attempt {}/{} failed: {}",
                            provider.name(),
                            attempt + 1,
                            self.max_retries,
                            err
                        );
                        last_error = Some(err);

                        // Exponential backoff: 1s * 2^attempt (skip on last attempt).
                        if attempt + 1 < self.max_retries {
                            let backoff = Duration::from_secs(1 << attempt);
                            tokio::time::sleep(backoff).await;
                        }
                    }
                }
            }
        }

        // All providers exhausted.
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no providers available")))
    }

    fn supports_tool_calling(&self) -> bool {
        self.providers
            .iter()
            .any(|p| p.supports_tool_calling())
    }

    fn context_window(&self) -> usize {
        self.providers
            .first()
            .map(|p| p.context_window())
            .unwrap_or(0)
    }

    fn supports_streaming(&self) -> bool {
        self.providers
            .iter()
            .any(|p| p.supports_streaming())
    }

    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let mut last_error: Option<anyhow::Error> = None;

        for (index, provider) in self.providers.iter().enumerate() {
            if self.is_on_cooldown(index) {
                continue;
            }

            let req = ChatRequest {
                system: request.system,
                messages: request.messages,
                tools: request.tools,
                max_tokens: request.max_tokens,
                temperature: request.temperature,
                thinking_level: request.thinking_level,
            };

            match provider.chat_stream(req).await {
                Ok(rx) => return Ok(rx),
                Err(err) => {
                    if Self::is_rate_limit_error(&err) {
                        self.mark_rate_limited(index);
                    }
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no providers available for streaming")))
    }
}
