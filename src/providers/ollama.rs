use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{ChatMessage, ChatRequest, ChatResponse, Provider, ToolCall, UsageInfo};

/// Ollama local model provider.
///
/// Communicates with a local Ollama instance via its HTTP API.
/// No authentication is required.
pub struct OllamaProvider {
    client: reqwest::Client,
    model: String,
    base_url: String,
    ctx_window: usize,
}

impl OllamaProvider {
    /// Create a new Ollama provider.
    ///
    /// Defaults: model `"llama3.1"`, base_url `"http://localhost:11434"`,
    /// context_window `8192`.
    pub fn new(
        model: Option<String>,
        base_url: Option<String>,
        context_window: Option<usize>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            model: model.unwrap_or_else(|| "llama3.1".to_string()),
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
            ctx_window: context_window.unwrap_or(8192),
        }
    }

    /// Convert our ChatMessages to Ollama API message format.
    fn convert_messages(
        system: Option<&str>,
        messages: &[ChatMessage],
    ) -> Vec<Value> {
        let mut api_messages = Vec::new();

        // System message as first message.
        if let Some(system_text) = system {
            api_messages.push(json!({
                "role": "system",
                "content": system_text
            }));
        }

        for msg in messages {
            match msg.role.as_str() {
                "assistant" => {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        let tc_array: Vec<Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments
                                    }
                                })
                            })
                            .collect();

                        let mut m = json!({
                            "role": "assistant",
                            "content": msg.content.as_deref().unwrap_or(""),
                            "tool_calls": tc_array
                        });
                        // Remove content if empty
                        if msg.content.as_deref().unwrap_or("").is_empty() {
                            m.as_object_mut().unwrap().remove("content");
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
                        "content": msg.content.as_deref().unwrap_or("")
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

    /// Convert our ToolSpec list to Ollama's tools format.
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

    /// Parse the Ollama response JSON into our ChatResponse.
    fn parse_response(body: &Value) -> Result<ChatResponse> {
        let message = body
            .get("message")
            .context("missing message in Ollama response")?;

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(tcs) = message.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let function = tc.get("function").unwrap_or(&Value::Null);
                let name = function
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = function
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Null);

                // Generate an ID since Ollama doesn't always provide one.
                let id = tc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("ollama_{}", uuid::Uuid::new_v4()));

                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }

        // Ollama may report token counts.
        let usage = if body.get("prompt_eval_count").is_some()
            || body.get("eval_count").is_some()
        {
            Some(UsageInfo {
                input_tokens: body
                    .get("prompt_eval_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: body
                    .get("eval_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_read_tokens: None,
            })
        } else {
            None
        };

        Ok(ChatResponse {
            content,
            tool_calls,
            usage,
        })
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        let messages = Self::convert_messages(request.system, request.messages);

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
        });

        // Add tools if provided.
        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(Self::convert_tools(tools));
            }
        }

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("sending request to Ollama API")?;

        let status = response.status();
        let response_body: Value = response
            .json()
            .await
            .context("parsing Ollama API response")?;

        if !status.is_success() {
            let error_msg = response_body
                .get("error")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Ollama API error ({}): {}", status, error_msg);
        }

        Self::parse_response(&response_body)
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        self.ctx_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_messages_with_system() {
        let msgs = vec![ChatMessage::user("hi")];
        let converted = OllamaProvider::convert_messages(Some("you are helpful"), &msgs);
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "system");
        assert_eq!(converted[0]["content"], "you are helpful");
        assert_eq!(converted[1]["role"], "user");
        assert_eq!(converted[1]["content"], "hi");
    }

    #[test]
    fn test_convert_messages_no_system() {
        let msgs = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi there"),
        ];
        let converted = OllamaProvider::convert_messages(None, &msgs);
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[1]["role"], "assistant");
    }

    #[test]
    fn test_parse_response_text() {
        let body = json!({
            "model": "llama3.1",
            "message": {
                "role": "assistant",
                "content": "Hello!"
            },
            "done": true,
            "prompt_eval_count": 10,
            "eval_count": 5
        });

        let response = OllamaProvider::parse_response(&body).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello!"));
        assert!(response.tool_calls.is_empty());
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_response_with_tool_calls() {
        let body = json!({
            "model": "llama3.1",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "get_weather",
                        "arguments": {"location": "SF"}
                    }
                }]
            },
            "done": true
        });

        let response = OllamaProvider::parse_response(&body).unwrap();
        assert!(response.content.is_none()); // empty string filtered out
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(response.tool_calls[0].arguments["location"], "SF");
    }

    #[test]
    fn test_convert_tools() {
        let specs = vec![crate::tools::traits::ToolSpec {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            }),
        }];
        let converted = OllamaProvider::convert_tools(&specs);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["function"]["name"], "search");
    }
}
