use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    /// Schedule expression: "every 30m", "every 1h", "every 24h", "every 7d"
    pub schedule: String,
    /// Message to send to the agent when this job fires.
    pub command: String,
    pub enabled: bool,
    pub last_run: Option<String>,
    /// The channel this job originated from (e.g. "telegram", "discord").
    #[serde(default)]
    pub origin_channel: Option<String>,
    /// The chat ID within the origin channel to deliver results to.
    #[serde(default)]
    pub origin_chat_id: Option<String>,
}

pub struct JobStore {
    jobs: Vec<CronJob>,
    path: PathBuf,
}

impl JobStore {
    /// Create a new `JobStore` backed by the given JSON file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            jobs: Vec::new(),
            path: path.into(),
        }
    }

    /// Load jobs from the backing file. If the file doesn't exist, start empty.
    pub fn load(&mut self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let data = std::fs::read_to_string(&self.path)
            .with_context(|| format!("reading job store from {}", self.path.display()))?;
        self.jobs = serde_json::from_str(&data)
            .with_context(|| format!("parsing job store from {}", self.path.display()))?;
        Ok(())
    }

    /// Persist the current job list to the backing file.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let data = serde_json::to_string_pretty(&self.jobs)
            .context("serializing job store")?;
        std::fs::write(&self.path, data)
            .with_context(|| format!("writing job store to {}", self.path.display()))?;
        Ok(())
    }

    /// Add a job to the store.
    pub fn add_job(&mut self, job: CronJob) {
        self.jobs.push(job);
    }

    /// Remove a job by ID. Returns `true` if the job was found and removed.
    pub fn remove_job(&mut self, id: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        self.jobs.len() < before
    }

    /// List all jobs.
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Get a mutable reference to a job by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// The file path this store persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Parse a schedule string like "every 30m", "every 1h", "every 7d", or a
/// bare duration like "5m", "1h" into the interval in seconds.
///
/// Bare durations (without the "every " prefix) are treated as one-shot
/// delays but still return the number of seconds, so that the scheduler
/// can fire the job once after the given delay.
///
/// Supported units:
/// - `s` — seconds
/// - `m` — minutes
/// - `h` — hours
/// - `d` — days
///
/// Returns `None` if the format is invalid.
pub fn parse_schedule(schedule: &str) -> Option<u64> {
    let trimmed = schedule.trim();

    // Strip optional "every " prefix; bare durations are also accepted.
    let rest = trimmed
        .strip_prefix("every ")
        .unwrap_or(trimmed)
        .trim();

    if rest.is_empty() {
        return None;
    }

    // Split numeric part from unit suffix.
    let unit_pos = rest.find(|c: char| c.is_alphabetic())?;
    let (num_str, unit) = rest.split_at(unit_pos);
    let num: u64 = num_str.trim().parse().ok()?;

    let multiplier = match unit.trim() {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };

    Some(num * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_schedule_minutes() {
        assert_eq!(parse_schedule("every 30m"), Some(1800));
    }

    #[test]
    fn test_parse_schedule_hours() {
        assert_eq!(parse_schedule("every 1h"), Some(3600));
        assert_eq!(parse_schedule("every 24h"), Some(86400));
    }

    #[test]
    fn test_parse_schedule_days() {
        assert_eq!(parse_schedule("every 7d"), Some(604800));
    }

    #[test]
    fn test_parse_schedule_seconds() {
        assert_eq!(parse_schedule("every 90s"), Some(90));
    }

    #[test]
    fn test_parse_schedule_bare_duration() {
        assert_eq!(parse_schedule("5m"), Some(300));
        assert_eq!(parse_schedule("1h"), Some(3600));
        assert_eq!(parse_schedule("30s"), Some(30));
    }

    #[test]
    fn test_parse_schedule_invalid() {
        assert_eq!(parse_schedule(""), None);
        assert_eq!(parse_schedule("every"), None);
        assert_eq!(parse_schedule("every abc"), None);
        assert_eq!(parse_schedule("every 30x"), None);
        assert_eq!(parse_schedule("abc"), None);
    }
}
