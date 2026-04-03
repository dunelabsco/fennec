use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
pub struct ChatRequest<'a> {
    pub system: Option<&'a str>,
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [crate::tools::traits::ToolSpec]>,
    pub max_tokens: usize,
    pub temperature: f64,
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
}
