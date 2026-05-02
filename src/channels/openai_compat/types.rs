//! OpenAI Chat Completions wire-format types, faithful to the live
//! spec at `openai/openai-openapi` (`manual_spec/openapi.yaml`,
//! verified 2025-04-29) and the SSE format documented in OpenAI's
//! Python SDK at `openai/openai-python` (`src/openai/_streaming.py`).
//!
//! Field naming is `snake_case` to match the wire format exactly.
//! `serde_with::skip_serializing_none` keeps optional fields out of
//! responses when they're `None`, which is what every OpenAI SDK
//! expects.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// -- Request -------------------------------------------------------

/// `POST /v1/chat/completions` request body. Required fields:
/// `model` and `messages`. Everything else is optional and uses
/// OpenAI's defaults when absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatRequestMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    /// Catchall for any field we don't model explicitly. The
    /// passthrough mode ignores these; later phases may consume
    /// them.
    #[serde(flatten)]
    pub extras: serde_json::Map<String, Value>,
}

/// `stream_options.include_usage` — when true, the final SSE chunk
/// (with `choices: []`) carries the token counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

/// `stop` accepts a single string or an array of strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopValue {
    Single(String),
    Many(Vec<String>),
}

/// One message in a request. `role` is `system` / `user` /
/// `assistant` / `tool` / `developer` / `function` (legacy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequestMessage {
    pub role: String,
    /// `content` may be a plain string or an array of typed parts
    /// (`{type: "text", text}` / `{type: "image_url", image_url:
    /// {url}}`). We keep it as `Value` to round-trip both shapes
    /// faithfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// Optional name for system/user/tool messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool call results: `tool_call_id` references the assistant's
    /// previously emitted `tool_calls[].id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Assistant messages may carry `tool_calls` to drive the next
    /// turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Tool-call shape on assistant messages. `arguments` is a string
/// containing JSON (per the spec — not a parsed object), so it
/// round-trips even when arguments aren't valid JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

// -- Non-streaming response ----------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    /// Always `"chat.completion"` for non-streaming.
    pub object: String,
    /// Unix timestamp.
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatResponseMessage,
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponseMessage {
    pub role: String,
    /// Plain string content — vision-style typed-parts arrays are
    /// only supported on the request side (the model emits text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// New `refusal` field for safety refusals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_prediction_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_prediction_tokens: Option<u32>,
}

// -- Streaming chunk -----------------------------------------------

/// One streaming chunk. Object is always
/// `"chat.completion.chunk"`. `choices[0].delta` carries the
/// incremental update; `delta.role` appears only on the first
/// chunk; `delta.content` is the text token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionStreamChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    /// Final usage chunk (when `stream_options.include_usage =
    /// true`) carries `choices: []` and this populated `usage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionStreamChoice {
    pub index: u32,
    pub delta: ChatCompletionDelta,
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatCompletionDelta {
    /// Present only on the first chunk of a stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// Tool calls in stream deltas use a partial shape: the first chunk
/// has the index + id + name, subsequent chunks accumulate the
/// `arguments` string fragment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<ToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunctionDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// -- /v1/models ----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    pub object: String,
    pub data: Vec<ModelObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelObject {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

// -- /v1/capabilities (Hermes-style advertisement) -----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub api_version: String,
    pub server: String,
    pub server_version: String,
    pub endpoints: Vec<String>,
    pub features: CapabilityFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityFeatures {
    pub streaming: bool,
    pub tool_calling: bool,
    pub multimodal: bool,
    pub session_continuity: bool,
}

// -- Error envelope ------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl ErrorEnvelope {
    pub fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        code: Option<String>,
    ) -> Self {
        Self {
            error: ErrorBody {
                message: message.into(),
                type_: kind.into(),
                param: None,
                code,
            },
        }
    }
}

// -- Helpers -------------------------------------------------------

/// Generate an OpenAI-shaped completion id (`chatcmpl-<random>`).
pub fn new_completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

/// Current Unix time in seconds for the `created` field.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Extract a plain text string from a request `content` value,
/// joining multipart text parts. Image parts and other typed
/// content are skipped with a debug log — multimodal handling
/// lands in a later phase.
pub fn flatten_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(t);
                        }
                    }
                    Some(other) => {
                        tracing::debug!(
                            kind = other,
                            "openai_compat: dropping non-text content part (multimodal not yet supported)"
                        );
                    }
                    None => {}
                }
            }
            out
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_minimal_request() {
        let raw = json!({
            "model": "fennec-agent",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let req: ChatCompletionRequest = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(req.model, "fennec-agent");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        // Round-trip serializes back to compatible JSON.
        let back = serde_json::to_value(&req).unwrap();
        assert_eq!(back["model"], "fennec-agent");
    }

    #[test]
    fn round_trip_streaming_request() {
        let raw = json!({
            "model": "fennec-agent",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
            "stream_options": { "include_usage": true },
            "temperature": 0.2,
            "top_p": 0.9,
            "stop": ["END"]
        });
        let req: ChatCompletionRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.stream_options.unwrap().include_usage, Some(true));
        assert_eq!(req.temperature, Some(0.2));
        assert!(matches!(req.stop, Some(StopValue::Many(_))));
    }

    #[test]
    fn round_trip_tool_calls_in_request() {
        let raw = json!({
            "model": "x",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc1",
                    "type": "function",
                    "function": {"name": "f", "arguments": "{\"a\": 1}"}
                }]
            }]
        });
        let req: ChatCompletionRequest = serde_json::from_value(raw).unwrap();
        let tcs = req.messages[0].tool_calls.as_ref().unwrap();
        assert_eq!(tcs[0].id, "tc1");
        assert_eq!(tcs[0].type_, "function");
        assert_eq!(tcs[0].function.name, "f");
        assert_eq!(tcs[0].function.arguments, "{\"a\": 1}");
    }

    #[test]
    fn flatten_content_string() {
        assert_eq!(flatten_content(&json!("hello")), "hello");
    }

    #[test]
    fn flatten_content_text_parts() {
        let v = json!([
            {"type": "text", "text": "hello"},
            {"type": "text", "text": "world"}
        ]);
        assert_eq!(flatten_content(&v), "hello\nworld");
    }

    #[test]
    fn flatten_content_drops_image_parts() {
        let v = json!([
            {"type": "text", "text": "describe"},
            {"type": "image_url", "image_url": {"url": "https://x"}}
        ]);
        assert_eq!(flatten_content(&v), "describe");
    }

    #[test]
    fn flatten_content_null() {
        assert_eq!(flatten_content(&Value::Null), "");
    }

    #[test]
    fn stop_value_serializes_both_shapes() {
        let s = StopValue::Single("END".into());
        let m = StopValue::Many(vec!["A".into(), "B".into()]);
        assert_eq!(serde_json::to_value(&s).unwrap(), json!("END"));
        assert_eq!(serde_json::to_value(&m).unwrap(), json!(["A", "B"]));
    }

    #[test]
    fn response_omits_none_fields() {
        let resp = ChatCompletionResponse {
            id: "x".into(),
            object: "chat.completion".into(),
            created: 1,
            model: "m".into(),
            choices: vec![],
            usage: None,
            system_fingerprint: None,
            service_tier: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("usage").is_none());
        assert!(v.get("system_fingerprint").is_none());
    }

    #[test]
    fn stream_chunk_role_only_on_first() {
        let first = ChatCompletionDelta {
            role: Some("assistant".into()),
            content: Some("Hel".into()),
            ..Default::default()
        };
        let mid = ChatCompletionDelta {
            content: Some("lo".into()),
            ..Default::default()
        };
        let f = serde_json::to_value(&first).unwrap();
        let m = serde_json::to_value(&mid).unwrap();
        assert_eq!(f["role"], "assistant");
        assert!(m.get("role").is_none(), "non-first chunk must omit role");
    }

    #[test]
    fn error_envelope_shape_matches_openai() {
        let env = ErrorEnvelope::new(
            "invalid_request_error",
            "missing field model",
            Some("missing_field".into()),
        );
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["error"]["message"], "missing field model");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert_eq!(v["error"]["code"], "missing_field");
    }

    #[test]
    fn new_completion_id_has_chatcmpl_prefix() {
        let id = new_completion_id();
        assert!(id.starts_with("chatcmpl-"));
        assert!(id.len() > 9);
    }
}
