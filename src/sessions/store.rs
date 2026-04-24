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
}

/// SQLite-backed session store with FTS5 search.
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

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;",
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
    pub fn create_session(&self, channel: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO sessions (id, channel, started_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, channel, now],
        )
        .context("inserting session")?;
        Ok(id)
    }

    /// Add a message to an existing session.
    pub fn add_message(&self, session_id: &str, role: &str, content: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO session_messages (session_id, role, content, timestamp)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, content, now],
        )
        .context("inserting session message")?;
        Ok(())
    }

    /// Mark a session as ended, optionally attaching a summary.
    pub fn end_session(&self, session_id: &str, summary: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sessions SET ended_at = ?1, summary = ?2 WHERE id = ?3",
            rusqlite::params![now, summary, session_id],
        )
        .context("ending session")?;
        Ok(())
    }

    /// Full-text search across all session messages. Returns results ranked by
    /// BM25 relevance score.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let fts_query: String = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w))
            .collect::<Vec<_>>()
            .join(" OR ");

        if fts_query.is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.lock();
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
        Ok(results)
    }

    /// List the most recent sessions.
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock();
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
        Ok(results)
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

    #[test]
    fn create_and_list_sessions() {
        let (store, _dir) = make_store();
        let id = store.create_session("telegram").unwrap();
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].channel, "telegram");
        assert!(sessions[0].ended_at.is_none());
    }

    #[test]
    fn add_message_and_search() {
        let (store, _dir) = make_store();
        let sid = store.create_session("cli").unwrap();
        store
            .add_message(&sid, "user", "How do I configure Fennec?")
            .unwrap();
        store
            .add_message(&sid, "assistant", "You can edit fennec.toml to configure Fennec.")
            .unwrap();

        let hits = store.search("configure Fennec", 10).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].content.contains("Fennec"));
    }

    #[test]
    fn end_session_with_summary() {
        let (store, _dir) = make_store();
        let sid = store.create_session("discord").unwrap();
        store
            .end_session(&sid, Some("Discussed configuration"))
            .unwrap();
        let sessions = store.list_sessions(10).unwrap();
        assert!(sessions[0].ended_at.is_some());
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Discussed configuration")
        );
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let (store, _dir) = make_store();
        let hits = store.search("", 10).unwrap();
        assert!(hits.is_empty());
    }
}
