use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Events emitted during a streaming response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content.
    Delta(String),
    /// A chunk of reasoning / extended-thinking content. Anthropic
    /// streams these as `thinking_delta` events on `content_block_delta`
    /// SSE frames. OpenAI's o1/o3/o4 don't stream reasoning live —
    /// providers emit a single `Reasoning(full_text)` just before
    /// [`Self::Done`] when the response carries it.
    Reasoning(String),
    /// A tool call has started.
    ToolCallStart { id: String, name: String },
    /// Incremental arguments JSON for a tool call.
    ToolCallDelta { id: String, arguments_delta: String },
    /// A tool call's arguments are complete.
    ToolCallEnd { id: String },
    /// Usage info for the in-flight request. Emitted once per
    /// streamed call, typically just before [`Self::Done`]. Lets
    /// the agent accumulate tokens / cost on the streaming path
    /// the same way the non-streaming path reads `ChatResponse.usage`.
    Usage(UsageInfo),
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
    /// Image attachments that should be sent inline with this
    /// message. Populated by `/image` and `/paste` for the next
    /// user turn; provider impls translate them into
    /// provider-specific image content blocks. `None` keeps the
    /// message as text-only (the common case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ImageAttachmentRef>>,
}

/// Provider-agnostic image payload carried on a `ChatMessage`.
/// `base64_data` is pre-encoded; `mime_type` is the MIME the
/// provider-side serialiser should declare.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachmentRef {
    pub mime_type: String,
    pub base64_data: String,
    /// Optional source-path display name for the chat (`/image`
    /// confirmation message uses this).
    #[serde(default)]
    pub display_name: Option<String>,
}

impl ChatMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            attachments: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            attachments: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            attachments: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            attachments: None,
        }
    }
}

/// A request to the chat API.
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
    /// Extended-thinking / reasoning text from the model. Anthropic
    /// returns this as separate `thinking` content blocks when
    /// extended thinking is enabled; OpenAI's o1/o3/o4 surface it
    /// as `message.reasoning`. `None` when the model didn't emit
    /// reasoning (or the user disabled it). Multiple thinking
    /// blocks from one response are joined with newlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// Token usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the prompt cache on this call (Anthropic
    /// `cache_creation_input_tokens`). OpenAI doesn't return this
    /// today; provider impls leave it `None` when unavailable.
    pub cache_write_tokens: Option<u64>,
}

/// Async trait for LLM providers.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable name for this provider.
    fn name(&self) -> &str;

    /// Currently-configured model identifier. Used by `/usage`
    /// (pricing lookup) and `/model` (display in the panel header).
    /// Default returns an empty string for providers that haven't
    /// adopted the accessor yet.
    fn model(&self) -> &str {
        ""
    }

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
        // Reasoning before content so consumers (the renderer)
        // see thinking text first, matching the order Anthropic's
        // streaming uses (thinking blocks always precede text).
        if let Some(reasoning) = response.reasoning {
            let _ = tx.send(StreamEvent::Reasoning(reasoning)).await;
        }
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
        if let Some(usage) = response.usage {
            let _ = tx.send(StreamEvent::Usage(usage)).await;
        }
        let _ = tx.send(StreamEvent::Done).await;
    });
    Ok(rx)
}
