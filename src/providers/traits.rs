use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Events emitted during a streaming response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content.
    Delta(String),
    /// A tool call has started.
    ToolCallStart { id: String, name: String },
    /// Incremental arguments JSON for a tool call.
    ToolCallDelta { id: String, arguments_delta: String },
    /// A tool call's arguments are complete.
    ToolCallEnd { id: String },
    /// The response is complete.
    Done,
    /// An error occurred during streaming.
    Error(String),
}

/// A single tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A chat message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A request to the chat API.
#[derive(Clone, Copy)]
pub struct ChatRequest<'a> {
    pub system: Option<&'a str>,
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [crate::tools::traits::ToolSpec]>,
    pub max_tokens: usize,
    pub temperature: f64,
    /// Reasoning / extended-thinking effort the agent has selected for this
    /// turn (via `/think:<level>` directives or programmatic config).
    /// Providers that support extended thinking (Anthropic, OpenAI o-series,
    /// OpenRouter) translate this to their native parameters via
    /// [`crate::agent::thinking::apply_thinking_params`]; others ignore it.
    pub thinking_level: crate::agent::thinking::ThinkingLevel,
}

/// The response from the chat API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<UsageInfo>,
}

/// Token usage information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
}

/// Async trait for LLM providers.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable name for this provider.
    fn name(&self) -> &str;

    /// Send a chat request and get a response.
    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse>;

    /// Whether this provider supports tool/function calling.
    fn supports_tool_calling(&self) -> bool;

    /// The provider's context window size in tokens.
    fn context_window(&self) -> usize;

    /// Whether this provider supports streaming responses.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Send a chat request and receive a stream of events.
    ///
    /// The default implementation falls back to [`Self::chat`] and emits the
    /// full response as a single [`StreamEvent::Delta`] followed by
    /// [`StreamEvent::Done`].
    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>>;
}

/// Default streaming implementation that delegates to [`Provider::chat`].
///
/// Providers that do not implement native streaming can call this from their
/// `chat_stream` method.
pub async fn default_chat_stream(
    provider: &dyn Provider,
    request: ChatRequest<'_>,
) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
    let response = provider.chat(request).await?;
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        if let Some(content) = response.content {
            let _ = tx.send(StreamEvent::Delta(content)).await;
        }
        for tc in &response.tool_calls {
            let _ = tx
                .send(StreamEvent::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                })
                .await;
            let _ = tx
                .send(StreamEvent::ToolCallDelta {
                    id: tc.id.clone(),
                    arguments_delta: tc.arguments.to_string(),
                })
                .await;
            let _ = tx
                .send(StreamEvent::ToolCallEnd { id: tc.id.clone() })
                .await;
        }
        let _ = tx.send(StreamEvent::Done).await;
    });
    Ok(rx)
}
