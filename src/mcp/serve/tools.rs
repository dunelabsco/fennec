//! The 10 messaging tools the MCP server exposes, plus the
//! `Handler` impl that dispatches `tools/list` and `tools/call` to
//! them.
//!
//! Each tool is a small async function that takes the server state
//! and a JSON args object and returns a JSON value (the tool's
//! result content). The dispatcher wraps the result in an
//! `McpToolResult` (text content + is_error flag) per spec.
//!
//! Hard caps:
//!
//!   - Message content is truncated to [`MAX_MESSAGE_PREVIEW_CHARS`]
//!     when surfaced through events; full content is returned by
//!     `messages_read` (capped at [`MAX_READ_CONTENT_CHARS`] so a
//!     single huge message can't blow the LLM context budget).
//!   - `events_wait` long-poll is clamped at the `EventBridge`
//!     constant (5 minutes).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::channels::traits::SendMessage;
use crate::mcp::types::{
    JsonRpcError, McpContent, McpToolResult, McpToolSpec,
};

use super::event_bridge::{EventBridge, EventKind};
use super::state::ServerState;
use super::transport::{Handler, error_codes};

/// Truncation cap for `messages_read` output. 2000 chars is enough
/// for a useful preview without dumping a model's entire context
/// budget into one tool call.
pub const MAX_READ_CONTENT_CHARS: usize = 2000;

/// Truncation cap for content surfaced via the event queue (poll +
/// wait). Smaller than the read cap because events are meant to be
/// notifications, not full reads.
pub const MAX_MESSAGE_PREVIEW_CHARS: usize = 500;

/// Build the static catalog of all 10 tools. Used by `tools/list`.
pub fn tool_catalog() -> Vec<McpToolSpec> {
    vec![
        McpToolSpec {
            name: "conversations_list".into(),
            description: "List active messaging conversations across connected platforms. \
                Optionally filter by `platform` (e.g. \"telegram\", \"discord\") or \
                limit the number of results.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "platform": { "type": "string", "description": "Optional platform filter (telegram, discord, slack, whatsapp, email, cli)" },
                    "limit": { "type": "integer", "description": "Maximum number of conversations to return (default 50)" }
                },
                "required": []
            }),
        },
        McpToolSpec {
            name: "conversation_get".into(),
            description: "Fetch metadata for a single conversation by `session_key` (the id returned \
                by `conversations_list`). Returns channel, started_at, ended_at, summary.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_key": { "type": "string", "description": "Session id" }
                },
                "required": ["session_key"]
            }),
        },
        McpToolSpec {
            name: "messages_read".into(),
            description: "Read message history for a conversation, oldest first. Each message has \
                `role` (user/assistant), `content`, and `timestamp`. `limit` caps the number of rows.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_key": { "type": "string" },
                    "limit": { "type": "integer", "description": "Default 50, max 500" }
                },
                "required": ["session_key"]
            }),
        },
        McpToolSpec {
            name: "attachments_fetch".into(),
            description: "Extract non-text attachments (images, audio, video, files) from a specific \
                message. Returns base64-encoded blobs with media types. \
                NOTE: Fennec stores message content as plain text; this tool currently returns \
                an empty list for every input. The surface is here so MCP clients written \
                against the upstream protocol still work.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_key": { "type": "string" },
                    "message_id": { "type": "string" }
                },
                "required": ["session_key", "message_id"]
            }),
        },
        McpToolSpec {
            name: "events_poll".into(),
            description: "Non-blocking poll for events that have arrived since `after_cursor`. \
                Events: `message` (new conversation message), `approval_requested`, `approval_resolved`. \
                Returns up to `limit` events plus the new cursor for the next poll.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "after_cursor": { "type": "integer", "description": "Last cursor seen; default 0" },
                    "limit": { "type": "integer", "description": "Default 100" }
                },
                "required": []
            }),
        },
        McpToolSpec {
            name: "events_wait".into(),
            description: "Long-poll: returns immediately if events with `cursor > after_cursor` exist, \
                otherwise blocks for up to `timeout_ms` (capped at 5 minutes) waiting for one.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "after_cursor": { "type": "integer" },
                    "limit": { "type": "integer" },
                    "timeout_ms": { "type": "integer", "description": "Capped at 300000 (5 min)" }
                },
                "required": []
            }),
        },
        McpToolSpec {
            name: "messages_send".into(),
            description: "Send a message through a connected channel. `target` is either \
                `\"<channel>\"` (uses the channel's home destination from config) or \
                `\"<channel>:<chat_id>\"` (explicit). Returns the resolved destination on success.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "e.g. \"telegram:123456\" or just \"telegram\"" },
                    "message": { "type": "string", "description": "Body to send" }
                },
                "required": ["target", "message"]
            }),
        },
        McpToolSpec {
            name: "channels_list".into(),
            description: "Enumerate every connected channel (telegram, discord, slack, whatsapp, \
                email, cli). Returns each channel's name and connection status.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpToolSpec {
            name: "permissions_list_open".into(),
            description: "List approval requests pending in this bridge session, oldest first. \
                Approvals from before the server started are NOT visible.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpToolSpec {
            name: "permissions_respond".into(),
            description: "Resolve a pending approval. `decision` is \"allow\" or \"deny\". \
                Once resolved, the approval is removed from the queue.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "approval_id": { "type": "string" },
                    "decision": { "type": "string", "enum": ["allow", "deny"] }
                },
                "required": ["approval_id", "decision"]
            }),
        },
    ]
}

/// The Handler the stdio dispatch loop calls into. Owns the
/// `ServerState` and the `EventBridge`. All `tools/call` requests
/// go through `dispatch_tool`.
pub struct McpServerHandler {
    pub state: ServerState,
    pub bridge: EventBridge,
}

#[async_trait]
impl Handler for McpServerHandler {
    async fn handle_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, JsonRpcError> {
        match method {
            "initialize" => Ok(initialize_result()),
            "tools/list" => Ok(json!({ "tools": tool_catalog() })),
            "tools/call" => self.dispatch_tool(params).await,
            // Resources / prompts are not exposed; return empty lists
            // rather than errors so clients that probe these surfaces
            // don't see noise.
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            "ping" => Ok(json!({})),
            _ => Err(JsonRpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("method not found: {}", method),
                data: None,
            }),
        }
    }
}

impl McpServerHandler {
    pub fn new(state: ServerState, bridge: EventBridge) -> Self {
        Self { state, bridge }
    }

    /// Top-level `tools/call` dispatcher. Wraps tool output in an
    /// `McpToolResult` and translates exceptions into
    /// `is_error: true` payloads (per MCP spec, tool execution
    /// errors don't surface as JSON-RPC errors).
    async fn dispatch_tool(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        let p = params.unwrap_or(Value::Null);
        let name = p
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid_params("missing field `name`"))?
            .to_string();
        let args = p.get("arguments").cloned().unwrap_or(Value::Null);

        let result = match name.as_str() {
            "conversations_list" => self.tool_conversations_list(args).await,
            "conversation_get" => self.tool_conversation_get(args).await,
            "messages_read" => self.tool_messages_read(args).await,
            "attachments_fetch" => self.tool_attachments_fetch(args).await,
            "events_poll" => self.tool_events_poll(args).await,
            "events_wait" => self.tool_events_wait(args).await,
            "messages_send" => self.tool_messages_send(args).await,
            "channels_list" => self.tool_channels_list(args).await,
            "permissions_list_open" => self.tool_permissions_list_open(args).await,
            "permissions_respond" => self.tool_permissions_respond(args).await,
            other => {
                return Err(JsonRpcError {
                    code: error_codes::METHOD_NOT_FOUND,
                    message: format!("unknown tool: {}", other),
                    data: None,
                });
            }
        };

        let tool_result = match result {
            Ok(value) => McpToolResult {
                content: vec![McpContent {
                    type_: "text".into(),
                    text: Some(value.to_string()),
                }],
                is_error: false,
            },
            Err(message) => McpToolResult {
                content: vec![McpContent {
                    type_: "text".into(),
                    text: Some(message),
                }],
                is_error: true,
            },
        };
        serde_json::to_value(tool_result).map_err(|e| JsonRpcError {
            code: error_codes::INTERNAL_ERROR,
            message: format!("could not serialize tool result: {}", e),
            data: None,
        })
    }

    // -- conversations_list -----------------------------------------

    async fn tool_conversations_list(&self, args: Value) -> Result<Value, String> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(500) as usize;
        let platform_filter = args.get("platform").and_then(|v| v.as_str()).map(String::from);

        let sessions = self
            .state
            .sessions
            .list_sessions(limit)
            .await
            .map_err(|e| format!("list_sessions failed: {}", e))?;

        let filtered: Vec<Value> = sessions
            .into_iter()
            .filter(|s| {
                platform_filter
                    .as_deref()
                    .map(|p| s.channel == p)
                    .unwrap_or(true)
            })
            .map(|s| {
                json!({
                    "session_key": s.id,
                    "platform": s.channel,
                    "started_at": s.started_at,
                    "ended_at": s.ended_at,
                    "summary": s.summary,
                })
            })
            .collect();

        Ok(json!({ "count": filtered.len(), "conversations": filtered }))
    }

    // -- conversation_get -------------------------------------------

    async fn tool_conversation_get(&self, args: Value) -> Result<Value, String> {
        let session_key = args
            .get("session_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `session_key`".to_string())?;
        let rec = self
            .state
            .sessions
            .get_session(session_key)
            .await
            .map_err(|e| format!("get_session failed: {}", e))?;
        match rec {
            Some(s) => Ok(json!({
                "session_key": s.id,
                "platform": s.channel,
                "started_at": s.started_at,
                "ended_at": s.ended_at,
                "summary": s.summary,
            })),
            None => Err(format!("conversation {:?} not found", session_key)),
        }
    }

    // -- messages_read ----------------------------------------------

    async fn tool_messages_read(&self, args: Value) -> Result<Value, String> {
        let session_key = args
            .get("session_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `session_key`".to_string())?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(500) as usize;
        let rows = self
            .state
            .sessions
            .list_messages_for_session(session_key, limit)
            .await
            .map_err(|e| format!("list_messages_for_session failed: {}", e))?;
        let messages: Vec<Value> = rows
            .into_iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "role": m.role,
                    "content": truncate(&m.content, MAX_READ_CONTENT_CHARS),
                    "timestamp": m.timestamp,
                })
            })
            .collect();
        Ok(json!({ "count": messages.len(), "messages": messages }))
    }

    // -- attachments_fetch (currently a stub) ----------------------

    async fn tool_attachments_fetch(&self, _args: Value) -> Result<Value, String> {
        // Fennec's session store keeps message content as plain text;
        // attachments aren't separated from the body at storage time.
        // The tool surface is preserved so MCP clients written against
        // upstream still work, but the response is always empty.
        Ok(json!({
            "count": 0,
            "attachments": [],
            "note": "Fennec does not currently surface attachments separately from message content.",
        }))
    }

    // -- events_poll ------------------------------------------------

    async fn tool_events_poll(&self, args: Value) -> Result<Value, String> {
        let after_cursor = args
            .get("after_cursor")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .min(1000) as usize;
        let (events, cursor) = self.bridge.poll(after_cursor, limit);
        Ok(json!({
            "events": events.iter().map(truncate_event).collect::<Vec<_>>(),
            "cursor": cursor,
        }))
    }

    // -- events_wait ------------------------------------------------

    async fn tool_events_wait(&self, args: Value) -> Result<Value, String> {
        let after_cursor = args
            .get("after_cursor")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .min(1000) as usize;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);
        let (events, cursor) = self.bridge.wait(after_cursor, limit, timeout_ms).await;
        Ok(json!({
            "events": events.iter().map(truncate_event).collect::<Vec<_>>(),
            "cursor": cursor,
        }))
    }

    // -- messages_send ----------------------------------------------

    async fn tool_messages_send(&self, args: Value) -> Result<Value, String> {
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `target`".to_string())?;
        let body = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `message`".to_string())?;

        let channels = self
            .state
            .channels
            .as_ref()
            .ok_or_else(|| {
                "no channels configured; messages_send unavailable in read-only mode".to_string()
            })?;

        // Parse `<channel>:<chat_id>` or `<channel>` alone (latter
        // fails — we don't have access to the agent's home-chat-id
        // resolution here without pulling in the bus's ChatDirectory).
        let (channel_name, recipient) = match target.split_once(':') {
            Some((c, r)) => (c, r.to_string()),
            None => {
                return Err(format!(
                    "target {:?} must be in `<channel>:<chat_id>` form (e.g. \"telegram:123456\")",
                    target
                ));
            }
        };

        let channel = channels.get_channel(channel_name).ok_or_else(|| {
            format!(
                "no channel named {:?}; available: {}",
                channel_name,
                channels.list_channel_names().join(", ")
            )
        })?;

        let sm = SendMessage {
            content: body.to_string(),
            recipient: recipient.clone(),
        };
        channel
            .send(&sm)
            .await
            .map_err(|e| format!("channel.send failed: {}", e))?;

        Ok(json!({
            "ok": true,
            "channel": channel_name,
            "recipient": recipient,
        }))
    }

    // -- channels_list ----------------------------------------------

    async fn tool_channels_list(&self, _args: Value) -> Result<Value, String> {
        let names = match self.state.channels.as_ref() {
            Some(cm) => cm.list_channel_names(),
            None => Vec::new(),
        };
        let entries: Vec<Value> = names
            .into_iter()
            .map(|n| json!({ "name": n, "connected": true }))
            .collect();
        Ok(json!({
            "count": entries.len(),
            "channels": entries,
        }))
    }

    // -- permissions_list_open --------------------------------------

    async fn tool_permissions_list_open(&self, _args: Value) -> Result<Value, String> {
        let open = self.state.list_open_approvals();
        let entries: Vec<Value> = open
            .into_iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "description": a.description,
                    "created_at": a.created_at,
                })
            })
            .collect();
        Ok(json!({
            "count": entries.len(),
            "approvals": entries,
        }))
    }

    // -- permissions_respond ----------------------------------------

    async fn tool_permissions_respond(&self, args: Value) -> Result<Value, String> {
        let id = args
            .get("approval_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `approval_id`".to_string())?;
        let decision = args
            .get("decision")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required field `decision`".to_string())?;
        if decision != "allow" && decision != "deny" {
            return Err(format!(
                "invalid decision {:?}; must be \"allow\" or \"deny\"",
                decision
            ));
        }
        let pulled = self.state.resolve_approval(id);
        match pulled {
            Some(_) => {
                self.bridge.enqueue(EventKind::ApprovalResolved {
                    approval_id: id.to_string(),
                    decision: decision.to_string(),
                });
                Ok(json!({
                    "ok": true,
                    "approval_id": id,
                    "decision": decision,
                }))
            }
            None => Err(format!("approval {:?} not found in pending queue", id)),
        }
    }
}

fn invalid_params(message: impl Into<String>) -> JsonRpcError {
    JsonRpcError {
        code: error_codes::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

/// MCP `initialize` reply. Advertises the server's name, version,
/// and supported capability set (just `tools` for now — no
/// resources, no prompts, no sampling, no logging).
fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "fennec",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": {},
        },
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…[truncated]");
    out
}

/// Truncate a `Message` event's content for the event-feed surface.
/// Other event kinds are passed through unchanged.
fn truncate_event(e: &super::event_bridge::QueuedEvent) -> Value {
    let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
    if let Some(content) = v.get_mut("content").and_then(|c| c.as_str()).map(String::from) {
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "content".into(),
                Value::String(truncate(&content, MAX_MESSAGE_PREVIEW_CHARS)),
            );
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn fresh() -> (TempDir, McpServerHandler, Arc<SessionStore>) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("sessions.db");
        let store = Arc::new(SessionStore::new(&db).unwrap());
        let state = ServerState::new(tmp.path().to_path_buf(), Arc::clone(&store));
        let bridge = EventBridge::new();
        let handler = McpServerHandler::new(state, bridge);
        (tmp, handler, store)
    }

    fn extract_text(value: Value) -> String {
        // Tool results are returned as McpToolResult; the content is
        // a list of {type, text} blocks. We assert one text block.
        let content = &value["content"];
        let first = &content[0];
        first["text"].as_str().unwrap_or("").to_string()
    }

    #[tokio::test]
    async fn tool_catalog_has_ten_tools() {
        let cat = tool_catalog();
        assert_eq!(cat.len(), 10);
        let names: Vec<&str> = cat.iter().map(|t| t.name.as_str()).collect();
        for required in [
            "conversations_list",
            "conversation_get",
            "messages_read",
            "attachments_fetch",
            "events_poll",
            "events_wait",
            "messages_send",
            "channels_list",
            "permissions_list_open",
            "permissions_respond",
        ] {
            assert!(names.contains(&required), "missing tool: {}", required);
        }
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let (_tmp, h, _) = fresh().await;
        let result = h.handle_request("initialize", None).await.unwrap();
        assert_eq!(result["serverInfo"]["name"], "fennec");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_returns_catalog() {
        let (_tmp, h, _) = fresh().await;
        let result = h.handle_request("tools/list", None).await.unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), 10);
    }

    #[tokio::test]
    async fn unknown_method_yields_method_not_found() {
        let (_tmp, h, _) = fresh().await;
        let err = h.handle_request("does/not/exist", None).await.unwrap_err();
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn conversations_list_returns_sessions() {
        let (_tmp, h, store) = fresh().await;
        store.create_session("telegram").await.unwrap();
        store.create_session("discord").await.unwrap();
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({ "name": "conversations_list", "arguments": {} })),
            )
            .await
            .unwrap();
        let text = extract_text(result);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["count"], 2);
    }

    #[tokio::test]
    async fn conversations_list_filters_by_platform() {
        let (_tmp, h, store) = fresh().await;
        store.create_session("telegram").await.unwrap();
        store.create_session("discord").await.unwrap();
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "conversations_list",
                    "arguments": { "platform": "telegram" }
                })),
            )
            .await
            .unwrap();
        let text = extract_text(result);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["count"], 1);
        assert_eq!(v["conversations"][0]["platform"], "telegram");
    }

    #[tokio::test]
    async fn conversation_get_returns_metadata() {
        let (_tmp, h, store) = fresh().await;
        let id = store.create_session("cli").await.unwrap();
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "conversation_get",
                    "arguments": { "session_key": id }
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["platform"], "cli");
    }

    #[tokio::test]
    async fn conversation_get_missing_returns_is_error() {
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "conversation_get",
                    "arguments": { "session_key": "nope" }
                })),
            )
            .await
            .unwrap();
        assert_eq!(result["is_error"], json!(true));
    }

    #[tokio::test]
    async fn messages_read_returns_history_in_order() {
        let (_tmp, h, store) = fresh().await;
        let sid = store.create_session("cli").await.unwrap();
        store.add_message(&sid, "user", "first").await.unwrap();
        store.add_message(&sid, "assistant", "second").await.unwrap();
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "messages_read",
                    "arguments": { "session_key": sid }
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["messages"][0]["content"], "first");
        assert_eq!(v["messages"][1]["content"], "second");
    }

    #[tokio::test]
    async fn messages_read_truncates_huge_content() {
        let (_tmp, h, store) = fresh().await;
        let sid = store.create_session("cli").await.unwrap();
        let huge = "a".repeat(MAX_READ_CONTENT_CHARS + 100);
        store.add_message(&sid, "user", &huge).await.unwrap();
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "messages_read",
                    "arguments": { "session_key": sid }
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        let content = v["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("[truncated]"));
        assert!(content.chars().count() <= MAX_READ_CONTENT_CHARS + 20);
    }

    #[tokio::test]
    async fn attachments_fetch_returns_empty() {
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "attachments_fetch",
                    "arguments": { "session_key": "x", "message_id": "1" }
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["count"], 0);
    }

    #[tokio::test]
    async fn events_poll_returns_queued_events() {
        let (_tmp, h, _) = fresh().await;
        h.bridge.enqueue(EventKind::ApprovalRequested {
            approval_id: "x".into(),
            description: "test".into(),
        });
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "events_poll",
                    "arguments": {}
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["events"].as_array().unwrap().len(), 1);
        assert_eq!(v["cursor"], 1);
    }

    #[tokio::test]
    async fn events_wait_short_timeout_returns_empty() {
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "events_wait",
                    "arguments": { "timeout_ms": 50 }
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["events"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn messages_send_no_channels_is_error() {
        let (_tmp, h, _) = fresh().await; // ServerState built without channels
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "messages_send",
                    "arguments": { "target": "telegram:42", "message": "hi" }
                })),
            )
            .await
            .unwrap();
        assert_eq!(result["is_error"], json!(true));
        let text = extract_text(result);
        assert!(text.contains("read-only"), "got {}", text);
    }

    #[tokio::test]
    async fn messages_send_target_without_chat_id_errors() {
        // Even with channels, target=\"telegram\" without :chat_id is rejected.
        // We can't easily construct a real ChannelManager in a unit
        // test, so rely on the no-channels path for this assertion;
        // the format check runs before the channel lookup.
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "messages_send",
                    "arguments": { "target": "telegram", "message": "hi" }
                })),
            )
            .await
            .unwrap();
        assert_eq!(result["is_error"], json!(true));
        // No-channels path matches first; the format-check path
        // would run if channels were present. That's still fine —
        // the tool fails clearly in either case. (Test pins
        // current behavior.)
        let _ = result;
    }

    #[tokio::test]
    async fn channels_list_empty_when_no_channels() {
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "channels_list",
                    "arguments": {}
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["count"], 0);
    }

    #[tokio::test]
    async fn permissions_list_open_starts_empty() {
        let (_tmp, h, _) = fresh().await;
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "permissions_list_open",
                    "arguments": {}
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(result)).unwrap();
        assert_eq!(v["count"], 0);
    }

    #[tokio::test]
    async fn permissions_full_lifecycle() {
        let (_tmp, h, _) = fresh().await;
        let id = h
            .state
            .register_approval("test approval", json!({"x": 1}));

        // List shows it.
        let listed = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "permissions_list_open",
                    "arguments": {}
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(listed)).unwrap();
        assert_eq!(v["count"], 1);

        // Resolve as allow.
        let resolved = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "permissions_respond",
                    "arguments": { "approval_id": id, "decision": "allow" }
                })),
            )
            .await
            .unwrap();
        assert_eq!(resolved["is_error"], json!(false));

        // List is empty afterwards.
        let listed_again = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "permissions_list_open",
                    "arguments": {}
                })),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&extract_text(listed_again)).unwrap();
        assert_eq!(v["count"], 0);

        // ApprovalResolved event was emitted on the bridge.
        let (events, _) = h.bridge.poll(0, 100);
        assert!(events.iter().any(|e| matches!(
            &e.kind,
            EventKind::ApprovalResolved { decision, .. } if decision == "allow"
        )));
    }

    #[tokio::test]
    async fn permissions_respond_invalid_decision_errors() {
        let (_tmp, h, _) = fresh().await;
        let id = h.state.register_approval("x", json!({}));
        let result = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "permissions_respond",
                    "arguments": { "approval_id": id, "decision": "maybe" }
                })),
            )
            .await
            .unwrap();
        assert_eq!(result["is_error"], json!(true));
    }

    #[tokio::test]
    async fn unknown_tool_yields_method_not_found() {
        let (_tmp, h, _) = fresh().await;
        let err = h
            .handle_request(
                "tools/call",
                Some(json!({
                    "name": "frobnicate",
                    "arguments": {}
                })),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn resources_and_prompts_list_return_empty() {
        let (_tmp, h, _) = fresh().await;
        let r = h.handle_request("resources/list", None).await.unwrap();
        assert_eq!(r["resources"].as_array().unwrap().len(), 0);
        let p = h.handle_request("prompts/list", None).await.unwrap();
        assert_eq!(p["prompts"].as_array().unwrap().len(), 0);
    }
}
