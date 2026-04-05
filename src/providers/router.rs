use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::traits::{ChatMessage, ChatRequest, ChatResponse, Provider, StreamEvent};

/// Classifies a task as simple or complex for routing purposes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskComplexity {
    Simple,
    Complex,
}

/// Keywords / patterns that suggest a complex task requiring a stronger model.
const COMPLEX_INDICATORS: &[&str] = &[
    "analyze",
    "implement",
    "refactor",
    "debug",
    "explain how",
    "write code",
    "write a function",
    "write a script",
    "step by step",
    "multi-step",
    "compare",
    "evaluate",
    "design",
    "architect",
    "optimize",
    "security",
    "vulnerability",
    "migration",
    "deploy",
    "integrate",
    "```",
    "function",
    "class ",
    "struct ",
    "impl ",
    "async fn",
    "def ",
    "SELECT ",
    "INSERT ",
    "CREATE TABLE",
];

/// Classify the complexity of a message based on simple keyword heuristics.
pub fn classify_complexity(message: &str) -> TaskComplexity {
    let lower = message.to_lowercase();

    // Very short messages (< 20 chars) are almost always simple.
    if message.len() < 20 {
        return TaskComplexity::Simple;
    }

    // Check for complex indicators (case-insensitive where appropriate).
    for indicator in COMPLEX_INDICATORS {
        if lower.contains(&indicator.to_lowercase()) {
            return TaskComplexity::Complex;
        }
    }

    // Messages with many words tend to be more complex.
    let word_count = message.split_whitespace().count();
    if word_count > 50 {
        return TaskComplexity::Complex;
    }

    TaskComplexity::Simple
}

/// A provider that routes requests to different underlying providers based on
/// task complexity classification.
pub struct RouterProvider {
    primary: Arc<dyn Provider>,
    auxiliary: Option<Arc<dyn Provider>>,
}

impl RouterProvider {
    /// Create a new router with a primary (strong) provider and an optional
    /// auxiliary (cheap/fast) provider for simple tasks.
    pub fn new(primary: Arc<dyn Provider>, auxiliary: Option<Arc<dyn Provider>>) -> Self {
        Self { primary, auxiliary }
    }

    /// Determine which provider to use for the given messages.
    fn select_provider(&self, messages: &[ChatMessage]) -> &Arc<dyn Provider> {
        // Look at the last user message for classification.
        let last_user_msg = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");

        let complexity = classify_complexity(last_user_msg);

        match (complexity, &self.auxiliary) {
            (TaskComplexity::Simple, Some(aux)) => {
                tracing::info!(
                    provider = aux.name(),
                    "Router: routing simple task to auxiliary provider"
                );
                aux
            }
            _ => {
                tracing::info!(
                    provider = self.primary.name(),
                    "Router: routing to primary provider"
                );
                &self.primary
            }
        }
    }
}

#[async_trait]
impl Provider for RouterProvider {
    fn name(&self) -> &str {
        "router"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        let provider = self.select_provider(request.messages);
        provider.chat(request).await
    }

    fn supports_tool_calling(&self) -> bool {
        self.primary.supports_tool_calling()
    }

    fn context_window(&self) -> usize {
        self.primary.context_window()
    }

    fn supports_streaming(&self) -> bool {
        self.primary.supports_streaming()
    }

    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let provider = self.select_provider(request.messages);
        provider.chat_stream(request).await
    }
}

// ---------------------------------------------------------------------------
// ModelSwitchTool (stub)
// ---------------------------------------------------------------------------

use crate::tools::traits::{Tool, ToolResult};

/// A tool that allows the agent to request switching models at runtime.
///
/// Currently implemented as a stub that logs the switch request. In a full
/// implementation, the agent loop would check for pending switch requests
/// after each turn and swap the active provider.
pub struct ModelSwitchTool;

impl ModelSwitchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ModelSwitchTool {
    fn name(&self) -> &str {
        "model_switch"
    }

    fn description(&self) -> &str {
        "Request switching to a different model at runtime"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "The model identifier to switch to"
                },
                "provider": {
                    "type": "string",
                    "description": "Optional provider name (anthropic, openai, etc.)"
                }
            },
            "required": ["model"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let model = args["model"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let provider = args["provider"]
            .as_str()
            .map(|s| s.to_string());

        if model.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("No model specified".into()),
            });
        }

        tracing::info!(
            model = %model,
            provider = ?provider,
            "Model switch requested (stub)"
        );

        Ok(ToolResult {
            success: true,
            output: format!(
                "Model switch requested: model={}{}\nNote: This is currently a stub. The switch will take effect after the agent loop processes the request.",
                model,
                provider.map(|p| format!(", provider={}", p)).unwrap_or_default()
            ),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_greeting_as_simple() {
        assert_eq!(classify_complexity("Hello!"), TaskComplexity::Simple);
        assert_eq!(classify_complexity("Hi there"), TaskComplexity::Simple);
        assert_eq!(classify_complexity("Thanks!"), TaskComplexity::Simple);
    }

    #[test]
    fn classify_code_request_as_complex() {
        assert_eq!(
            classify_complexity("Write a function to sort a list"),
            TaskComplexity::Complex
        );
        assert_eq!(
            classify_complexity("Can you debug this issue?"),
            TaskComplexity::Complex
        );
        assert_eq!(
            classify_complexity("Please analyze the performance of this query"),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn classify_code_block_as_complex() {
        let msg = "What does this do?\n```rust\nfn main() { println!(\"hello\"); }\n```";
        assert_eq!(classify_complexity(msg), TaskComplexity::Complex);
    }

    #[test]
    fn classify_short_question_as_simple() {
        assert_eq!(
            classify_complexity("What time is it?"),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn classify_long_message_as_complex() {
        let msg = "word ".repeat(60);
        assert_eq!(classify_complexity(&msg), TaskComplexity::Complex);
    }
}
