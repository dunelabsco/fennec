use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::traits::{
    ChatMessage, ChatRequest, ChatResponse, Provider, StreamEvent, ToolCall, UsageInfo,
};

/// How the Anthropic provider authenticates its requests.
#[derive(Debug, Clone)]
pub enum AnthropicAuthMode {
    /// Traditional API key sent as `x-api-key` header.
    ApiKey(String),
    /// OAuth Bearer token sent as `Authorization: Bearer <token>`.
    OAuthBearer(String),
}

/// Anthropic Claude API provider.
pub struct AnthropicProvider {
    auth: AnthropicAuthMode,
    client: reqwest::Client,
    default_model: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider using an API key.
    ///
    /// - `api_key`: Anthropic API key.
    /// - `model`: Override the default model. Defaults to `claude-sonnet-4-20250514`.
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            auth: AnthropicAuthMode::ApiKey(api_key),
            client: reqwest::Client::new(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
        }
    }

    /// Create a new Anthropic provider using an OAuth Bearer token.
    pub fn new_with_oauth(token: String, model: Option<String>) -> Self {
        Self {
            auth: AnthropicAuthMode::OAuthBearer(token),
            client: reqwest::Client::new(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
        }
    }

    /// Apply the appropriate authentication header to a request builder.
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            AnthropicAuthMode::ApiKey(key) => builder.header("x-api-key", key),
            AnthropicAuthMode::OAuthBearer(token) => {
                builder
                    .header("Authorization", format!("Bearer {}", token))
                    // OAuth requires beta headers to work with the Messages API
                    .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20")
                    .header("User-Agent", "claude-cli/1.0 (external, cli)")
            }
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

    /// Build the request body for the Anthropic API (shared between chat and chat_stream).
    fn build_request_body(
        &self,
        request: &ChatRequest<'_>,
        stream: bool,
    ) -> Value {
        let messages = Self::convert_messages(request.messages);

        let mut body = json!({
            "model": self.default_model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        if stream {
            body["stream"] = json!(true);
        }

        if let Some(system_text) = request.system {
            body["system"] = json!([{
                "type": "text",
                "text": system_text,
                "cache_control": {"type": "ephemeral"}
            }]);
        }

        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(Self::convert_tools(tools));
            }
        }

        // Apply extended thinking parameters if the agent selected a level.
        crate::agent::thinking::apply_thinking_params(
            &mut body,
            request.thinking_level,
            "anthropic",
        );

        body
    }

    /// Parse a single SSE line from Anthropic's streaming API and emit
    /// [`StreamEvent`]s on the provided sender.
    async fn handle_sse_event(
        event_type: &str,
        data: &Value,
        tx: &tokio::sync::mpsc::Sender<StreamEvent>,
    ) {
        match event_type {
            "content_block_start" => {
                if let Some(block) = data.get("content_block") {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let _ = tx.send(StreamEvent::ToolCallStart { id, name }).await;
                    }
                }
            }
            "content_block_delta" => {
                if let Some(delta) = data.get("delta") {
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                let _ = tx.send(StreamEvent::Delta(text.to_string())).await;
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                                // We need the index to map to the tool call id.
                                // The index comes from the content_block_start; for
                                // simplicity we use the index as a stand-in id here.
                                let idx = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                                let _ = tx
                                    .send(StreamEvent::ToolCallDelta {
                                        id: idx.to_string(),
                                        arguments_delta: partial.to_string(),
                                    })
                                    .await;
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let idx = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                // We emit ToolCallEnd for every block stop; consumers that
                // track tool calls by id can use this.
                let _ = tx
                    .send(StreamEvent::ToolCallEnd {
                        id: idx.to_string(),
                    })
                    .await;
            }
            "message_stop" => {
                let _ = tx.send(StreamEvent::Done).await;
            }
            "error" => {
                let msg = data
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown streaming error");
                let _ = tx.send(StreamEvent::Error(msg.to_string())).await;
            }
            _ => { /* message_start, ping, etc. — ignore */ }
        }
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
        let body = self.build_request_body(&request, false);

        let req = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body);
        let req = self.apply_auth(req);

        let response = req
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

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let body = self.build_request_body(&request, true);

        let req = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body);
        let req = self.apply_auth(req);

        let response = req
            .send()
            .await
            .context("sending streaming request to Anthropic API")?;

        let status = response.status();
        if !status.is_success() {
            let response_body: Value = response
                .json()
                .await
                .context("parsing Anthropic streaming error response")?;
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Anthropic API error ({}): {}", status, error_msg);
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut current_event_type = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines from the SSE stream.
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim_end().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        // Empty line = end of event.
                        current_event_type.clear();
                        continue;
                    }

                    if let Some(evt) = line.strip_prefix("event: ") {
                        current_event_type = evt.to_string();
                    } else if let Some(data_str) = line.strip_prefix("data: ") {
                        if let Ok(data) = serde_json::from_str::<Value>(data_str) {
                            Self::handle_sse_event(&current_event_type, &data, &tx).await;
                        }
                    }
                }
            }

            // If we exit without a message_stop, send Done anyway.
            let _ = tx.send(StreamEvent::Done).await;
        });

        Ok(rx)
    }
}
