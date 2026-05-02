//! Session-id helpers for the OpenAI-compat channel.
//!
//! Fennec's existing `sessions::SessionStore` keys conversations
//! by an internally-generated UUID. The OpenAI-compat channel wants
//! to honor a *client-supplied* `X-Fennec-Session-Id` so external
//! clients can keep track of their own conversations stably across
//! requests.
//!
//! We solve the conflict cheaply: prefix every client-supplied id
//! with `openai_compat:` before storing in `sessions.db`. That way:
//!
//!   - clients can use any id shape they like (uuid, slug, hash)
//!     without colliding with auto-generated session ids from
//!     other channels
//!   - the existing `SessionStore::list_sessions` and search tools
//!     surface OpenAI conversations naturally (channel field set
//!     to `"openai_compat"`)
//!   - cleanup or per-user filtering can match the prefix easily

use std::sync::Arc;

use anyhow::Result;

use crate::providers::traits::ChatMessage;
use crate::sessions::SessionStore;

/// Prefix every OpenAI-compat session id gets in `sessions.db`.
pub const SESSION_ID_PREFIX: &str = "openai_compat:";

/// The channel value rows are tagged with in `sessions.sessions.channel`.
pub const CHANNEL_NAME: &str = "openai_compat";

/// Cap on per-session messages we read into the agent's context.
/// Above this, older history is silently dropped to keep the
/// prompt budget bounded.
pub const MAX_HISTORY_MESSAGES: usize = 200;

/// Convert a client-supplied session id (`"abc-123"`) to the
/// stored key in `sessions.db` (`"openai_compat:abc-123"`).
pub fn to_storage_id(client_id: &str) -> String {
    if client_id.starts_with(SESSION_ID_PREFIX) {
        client_id.to_string()
    } else {
        format!("{}{}", SESSION_ID_PREFIX, client_id)
    }
}

/// Reverse: strip the prefix when surfacing back to the client.
/// Returns the original input unchanged if no prefix is present.
pub fn from_storage_id(storage_id: &str) -> String {
    storage_id
        .strip_prefix(SESSION_ID_PREFIX)
        .unwrap_or(storage_id)
        .to_string()
}

/// Load the conversation history for a client-supplied session id,
/// or an empty list if the session doesn't exist yet.
///
/// Internally maps to `<prefix><client_id>` and looks up
/// `SessionStore::list_messages_for_session`. Caps at
/// [`MAX_HISTORY_MESSAGES`] (most recent kept).
pub async fn load_history(
    store: &Arc<SessionStore>,
    client_session_id: &str,
) -> Result<Vec<ChatMessage>> {
    let storage_id = to_storage_id(client_session_id);
    // Existence check first — `list_messages_for_session` returns
    // an empty Vec for unknown ids, which would also be a valid
    // "first turn" outcome. We don't differentiate.
    let rows = store
        .list_messages_for_session(&storage_id, MAX_HISTORY_MESSAGES)
        .await?;
    let messages: Vec<ChatMessage> = rows
        .into_iter()
        .map(|m| ChatMessage {
            role: m.role,
            content: Some(m.content),
            tool_calls: None,
            tool_call_id: None,
        })
        .collect();
    Ok(messages)
}

/// Ensure a session row exists for the client-supplied id. If the
/// session is brand-new, creates it with `channel = "openai_compat"`.
pub async fn ensure_session(
    store: &Arc<SessionStore>,
    client_session_id: &str,
) -> Result<()> {
    let storage_id = to_storage_id(client_session_id);
    if store.get_session(&storage_id).await?.is_some() {
        return Ok(());
    }
    // SessionStore::create_session generates its own UUID; for
    // OpenAI-compat we want the client-supplied id to BE the
    // storage id. `create_session_with_id` is the explicit-id
    // variant.
    store
        .create_session_with_id(&storage_id, CHANNEL_NAME)
        .await
}

/// Persist the new messages produced by `Agent::turn_with_history`
/// to the session store. Each message becomes a row in
/// `session_messages`.
pub async fn append_messages(
    store: &Arc<SessionStore>,
    client_session_id: &str,
    messages: &[ChatMessage],
) -> Result<()> {
    let storage_id = to_storage_id(client_session_id);
    for msg in messages {
        let content = msg.content.clone().unwrap_or_default();
        if content.is_empty() {
            // Skip tool-call-only messages with no body — they have
            // no useful representation in the plain-text store.
            continue;
        }
        store.add_message(&storage_id, &msg.role, &content).await?;
    }
    Ok(())
}

// `create_session_with_id` lives on `SessionStore` itself so it
// can be called from elsewhere (sessions search, future hub, etc.).

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (Arc<SessionStore>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("sessions.db");
        let store = Arc::new(SessionStore::new(&db).unwrap());
        (store, tmp)
    }

    #[test]
    fn to_storage_id_adds_prefix() {
        assert_eq!(to_storage_id("abc"), "openai_compat:abc");
    }

    #[test]
    fn to_storage_id_idempotent() {
        assert_eq!(
            to_storage_id("openai_compat:already-prefixed"),
            "openai_compat:already-prefixed"
        );
    }

    #[test]
    fn from_storage_id_strips_prefix() {
        assert_eq!(from_storage_id("openai_compat:abc"), "abc");
    }

    #[test]
    fn from_storage_id_passes_through_when_unprefixed() {
        assert_eq!(from_storage_id("abc"), "abc");
    }

    #[tokio::test]
    async fn load_history_empty_for_new_session() {
        let (store, _tmp) = make_store();
        let h = load_history(&store, "fresh-id").await.unwrap();
        assert!(h.is_empty());
    }

    #[tokio::test]
    async fn ensure_session_creates_row() {
        let (store, _tmp) = make_store();
        ensure_session(&store, "client-1").await.unwrap();
        let rec = store
            .get_session("openai_compat:client-1")
            .await
            .unwrap()
            .expect("session row created");
        assert_eq!(rec.channel, CHANNEL_NAME);
    }

    #[tokio::test]
    async fn ensure_session_idempotent() {
        let (store, _tmp) = make_store();
        ensure_session(&store, "client-1").await.unwrap();
        ensure_session(&store, "client-1").await.unwrap();
        // Still one row.
        let sessions = store.list_sessions(10).await.unwrap();
        let hits: Vec<_> = sessions
            .iter()
            .filter(|s| s.id == "openai_compat:client-1")
            .collect();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn append_messages_round_trips() {
        let (store, _tmp) = make_store();
        ensure_session(&store, "c1").await.unwrap();
        let msgs = vec![
            ChatMessage {
                role: "user".into(),
                content: Some("hello".into()),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: Some("hi there".into()),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        append_messages(&store, "c1", &msgs).await.unwrap();
        let loaded = load_history(&store, "c1").await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content.as_deref(), Some("hello"));
        assert_eq!(loaded[1].role, "assistant");
    }

    #[tokio::test]
    async fn append_messages_skips_empty_content() {
        let (store, _tmp) = make_store();
        ensure_session(&store, "c1").await.unwrap();
        let msgs = vec![
            ChatMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![crate::providers::traits::ToolCall {
                    id: "tc1".into(),
                    name: "f".into(),
                    arguments: serde_json::json!({}),
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: Some("final".into()),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        append_messages(&store, "c1", &msgs).await.unwrap();
        let loaded = load_history(&store, "c1").await.unwrap();
        // Only the message with content was stored.
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content.as_deref(), Some("final"));
    }
}
