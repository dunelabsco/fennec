use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{ChatMessage, ChatRequest, ChatResponse, Provider, ToolCall, UsageInfo};

/// Anthropic Claude API provider.
pub struct AnthropicProvider {
    api_key: String,
    client: reqwest::Client,
    default_model: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    ///
    /// - `api_key`: Anthropic API key.
    /// - `model`: Override the default model. Defaults to `claude-sonnet-4-20250514`.
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
        }
    }

    /// Convert our ChatMessages to the Anthropic API message format.
    fn convert_messages(messages: &[ChatMessage]) -> Vec<Value> {
        let mut api_messages = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    // System messages are handled separately via the top-level `system` field.
                    // Skip them here.
                }
                "assistant" => {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        // Assistant message with tool use blocks.
                        let mut content_blocks = Vec::new();

                        // Include text content if present.
                        if let Some(ref text) = msg.content {
                            if !text.is_empty() {
                                content_blocks.push(json!({
                                    "type": "text",
                                    "text": text
                                }));
                            }
                        }

                        // Add tool_use blocks.
                        for tc in tool_calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments
                            }));
                        }

                        api_messages.push(json!({
                            "role": "assistant",
                            "content": content_blocks
                        }));
                    } else {
                        api_messages.push(json!({
                            "role": "assistant",
                            "content": msg.content.as_deref().unwrap_or("")
                        }));
                    }
                }
                "tool" => {
                    // Tool results become user messages with tool_result content blocks.
                    let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
                    let content = msg.content.as_deref().unwrap_or("");

                    api_messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content
                        }]
                    }));
                }
                _ => {
                    // "user" and anything else.
                    api_messages.push(json!({
                        "role": msg.role,
                        "content": msg.content.as_deref().unwrap_or("")
                    }));
                }
            }
        }

        api_messages
    }

    /// Convert our ToolSpec list to Anthropic's tools format.
    fn convert_tools(tools: &[crate::tools::traits::ToolSpec]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                })
            })
            .collect()
    }

    /// Parse the Anthropic response JSON into our ChatResponse.
    fn parse_response(body: &Value) -> Result<ChatResponse> {
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(content) = body.get("content").and_then(|c| c.as_array()) {
            for block in content {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(text.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);

                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments,
                        });
                    }
                    _ => {}
                }
            }
        }

        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        // Parse usage info.
        let usage = body.get("usage").map(|u| UsageInfo {
            input_tokens: u
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: u
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_read_tokens: u
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64()),
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        let messages = Self::convert_messages(request.messages);

        let mut body = json!({
            "model": self.default_model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        // Add system prompt with cache control for prompt caching.
        if let Some(system_text) = request.system {
            body["system"] = json!([{
                "type": "text",
                "text": system_text,
                "cache_control": {"type": "ephemeral"}
            }]);
        }

        // Add tools if provided.
        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(Self::convert_tools(tools));
            }
        }

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("sending request to Anthropic API")?;

        let status = response.status();
        let response_body: Value = response
            .json()
            .await
            .context("parsing Anthropic API response")?;

        if !status.is_success() {
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Anthropic API error ({}): {}", status, error_msg);
        }

        Self::parse_response(&response_body)
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        200_000
    }
}
