//! HTTP handlers for the OpenAI-compat endpoints.
//!
//! Each handler is small and self-contained. The shared server
//! state (`ServerState`) carries the LLM provider, the bearer key,
//! and the advertised model name; handlers pull from it via axum's
//! `State` extractor.
//!
//! Handler responsibilities:
//!   - Auth check via [`super::auth::check_bearer`].
//!   - Parse the OpenAI-shaped request.
//!   - Convert to Fennec's internal `ChatRequest` (passthrough
//!     mode) — message contents are flattened to plain text via
//!     [`super::types::flatten_content`].
//!   - Call `provider.chat()` or `provider.chat_stream()`.
//!   - Convert the result back to the OpenAI wire format.
//!   - Stream as SSE when `stream: true`, return JSON otherwise.

use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response, sse::KeepAlive},
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::agent::thinking::ThinkingLevel;
use crate::providers::traits::{ChatMessage, ChatRequest, Provider, StreamEvent};

use super::auth::check_bearer;
use super::types::*;

/// Server state that handlers share. Cloned cheaply via the inner
/// `Arc`s.
#[derive(Clone)]
pub struct ServerState {
    pub provider: Arc<dyn Provider>,
    pub api_key: Arc<String>,
    pub model_name: Arc<String>,
}

// -- /health -------------------------------------------------------

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

pub async fn health_detailed(State(state): State<ServerState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "provider": state.provider.name(),
            "model": *state.model_name,
            "tool_calling": state.provider.supports_tool_calling(),
            "streaming": state.provider.supports_streaming(),
        })),
    )
}

// -- /v1/models ----------------------------------------------------

pub async fn list_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Response {
    if let Err(reason) = check_bearer(&headers, &state.api_key) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid_request_error", reason, None);
    }
    let body = ModelListResponse {
        object: "list".into(),
        data: vec![ModelObject {
            id: (*state.model_name).clone(),
            object: "model".into(),
            created: now_unix(),
            owned_by: "fennec".into(),
        }],
    };
    (StatusCode::OK, Json(body)).into_response()
}

// -- /v1/capabilities ----------------------------------------------

pub async fn capabilities(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Response {
    if let Err(reason) = check_bearer(&headers, &state.api_key) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid_request_error", reason, None);
    }
    let body = CapabilitiesResponse {
        api_version: "v1".into(),
        server: "fennec".into(),
        server_version: env!("CARGO_PKG_VERSION").into(),
        endpoints: vec![
            "/v1/chat/completions".into(),
            "/v1/models".into(),
            "/v1/capabilities".into(),
            "/health".into(),
            "/health/detailed".into(),
        ],
        features: CapabilityFeatures {
            streaming: state.provider.supports_streaming(),
            tool_calling: state.provider.supports_tool_calling(),
            // E-2-1 ships passthrough only — no multimodal, no
            // session continuity yet. Both fields surface as false
            // so clients don't expect features that aren't there.
            multimodal: false,
            session_continuity: false,
        },
    };
    (StatusCode::OK, Json(body)).into_response()
}

// -- /v1/chat/completions ------------------------------------------

pub async fn chat_completions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    if let Err(reason) = check_bearer(&headers, &state.api_key) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid_request_error", reason, None);
    }
    if request.messages.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "messages must contain at least one message",
            Some("messages".into()),
        );
    }

    // Convert OpenAI-shaped messages to Fennec's internal shape.
    let internal: Vec<ChatMessage> = request
        .messages
        .iter()
        .map(openai_message_to_internal)
        .collect();

    let max_tokens = request
        .max_completion_tokens
        .or(request.max_tokens)
        .unwrap_or(4096) as usize;
    let temperature = request.temperature.unwrap_or(0.7);

    let internal_req = ChatRequest {
        system: None,
        messages: &internal,
        tools: None,
        max_tokens,
        temperature,
        thinking_level: ThinkingLevel::Off,
    };

    let model_name = (*state.model_name).clone();
    let stream_requested = request.stream.unwrap_or(false);
    let include_usage = request
        .stream_options
        .as_ref()
        .and_then(|o| o.include_usage)
        .unwrap_or(false);

    if stream_requested {
        stream_response(
            Arc::clone(&state.provider),
            internal_req,
            model_name,
            include_usage,
        )
        .await
    } else {
        match state.provider.chat(internal_req).await {
            Ok(resp) => {
                let body = build_non_streaming_response(model_name, resp);
                (StatusCode::OK, Json(body)).into_response()
            }
            Err(e) => error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                format!("provider returned error: {}", e),
                None,
            ),
        }
    }
}

/// Build a non-streaming Chat Completions response from a provider
/// `ChatResponse`.
fn build_non_streaming_response(
    model: String,
    response: crate::providers::traits::ChatResponse,
) -> ChatCompletionResponse {
    let usage = response.usage.as_ref().map(|u| Usage {
        prompt_tokens: u.input_tokens.try_into().unwrap_or(0),
        completion_tokens: u.output_tokens.try_into().unwrap_or(0),
        total_tokens: (u.input_tokens + u.output_tokens).try_into().unwrap_or(0),
        prompt_tokens_details: u.cache_read_tokens.map(|c| PromptTokensDetails {
            cached_tokens: Some(c.try_into().unwrap_or(0)),
            audio_tokens: None,
        }),
        completion_tokens_details: None,
    });
    let tool_calls = if response.tool_calls.is_empty() {
        None
    } else {
        Some(
            response
                .tool_calls
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    type_: "function".into(),
                    function: ToolCallFunction {
                        name: tc.name,
                        arguments: serde_json::to_string(&tc.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })
                .collect(),
        )
    };
    let finish_reason = if tool_calls.is_some() {
        "tool_calls"
    } else {
        "stop"
    };

    ChatCompletionResponse {
        id: new_completion_id(),
        object: "chat.completion".into(),
        created: now_unix(),
        model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatResponseMessage {
                role: "assistant".into(),
                content: response.content,
                tool_calls,
                refusal: None,
            },
            finish_reason: Some(finish_reason.into()),
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    }
}

/// Stream the LLM response as SSE chunks per the OpenAI streaming
/// contract.
async fn stream_response(
    provider: Arc<dyn Provider>,
    request: ChatRequest<'_>,
    model: String,
    include_usage: bool,
) -> Response {
    let stream_id = new_completion_id();
    let created = now_unix();

    // Kick off the provider stream first so we can surface failures
    // before we commit to an SSE response shape.
    let rx = match provider.chat_stream(request).await {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                format!("provider stream open failed: {}", e),
                None,
            );
        }
    };

    let event_stream = async_stream::stream! {
        // First chunk announces role.
        let first = ChatCompletionStreamChunk {
            id: stream_id.clone(),
            object: "chat.completion.chunk".into(),
            created,
            model: model.clone(),
            choices: vec![ChatCompletionStreamChoice {
                index: 0,
                delta: ChatCompletionDelta {
                    role: Some("assistant".into()),
                    ..Default::default()
                },
                finish_reason: None,
                logprobs: None,
            }],
            system_fingerprint: None,
            usage: None,
        };
        yield sse_data(&first);

        let mut rx = rx;
        let mut tool_call_index: u32 = 0;
        let mut finish_reason: Option<String> = Some("stop".into());

        while let Some(ev) = rx.recv().await {
            match ev {
                StreamEvent::Delta(text) => {
                    let chunk = ChatCompletionStreamChunk {
                        id: stream_id.clone(),
                        object: "chat.completion.chunk".into(),
                        created,
                        model: model.clone(),
                        choices: vec![ChatCompletionStreamChoice {
                            index: 0,
                            delta: ChatCompletionDelta {
                                content: Some(text),
                                ..Default::default()
                            },
                            finish_reason: None,
                            logprobs: None,
                        }],
                        system_fingerprint: None,
                        usage: None,
                    };
                    yield sse_data(&chunk);
                }
                StreamEvent::ToolCallStart { id, name } => {
                    finish_reason = Some("tool_calls".into());
                    let chunk = ChatCompletionStreamChunk {
                        id: stream_id.clone(),
                        object: "chat.completion.chunk".into(),
                        created,
                        model: model.clone(),
                        choices: vec![ChatCompletionStreamChoice {
                            index: 0,
                            delta: ChatCompletionDelta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: tool_call_index,
                                    id: Some(id),
                                    type_: Some("function".into()),
                                    function: Some(ToolCallFunctionDelta {
                                        name: Some(name),
                                        arguments: None,
                                    }),
                                }]),
                                ..Default::default()
                            },
                            finish_reason: None,
                            logprobs: None,
                        }],
                        system_fingerprint: None,
                        usage: None,
                    };
                    yield sse_data(&chunk);
                    tool_call_index += 1;
                }
                StreamEvent::ToolCallDelta { id: _, arguments_delta } => {
                    let chunk = ChatCompletionStreamChunk {
                        id: stream_id.clone(),
                        object: "chat.completion.chunk".into(),
                        created,
                        model: model.clone(),
                        choices: vec![ChatCompletionStreamChoice {
                            index: 0,
                            delta: ChatCompletionDelta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: tool_call_index.saturating_sub(1),
                                    id: None,
                                    type_: None,
                                    function: Some(ToolCallFunctionDelta {
                                        name: None,
                                        arguments: Some(arguments_delta),
                                    }),
                                }]),
                                ..Default::default()
                            },
                            finish_reason: None,
                            logprobs: None,
                        }],
                        system_fingerprint: None,
                        usage: None,
                    };
                    yield sse_data(&chunk);
                }
                StreamEvent::ToolCallEnd { .. } => {
                    // No-op — the OpenAI wire protocol doesn't have a
                    // separate "tool-call done" event; clients treat
                    // the next chunk's absence of further tool-call
                    // deltas as the end.
                }
                StreamEvent::Done => {
                    let final_chunk = ChatCompletionStreamChunk {
                        id: stream_id.clone(),
                        object: "chat.completion.chunk".into(),
                        created,
                        model: model.clone(),
                        choices: vec![ChatCompletionStreamChoice {
                            index: 0,
                            delta: ChatCompletionDelta::default(),
                            finish_reason: finish_reason.clone(),
                            logprobs: None,
                        }],
                        system_fingerprint: None,
                        usage: None,
                    };
                    yield sse_data(&final_chunk);
                    if include_usage {
                        // Usage details aren't surfaced through the
                        // current StreamEvent enum; emit a usage
                        // chunk with zeros so clients that gate on
                        // its presence still terminate cleanly. A
                        // future provider-trait extension can fill
                        // these with real numbers.
                        let usage_chunk = ChatCompletionStreamChunk {
                            id: stream_id.clone(),
                            object: "chat.completion.chunk".into(),
                            created,
                            model: model.clone(),
                            choices: Vec::new(),
                            system_fingerprint: None,
                            usage: Some(Usage {
                                prompt_tokens: 0,
                                completion_tokens: 0,
                                total_tokens: 0,
                                prompt_tokens_details: None,
                                completion_tokens_details: None,
                            }),
                        };
                        yield sse_data(&usage_chunk);
                    }
                    yield Ok::<_, std::io::Error>(b"data: [DONE]\n\n".to_vec());
                    break;
                }
                StreamEvent::Error(msg) => {
                    let err_chunk = json!({
                        "error": {
                            "message": msg,
                            "type": "upstream_error",
                        }
                    });
                    let line = format!("data: {}\n\n", err_chunk);
                    yield Ok(line.into_bytes());
                    yield Ok(b"data: [DONE]\n\n".to_vec());
                    break;
                }
            }
        }
    };

    let body = Body::from_stream(event_stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap_or_else(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "could not build SSE response",
                None,
            )
        })
}

/// Serialize a stream chunk as `data: <json>\n\n`.
fn sse_data<T: serde::Serialize>(chunk: &T) -> Result<Vec<u8>, std::io::Error> {
    let json = serde_json::to_string(chunk).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    let line = format!("data: {}\n\n", json);
    Ok(line.into_bytes())
}

/// Convert a request-side OpenAI message (string-or-array content,
/// optional tool_calls / tool_call_id) into Fennec's internal
/// `ChatMessage` (single string content; the array form is
/// flattened to text, and `tool` role messages map to
/// `tool_call_id`-bearing internal messages).
fn openai_message_to_internal(m: &ChatRequestMessage) -> ChatMessage {
    let content = m
        .content
        .as_ref()
        .map(flatten_content);
    ChatMessage {
        role: m.role.clone(),
        content,
        tool_calls: m
            .tool_calls
            .as_ref()
            .map(|tcs| {
                tcs.iter()
                    .map(|tc| crate::providers::traits::ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(Value::Null),
                    })
                    .collect()
            }),
        tool_call_id: m.tool_call_id.clone(),
    }
}

/// Build a JSON error response in the OpenAI envelope shape.
pub fn error_response(
    status: StatusCode,
    kind: &str,
    message: impl Into<String>,
    param: Option<String>,
) -> Response {
    let mut env = ErrorEnvelope::new(kind, message, None);
    env.error.param = param;
    (status, Json(env)).into_response()
}

// keep_alive() helper documents the SSE keep-alive shape we'd use
// if we adopted axum's `Sse` extractor; current code uses raw
// `Body::from_stream` to keep tight control of the chunk format.
#[allow(dead_code)]
fn _keep_alive_marker() -> KeepAlive {
    KeepAlive::default()
}

// `StreamExt` is in scope so async-stream's combinators work; the
// underscore alias suppresses the unused-import lint when the macro
// inlines the stream.
#[allow(dead_code)]
fn _stream_ext_alive(_: impl StreamExt) {}
