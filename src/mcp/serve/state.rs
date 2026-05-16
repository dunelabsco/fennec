//! Shared in-memory state held by the MCP server for one stdio
//! session. Owns the approval queue, the event-bridge handle, and
//! references to the underlying Fennec subsystems the tools call
//! into (session store, channel manager, send-message handle).
//!
//! State lives only as long as the stdio process — when the MCP
//! client kills us, everything goes. That's the design: pending
//! approvals from before this server started are explicitly NOT
//! reachable through `permissions_list_open`, mirroring upstream.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::channels::ChannelManager;
use crate::sessions::SessionStore;

/// One pending approval, surfaced through `permissions_list_open`
/// and resolved through `permissions_respond`. Created when a tool
/// the server gates needs operator confirmation.
///
/// We use a UUID-style id so the MCP client doesn't have to carry
/// raw command lines around when responding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub id: String,
    /// What's being asked for (e.g. "send a message to telegram:42").
    pub description: String,
    /// When the approval was registered. Surfaced so the MCP client
    /// can sort by recency.
    pub created_at: DateTime<Utc>,
    /// Free-form metadata the requesting tool wants back when the
    /// approval resolves (e.g. the destination + body for a deferred
    /// `messages_send`).
    pub context: serde_json::Value,
}

/// Result of `permissions_respond`. Whichever tool registered the
/// approval pulls the resolution off the queue and acts on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

/// Bridge-session state. Cheap to clone the `Arc`; cheap to lock
/// the mutex (per-call, not held across awaits).
#[derive(Clone)]
pub struct ServerState {
    /// Configurable home directory (`~/.fennec` by default). The
    /// server uses this to find sub-resources (the session DB,
    /// future attachments cache, etc.).
    pub home_dir: PathBuf,
    /// Session store handle — read-only for the MCP server (we
    /// never call `add_message` from here; that's the gateway's
    /// job).
    pub sessions: Arc<SessionStore>,
    /// Channel manager — used by `messages_send` and
    /// `channels_list`. Optional because the MCP server can run
    /// in read-only mode without a live gateway (read history /
    /// list conversations) when no channels are configured.
    pub channels: Option<Arc<ChannelManager>>,
    /// Approval queue: id → pending request. Ordered insertion is
    /// achieved by sorting on `created_at` at list time, so the
    /// `HashMap` is fine.
    pub approvals: Arc<Mutex<HashMap<String, PendingApproval>>>,
}

impl ServerState {
    /// Construct with no channel manager (read-only mode). Send
    /// operations through `messages_send` will return a clear
    /// error in that mode.
    pub fn new(home_dir: PathBuf, sessions: Arc<SessionStore>) -> Self {
        Self {
            home_dir,
            sessions,
            channels: None,
            approvals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach a channel manager so `messages_send` can route. Used
    /// by `run_mcp_server` when the operator's config has at least
    /// one enabled channel.
    pub fn with_channels(mut self, channels: Arc<ChannelManager>) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Register a pending approval. Returns the assigned id.
    pub fn register_approval(
        &self,
        description: impl Into<String>,
        context: serde_json::Value,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let pending = PendingApproval {
            id: id.clone(),
            description: description.into(),
            created_at: Utc::now(),
            context,
        };
        self.approvals.lock().insert(id.clone(), pending);
        id
    }

    /// Snapshot the open approvals, sorted oldest-first.
    pub fn list_open_approvals(&self) -> Vec<PendingApproval> {
        let inner = self.approvals.lock();
        let mut v: Vec<PendingApproval> = inner.values().cloned().collect();
        v.sort_by_key(|a| a.created_at);
        v
    }

    /// Pull an approval off the queue and return it, or `None` if
    /// no approval with that id is open. The caller (the tool that
    /// registered it) decides what to do with the decision.
    pub fn resolve_approval(&self, id: &str) -> Option<PendingApproval> {
        self.approvals.lock().remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn fresh_state() -> (TempDir, ServerState) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("sessions.db");
        let store = Arc::new(SessionStore::new(&db).unwrap());
        let state = ServerState::new(tmp.path().to_path_buf(), store);
        (tmp, state)
    }

    #[tokio::test]
    async fn register_and_list_approval() {
        let (_tmp, state) = fresh_state().await;
        let id = state.register_approval("send to telegram:42", json!({"to": "telegram:42"}));
        let open = state.list_open_approvals();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, id);
        assert_eq!(open[0].description, "send to telegram:42");
    }

    #[tokio::test]
    async fn approvals_sorted_oldest_first() {
        let (_tmp, state) = fresh_state().await;
        state.register_approval("a", json!({}));
        // Force timestamp difference (the test doesn't need real
        // time-travel, just enough resolution that the sort sees a
        // gap; nanosecond Utc::now() resolution is plenty).
        std::thread::sleep(std::time::Duration::from_millis(2));
        state.register_approval("b", json!({}));
        let open = state.list_open_approvals();
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].description, "a");
        assert_eq!(open[1].description, "b");
    }

    #[tokio::test]
    async fn resolve_removes_approval() {
        let (_tmp, state) = fresh_state().await;
        let id = state.register_approval("x", json!({}));
        let pulled = state.resolve_approval(&id);
        assert!(pulled.is_some());
        assert!(state.list_open_approvals().is_empty());
        // Second resolve is a no-op.
        assert!(state.resolve_approval(&id).is_none());
    }

    #[tokio::test]
    async fn read_only_mode_has_no_channels() {
        let (_tmp, state) = fresh_state().await;
        assert!(state.channels.is_none());
    }
}
