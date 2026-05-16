//! Fennec as an MCP server.
//!
//! Exposes Fennec's messaging surface — read conversation history,
//! list channels, send messages, watch for new events — over a
//! stdio MCP server. The transport is line-delimited JSON-RPC; an
//! MCP client (Claude Desktop, Cursor, Codex) spawns `fennec mcp
//! serve` as a subprocess and pipes stdin/stdout.
//!
//! Tool surface (10 tools, matching the upstream messaging-bridge
//! design):
//!
//!   - `conversations_list`     — list active sessions, optionally filtered
//!   - `conversation_get`       — metadata for one session
//!   - `messages_read`          — read message history with limit
//!   - `attachments_fetch`      — extract non-text attachments from a message
//!   - `events_poll`            — non-blocking poll for new events since cursor
//!   - `events_wait`            — long-poll for next event
//!   - `messages_send`          — send a message through a channel
//!   - `channels_list`          — enumerate available channel destinations
//!   - `permissions_list_open`  — pending approvals seen this bridge session
//!   - `permissions_respond`    — approve/deny a pending approval
//!
//! Skills are deliberately NOT exposed. This is a messaging
//! bridge, not a general-purpose tool surface.

pub mod event_bridge;
pub mod state;
pub mod tools;
pub mod transport;

pub use event_bridge::{
    EventBridge, EventKind, POLL_INTERVAL, QUEUE_LIMIT, QueuedEvent, WAIT_TIMEOUT_MAX_MS,
};
pub use state::{ApprovalDecision, PendingApproval, ServerState};
pub use tools::{McpServerHandler, tool_catalog};
pub use transport::{Handler, error_codes, run_stdio};

#[cfg(test)]
pub use transport::run_sync;
