use std::sync::Arc;

use anyhow::Result;

use crate::collective::traits::{CollectiveSearchResult, OutcomeReports};
use crate::memory::sqlite::SqliteMemory;

pub struct CollectiveCache {
    memory: Arc<SqliteMemory>,
}

impl CollectiveCache {
    pub fn new(memory: Arc<SqliteMemory>) -> Self {
        Self { memory }
    }

    /// Cache a search result from the collective.
    pub async fn cache_result(
        &self,
        result: &CollectiveSearchResult,
        source: &str,
    ) -> Result<()> {
        let conn = self.memory.connection();
        let id = uuid::Uuid::new_v4().to_string();
        let original_id = result.id.clone();
        let source = source.to_string();
        let goal = result.goal.clone();
        let solution = result.solution.clone();
        let gotchas_json = serde_json::to_string(&result.gotchas)?;
        let trust_score = result.trust_score;
        let relevance_score = result.relevance_score;
        let outcome_success = result.outcome_reports.success as i64;
        let outcome_failure = result.outcome_reports.failure as i64;
        let cached_at = chrono::Utc::now().to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            // Single-statement atomic upsert keyed on original_id. The old
            // implementation was DELETE + INSERT, which (a) wasn't atomic —
            // two concurrent callers with the same original_id could both
            // delete, both insert, and land two rows — and (b) discarded
            // the row's `last_used` history on every re-cache.
            //
            // ON CONFLICT targets the UNIQUE index
            // `idx_collective_cache_original_id` added in the schema. The
            // DO UPDATE clause intentionally leaves `id`, `original_id`,
            // and `last_used` untouched so mark_used() history survives.
            conn.execute(
                "INSERT INTO collective_cache
                 (id, original_id, source_server, goal, solution, gotchas_json,
                  trust_score, relevance_score, outcome_success, outcome_failure, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(original_id) DO UPDATE SET
                     source_server   = excluded.source_server,
                     goal            = excluded.goal,
                     solution        = excluded.solution,
                     gotchas_json    = excluded.gotchas_json,
                     trust_score     = excluded.trust_score,
                     relevance_score = excluded.relevance_score,
                     outcome_success = excluded.outcome_success,
                     outcome_failure = excluded.outcome_failure,
                     cached_at       = excluded.cached_at",
                rusqlite::params![
                    id,
                    original_id,
                    source,
                    goal,
                    solution,
                    gotchas_json,
                    trust_score,
                    relevance_score,
                    outcome_success,
                    outcome_failure,
                    cached_at,
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// Search the local cache using FTS5.
    pub async fn search_cache(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<CollectiveSearchResult>> {
        let conn = self.memory.connection();
        let query = query.to_string();

        tokio::task::spawn_blocking(move || {
            // Build FTS5 query: wrap each word in quotes, join with OR.
            let fts_query: String = query
                .split_whitespace()
                .map(|w| format!("\"{}\"", w))
                .collect::<Vec<_>>()
                .join(" OR ");

            if fts_query.is_empty() {
                return Ok(vec![]);
            }

            let conn = conn.lock();
            let mut stmt = conn.prepare(
                "SELECT c.original_id, c.goal, c.solution, c.gotchas_json,
                        c.trust_score, c.relevance_score, c.outcome_success, c.outcome_failure
                 FROM collective_cache_fts AS f
                 JOIN collective_cache AS c ON c.rowid = f.rowid
                 WHERE collective_cache_fts MATCH ?1
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![fts_query, limit as i64], |row| {
                let original_id: String = row.get(0)?;
                let goal: String = row.get(1)?;
                let solution: Option<String> = row.get(2)?;
                let gotchas_json: String = row.get(3)?;
                let trust_score: f64 = row.get(4)?;
                let relevance_score: Option<f64> = row.get(5)?;
                let outcome_success: i64 = row.get(6)?;
                let outcome_failure: i64 = row.get(7)?;

                Ok((
                    original_id,
                    goal,
                    solution,
                    gotchas_json,
                    trust_score,
                    relevance_score.unwrap_or(0.0),
                    outcome_success as u32,
                    outcome_failure as u32,
                ))
            })?;

            let mut results = Vec::new();
            for row in rows {
                let (original_id, goal, solution, gotchas_json, trust_score, relevance_score, success, failure) = row?;
                let gotchas: Vec<String> =
                    serde_json::from_str(&gotchas_json).unwrap_or_default();

                results.push(CollectiveSearchResult {
                    id: original_id,
                    goal,
                    solution,
                    gotchas,
                    trust_score,
                    relevance_score,
                    outcome_reports: OutcomeReports { success, failure },
                });
            }

            Ok(results)
        })
        .await?
    }

    /// Mark a cached entry as recently used.
    pub async fn mark_used(&self, original_id: &str) -> Result<()> {
        let conn = self.memory.connection();
        let original_id = original_id.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "UPDATE collective_cache SET last_used = ?1 WHERE original_id = ?2",
                rusqlite::params![now, original_id],
            )?;
            Ok(())
        })
        .await?
    }

    /// Decay trust scores for stale entries (no last_used in N days).
    pub async fn decay_stale(&self, days: u64) -> Result<usize> {
        let conn = self.memory.connection();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let affected = conn.execute(
                "UPDATE collective_cache SET trust_score = trust_score * 0.9
                 WHERE last_used IS NULL OR last_used < ?1",
                rusqlite::params![cutoff_str],
            )?;
            Ok(affected)
        })
        .await?
    }

    /// Evict entries below trust threshold.
    pub async fn evict_low_trust(&self, threshold: f64) -> Result<usize> {
        let conn = self.memory.connection();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let deleted = conn.execute(
                "DELETE FROM collective_cache WHERE trust_score < ?1",
                rusqlite::params![threshold],
            )?;
            Ok(deleted)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collective::traits::OutcomeReports;
    use crate::memory::embedding::NoopEmbedding;
    use crate::memory::sqlite::SqliteMemory;
    use tempfile::TempDir;

    async fn fixture() -> (Arc<SqliteMemory>, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mem.db");
        let mem = SqliteMemory::new(
            db_path,
            0.7,
            0.3,
            1024,
            Arc::new(NoopEmbedding::new(1536)),
        )
        .unwrap();
        (Arc::new(mem), dir)
    }

    fn mk_result(id: &str, goal: &str) -> CollectiveSearchResult {
        CollectiveSearchResult {
            id: id.to_string(),
            goal: goal.to_string(),
            solution: Some("sol".to_string()),
            gotchas: vec![],
            trust_score: 0.8,
            relevance_score: 0.9,
            outcome_reports: OutcomeReports {
                success: 1,
                failure: 0,
            },
        }
    }

    /// Regression for T3-F: caching twice with the same original_id
    /// used to race on DELETE+INSERT and could produce two rows.
    /// With the UNIQUE index + ON CONFLICT DO UPDATE, the second
    /// call atomically updates the first row.
    #[tokio::test]
    async fn cache_result_twice_leaves_single_row() {
        let (mem, _dir) = fixture().await;
        let cache = CollectiveCache::new(Arc::clone(&mem));

        let first = mk_result("same-id", "first goal");
        let second = mk_result("same-id", "second goal");

        cache.cache_result(&first, "plurum").await.unwrap();
        cache.cache_result(&second, "plurum").await.unwrap();

        // Exactly one row should exist for this original_id.
        let conn = mem.connection();
        let count: i64 = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["same-id"],
                |row| row.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(count, 1, "expected exactly one row per original_id");

        // And the row should reflect the SECOND call's fields.
        let conn = mem.connection();
        let goal: String = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.query_row(
                "SELECT goal FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["same-id"],
                |row| row.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(goal, "second goal");
    }

    /// Regression for T3-F: mark_used sets last_used. Re-caching the
    /// same original_id must NOT reset last_used (the ON CONFLICT
    /// clause is deliberately selective about which columns to
    /// overwrite).
    #[tokio::test]
    async fn re_cache_preserves_last_used() {
        let (mem, _dir) = fixture().await;
        let cache = CollectiveCache::new(Arc::clone(&mem));

        cache.cache_result(&mk_result("k", "g"), "plurum").await.unwrap();
        cache.mark_used("k").await.unwrap();

        // Grab the last_used timestamp.
        let conn = mem.connection();
        let before: Option<String> = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.query_row(
                "SELECT last_used FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["k"],
                |row| row.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert!(before.is_some());

        // Re-cache.
        cache.cache_result(&mk_result("k", "new goal"), "plurum").await.unwrap();

        // last_used must still be the value mark_used set.
        let conn = mem.connection();
        let after: Option<String> = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.query_row(
                "SELECT last_used FROM collective_cache WHERE original_id = ?1",
                rusqlite::params!["k"],
                |row| row.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(after, before, "re-cache must not clear last_used");
    }

    /// Sanity: the UNIQUE index on original_id is in place — a raw
    /// INSERT with a duplicate must fail. (This guards against future
    /// schema edits accidentally dropping the index.)
    #[tokio::test]
    async fn unique_index_on_original_id_enforced() {
        let (mem, _dir) = fixture().await;
        let cache = CollectiveCache::new(Arc::clone(&mem));
        cache.cache_result(&mk_result("unique-test", "g"), "plurum").await.unwrap();

        // Try to bypass the upsert path and do a raw INSERT — should fail.
        let conn = mem.connection();
        let dup_err = tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO collective_cache
                 (id, original_id, source_server, goal, solution, gotchas_json,
                  trust_score, relevance_score, outcome_success, outcome_failure, cached_at)
                 VALUES ('another-uuid', 'unique-test', 's', 'g', NULL, '[]', 0.5, NULL, 0, 0, 'now')",
                [],
            )
        })
        .await
        .unwrap();
        assert!(
            dup_err.is_err(),
            "raw INSERT with duplicate original_id must fail the UNIQUE index"
        );
    }
}
