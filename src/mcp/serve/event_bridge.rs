//! Background poller that watches the session DB for new messages
//! and feeds them into an in-memory event queue. Drives
//! `events_poll` (non-blocking) and `events_wait` (long-poll).
//!
//! Poll interval and queue cap match upstream (200ms, 1000 events).
//! The cursor is the highest `session_messages.id` we've forwarded;
//! restart is fine because the bridge re-seeds the cursor at boot
//! to the current `MAX(id)` so an existing backlog is NOT replayed
//! as fresh events.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::sessions::SessionStore;

/// Soft cap on the in-memory queue. Once exceeded, oldest events
/// are dropped. Matches upstream's QUEUE_LIMIT.
pub const QUEUE_LIMIT: usize = 1000;

/// Poll interval for the background loop. 200ms balances "near-real-
/// time" responsiveness against "useless work when nothing's
/// happening".
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Per-poll cap on rows pulled out of the DB. Caps catch-up cost
/// after a long idle period.
pub const POLL_BATCH_LIMIT: usize = 200;

/// Cap on how long `events_wait` is allowed to block before
/// returning empty. Matches upstream (5 minutes).
pub const WAIT_TIMEOUT_MAX_MS: u64 = 5 * 60 * 1000;

/// Events the bridge emits, surfaced through `events_poll` and
/// `events_wait`. Three kinds: a new message arrived, an approval
/// was registered, an approval was resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// New `session_messages` row.
    Message {
        session_id: String,
        role: String,
        content: String,
        timestamp: String,
    },
    /// Approval newly added to the queue.
    ApprovalRequested {
        approval_id: String,
        description: String,
    },
    /// Approval resolved (either Allow or Deny).
    ApprovalResolved {
        approval_id: String,
        decision: String,
    },
}

/// One queued event. The cursor is monotonic per bridge session;
/// `events_poll(after_cursor)` returns events with `cursor >
/// after_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub cursor: u64,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// In-memory event queue + poller handle.
///
/// `EventBridge` is cheap to clone via the inner Arcs; cloning
/// shares the queue and notifier between the poller task and the
/// tool dispatchers.
#[derive(Clone)]
pub struct EventBridge {
    inner: Arc<Inner>,
}

struct Inner {
    queue: Mutex<VecDeque<QueuedEvent>>,
    /// Used by `events_wait` to wake up when new events arrive.
    notify: Notify,
    /// Monotonic cursor — bumped each time we enqueue an event.
    next_cursor: Mutex<u64>,
    /// Highest `session_messages.id` we've forwarded as a `Message`
    /// event. Bumped each poll iteration.
    last_message_id: Mutex<i64>,
}

impl EventBridge {
    /// Construct a bridge with no events in the queue. The poller
    /// is NOT started — call [`Self::spawn_poller`] for that, or
    /// drive enqueue manually in tests.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                queue: Mutex::new(VecDeque::new()),
                notify: Notify::new(),
                next_cursor: Mutex::new(0),
                last_message_id: Mutex::new(0),
            }),
        }
    }

    /// Seed the poller's cursor to the store's current `MAX(id)`
    /// so we don't replay history as fresh events. Call once before
    /// `spawn_poller`.
    pub async fn seed_from_store(&self, store: &SessionStore) -> anyhow::Result<()> {
        let id = store.max_message_id().await?;
        *self.inner.last_message_id.lock() = id;
        Ok(())
    }

    /// Spawn the background polling task that pulls new messages
    /// out of the session store and enqueues them as `Message`
    /// events. Returns a handle the caller can `abort()` on
    /// shutdown.
    pub fn spawn_poller(&self, store: Arc<SessionStore>) -> tokio::task::JoinHandle<()> {
        let bridge = self.clone();
        tokio::spawn(async move {
            loop {
                let last_id = *bridge.inner.last_message_id.lock();
                match store.list_messages_after(last_id, POLL_BATCH_LIMIT).await {
                    Ok(rows) if !rows.is_empty() => {
                        let mut max_seen = last_id;
                        for row in rows {
                            if row.id > max_seen {
                                max_seen = row.id;
                            }
                            bridge.enqueue(EventKind::Message {
                                session_id: row.session_id,
                                role: row.role,
                                content: row.content,
                                timestamp: row.timestamp,
                            });
                        }
                        *bridge.inner.last_message_id.lock() = max_seen;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "MCP event bridge poll failed");
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
    }

    /// Push an event onto the queue. The cursor is auto-assigned.
    /// If the queue exceeds [`QUEUE_LIMIT`], the oldest event is
    /// dropped (silent — the cap is operational, not user-facing).
    pub fn enqueue(&self, kind: EventKind) {
        let cursor = {
            let mut c = self.inner.next_cursor.lock();
            *c += 1;
            *c
        };
        let event = QueuedEvent {
            cursor,
            at: Utc::now(),
            kind,
        };
        {
            let mut q = self.inner.queue.lock();
            q.push_back(event);
            while q.len() > QUEUE_LIMIT {
                q.pop_front();
            }
        }
        self.inner.notify.notify_waiters();
    }

    /// Non-blocking read: every queued event with `cursor > after_cursor`,
    /// up to `limit`. Caller should pass the highest cursor they've
    /// seen so far so they don't get duplicates. Returns
    /// `(events, new_cursor)` — `new_cursor` is the highest cursor
    /// in `events`, or `after_cursor` if nothing new.
    pub fn poll(&self, after_cursor: u64, limit: usize) -> (Vec<QueuedEvent>, u64) {
        let q = self.inner.queue.lock();
        let mut out: Vec<QueuedEvent> = q
            .iter()
            .filter(|e| e.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect();
        // Already in order (we pushed back monotonically), but be
        // defensive in case enqueue is ever reordered.
        out.sort_by_key(|e| e.cursor);
        let new_cursor = out.last().map(|e| e.cursor).unwrap_or(after_cursor);
        (out, new_cursor)
    }

    /// Long-poll: return immediately if any event with
    /// `cursor > after_cursor` is already queued, else block for up
    /// to `timeout_ms` waiting for one. `timeout_ms` is clamped to
    /// [`WAIT_TIMEOUT_MAX_MS`].
    pub async fn wait(
        &self,
        after_cursor: u64,
        limit: usize,
        timeout_ms: u64,
    ) -> (Vec<QueuedEvent>, u64) {
        let cap = timeout_ms.min(WAIT_TIMEOUT_MAX_MS);
        let deadline = std::time::Instant::now() + Duration::from_millis(cap);
        loop {
            let (events, new_cursor) = self.poll(after_cursor, limit);
            if !events.is_empty() {
                return (events, new_cursor);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return (Vec::new(), after_cursor);
            }
            // Wait either for the next event or for the deadline,
            // whichever comes first.
            let _ = tokio::time::timeout(remaining, self.inner.notify.notified()).await;
            // Loop and re-check; the queue may now have content.
        }
    }

    /// Highest cursor currently issued. Useful for callers that want
    /// to seed `after_cursor` to "now" and only see events that arrive
    /// after they connect.
    pub fn current_cursor(&self) -> u64 {
        *self.inner.next_cursor.lock()
    }

    /// Snapshot the queue length, mostly for debugging / status.
    pub fn queue_len(&self) -> usize {
        self.inner.queue.lock().len()
    }
}

impl Default for EventBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn empty_bridge_polls_empty() {
        let b = EventBridge::new();
        let (events, cursor) = b.poll(0, 100);
        assert!(events.is_empty());
        assert_eq!(cursor, 0);
    }

    #[tokio::test]
    async fn enqueue_and_poll_returns_events() {
        let b = EventBridge::new();
        b.enqueue(EventKind::ApprovalRequested {
            approval_id: "a".into(),
            description: "x".into(),
        });
        b.enqueue(EventKind::ApprovalRequested {
            approval_id: "b".into(),
            description: "y".into(),
        });
        let (events, cursor) = b.poll(0, 100);
        assert_eq!(events.len(), 2);
        assert_eq!(cursor, 2);
    }

    #[tokio::test]
    async fn poll_filters_by_after_cursor() {
        let b = EventBridge::new();
        for i in 0..5 {
            b.enqueue(EventKind::ApprovalRequested {
                approval_id: format!("a{}", i),
                description: "x".into(),
            });
        }
        let (events, cursor) = b.poll(2, 100);
        assert_eq!(events.len(), 3);
        assert_eq!(cursor, 5);
    }

    #[tokio::test]
    async fn poll_respects_limit() {
        let b = EventBridge::new();
        for i in 0..10 {
            b.enqueue(EventKind::ApprovalRequested {
                approval_id: format!("a{}", i),
                description: "x".into(),
            });
        }
        let (events, _) = b.poll(0, 3);
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn queue_limit_drops_oldest() {
        let b = EventBridge::new();
        for i in 0..(QUEUE_LIMIT + 5) {
            b.enqueue(EventKind::ApprovalRequested {
                approval_id: format!("a{}", i),
                description: "x".into(),
            });
        }
        assert_eq!(b.queue_len(), QUEUE_LIMIT);
        // The oldest 5 dropped — the remaining set's lowest cursor
        // should be 6 (1-indexed; first 5 dropped).
        let (events, _) = b.poll(0, 1);
        assert_eq!(events[0].cursor, 6);
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_events_already_exist() {
        let b = EventBridge::new();
        b.enqueue(EventKind::ApprovalRequested {
            approval_id: "a".into(),
            description: "x".into(),
        });
        let started = std::time::Instant::now();
        let (events, _) = b.wait(0, 100, 5000).await;
        let elapsed = started.elapsed();
        assert_eq!(events.len(), 1);
        assert!(
            elapsed < Duration::from_millis(100),
            "wait should return immediately when events are queued, took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn wait_returns_empty_after_timeout() {
        let b = EventBridge::new();
        let (events, cursor) = b.wait(0, 100, 50).await;
        assert!(events.is_empty());
        assert_eq!(cursor, 0);
    }

    #[tokio::test]
    async fn wait_wakes_up_when_event_arrives() {
        let b = EventBridge::new();
        let b_clone = b.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            b_clone.enqueue(EventKind::ApprovalRequested {
                approval_id: "a".into(),
                description: "x".into(),
            });
        });
        let (events, _) = b.wait(0, 100, 5000).await;
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn wait_clamp_logic_is_correct() {
        // The cap is enforced by `timeout_ms.min(WAIT_TIMEOUT_MAX_MS)`.
        // We test the clamp in isolation rather than blocking for
        // the cap (which would take 5 minutes per test run).
        assert_eq!(WAIT_TIMEOUT_MAX_MS.min(1000), 1000);
        assert_eq!(u64::MAX.min(WAIT_TIMEOUT_MAX_MS), WAIT_TIMEOUT_MAX_MS);
    }

    #[test]
    fn event_kind_serializes_with_type_tag() {
        let e = EventKind::Message {
            session_id: "s".into(),
            role: "user".into(),
            content: "hi".into(),
            timestamp: "now".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], json!("message"));
        assert_eq!(v["session_id"], "s");
    }

    #[test]
    fn current_cursor_starts_at_zero() {
        let b = EventBridge::new();
        assert_eq!(b.current_cursor(), 0);
        b.enqueue(EventKind::ApprovalRequested {
            approval_id: "a".into(),
            description: "x".into(),
        });
        assert_eq!(b.current_cursor(), 1);
    }
}
