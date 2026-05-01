//! Persistent curator state at `<home>/skills/.curator_state`.
//!
//! Stores when the curator last ran, how long it took, a one-line
//! summary of what changed, and whether the user has paused
//! automatic runs. The file is rewritten atomically (temp + rename)
//! after every run.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// On-disk shape of `.curator_state`. All fields default so an
/// existing file with fewer fields migrates forward without a wipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CuratorState {
    /// Wall-clock time the most recent run started.
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    /// How long the most recent run took, in seconds.
    #[serde(default)]
    pub last_run_duration_seconds: f64,
    /// One-line summary written into the state file at the end of
    /// each run. Surfaced by `fennec curator status`.
    #[serde(default)]
    pub last_run_summary: String,
    /// Path to the most recent run's report directory under
    /// `<home>/logs/curator/`. None when no run has happened yet.
    #[serde(default)]
    pub last_report_path: Option<PathBuf>,
    /// User-set: when true, automatic runs are skipped. Manual runs
    /// (`fennec curator run`) still work.
    #[serde(default)]
    pub paused: bool,
    /// Total number of runs since the file was created.
    #[serde(default)]
    pub run_count: u64,
}

/// Thread-safe owner of the on-disk state file. Cheap to construct;
/// holds an in-memory copy that is read on demand and written back
/// atomically on every mutation.
pub struct CuratorStateStore {
    path: PathBuf,
    inner: Mutex<CuratorState>,
}

impl CuratorStateStore {
    /// Open the state file at `<skills_root>/.curator_state`. A
    /// missing file produces an empty state; an unreadable or
    /// malformed file is logged at warn level and replaced (the
    /// damaged file is preserved as `.corrupt-<unixtime>`).
    pub fn open(skills_root: &Path) -> Self {
        let path = skills_root.join(".curator_state");
        let data = match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => CuratorState::default(),
            Ok(bytes) => match serde_json::from_slice::<CuratorState>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    let preserve = path.with_extension(format!(
                        "corrupt-{}",
                        Utc::now().timestamp()
                    ));
                    let _ = std::fs::rename(&path, &preserve);
                    tracing::warn!(
                        path = %path.display(),
                        preserved = %preserve.display(),
                        error = %e,
                        "curator state file corrupt; starting fresh"
                    );
                    CuratorState::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CuratorState::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not read curator state; starting empty"
                );
                CuratorState::default()
            }
        };
        Self {
            path,
            inner: Mutex::new(data),
        }
    }

    /// Snapshot of the current in-memory state.
    pub fn snapshot(&self) -> CuratorState {
        self.inner.lock().clone()
    }

    /// Set the paused flag. Persisted immediately.
    pub fn set_paused(&self, paused: bool) -> std::io::Result<()> {
        let snapshot = {
            let mut s = self.inner.lock();
            s.paused = paused;
            s.clone()
        };
        self.write(&snapshot)
    }

    /// Record a completed run. Updates `last_run_at`, duration,
    /// summary, report path, and increments `run_count`.
    pub fn record_run(
        &self,
        started_at: DateTime<Utc>,
        duration_seconds: f64,
        summary: String,
        report_path: Option<PathBuf>,
    ) -> std::io::Result<()> {
        let snapshot = {
            let mut s = self.inner.lock();
            s.last_run_at = Some(started_at);
            s.last_run_duration_seconds = duration_seconds;
            s.last_run_summary = summary;
            s.last_report_path = report_path;
            s.run_count = s.run_count.saturating_add(1);
            s.clone()
        };
        self.write(&snapshot)
    }

    fn write(&self, data: &CuratorState) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_name = format!(
            ".curator_state.tmp-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let tmp = self
            .path
            .parent()
            .map(|p| p.join(&tmp_name))
            .unwrap_or_else(|| PathBuf::from(&tmp_name));
        let json = serde_json::to_vec_pretty(data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let store = CuratorStateStore::open(tmp.path());
        let s = store.snapshot();
        assert_eq!(s, CuratorState::default());
    }

    #[test]
    fn record_run_persists() {
        let tmp = TempDir::new().unwrap();
        let store = CuratorStateStore::open(tmp.path());
        let now = Utc::now();
        store
            .record_run(now, 12.5, "did things".into(), Some(PathBuf::from("/log/x")))
            .unwrap();

        let store2 = CuratorStateStore::open(tmp.path());
        let s = store2.snapshot();
        assert_eq!(s.last_run_at, Some(now));
        assert_eq!(s.last_run_duration_seconds, 12.5);
        assert_eq!(s.last_run_summary, "did things");
        assert_eq!(s.last_report_path, Some(PathBuf::from("/log/x")));
        assert_eq!(s.run_count, 1);
    }

    #[test]
    fn run_count_increments() {
        let tmp = TempDir::new().unwrap();
        let store = CuratorStateStore::open(tmp.path());
        for _ in 0..3 {
            store
                .record_run(Utc::now(), 1.0, "x".into(), None)
                .unwrap();
        }
        assert_eq!(store.snapshot().run_count, 3);
    }

    #[test]
    fn pause_persists() {
        let tmp = TempDir::new().unwrap();
        let store = CuratorStateStore::open(tmp.path());
        store.set_paused(true).unwrap();
        let store2 = CuratorStateStore::open(tmp.path());
        assert!(store2.snapshot().paused);
        store2.set_paused(false).unwrap();
        let store3 = CuratorStateStore::open(tmp.path());
        assert!(!store3.snapshot().paused);
    }

    #[test]
    fn corrupt_file_preserved_and_replaced() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".curator_state"), b"not json {").unwrap();
        let store = CuratorStateStore::open(tmp.path());
        assert_eq!(store.snapshot(), CuratorState::default());
        let preserved: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".corrupt-")
            })
            .collect();
        assert_eq!(preserved.len(), 1);
    }
}
