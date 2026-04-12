use std::sync::Arc;

use anyhow::{Result, bail};

use crate::collective::search::{CollectiveSearch, RankedExperience, SearchConfidence};
use crate::memory::decay::apply_time_decay;
use crate::memory::traits::Memory;
use crate::providers::traits::{ChatMessage, ChatRequest, ChatResponse, Provider, StreamEvent};
use crate::security::prompt_guard::{PromptGuard, ScanResult};
use crate::tools::traits::{Tool, ToolSpec};

use super::context::SystemPromptBuilder;
use super::scrub;
use super::thinking::{self, ThinkingLevel};

/// The core agent that orchestrates provider calls, tool execution, and memory.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    tool_specs: Vec<ToolSpec>,
    memory: Arc<dyn Memory>,
    prompt_builder: SystemPromptBuilder,
    max_tool_iterations: usize,
    history: Vec<ChatMessage>,
    system_prompt: Option<String>,
    max_tokens: usize,
    temperature: f64,
    memory_context_limit: usize,
    half_life_days: f64,
    prompt_guard: Option<PromptGuard>,
    collective: Option<Arc<CollectiveSearch>>,
    thinking_level: ThinkingLevel,
    // Token usage tracking
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_read_tokens: u64,
    turn_count: u64,
}

impl Agent {
    /// Execute a single conversational turn.
    ///
    /// On the first turn the system prompt is built (with memory context) and
    /// frozen for the remainder of the session. Subsequent turns reuse it.
    pub async fn turn(&mut self, user_message: &str) -> Result<String> {
        // Parse thinking directive before any other processing.
        let (thinking_override, user_message) = thinking::parse_thinking_directive(user_message);
        if let Some(level) = thinking_override {
            self.thinking_level = level;
            tracing::info!(?level, "Thinking level set via /think directive");
        }
        let user_message: &str = &user_message;

        // Prompt guard scan.
        if let Some(ref guard) = self.prompt_guard {
            match guard.scan(user_message) {
                ScanResult::Blocked(reason) => {
                    bail!("{reason}");
                }
                ScanResult::Suspicious(categories, score) => {
                    tracing::warn!(
                        ?categories,
                        score,
                        "Prompt guard: suspicious input detected"
                    );
                }
                ScanResult::Safe => {}
            }
        }

        // Step 3: Search collective for relevant experiences.
        let collective_context = if let Some(ref collective) = self.collective {
            match collective.search(user_message, 3).await {
                Ok(result) => match result.confidence {
                    SearchConfidence::High => {
                        let formatted =
                            self.format_collective_results(&result.experiences, true);
                        Some(formatted)
                    }
                    SearchConfidence::Partial => {
                        let formatted =
                            self.format_collective_results(&result.experiences, false);
                        Some(formatted)
                    }
                    SearchConfidence::None => None,
                },
                Err(e) => {
                    tracing::warn!("Collective search failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Build system prompt on first turn.
        if self.system_prompt.is_none() {
            let memory_context = self.load_memory_context(user_message).await?;
            let tool_names: Vec<String> = self.tools.iter().map(|t| t.name().to_string()).collect();
            let prompt = self.prompt_builder.build(&memory_context, &tool_names);
            self.system_prompt = Some(prompt);
        }

        // Inject collective context into user message if available.
        let effective_message = if let Some(ref ctx) = collective_context {
            format!("[Collective context]\n{ctx}\n[User message]\n{user_message}")
        } else {
            user_message.to_string()
        };

        // Push user message to history.
        self.history.push(ChatMessage::user(&effective_message));

        self.turn_count += 1;
        let turn_start = std::time::Instant::now();
        let tokens_before_input = self.total_input_tokens;
        let tokens_before_output = self.total_output_tokens;

        // Tool call loop.
        for _iteration in 0..self.max_tool_iterations {
            let response = self.call_provider().await?;

            // Track token usage from this API call.
            if let Some(ref usage) = response.usage {
                self.total_input_tokens += usage.input_tokens;
                self.total_output_tokens += usage.output_tokens;
                if let Some(cache) = usage.cache_read_tokens {
                    self.total_cache_read_tokens += cache;
                }
            }

            if response.tool_calls.is_empty() {
                // No tool calls — final assistant response.
                let text = response.content.unwrap_or_default();
                self.history.push(ChatMessage::assistant(&text));

                // Log token usage for this turn.
                let turn_input = self.total_input_tokens - tokens_before_input;
                let turn_output = self.total_output_tokens - tokens_before_output;
                let elapsed = turn_start.elapsed();
                tracing::info!(
                    turn = self.turn_count,
                    turn_input_tokens = turn_input,
                    turn_output_tokens = turn_output,
                    turn_total_tokens = turn_input + turn_output,
                    turn_time_ms = elapsed.as_millis() as u64,
                    session_total_input = self.total_input_tokens,
                    session_total_output = self.total_output_tokens,
                    session_total_tokens = self.total_input_tokens + self.total_output_tokens,
                    session_cache_read = self.total_cache_read_tokens,
                    "Turn complete"
                );

                return Ok(text);
            }

            // Push assistant message with tool calls.
            let mut assistant_msg = ChatMessage::assistant(
                response.content.as_deref().unwrap_or(""),
            );
            assistant_msg.tool_calls = Some(response.tool_calls.clone());
            self.history.push(assistant_msg);

            // Execute each tool call and push results.
            for tc in &response.tool_calls {
                let result = self.execute_tool(&tc.name, &tc.arguments).await;
                self.history
                    .push(ChatMessage::tool_result(&tc.id, &result));
            }
        }

        bail!("max tool iterations ({}) exceeded", self.max_tool_iterations)
    }

    /// Execute a single conversational turn with streaming.
    ///
    /// Returns a receiver of [`StreamEvent`]s. Text deltas are forwarded in
    /// real-time while tool calls are processed internally (their results are
    /// *not* streamed back; only the final text response is).
    pub async fn turn_streamed(
        &mut self,
        user_message: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        // Prompt guard scan.
        if let Some(ref guard) = self.prompt_guard {
            match guard.scan(user_message) {
                ScanResult::Blocked(reason) => {
                    bail!("{reason}");
                }
                ScanResult::Suspicious(categories, score) => {
                    tracing::warn!(
                        ?categories,
                        score,
                        "Prompt guard: suspicious input detected (stream)"
                    );
                }
                ScanResult::Safe => {}
            }
        }

        // Build system prompt on first turn.
        if self.system_prompt.is_none() {
            let memory_context = self.load_memory_context(user_message).await?;
            let tool_names: Vec<String> = self.tools.iter().map(|t| t.name().to_string()).collect();
            let prompt = self.prompt_builder.build(&memory_context, &tool_names);
            self.system_prompt = Some(prompt);
        }

        // Push user message to history.
        self.history.push(ChatMessage::user(user_message));

        // Get streaming receiver from provider.
        let system = self.system_prompt.as_deref();
        let tools = if self.tool_specs.is_empty() {
            None
        } else {
            Some(self.tool_specs.as_slice())
        };

        let request = ChatRequest {
            system,
            messages: &self.history,
            tools,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        };

        let rx = self.provider.chat_stream(request).await?;
        Ok(rx)
    }

    /// Call the provider with the current history and system prompt.
    async fn call_provider(&self) -> Result<ChatResponse> {
        let system = self.system_prompt.as_deref();
        let tools = if self.tool_specs.is_empty() {
            None
        } else {
            Some(self.tool_specs.as_slice())
        };

        let request = ChatRequest {
            system,
            messages: &self.history,
            tools,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        };

        self.provider.chat(request).await
    }

    /// Find a tool by name and execute it. Returns the formatted output string
    /// with credentials scrubbed.
    async fn execute_tool(&self, name: &str, args: &serde_json::Value) -> String {
        let raw = match self.tools.iter().find(|t| t.name() == name) {
            Some(t) => match t.execute(args.clone()).await {
                Ok(result) => {
                    if result.success {
                        result.output
                    } else {
                        format!(
                            "Error: {}",
                            result.error.unwrap_or_else(|| "unknown error".to_string())
                        )
                    }
                }
                Err(e) => format!("Tool execution failed: {e}"),
            },
            None => format!("Unknown tool: {name}"),
        };
        scrub::scrub_credentials(&raw)
    }

    /// Load memory context for the given query.
    ///
    /// Recalls entries from memory, applies time decay, sorts by score
    /// descending, and formats as `"[key] content"` strings.
    async fn load_memory_context(&self, query: &str) -> Result<Vec<String>> {
        let mut entries = self.memory.recall(query, self.memory_context_limit).await?;

        apply_time_decay(&mut entries, self.half_life_days);

        // Sort by score descending.
        entries.sort_by(|a, b| {
            let sa = a.score.unwrap_or(0.0);
            let sb = b.score.unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

        let formatted: Vec<String> = entries
            .iter()
            .map(|e| format!("[{}] {}", e.key, e.content))
            .collect();

        Ok(formatted)
    }

    /// Format collective search results for injection into the user message.
    fn format_collective_results(
        &self,
        results: &[RankedExperience],
        high_confidence: bool,
    ) -> String {
        let mut output = if high_confidence {
            "The collective has high-confidence experience with this:\n\n".to_string()
        } else {
            "Related experiences from the collective:\n\n".to_string()
        };
        for (i, ranked) in results.iter().enumerate() {
            output.push_str(&format!("{}. Goal: {}\n", i + 1, ranked.result.goal));
            if let Some(ref solution) = ranked.result.solution {
                output.push_str(&format!("   Solution: {}\n", solution));
            }
            if !ranked.result.gotchas.is_empty() {
                output.push_str(&format!(
                    "   Gotchas: {}\n",
                    ranked.result.gotchas.join("; ")
                ));
            }
            output.push('\n');
        }
        output
    }

    /// Publish an experience to the collective if configured and publish is enabled.
    ///
    /// This can be called when an experience is manually extracted or when
    /// consolidation creates one. It scrubs the experience before publishing.
    pub async fn publish_experience_if_configured(
        &self,
        experience: &crate::memory::experience::Experience,
    ) -> Result<Option<String>> {
        let _collective = match self.collective {
            Some(ref c) => c,
            None => return Ok(None),
        };
        // We only have search access; publishing requires the remote layer.
        // For now log that publishing would occur.
        tracing::info!(
            experience_id = %experience.id,
            "Would publish experience to collective (requires direct remote access)"
        );
        Ok(None)
    }

    /// Clear conversation history and system prompt, resetting for a new session.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.system_prompt = None;
    }

    /// Get a reference to the shared provider.
    pub fn provider(&self) -> &Arc<dyn Provider> {
        &self.provider
    }

    /// Get a reference to the shared memory.
    pub fn memory(&self) -> &Arc<dyn Memory> {
        &self.memory
    }

    /// Get the current thinking level.
    pub fn thinking_level(&self) -> ThinkingLevel {
        self.thinking_level
    }

    /// Set the thinking level programmatically.
    pub fn set_thinking_level(&mut self, level: ThinkingLevel) {
        self.thinking_level = level;
    }
}

// ---------------------------------------------------------------------------
// AgentBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing an [`Agent`] with validated configuration.
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider>>,
    tools: Vec<Box<dyn Tool>>,
    memory: Option<Arc<dyn Memory>>,
    identity_name: Option<String>,
    identity_persona: Option<String>,
    max_tool_iterations: Option<usize>,
    max_tokens: Option<usize>,
    temperature: Option<f64>,
    memory_context_limit: Option<usize>,
    half_life_days: Option<f64>,
    prompt_guard: Option<PromptGuard>,
    collective: Option<Arc<CollectiveSearch>>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            provider: None,
            tools: Vec::new(),
            memory: None,
            identity_name: None,
            identity_persona: None,
            max_tool_iterations: None,
            max_tokens: None,
            temperature: None,
            memory_context_limit: None,
            half_life_days: None,
            prompt_guard: None,
            collective: None,
        }
    }

    pub fn provider(mut self, provider: impl Into<Arc<dyn Provider>>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    pub fn memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn identity_name(mut self, name: impl Into<String>) -> Self {
        self.identity_name = Some(name.into());
        self
    }

    pub fn identity_persona(mut self, persona: impl Into<String>) -> Self {
        self.identity_persona = Some(persona.into());
        self
    }

    pub fn max_tool_iterations(mut self, n: usize) -> Self {
        self.max_tool_iterations = Some(n);
        self
    }

    pub fn max_tokens(mut self, n: usize) -> Self {
        self.max_tokens = Some(n);
        self
    }

    pub fn temperature(mut self, t: f64) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn memory_context_limit(mut self, n: usize) -> Self {
        self.memory_context_limit = Some(n);
        self
    }

    pub fn half_life_days(mut self, d: f64) -> Self {
        self.half_life_days = Some(d);
        self
    }

    pub fn prompt_guard(mut self, guard: PromptGuard) -> Self {
        self.prompt_guard = Some(guard);
        self
    }

    pub fn collective(mut self, search: Arc<CollectiveSearch>) -> Self {
        self.collective = Some(search);
        self
    }

    /// Build the [`Agent`], validating that required fields are set.
    pub fn build(self) -> Result<Agent> {
        let provider = self
            .provider
            .ok_or_else(|| anyhow::anyhow!("AgentBuilder: provider is required"))?;
        let memory = self
            .memory
            .ok_or_else(|| anyhow::anyhow!("AgentBuilder: memory is required"))?;

        let name = self.identity_name.unwrap_or_else(|| "Fennec".to_string());
        let persona = self.identity_persona.unwrap_or_else(|| {
            "A fast, helpful AI assistant with collective intelligence.".to_string()
        });
        let prompt_builder = SystemPromptBuilder::new(name, persona);

        let tool_specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec()).collect();

        Ok(Agent {
            provider,
            tools: self.tools,
            tool_specs,
            memory,
            prompt_builder,
            max_tool_iterations: self.max_tool_iterations.unwrap_or(15),
            history: Vec::new(),
            system_prompt: None,
            max_tokens: self.max_tokens.unwrap_or(8192),
            temperature: self.temperature.unwrap_or(0.7),
            memory_context_limit: self.memory_context_limit.unwrap_or(5),
            half_life_days: self.half_life_days.unwrap_or(7.0),
            prompt_guard: self.prompt_guard,
            collective: self.collective,
            thinking_level: ThinkingLevel::Off,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            turn_count: 0,
        })
    }
}
