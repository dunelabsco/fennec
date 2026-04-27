//! Turn-scoped orchestration primitives shared across tools.
//!
//! Several tools need to coordinate around the channel/chat that
//! triggered the current agent turn:
//!
//! - `cron_tool` records the origin so cron-fired results route back to
//!   the channel the schedule was created from.
//! - `ask_user_tool` needs to send a question to the originating chat
//!   AND wait for the next reply *from that chat* without competing with
//!   the gateway's main inbound listener.
//! - `send_message_tool` needs a default destination ("home channel")
//!   when the LLM doesn't supply one, plus a directory of recently-seen
//!   chats so it can resolve friendly references like `target='telegram'`
//!   into a real chat_id.
//!
//! Each of these used to be solved ad-hoc in the tool that needed it,
//! which produced duplication AND, in `ask_user`'s case, an actual bug:
//! it spawned a second `channel.listen()` task that raced the gateway's
//! own poller for `getUpdates` on Telegram. Centralising the
//! orchestration here gives the gateway a single place to update state
//! when an inbound message arrives, and gives the tools a single place
//! to query/await.
//!
//! All three primitives are designed to be cheap to clone (everything
//! is wrapped in `Arc` internally) so the gateway can hand the same
//! handle to every tool that needs it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::events::InboundMessage;

/// Identifies the channel and chat that produced the current turn.
///
/// Replaces the older `CronOrigin` from `cron_tool`. Same shape, just a
/// neutral name now that more than one tool reads it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TurnOrigin {
    pub channel: String,
    pub chat_id: String,
}

/// Shared, mutable handle to the current turn's origin.
///
/// The gateway sets this before invoking the agent for each inbound
/// message; tools read it during their `execute()`. `None` outside of an
/// active turn (e.g. during background CronScheduler-fired turns the
/// CronScheduler sets it from the job's stored origin before running).
pub type TurnOriginHandle = Arc<std::sync::Mutex<Option<TurnOrigin>>>;

/// Construct a fresh, empty origin handle.
pub fn new_turn_origin() -> TurnOriginHandle {
    Arc::new(std::sync::Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Pending replies — `ask_user` and similar tools wait here for the next
// inbound from a specific (channel, chat_id).
// ---------------------------------------------------------------------------

/// Registry of one-shot reply expectations, keyed by `(channel, chat_id)`.
///
/// Workflow:
///
///   1. Tool that wants to wait for the next inbound from a chat calls
///      [`PendingReplies::register`], getting back a [`oneshot::Receiver`].
///   2. Tool sends its prompt (the question) and `await`s the receiver,
///      typically with a timeout.
///   3. When the gateway's inbound dispatch sees a message, it consults
///      [`PendingReplies::take`] *before* forwarding to the agent. If
///      there's a registered waiter for that key, the message is
///      delivered through the oneshot and **not** forwarded to the
///      agent (so a clarification reply doesn't kick off another turn).
///   4. If the tool times out first, it calls
///      [`PendingReplies::cancel`] to drop its sender and let normal
///      inbound flow resume.
///
/// We deliberately use a single in-flight reply per `(channel, chat_id)`
/// rather than a queue: an LLM that asks two questions in the same chat
/// without awaiting the first is a UX bug, not a use case to support.
#[derive(Clone, Default)]
pub struct PendingReplies {
    inner: Arc<Mutex<HashMap<TurnOrigin, oneshot::Sender<InboundMessage>>>>,
}

impl PendingReplies {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a wait. Replaces (and drops) any existing waiter for the
    /// same key — that older waiter sees its receiver close, which it
    /// should treat as "abandoned, re-issue if needed."
    pub fn register(&self, origin: TurnOrigin) -> oneshot::Receiver<InboundMessage> {
        let (tx, rx) = oneshot::channel();
        let mut map = self.inner.lock();
        map.insert(origin, tx);
        rx
    }

    /// Take the registered sender for `origin` and try to deliver `msg`.
    ///
    /// Returns `true` if a waiter was registered and the message was
    /// delivered (caller should NOT also forward to the agent).
    /// Returns `false` if there was no waiter (caller should forward
    /// normally).
    pub fn take_and_deliver(&self, origin: &TurnOrigin, msg: InboundMessage) -> bool {
        let sender = {
            let mut map = self.inner.lock();
            map.remove(origin)
        };
        match sender {
            Some(tx) => {
                // If the receiver was dropped (timeout fired, tool already
                // gave up), the send fails silently — the inbound is then
                // effectively lost from the tool's POV but the gateway has
                // already chosen to deliver here, so we don't double-fire.
                let _ = tx.send(msg);
                true
            }
            None => false,
        }
    }

    /// Drop the waiter for `origin` without delivering anything. Called
    /// by tools whose own timeout fires before the user replies, so the
    /// next legitimate inbound from this chat goes to the agent loop
    /// rather than being silently consumed.
    pub fn cancel(&self, origin: &TurnOrigin) {
        let mut map = self.inner.lock();
        map.remove(origin);
    }

    /// Test helper.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

// ---------------------------------------------------------------------------
// Chat directory — recent (channel, chat_id) the bot has seen, so the
// `send_message` tool can resolve friendly references and validate
// destinations against a known set.
// ---------------------------------------------------------------------------

/// Cap on how many distinct chats we remember per process. When exceeded
/// we evict the oldest entry on insert.
const CHAT_DIRECTORY_CAP: usize = 1024;

/// Directory of recently-seen `(channel, chat_id)` pairs.
///
/// The gateway's inbound dispatch calls [`ChatDirectory::record`] on
/// every inbound message. Tools call [`ChatDirectory::list`] to see
/// what's recent and [`ChatDirectory::contains`] to validate that a
/// candidate destination has been seen at some point.
///
/// We don't use this as a hard allowlist — the LLM can still send to a
/// numeric chat_id we've never seen — but it's the source of truth for
/// the `list` action so the LLM has a real menu to pick from instead of
/// inventing IDs.
#[derive(Clone, Default)]
pub struct ChatDirectory {
    inner: Arc<Mutex<Vec<DirectoryEntry>>>,
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub channel: String,
    pub chat_id: String,
    pub last_seen: Instant,
}

impl ChatDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an inbound message's `(channel, chat_id)`. Updates
    /// `last_seen` if the entry already exists; inserts otherwise.
    pub fn record(&self, channel: &str, chat_id: &str) {
        let mut entries = self.inner.lock();
        let now = Instant::now();
        if let Some(e) = entries
            .iter_mut()
            .find(|e| e.channel == channel && e.chat_id == chat_id)
        {
            e.last_seen = now;
            return;
        }
        if entries.len() >= CHAT_DIRECTORY_CAP {
            // Evict the entry with the smallest last_seen.
            if let Some(idx) = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_seen)
                .map(|(i, _)| i)
            {
                entries.swap_remove(idx);
            }
        }
        entries.push(DirectoryEntry {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            last_seen: now,
        });
    }

    /// Return entries seen in the last `within` duration, most recent
    /// first.
    pub fn list_recent(&self, within: Duration) -> Vec<DirectoryEntry> {
        let cutoff = Instant::now()
            .checked_sub(within)
            .unwrap_or_else(Instant::now);
        let entries = self.inner.lock();
        let mut out: Vec<DirectoryEntry> = entries
            .iter()
            .filter(|e| e.last_seen >= cutoff)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        out
    }

    /// True if `(channel, chat_id)` has ever been recorded (subject to
    /// eviction). Used by `send_message` to gate friendly-name lookups.
    pub fn contains(&self, channel: &str, chat_id: &str) -> bool {
        self.inner
            .lock()
            .iter()
            .any(|e| e.channel == channel && e.chat_id == chat_id)
    }

    /// Return the most-recently-seen `chat_id` on `channel`, if any.
    /// `send_message` falls back to this when no `home_channel` is
    /// configured but at least one inbound has arrived.
    pub fn most_recent_for(&self, channel: &str) -> Option<String> {
        self.inner
            .lock()
            .iter()
            .filter(|e| e.channel == channel)
            .max_by_key(|e| e.last_seen)
            .map(|e| e.chat_id.clone())
    }

    /// Test helper.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn dummy_msg(channel: &str, chat_id: &str) -> InboundMessage {
        InboundMessage {
            id: "id".to_string(),
            sender: "u".to_string(),
            content: "hi".to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            timestamp: 0,
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn pending_reply_delivers_then_clears() {
        let r = PendingReplies::new();
        let origin = TurnOrigin {
            channel: "telegram".into(),
            chat_id: "42".into(),
        };
        let rx = r.register(origin.clone());
        assert_eq!(r.len(), 1);

        let delivered = r.take_and_deliver(&origin, dummy_msg("telegram", "42"));
        assert!(delivered);
        let got = rx.await.unwrap();
        assert_eq!(got.chat_id, "42");
        assert_eq!(r.len(), 0, "delivery must remove the registration");
    }

    #[tokio::test]
    async fn pending_reply_take_returns_false_when_no_waiter() {
        let r = PendingReplies::new();
        let origin = TurnOrigin {
            channel: "x".into(),
            chat_id: "y".into(),
        };
        let delivered = r.take_and_deliver(&origin, dummy_msg("x", "y"));
        assert!(!delivered);
    }

    #[tokio::test]
    async fn pending_reply_cancel_drops_waiter() {
        let r = PendingReplies::new();
        let origin = TurnOrigin {
            channel: "x".into(),
            chat_id: "y".into(),
        };
        let rx = r.register(origin.clone());
        r.cancel(&origin);
        // After cancel, the registration is gone.
        assert_eq!(r.len(), 0);
        // The receiver sees a closed channel (sender dropped).
        assert!(rx.await.is_err());
    }

    #[tokio::test]
    async fn pending_reply_register_replaces_existing_waiter() {
        let r = PendingReplies::new();
        let origin = TurnOrigin {
            channel: "x".into(),
            chat_id: "y".into(),
        };
        let rx1 = r.register(origin.clone());
        let _rx2 = r.register(origin.clone());
        assert!(rx1.await.is_err(), "old waiter should see closed channel");
    }

    #[test]
    fn directory_records_and_lists() {
        let d = ChatDirectory::new();
        d.record("telegram", "1");
        d.record("telegram", "2");
        d.record("discord", "abc");
        assert_eq!(d.len(), 3);
        assert!(d.contains("telegram", "1"));
        assert!(!d.contains("telegram", "3"));
    }

    #[test]
    fn directory_record_updates_existing_last_seen() {
        let d = ChatDirectory::new();
        d.record("telegram", "1");
        let len_before = d.len();
        d.record("telegram", "1");
        assert_eq!(d.len(), len_before);
    }

    #[test]
    fn directory_most_recent_for_picks_latest() {
        let d = ChatDirectory::new();
        d.record("telegram", "1");
        std::thread::sleep(Duration::from_millis(2));
        d.record("telegram", "2");
        std::thread::sleep(Duration::from_millis(2));
        d.record("telegram", "3");
        assert_eq!(d.most_recent_for("telegram"), Some("3".to_string()));
        assert_eq!(d.most_recent_for("discord"), None);
    }

    #[test]
    fn directory_evicts_oldest_at_cap() {
        let d = ChatDirectory::new();
        // Insert just past cap to trigger one eviction.
        for i in 0..(CHAT_DIRECTORY_CAP + 1) {
            d.record("telegram", &format!("{i}"));
        }
        assert_eq!(d.len(), CHAT_DIRECTORY_CAP);
        // Entry "0" was the oldest and must be gone.
        assert!(!d.contains("telegram", "0"));
        // The newest "CHAT_DIRECTORY_CAP" must still be there.
        assert!(d.contains("telegram", &CHAT_DIRECTORY_CAP.to_string()));
    }
}
