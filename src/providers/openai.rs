use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::traits::{
    ChatMessage, ChatRequest, ChatResponse, Provider, StreamEvent, ToolCall, UsageInfo,
};

/// OpenAI-compatible API provider.
///
/// Works with OpenAI, Azure OpenAI, and any API that follows the
/// OpenAI chat completions format.
pub struct OpenAIProvider {
    api_key: String,
    client: reqwest::Client,
    model: String,
    base_url: String,
    ctx_window: usize,
    extra_headers: Vec<(String, String)>,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider.
    ///
    /// Defaults: model `"gpt-4o"`, base_url `"https://api.openai.com/v1"`,
    /// context_window `128_000`.
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        context_window: Option<usize>,
    ) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
            model: model.unwrap_or_else(|| "gpt-4o".to_string()),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            ctx_window: context_window.unwrap_or(128_000),
            extra_headers: Vec::new(),
        }
    }

    /// Add extra headers to be sent with every request.
    pub fn with_extra_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// Convert our ChatMessages to OpenAI API message format.
    fn convert_messages(messages: &[ChatMessage]) -> Vec<Value> {
        let mut api_messages = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "assistant" => {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        let tc_array: Vec<Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments.to_string()
                                    }
                                })
                            })
                            .collect();

                        let mut m = json!({
                            "role": "assistant",
                            "tool_calls": tc_array
                        });
                        if let Some(ref content) = msg.content {
                            if !content.is_empty() {
                                m["content"] = json!(content);
                            }
                        }
                        api_messages.push(m);
                    } else {
                        api_messages.push(json!({
                            "role": "assistant",
                            "content": msg.content.as_deref().unwrap_or("")
                        }));
                    }
                }
                "tool" => {
                    api_messages.push(json!({
                        "role": "tool",
                        "tool_call_id": msg.tool_call_id.as_deref().unwrap_or(""),
                        "content": msg.content.as_deref().unwrap_or("")
                    }));
                }
                _ => {
                    // "system", "user", etc.
                    api_messages.push(json!({
                        "role": msg.role,
                        "content": msg.content.as_deref().unwrap_or("")
                    }));
                }
            }
        }

        api_messages
    }

    /// Convert our ToolSpec list to OpenAI's function calling format.
    fn convert_tools(tools: &[crate::tools::traits::ToolSpec]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect()
    }

    /// Build the common request body (shared by chat and chat_stream).
    fn build_request_body(
        &self,
        request: &ChatRequest<'_>,
        stream: bool,
    ) -> (Vec<Value>, Value) {
        let mut messages = Vec::new();

        if let Some(system_text) = request.system {
            messages.push(json!({
                "role": "system",
                "content": system_text
            }));
        }

        messages.extend(Self::convert_messages(request.messages));

        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": messages,
        });

        if stream {
            body["stream"] = json!(true);
        }

        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(Self::convert_tools(tools));
            }
        }

        (messages, body)
    }

    /// Parse the OpenAI response JSON into our ChatResponse.
    fn parse_response(body: &Value) -> Result<ChatResponse> {
        let choice = body
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .context("missing choices[0] in response")?;

        let message = choice
            .get("message")
            .context("missing message in choice")?;

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(tcs) = message.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let id = tc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let function = tc.get("function").unwrap_or(&Value::Null);
                let name = function
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments_str = function
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let arguments: Value =
                    serde_json::from_str(arguments_str).unwrap_or(Value::Null);

                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }

        let usage = body.get("usage").map(|u| UsageInfo {
            input_tokens: u
                .get("prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_read_tokens: u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
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
impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        let (_messages, body) = self.build_request_body(&request, false);

        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.extra_headers {
            req = req.header(key.as_str(), value.as_str());
        }

        let response = req
            .json(&body)
            .send()
            .await
            .context("sending request to OpenAI-compatible API")?;

        let status = response.status();
        let response_body: Value = response
            .json()
            .await
            .context("parsing OpenAI API response")?;

        if !status.is_success() {
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("OpenAI API error ({}): {}", status, error_msg);
        }

        Self::parse_response(&response_body)
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        self.ctx_window
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let (_messages, body) = self.build_request_body(&request, true);

        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        for (key, value) in &self.extra_headers {
            req = req.header(key.as_str(), value.as_str());
        }

        let response = req
            .json(&body)
            .send()
            .await
            .context("sending streaming request to OpenAI-compatible API")?;

        let status = response.status();
        if !status.is_success() {
            let response_body: Value = response
                .json()
                .await
                .context("parsing OpenAI streaming error response")?;
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("OpenAI API error ({}): {}", status, error_msg);
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();

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

                    let data_str = match line.strip_prefix("data: ") {
                        Some(s) => s,
                        None => continue,
                    };

                    if data_str == "[DONE]" {
                        let _ = tx.send(StreamEvent::Done).await;
                        return;
                    }

                    let data: Value = match serde_json::from_str(data_str) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Extract the first choice's delta.
                    let choice = match data
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                    {
                        Some(c) => c,
                        None => continue,
                    };

                    // Check finish_reason.
                    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                        if reason == "stop" || reason == "tool_calls" {
                            let _ = tx.send(StreamEvent::Done).await;
                            return;
                        }
                    }

                    if let Some(delta) = choice.get("delta") {
                        // Text content delta.
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                            if !content.is_empty() {
                                let _ = tx.send(StreamEvent::Delta(content.to_string())).await;
                            }
                        }

                        // Tool call deltas.
                        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                            for tc in tcs {
                                let id = tc
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let func = tc.get("function").unwrap_or(&Value::Null);

                                // If function.name is present, this is the start.
                                if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                    let _ = tx
                                        .send(StreamEvent::ToolCallStart {
                                            id: id.clone(),
                                            name: name.to_string(),
                                        })
                                        .await;
                                }

                                // If function.arguments is present, emit delta.
                                if let Some(args) =
                                    func.get("arguments").and_then(|a| a.as_str())
                                {
                                    if !args.is_empty() {
                                        let _ = tx
                                            .send(StreamEvent::ToolCallDelta {
                                                id: id.clone(),
                                                arguments_delta: args.to_string(),
                                            })
                                            .await;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // If we exit without [DONE], send Done anyway.
            let _ = tx.send(StreamEvent::Done).await;
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_messages_user() {
        let msgs = vec![ChatMessage::user("hello")];
        let converted = OpenAIProvider::convert_messages(&msgs);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"], "hello");
    }

    #[test]
    fn test_convert_messages_with_tool_calls() {
        let mut msg = ChatMessage::assistant("thinking...");
        msg.tool_calls = Some(vec![ToolCall {
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({"path": "/tmp/test.txt"}),
        }]);

        let converted = OpenAIProvider::convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "assistant");
        assert!(converted[0]["tool_calls"].is_array());
        let tc = &converted[0]["tool_calls"][0];
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let msg = ChatMessage::tool_result("tc_1", "file contents");
        let converted = OpenAIProvider::convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "tool");
        assert_eq!(converted[0]["tool_call_id"], "tc_1");
        assert_eq!(converted[0]["content"], "file contents");
    }

    #[test]
    fn test_parse_response_text() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });

        let response = OpenAIProvider::parse_response(&body).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello!"));
        assert!(response.tool_calls.is_empty());
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_response_with_tool_calls() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"SF\"}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 10
            }
        });

        let response = OpenAIProvider::parse_response(&body).unwrap();
        assert!(response.content.is_none());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_abc");
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(response.tool_calls[0].arguments["location"], "SF");
    }

    #[test]
    fn test_convert_tools() {
        let specs = vec![crate::tools::traits::ToolSpec {
            name: "my_tool".to_string(),
            description: "Does stuff".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "arg": {"type": "string"}
                }
            }),
        }];
        let converted = OpenAIProvider::convert_tools(&specs);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["function"]["name"], "my_tool");
        assert_eq!(converted[0]["function"]["description"], "Does stuff");
    }
}
