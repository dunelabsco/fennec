use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::sse::SseBuffer;
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

/// What kind of content block occupies a particular `index` in the
/// streaming response. We remember this so `content_block_stop` can tell
/// which block types deserve a `ToolCallEnd` event.
#[derive(Debug, Clone)]
enum ToolBlockInfo {
    ToolUse(String),
    Other,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider using an API key.
    ///
    /// - `api_key`: Anthropic API key.
    /// - `model`: Override the default model. Defaults to `claude-sonnet-4-6`
    ///   (Sonnet 4.6 alias). The previous default `claude-sonnet-4-20250514`
    ///   is deprecated by Anthropic and retires June 15, 2026.
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            auth: AnthropicAuthMode::ApiKey(api_key),
            client: reqwest::Client::new(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
        }
    }

    /// Create a new Anthropic provider using an OAuth Bearer token.
    pub fn new_with_oauth(token: String, model: Option<String>) -> Self {
        Self {
            auth: AnthropicAuthMode::OAuthBearer(token),
            client: reqwest::Client::new(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
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

    /// Parse a single SSE event from Anthropic's streaming API and emit
    /// [`StreamEvent`]s on the provided sender.
    ///
    /// `index_to_tool` maps each `content_block_*` event's `index` to the
    /// `tool_use_id` advertised at `content_block_start`. Without this
    /// mapping, `input_json_delta` events (which carry only an index) and
    /// `content_block_stop` events can't be correlated back to the right
    /// tool call. Text blocks are tracked with a sentinel so we know NOT
    /// to emit a spurious `ToolCallEnd` for them.
    async fn handle_sse_event(
        event_type: &str,
        data: &Value,
        tx: &tokio::sync::mpsc::Sender<StreamEvent>,
        index_to_tool: &mut HashMap<u64, ToolBlockInfo>,
    ) {
        match event_type {
            "content_block_start" => {
                let idx = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let Some(block) = data.get("content_block") else { return };
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "tool_use" => {
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
                        index_to_tool.insert(idx, ToolBlockInfo::ToolUse(id.clone()));
                        let _ = tx.send(StreamEvent::ToolCallStart { id, name }).await;
                    }
                    _ => {
                        // Text (or future block types) — remember it's NOT a
                        // tool_use so the matching stop doesn't emit ToolCallEnd.
                        index_to_tool.insert(idx, ToolBlockInfo::Other);
                    }
                }
            }
            "content_block_delta" => {
                let Some(delta) = data.get("delta") else { return };
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            let _ = tx.send(StreamEvent::Delta(text.to_string())).await;
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) =
                            delta.get("partial_json").and_then(|t| t.as_str())
                        {
                            let idx = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                            // Resolve index → real tool_use_id.
                            let Some(ToolBlockInfo::ToolUse(id)) = index_to_tool.get(&idx)
                            else {
                                // Delta for a block we never saw a tool_use start
                                // for — drop it rather than emit with a wrong id.
                                return;
                            };
                            let _ = tx
                                .send(StreamEvent::ToolCallDelta {
                                    id: id.clone(),
                                    arguments_delta: partial.to_string(),
                                })
                                .await;
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                // Only emit ToolCallEnd for tool_use blocks. Text blocks
                // have their own completion signaled by the absence of
                // further text_deltas; surfacing a phantom ToolCallEnd for
                // them confuses consumers tracking tool state by id.
                if let Some(ToolBlockInfo::ToolUse(id)) = index_to_tool.remove(&idx) {
                    let _ = tx.send(StreamEvent::ToolCallEnd { id }).await;
                }
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

        // Read raw bytes first so a non-JSON error body (e.g. a proxy/gateway
        // 502 HTML page) doesn't lose the real HTTP status context. The old
        // order — .json() first, then check status — produced a cryptic
        // "parsing Anthropic API response" failure with no status/body info.
        let status = response.status();
        let raw_body = response
            .bytes()
            .await
            .context("reading Anthropic API response body")?;

        if !status.is_success() {
            let error_msg = serde_json::from_slice::<Value>(&raw_body)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| {
                    let preview = String::from_utf8_lossy(&raw_body);
                    preview.chars().take(200).collect::<String>()
                });
            anyhow::bail!("Anthropic API error ({}): {}", status, error_msg);
        }

        let response_body: Value = serde_json::from_slice(&raw_body)
            .context("parsing Anthropic API response as JSON")?;
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
            // Defensive body-read: non-streaming 4xx/5xx may not be JSON.
            let raw_body = response
                .bytes()
                .await
                .context("reading Anthropic streaming error body")?;
            let error_msg = serde_json::from_slice::<Value>(&raw_body)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| {
                    String::from_utf8_lossy(&raw_body)
                        .chars()
                        .take(200)
                        .collect()
                });
            anyhow::bail!("Anthropic API error ({}): {}", status, error_msg);
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut sse = SseBuffer::new();
            let mut current_event_type = String::new();
            let mut index_to_tool: HashMap<u64, ToolBlockInfo> = HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        return;
                    }
                };
                sse.extend(&chunk);

                while let Some(line_bytes) = sse.next_line() {
                    // Decode once we have a complete line — safe from the
                    // UTF-8-split-across-chunks bug.
                    let line = match std::str::from_utf8(&line_bytes) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

                    if line.is_empty() {
                        current_event_type.clear();
                        continue;
                    }

                    if let Some(evt) = line.strip_prefix("event: ") {
                        current_event_type = evt.to_string();
                    } else if let Some(data_str) = line.strip_prefix("data: ") {
                        if let Ok(data) = serde_json::from_str::<Value>(data_str) {
                            Self::handle_sse_event(
                                &current_event_type,
                                &data,
                                &tx,
                                &mut index_to_tool,
                            )
                            .await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Drive `handle_sse_event` against a fixed sequence of events and
    /// return every emitted StreamEvent. Verifies the index→id stitching.
    async fn replay(events: Vec<(&str, Value)>) -> Vec<StreamEvent> {
        let (tx, mut rx) = mpsc::channel(64);
        let mut map: HashMap<u64, ToolBlockInfo> = HashMap::new();
        for (ty, data) in events {
            AnthropicProvider::handle_sse_event(ty, &data, &tx, &mut map).await;
        }
        drop(tx);
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        out
    }

    fn is_tool_call_delta_for(e: &StreamEvent, expected_id: &str) -> bool {
        matches!(
            e,
            StreamEvent::ToolCallDelta { id, .. } if id == expected_id
        )
    }

    #[tokio::test]
    async fn tool_call_delta_uses_real_id_not_index() {
        // Event shape from Anthropic:
        //   content_block_start  index=1, content_block={type: tool_use, id: "toolu_abc", name: "read_file"}
        //   content_block_delta  index=1, delta={type: input_json_delta, partial_json: "{\"pat"}
        //   content_block_delta  index=1, delta={type: input_json_delta, partial_json: "h\":\"/tmp\"}"}
        //   content_block_stop   index=1
        let events = vec![
            (
                "content_block_start",
                json!({
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_abc",
                        "name": "read_file"
                    }
                }),
            ),
            (
                "content_block_delta",
                json!({
                    "index": 1,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"pat"
                    }
                }),
            ),
            (
                "content_block_delta",
                json!({
                    "index": 1,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "h\":\"/tmp\"}"
                    }
                }),
            ),
            ("content_block_stop", json!({ "index": 1 })),
        ];

        let out = replay(events).await;

        // Should emit: Start{id=toolu_abc}, Delta{id=toolu_abc}, Delta{id=toolu_abc}, End{id=toolu_abc}
        assert_eq!(out.len(), 4, "events: {:?}", out);
        assert!(matches!(&out[0], StreamEvent::ToolCallStart { id, name }
            if id == "toolu_abc" && name == "read_file"));
        assert!(is_tool_call_delta_for(&out[1], "toolu_abc"));
        assert!(is_tool_call_delta_for(&out[2], "toolu_abc"));
        assert!(matches!(&out[3], StreamEvent::ToolCallEnd { id } if id == "toolu_abc"));
    }

    #[tokio::test]
    async fn text_block_stop_does_not_emit_tool_call_end() {
        // Text block at index 0 should NOT produce a ToolCallEnd at stop.
        // Previous implementation emitted ToolCallEnd { id: "0" } which
        // confused consumers tracking tool state.
        let events = vec![
            (
                "content_block_start",
                json!({
                    "index": 0,
                    "content_block": { "type": "text", "text": "" }
                }),
            ),
            (
                "content_block_delta",
                json!({
                    "index": 0,
                    "delta": { "type": "text_delta", "text": "Hello" }
                }),
            ),
            ("content_block_stop", json!({ "index": 0 })),
        ];

        let out = replay(events).await;

        // Only one emission expected: the Delta.
        assert_eq!(out.len(), 1, "events: {:?}", out);
        assert!(matches!(&out[0], StreamEvent::Delta(t) if t == "Hello"));
    }

    #[tokio::test]
    async fn mixed_text_and_tool_blocks_stitch_correctly() {
        // Realistic: text at index 0, tool_use at index 1 (with different id),
        // interleaved deltas. Ids must never get confused.
        let events = vec![
            (
                "content_block_start",
                json!({
                    "index": 0,
                    "content_block": { "type": "text", "text": "" }
                }),
            ),
            (
                "content_block_delta",
                json!({ "index": 0, "delta": { "type": "text_delta", "text": "Let me " } }),
            ),
            (
                "content_block_start",
                json!({
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_xyz",
                        "name": "search"
                    }
                }),
            ),
            (
                "content_block_delta",
                json!({ "index": 0, "delta": { "type": "text_delta", "text": "search." } }),
            ),
            (
                "content_block_delta",
                json!({
                    "index": 1,
                    "delta": { "type": "input_json_delta", "partial_json": "{\"q\":\"x\"}" }
                }),
            ),
            ("content_block_stop", json!({ "index": 0 })),
            ("content_block_stop", json!({ "index": 1 })),
            ("message_stop", json!({})),
        ];

        let out = replay(events).await;

        // Expect: tool Start, text Delta, text Delta, tool Delta, tool End, Done.
        // The text block stop must NOT produce a ToolCallEnd.
        let tool_ends: Vec<&StreamEvent> = out
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallEnd { .. }))
            .collect();
        assert_eq!(tool_ends.len(), 1, "expected exactly 1 ToolCallEnd, got {:?}", tool_ends);
        let StreamEvent::ToolCallEnd { id } = tool_ends[0] else { unreachable!() };
        assert_eq!(id, "toolu_xyz");

        // And every ToolCallDelta uses toolu_xyz.
        for e in &out {
            if let StreamEvent::ToolCallDelta { id, .. } = e {
                assert_eq!(id, "toolu_xyz", "unexpected tool delta id");
            }
        }
    }

    #[tokio::test]
    async fn delta_without_prior_start_is_dropped() {
        // If we somehow see a delta for an index we never saw start, we
        // must NOT emit with an incorrect id (the old code would emit
        // with id=index_as_string). Dropping is the safe choice.
        let events = vec![(
            "content_block_delta",
            json!({
                "index": 42,
                "delta": { "type": "input_json_delta", "partial_json": "{}" }
            }),
        )];
        let out = replay(events).await;
        assert!(out.is_empty(), "orphan delta should be dropped: {:?}", out);
    }
}
