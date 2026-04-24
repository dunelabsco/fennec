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
            // Use original_id for deduplication: delete any existing entry with same original_id
            // then insert the new one. This effectively does INSERT OR REPLACE keyed on original_id.
            conn.execute(
                "DELETE FROM collective_cache WHERE original_id = ?1",
                rusqlite::params![original_id],
            )?;
            conn.execute(
                "INSERT INTO collective_cache
                 (id, original_id, source_server, goal, solution, gotchas_json,
                  trust_score, relevance_score, outcome_success, outcome_failure, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            let Some(fts_query) = crate::memory::fts::build_match_query(&query) else {
                return Ok(vec![]);
            };

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
