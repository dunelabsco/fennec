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

/// Metadata for a session.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub channel: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
    /// Human label set by `/title <name>`. Distinct from
    /// `summary` (which is the system-generated end-of-session
    /// description). `None` until the user names the session.
    pub title: Option<String>,
}

/// One persisted message inside a session — replayed into
/// [`Agent::replace_history`] when `/resume` loads a prior
/// conversation.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
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
                summary TEXT,
                title TEXT
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

        // Migration for older dbs that pre-date the `title` column.
        // SQLite has no `IF NOT EXISTS` for ADD COLUMN, so check the
        // table's pragma list and only run the ALTER when needed.
        // Failure here is non-fatal — if the column already exists
        // the duplicate-column error is the expected case.
        let has_title_col: bool = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(sessions)")
                .context("preparing pragma table_info(sessions)")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .context("querying table_info(sessions)")?;
            let mut found = false;
            for r in rows {
                if r? == "title" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_title_col {
            conn.execute("ALTER TABLE sessions ADD COLUMN title TEXT", [])
                .context("adding title column to sessions")?;
        }

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

    /// List the most recent sessions.
    pub async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, channel, started_at, ended_at, summary, title
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
                    title: row.get(5)?,
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

    /// Look up a single session by exact id, then by exact title
    /// as a fallback. Mirrors Hermes' `db.get_session_by_title()`
    /// fallback (`tui_gateway/server.py:2180-2221`) so users who
    /// type `/resume my-experiment` instead of the UUID can still
    /// land on the right session. Returns `None` when neither
    /// match.
    pub async fn get_session(&self, id_or_title: &str) -> Result<Option<SessionRecord>> {
        let conn = Arc::clone(&self.conn);
        let key = id_or_title.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            // Try id first.
            let mut stmt = conn.prepare(
                "SELECT id, channel, started_at, ended_at, summary, title
                 FROM sessions WHERE id = ?1 LIMIT 1",
            )?;
            let mut rows = stmt.query_map(rusqlite::params![key], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                    summary: row.get(4)?,
                    title: row.get(5)?,
                })
            })?;
            if let Some(row) = rows.next() {
                return Ok::<_, anyhow::Error>(Some(row?));
            }
            // Title fallback.
            let mut stmt = conn.prepare(
                "SELECT id, channel, started_at, ended_at, summary, title
                 FROM sessions WHERE title = ?1 ORDER BY started_at DESC LIMIT 1",
            )?;
            let mut rows = stmt.query_map(rusqlite::params![key], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                    summary: row.get(4)?,
                    title: row.get(5)?,
                })
            })?;
            if let Some(row) = rows.next() {
                return Ok(Some(row?));
            }
            Ok(None)
        })
        .await?
    }

    /// Set (or clear) the user-facing title for a session. Empty
    /// title clears the field. Returns the number of rows
    /// affected — `0` means the session id wasn't found.
    pub async fn set_session_title(&self, id: &str, title: &str) -> Result<usize> {
        let conn = Arc::clone(&self.conn);
        let id = id.to_string();
        let title = if title.trim().is_empty() {
            None
        } else {
            Some(title.trim().to_string())
        };
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let n = conn
                .execute(
                    "UPDATE sessions SET title = ?1 WHERE id = ?2",
                    rusqlite::params![title, id],
                )
                .context("updating session title")?;
            Ok::<_, anyhow::Error>(n)
        })
        .await?
    }

    /// Fetch every persisted message for `session_id` in
    /// chronological order. Used by `/resume` to repopulate
    /// `Agent::history` so the next turn sees the full prior
    /// context, matching Hermes' `db.get_messages_as_conversation`
    /// (`tui_gateway/server.py:2180-2221`).
    pub async fn get_session_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let conn = Arc::clone(&self.conn);
        let sid = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT role, content, timestamp
                 FROM session_messages
                 WHERE session_id = ?1
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![sid], |row| {
                Ok(StoredMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    timestamp: row.get(2)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok::<_, anyhow::Error>(out)
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
    async fn set_and_get_session_title_round_trip() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        // Initially title is None.
        let rec = store.get_session(&sid).await.unwrap().unwrap();
        assert!(rec.title.is_none());
        // Set title.
        let n = store
            .set_session_title(&sid, "experiment-1")
            .await
            .unwrap();
        assert_eq!(n, 1);
        let rec = store.get_session(&sid).await.unwrap().unwrap();
        assert_eq!(rec.title.as_deref(), Some("experiment-1"));
        // Empty title clears.
        let n = store.set_session_title(&sid, "  ").await.unwrap();
        assert_eq!(n, 1);
        let rec = store.get_session(&sid).await.unwrap().unwrap();
        assert!(rec.title.is_none());
    }

    #[tokio::test]
    async fn get_session_falls_back_to_title_when_id_missing() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store.set_session_title(&sid, "alpha").await.unwrap();
        // Lookup by exact title string.
        let rec = store.get_session("alpha").await.unwrap().unwrap();
        assert_eq!(rec.id, sid);
    }

    #[tokio::test]
    async fn get_session_messages_returns_in_chronological_order() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").await.unwrap();
        store.add_message(&sid, "user", "first").await.unwrap();
        store.add_message(&sid, "assistant", "second").await.unwrap();
        store.add_message(&sid, "user", "third").await.unwrap();
        let msgs = store.get_session_messages(&sid).await.unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "first");
        assert_eq!(msgs[2].content, "third");
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
