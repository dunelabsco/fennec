//! Minimal LLM tool-using loop, sized for the curator's review.
//!
//! Sends an initial conversation to the auxiliary client, executes
//! any tool calls in the response, appends the results to the
//! conversation, and re-asks. Terminates when the model returns a
//! response with no tool calls (the natural-language summary) or
//! when `max_iterations` is hit.
//!
//! This is *not* a re-implementation of the agent loop. It has no
//! channels, no streaming, no thinking-level negotiation, no
//! lifecycle hooks. It exists because the curator wants a focused
//! tool-using sub-conversation against a different (cheap, fast)
//! provider chain than the main agent uses.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::agent::thinking::ThinkingLevel;
use crate::providers::{AuxiliaryClient, TaskKind};
use crate::providers::traits::{ChatMessage, ChatRequest};
use crate::tools::traits::{Tool, ToolSpec};

/// Tunables for the loop.
#[derive(Debug, Clone, Copy)]
pub struct ToolLoopConfig {
    /// Maximum LLM round-trips. Each iteration is one provider call
    /// plus zero-or-more tool executions. Default 30 — enough for
    /// the curator to walk a sizeable cluster, small enough to bound
    /// runaway loops.
    pub max_iterations: usize,
    /// `max_tokens` for each provider call. Default 4096.
    pub max_tokens: usize,
    /// Sampling temperature. Default 0.2 — the curator should be
    /// deterministic, not creative.
    pub temperature: f64,
    /// Truncation cap for the tool-result string captured into the
    /// audit. The full result still lands in the conversation; this
    /// only trims what we save for the run report. Default 800.
    pub audit_result_chars: usize,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 30,
            max_tokens: 4096,
            temperature: 0.2,
            audit_result_chars: 800,
        }
    }
}

/// What happened during one tool call. Captured for the run report
/// so the user can audit what the curator did.
#[derive(Debug, Clone, Serialize)]
pub struct RecordedToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    pub success: bool,
    /// Truncated tool output (or error) for the audit log. Length
    /// capped by `ToolLoopConfig.audit_result_chars`.
    pub result: String,
}

/// What the loop returns.
#[derive(Debug, Clone)]
pub struct ToolLoopOutcome {
    /// The final natural-language response from the model (or empty
    /// if the loop ran out of iterations).
    pub final_summary: String,
    /// Every tool call that fired, in order.
    pub tool_calls: Vec<RecordedToolCall>,
    /// How many provider round-trips happened.
    pub iterations: usize,
    /// True if the loop terminated because `max_iterations` was hit
    /// rather than because the model issued a final response.
    pub hit_iteration_cap: bool,
}

/// Run the loop. `tools` is the set the LLM is allowed to call —
/// the model sees their `ToolSpec` advertisements but can't invoke
/// anything outside this list.
pub async fn run(
    aux: &AuxiliaryClient,
    system_prompt: &str,
    initial_user: &str,
    tools: &[Arc<dyn Tool>],
    config: ToolLoopConfig,
) -> Result<ToolLoopOutcome> {
    let tool_specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();

    let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(initial_user)];
    let mut recorded: Vec<RecordedToolCall> = Vec::new();
    let mut iterations = 0usize;

    for i in 0..config.max_iterations {
        iterations = i + 1;
        let request = ChatRequest {
            system: Some(system_prompt),
            messages: &messages,
            tools: Some(&tool_specs),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            thinking_level: ThinkingLevel::Off,
        };

        let response = aux
            .call_for(TaskKind::Curator, request)
            .await
            .context("curator tool loop: provider call failed")?;

        // Append the assistant message to history (regardless of
        // whether it has tool calls — the conversation needs to
        // include the model's intent for the next round).
        let assistant_msg = ChatMessage {
            role: "assistant".to_string(),
            content: response.content.clone(),
            tool_calls: if response.tool_calls.is_empty() {
                None
            } else {
                Some(response.tool_calls.clone())
            },
            tool_call_id: None,
        };
        messages.push(assistant_msg);

        if response.tool_calls.is_empty() {
            return Ok(ToolLoopOutcome {
                final_summary: response.content.unwrap_or_default(),
                tool_calls: recorded,
                iterations,
                hit_iteration_cap: false,
            });
        }

        // Execute every tool call the model issued and append a
        // tool-result message for each.
        for tc in response.tool_calls {
            let executed = execute_tool_call(tools, &tc.name, &tc.arguments).await;
            let tool_result_text = match &executed {
                Ok(r) if r.success => r.output.clone(),
                Ok(r) => format!(
                    "[tool error] {}",
                    r.error.clone().unwrap_or_else(|| "unknown".into())
                ),
                Err(e) => format!("[dispatch error] {}", e),
            };
            messages.push(ChatMessage::tool_result(&tc.id, tool_result_text.clone()));

            let success = matches!(&executed, Ok(r) if r.success);
            let truncated = truncate(&tool_result_text, config.audit_result_chars);
            recorded.push(RecordedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
                success,
                result: truncated,
            });
        }
    }

    // If we're here, the loop ran out without a terminal response.
    // Return what we have so far.
    Ok(ToolLoopOutcome {
        final_summary: format!(
            "[curator hit iteration cap of {} without terminating]",
            config.max_iterations
        ),
        tool_calls: recorded,
        iterations,
        hit_iteration_cap: true,
    })
}

async fn execute_tool_call(
    tools: &[Arc<dyn Tool>],
    name: &str,
    args: &serde_json::Value,
) -> Result<crate::tools::traits::ToolResult> {
    let tool = tools
        .iter()
        .find(|t| t.name() == name)
        .ok_or_else(|| anyhow::anyhow!("tool not available to curator: {:?}", name))?;
    tool.execute(args.clone()).await
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_passes_short_strings_through() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_appends_marker_for_long_strings() {
        let s = "a".repeat(100);
        let t = truncate(&s, 10);
        assert!(t.starts_with("aaaaaaaaaa"));
        assert!(t.ends_with("…[truncated]"));
    }

    #[test]
    fn config_default_has_sane_values() {
        let cfg = ToolLoopConfig::default();
        assert!(cfg.max_iterations >= 10);
        assert!(cfg.max_tokens > 0);
    }

    // The full provider-coupled tool loop is exercised via the
    // runner-level integration tests (skills::curator::runner).
    // Here we only test the pure helpers — the loop itself takes
    // a real `AuxiliaryClient` and provider mocks live in the
    // `auxiliary` module's own test fixtures.
}
