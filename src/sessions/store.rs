use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::Connection;

/// A search hit from FTS5 across session messages.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub score: f64,
}

/// One message read from a session, in time order. Returned by
/// [`SessionStore::list_messages_for_session`] and
/// [`SessionStore::list_messages_after`] (the MCP server's
/// `messages_read` and event-bridge polling both consume this).
#[derive(Debug, Clone)]
pub struct MessageRow {
    /// Auto-increment primary key. Used by the MCP server's event
    /// bridge as a polling cursor — ask for messages with id > the
    /// last-seen cursor and forward them as `message` events.
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Metadata for a session.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub channel: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

/// SQLite-backed session store with FTS5 search.
///
/// All read/write methods are `async` and internally delegate to
/// `tokio::task::spawn_blocking` so that holding the connection mutex
/// during a synchronous SQLite operation does not block a tokio worker
/// thread. The `&self` methods are therefore safe to call from any
/// async context (agent loop, Telegram handler, etc.).
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SessionStore {
    /// Open (or create) the session database at `db_path` and initialise the
    /// schema including FTS5 virtual table and sync triggers.
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dirs for {}", db_path.display()))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("opening sessions db at {}", db_path.display()))?;

        // Performance + integrity PRAGMAs. `foreign_keys = ON` is required
        // for the declared `session_messages.session_id REFERENCES
        // sessions(id)` clause to actually be enforced — without it SQLite
        // parses the constraint but never checks it, so orphan messages
        // pointing at a deleted or never-created session can accumulate.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;",
        )
        .context("applying PRAGMAs")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                channel TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                summary TEXT
            );

            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL
            );

            -- Indexes for per-channel session lookup and for the
            -- session_id FK join in search. Without these, every
            -- `list_sessions` scan and every JOIN in search had to
            -- full-scan.
            CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel);
            CREATE INDEX IF NOT EXISTS idx_session_messages_session_id
                ON session_messages(session_id);

            CREATE VIRTUAL TABLE IF NOT EXISTS session_messages_fts USING fts5(
                content,
                content=session_messages,
                content_rowid=id
            );

            -- FTS sync triggers
            CREATE TRIGGER IF NOT EXISTS sm_ai AFTER INSERT ON session_messages BEGIN
                INSERT INTO session_messages_fts(rowid, content)
                VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS sm_ad AFTER DELETE ON session_messages BEGIN
                INSERT INTO session_messages_fts(session_messages_fts, rowid, content)
                VALUES ('delete', old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS sm_au AFTER UPDATE ON session_messages BEGIN
                INSERT INTO session_messages_fts(session_messages_fts, rowid, content)
                VALUES ('delete', old.id, old.content);
                INSERT INTO session_messages_fts(rowid, content)
                VALUES (new.id, new.content);
            END;",
        )
        .context("creating sessions schema")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Create a new session for the given channel. Returns the session ID.
    pub async fn create_session(&self, channel: &str) -> Result<String> {
        let conn = Arc::clone(&self.conn);
        let channel = channel.to_string();
        tokio::task::spawn_blocking(move || {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO sessions (id, channel, started_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, channel, now],
            )
            .context("inserting session")?;
            Ok::<_, anyhow::Error>(id)
        })
        .await?
    }

    /// Add a message to an existing session.
    pub async fn add_message(&self, session_id: &str, role: &str, content: &str) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let session_id = session_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO session_messages (session_id, role, content, timestamp)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, role, content, now],
            )
            .context("inserting session message")?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Mark a session as ended, optionally attaching a summary.
    pub async fn end_session(&self, session_id: &str, summary: Option<&str>) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let session_id = session_id.to_string();
        let summary = summary.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let conn = conn.lock();
            conn.execute(
                "UPDATE sessions SET ended_at = ?1, summary = ?2 WHERE id = ?3",
                rusqlite::params![now, summary, session_id],
            )
            .context("ending session")?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Full-text search across all session messages. Returns results ranked by
    /// BM25 relevance score.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(fts_query) = crate::memory::fts::build_match_query(query) else {
            return Ok(vec![]);
        };

        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT m.session_id, m.role, m.content, m.timestamp, bm25(session_messages_fts) AS score
                 FROM session_messages_fts AS f
                 JOIN session_messages AS m ON m.id = f.rowid
                 WHERE session_messages_fts MATCH ?1
                 ORDER BY score
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![fts_query, limit as i64], |row| {
                let raw_score: f64 = row.get(4)?;
                Ok(SearchHit {
                    session_id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    score: -raw_score, // negate BM25 so higher = more relevant
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<_, anyhow::Error>(results)
        })
        .await?
    }

    /// Get a single session by id.
    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        let conn = Arc::clone(&self.conn);
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, channel, started_at, ended_at, summary
                 FROM sessions
                 WHERE id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![session_id])?;
            if let Some(row) = rows.next()? {
                Ok::<_, anyhow::Error>(Some(SessionRecord {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                    summary: row.get(4)?,
                }))
            } else {
                Ok(None)
            }
        })
        .await?
    }

    /// List recent messages for a single session, oldest first.
    /// `limit` caps the number of rows returned. Used by the MCP
    /// server's `messages_read` tool.
    pub async fn list_messages_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRow>> {
        let conn = Arc::clone(&self.conn);
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, session_id, role, content, timestamp
                 FROM session_messages
                 WHERE session_id = ?1
                 ORDER BY id ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![session_id, limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok::<_, anyhow::Error>(out)
        })
        .await?
    }

    /// List every message inserted with rowid greater than `after_id`,
    /// across every session, oldest first. The MCP server's event
    /// bridge calls this on each poll: it remembers the highest id
    /// it has emitted as `message` events and forwards anything new.
    /// `limit` is a safety cap so a backlog after a long idle gap
    /// can't pull thousands of rows in one tick.
    pub async fn list_messages_after(
        &self,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<MessageRow>> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, session_id, role, content, timestamp
                 FROM session_messages
                 WHERE id > ?1
                 ORDER BY id ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![after_id, limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok::<_, anyhow::Error>(out)
        })
        .await?
    }

    /// Highest message id currently in the store. Used by the event
    /// bridge to seed its cursor at startup so we don't replay every
    /// message in history as "new".
    pub async fn max_message_id(&self) -> Result<i64> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let id: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM session_messages",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok::<_, anyhow::Error>(id)
        })
        .await?
    }

    /// List the most recent sessions.
    pub async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, channel, started_at, ended_at, summary
                 FROM sessions
                 ORDER BY started_at DESC
                 LIMIT ?1",
            )?;

            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                    summary: row.get(4)?,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok::<_, anyhow::Error>(results)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (SessionStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sessions.db");
        let store = SessionStore::new(&db).unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn create_and_list_sessions() {
        let (store, _dir) = make_store();
        let id = store.create_session("telegram").await.unwrap();
        let sessions = store.list_sessions(10).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].channel, "telegram");
        assert!(sessions[0].ended_at.is_none());
    }

    #[tokio::test]
    async fn add_message_and_search() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store
            .add_message(&sid, "user", "How do I configure Fennec?")
            .await
            .unwrap();
        store
            .add_message(&sid, "assistant", "You can edit fennec.toml to configure Fennec.")
            .await
            .unwrap();

        let hits = store.search("configure Fennec", 10).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].content.contains("Fennec"));
    }

    #[tokio::test]
    async fn end_session_with_summary() {
        let (store, _dir) = make_store();
        let sid = store.create_session("discord").await.unwrap();
        store
            .end_session(&sid, Some("Discussed configuration"))
            .await
            .unwrap();
        let sessions = store.list_sessions(10).await.unwrap();
        assert!(sessions[0].ended_at.is_some());
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Discussed configuration")
        );
    }

    #[tokio::test]
    async fn search_empty_query_returns_empty() {
        let (store, _dir) = make_store();
        let hits = store.search("", 10).await.unwrap();
        assert!(hits.is_empty());
    }

    /// Regression: a user query containing `"` used to produce an
    /// FTS5 syntax error because the hand-rolled tokenizer emitted
    /// `"foo"bar"` instead of a safe quoted phrase. Now strips the
    /// inner quote and searches for `"foobar"`.
    #[tokio::test]
    async fn search_tolerates_embedded_quote() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store.add_message(&sid, "user", "foobar works fine").await.unwrap();
        let hits = store.search("foo\"bar", 10).await.unwrap();
        // Either finds the message or returns empty — the critical
        // invariant is that the call does not error.
        let _ = hits;
    }

    #[tokio::test]
    async fn list_messages_for_session_returns_in_time_order() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store.add_message(&sid, "user", "first").await.unwrap();
        store.add_message(&sid, "assistant", "second").await.unwrap();
        store.add_message(&sid, "user", "third").await.unwrap();
        let rows = store
            .list_messages_for_session(&sid, 100)
            .await
            .unwrap();
        let contents: Vec<&str> = rows.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(contents, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn list_messages_for_session_respects_limit() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        for i in 0..5 {
            store
                .add_message(&sid, "user", &format!("msg{}", i))
                .await
                .unwrap();
        }
        let rows = store
            .list_messages_for_session(&sid, 3)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn list_messages_after_pagination_works() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        for i in 0..4 {
            store
                .add_message(&sid, "user", &format!("m{}", i))
                .await
                .unwrap();
        }
        let all = store
            .list_messages_for_session(&sid, 100)
            .await
            .unwrap();
        let cursor = all[1].id;
        let after = store.list_messages_after(cursor, 100).await.unwrap();
        let contents: Vec<&str> = after.iter().map(|r| r.content.as_str()).collect();
        // Cursor was message #2; we should get #3 and #4 (m2, m3).
        assert_eq!(contents, vec!["m2", "m3"]);
    }

    #[tokio::test]
    async fn max_message_id_zero_when_empty() {
        let (store, _dir) = make_store();
        assert_eq!(store.max_message_id().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn max_message_id_tracks_inserts() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store.add_message(&sid, "user", "x").await.unwrap();
        let after_one = store.max_message_id().await.unwrap();
        store.add_message(&sid, "user", "y").await.unwrap();
        let after_two = store.max_message_id().await.unwrap();
        assert!(after_two > after_one);
    }

    #[tokio::test]
    async fn get_session_returns_record() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        let rec = store.get_session(&sid).await.unwrap();
        assert!(rec.is_some());
        assert_eq!(rec.unwrap().channel, "cli");
    }

    #[tokio::test]
    async fn get_session_none_when_missing() {
        let (store, _dir) = make_store();
        assert!(store.get_session("nope").await.unwrap().is_none());
    }

    /// Regression: the store's FK declaration (session_messages
    /// REFERENCES sessions) was not being enforced because
    /// `PRAGMA foreign_keys = ON` was missing. Inserting a message
    /// pointing at a non-existent session must now fail.
    #[tokio::test]
    async fn foreign_key_constraint_is_enforced() {
        let (store, _dir) = make_store();
        let r = store
            .add_message("non-existent-session-id", "user", "oops")
            .await;
        assert!(
            r.is_err(),
            "message with dangling session_id must be rejected (got Ok) — PRAGMA foreign_keys not applied?"
        );
    }
}
