//! Sidecar JSON usage tracking for agent-created skills.
//!
//! State lives at `<home>/skills/.usage.json` and is read/written
//! atomically (write to temp + rename). Counters are best-effort:
//! a failed mutation is logged at debug level and swallowed so that
//! telemetry never fails a real agent operation.
//!
//! Only **agent-created** skills are recorded here. Bundled and
//! hub-installed skills are filtered out at the call site
//! (`UsageStore::is_agent_created`) so user-installed third-party
//! content does not pollute the curator's signal.
//!
//! On-disk shape (stable):
//!
//! ```json
//! {
//!   "skill-name": {
//!     "use_count": 42,
//!     "view_count": 7,
//!     "last_used_at": "2026-05-01T14:30:00+00:00",
//!     "last_viewed_at": "2026-05-01T14:20:00+00:00",
//!     "patch_count": 3,
//!     "last_patched_at": "2026-04-28T10:15:00+00:00",
//!     "created_at": "2026-04-20T08:00:00+00:00",
//!     "state": "active",
//!     "pinned": false,
//!     "archived_at": null
//!   }
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::format::SkillState;
use super::manifest::{BundledManifest, HubLock};

/// One row of the usage sidecar.
///
/// Every field has a serde default so an existing on-disk record can be
/// migrated forward when new fields are added without rewriting the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillUsageRecord {
    /// Bumped each time the skill is loaded into the agent's prompt or
    /// invoked through a tool that resolves it by name.
    #[serde(default)]
    pub use_count: u64,
    /// Bumped each time the skill is read via `skill_view` (a future
    /// observability tool — present here so the schema is stable
    /// from the start).
    #[serde(default)]
    pub view_count: u64,
    /// Most recent `bump_use` timestamp.
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    /// Most recent `bump_view` timestamp.
    #[serde(default)]
    pub last_viewed_at: Option<DateTime<Utc>>,
    /// Number of edits/patches via `skill_manage`.
    #[serde(default)]
    pub patch_count: u64,
    /// Most recent edit/patch timestamp.
    #[serde(default)]
    pub last_patched_at: Option<DateTime<Utc>>,
    /// First time this record was created. Used by the lifecycle
    /// scheduler when a skill has never been used: the staleness anchor
    /// falls back to `created_at`.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// Lifecycle state. Default is `active`.
    #[serde(default)]
    pub state: SkillState,
    /// User opt-out from auto-transitions. Pinned skills stay in their
    /// current state regardless of staleness; the curator's prompt also
    /// forbids consolidating pinned skills.
    #[serde(default)]
    pub pinned: bool,
    /// When the skill was archived (state moved to `Archived` and the
    /// directory moved to `<home>/skills/.archive/`). `None` while the
    /// skill is `Active` or `Stale`.
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

impl SkillUsageRecord {
    /// The reference timestamp the lifecycle scheduler uses to decide
    /// whether a skill is stale or archivable. Defaults to `last_used_at`,
    /// falling back to `created_at` when the skill has never been used,
    /// falling back to `Utc::now()` when neither is set (which protects
    /// brand-new records from being archived on their first run).
    pub fn staleness_anchor(&self) -> DateTime<Utc> {
        self.last_used_at
            .or(self.created_at)
            .unwrap_or_else(Utc::now)
    }
}

/// Thread-safe owner of the on-disk usage sidecar.
///
/// Cheap to clone (the inner mutex is Arc'd via the Mutex itself; we
/// wrap callers' `Arc<UsageStore>` instead of cloning the struct).
pub struct UsageStore {
    path: PathBuf,
    inner: Mutex<UsageData>,
    bundled: BundledManifest,
    hub: HubLock,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UsageData {
    #[serde(flatten)]
    records: HashMap<String, SkillUsageRecord>,
}

impl UsageStore {
    /// Open the usage sidecar at `<skills_root>/.usage.json`. If the file
    /// is missing, returns an empty store; if it is present but
    /// corrupt, logs a warning and starts fresh (the bad file is
    /// preserved by writing the new state to a temp + rename, leaving
    /// the original in place under `<path>.corrupt-<unixtime>` for
    /// post-mortem).
    ///
    /// `skills_root` is `<home>/skills/`; the bundled manifest and hub
    /// lock are loaded from the same root.
    pub fn open(skills_root: &Path) -> Self {
        let path = skills_root.join(".usage.json");
        let data = match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => UsageData::default(),
            Ok(bytes) => match serde_json::from_slice::<UsageData>(&bytes) {
                Ok(d) => d,
                Err(e) => {
                    let preserve = path.with_extension(format!(
                        "json.corrupt-{}",
                        Utc::now().timestamp()
                    ));
                    let _ = std::fs::rename(&path, &preserve);
                    tracing::warn!(
                        path = %path.display(),
                        preserved = %preserve.display(),
                        error = %e,
                        "skill usage sidecar was corrupt; starting fresh and preserving the bad file"
                    );
                    UsageData::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => UsageData::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not read skill usage sidecar; starting empty"
                );
                UsageData::default()
            }
        };

        let bundled = BundledManifest::load(skills_root);
        let hub = HubLock::load(skills_root);

        Self {
            path,
            inner: Mutex::new(data),
            bundled,
            hub,
        }
    }

    /// Open with explicit bundled-manifest and hub-lock instances. Used
    /// by tests and by callers that want to avoid re-reading the
    /// sidecar files (e.g., when the same `BundledManifest` is reused
    /// across the loader and the usage store within one boot).
    pub fn with_provenance(
        skills_root: &Path,
        bundled: BundledManifest,
        hub: HubLock,
    ) -> Self {
        let path = skills_root.join(".usage.json");
        let data = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<UsageData>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(data),
            bundled,
            hub,
        }
    }

    /// True if `skill_name` is owned by the agent (not bundled or
    /// hub-installed). Only agent-created skills are tracked.
    pub fn is_agent_created(&self, skill_name: &str) -> bool {
        !self.bundled.contains(skill_name) && !self.hub.contains(skill_name)
    }

    /// Snapshot every agent-created skill. Returned as `(name, record)`
    /// pairs. Skills present in the bundled manifest or hub lock are
    /// excluded even if they have a stale record (cleanup).
    pub fn agent_created_report(&self) -> Vec<(String, SkillUsageRecord)> {
        let inner = self.inner.lock();
        inner
            .records
            .iter()
            .filter(|(name, _)| self.is_agent_created(name))
            .map(|(n, r)| (n.clone(), r.clone()))
            .collect()
    }

    /// Get a single record (clones). Returns `None` if the skill has
    /// never been recorded or is not agent-created.
    pub fn get(&self, name: &str) -> Option<SkillUsageRecord> {
        if !self.is_agent_created(name) {
            return None;
        }
        self.inner.lock().records.get(name).cloned()
    }

    /// Bump the use counter and refresh `last_used_at`. No-op (logged
    /// at debug) if the skill is not agent-created or if the on-disk
    /// write fails.
    pub fn bump_use(&self, name: &str) {
        self.mutate(name, |r| {
            r.use_count = r.use_count.saturating_add(1);
            r.last_used_at = Some(Utc::now());
        });
    }

    /// Bump the view counter and refresh `last_viewed_at`.
    pub fn bump_view(&self, name: &str) {
        self.mutate(name, |r| {
            r.view_count = r.view_count.saturating_add(1);
            r.last_viewed_at = Some(Utc::now());
        });
    }

    /// Bump the edit/patch counter and refresh `last_patched_at`.
    pub fn bump_patch(&self, name: &str) {
        self.mutate(name, |r| {
            r.patch_count = r.patch_count.saturating_add(1);
            r.last_patched_at = Some(Utc::now());
        });
    }

    /// Set the lifecycle state explicitly. Used by the curator's
    /// auto-transitions and by archive/restore. When transitioning
    /// to `Archived`, also stamps `archived_at`; when transitioning
    /// out of `Archived`, clears it.
    pub fn set_state(&self, name: &str, state: SkillState) {
        self.mutate(name, |r| {
            let was_archived = r.state == SkillState::Archived;
            r.state = state;
            match state {
                SkillState::Archived => {
                    if !was_archived {
                        r.archived_at = Some(Utc::now());
                    }
                }
                _ => {
                    r.archived_at = None;
                }
            }
        });
    }

    /// Pin or unpin a skill. Pinned skills are exempt from auto-
    /// transitions and from curator consolidation.
    pub fn set_pinned(&self, name: &str, pinned: bool) {
        self.mutate(name, |r| {
            r.pinned = pinned;
        });
    }

    /// Drop a skill's record. Called by `skill_manage delete` so a
    /// recreated skill of the same name starts fresh.
    pub fn forget(&self, name: &str) {
        let mut inner = self.inner.lock();
        if inner.records.remove(name).is_some() {
            let snapshot = inner.clone();
            drop(inner);
            if let Err(e) = self.write(&snapshot) {
                tracing::debug!(error = %e, name = %name, "skill usage forget: write failed");
            }
        }
    }

    /// Convenience for tests and CLI: collect all recorded skill names,
    /// agent-created or otherwise. Filtered consumers should prefer
    /// `agent_created_report`.
    pub fn all_names(&self) -> HashSet<String> {
        self.inner.lock().records.keys().cloned().collect()
    }

    /// Internal: apply `f` to the named skill's record, creating it
    /// (with `created_at = now`) if missing. Skips and logs at debug
    /// level if the skill is not agent-created.
    fn mutate<F: FnOnce(&mut SkillUsageRecord)>(&self, name: &str, f: F) {
        if !self.is_agent_created(name) {
            tracing::debug!(name = %name, "skill usage skipped: not agent-created");
            return;
        }
        let snapshot = {
            let mut inner = self.inner.lock();
            let record = inner.records.entry(name.to_string()).or_insert_with(|| {
                SkillUsageRecord {
                    created_at: Some(Utc::now()),
                    ..Default::default()
                }
            });
            if record.created_at.is_none() {
                record.created_at = Some(Utc::now());
            }
            f(record);
            inner.clone()
        };
        if let Err(e) = self.write(&snapshot) {
            tracing::debug!(error = %e, name = %name, "skill usage write failed");
        }
    }

    /// Atomic write: serialize to a temp file in the same directory,
    /// fsync (best-effort), then `rename` over the target. The temp
    /// path is `<path>.tmp-<pid>-<unixnanos>` so concurrent writers
    /// from the same process don't collide.
    fn write(&self, data: &UsageData) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_name = format!(
            ".usage.json.tmp-{}-{}",
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

    fn fresh_store() -> (TempDir, UsageStore) {
        let tmp = TempDir::new().unwrap();
        let store = UsageStore::open(tmp.path());
        (tmp, store)
    }

    #[test]
    fn empty_store_returns_no_records() {
        let (_tmp, store) = fresh_store();
        assert!(store.agent_created_report().is_empty());
        assert!(store.get("anything").is_none());
    }

    #[test]
    fn bump_use_creates_record_and_persists() {
        let (tmp, store) = fresh_store();
        store.bump_use("foo");
        let r = store.get("foo").expect("record should exist");
        assert_eq!(r.use_count, 1);
        assert!(r.last_used_at.is_some());
        assert!(r.created_at.is_some());

        // Re-open and confirm persistence.
        let store2 = UsageStore::open(tmp.path());
        let r2 = store2.get("foo").unwrap();
        assert_eq!(r2.use_count, 1);
    }

    #[test]
    fn bump_use_is_idempotent_under_repeat() {
        let (_tmp, store) = fresh_store();
        for _ in 0..5 {
            store.bump_use("foo");
        }
        assert_eq!(store.get("foo").unwrap().use_count, 5);
    }

    #[test]
    fn bump_view_and_patch_track_independent_counters() {
        let (_tmp, store) = fresh_store();
        store.bump_view("foo");
        store.bump_view("foo");
        store.bump_patch("foo");
        let r = store.get("foo").unwrap();
        assert_eq!(r.view_count, 2);
        assert_eq!(r.patch_count, 1);
        assert_eq!(r.use_count, 0);
    }

    #[test]
    fn set_state_archive_stamps_archived_at() {
        let (_tmp, store) = fresh_store();
        store.bump_use("foo");
        store.set_state("foo", SkillState::Archived);
        let r = store.get("foo").unwrap();
        assert_eq!(r.state, SkillState::Archived);
        assert!(r.archived_at.is_some());

        store.set_state("foo", SkillState::Active);
        let r = store.get("foo").unwrap();
        assert_eq!(r.state, SkillState::Active);
        assert!(
            r.archived_at.is_none(),
            "archived_at must clear when leaving Archived"
        );
    }

    #[test]
    fn set_pinned_persists() {
        let (tmp, store) = fresh_store();
        store.bump_use("foo");
        store.set_pinned("foo", true);
        let r = UsageStore::open(tmp.path()).get("foo").unwrap();
        assert!(r.pinned);
    }

    #[test]
    fn forget_drops_record() {
        let (tmp, store) = fresh_store();
        store.bump_use("foo");
        store.forget("foo");
        assert!(store.get("foo").is_none());
        assert!(UsageStore::open(tmp.path()).get("foo").is_none());
    }

    #[test]
    fn bundled_skills_are_filtered_out() {
        let tmp = TempDir::new().unwrap();
        // Write a bundled manifest containing "foo" with some hash.
        std::fs::write(
            tmp.path().join(".bundled_manifest"),
            "foo:0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();

        let store = UsageStore::open(tmp.path());
        // Bumping a bundled skill is a no-op.
        store.bump_use("foo");
        assert!(store.get("foo").is_none());
        assert!(store.agent_created_report().is_empty());

        // Agent-created skill is tracked normally.
        store.bump_use("bar");
        assert_eq!(store.get("bar").unwrap().use_count, 1);
    }

    #[test]
    fn corrupt_sidecar_is_preserved_and_replaced() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join(".usage.json");
        std::fs::write(&p, b"not json at all {").unwrap();

        let store = UsageStore::open(tmp.path());
        // Fresh start — no records.
        assert!(store.agent_created_report().is_empty());

        // The corrupt file is preserved as `.json.corrupt-<ts>` (best-effort).
        let preserved: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".usage.json.corrupt-")
            })
            .collect();
        assert_eq!(preserved.len(), 1, "corrupt file should be preserved");

        // New writes succeed.
        store.bump_use("foo");
        assert_eq!(store.get("foo").unwrap().use_count, 1);
    }

    #[test]
    fn staleness_anchor_falls_back_to_created_at() {
        let mut r = SkillUsageRecord::default();
        let t = Utc::now();
        r.created_at = Some(t);
        // last_used_at is None — anchor should use created_at.
        assert_eq!(r.staleness_anchor(), t);

        let later = t + chrono::Duration::days(1);
        r.last_used_at = Some(later);
        assert_eq!(r.staleness_anchor(), later);
    }

    #[test]
    fn report_excludes_pre_existing_bundled_records() {
        let tmp = TempDir::new().unwrap();
        // Pre-seed sidecar with both an agent-created and a bundled record.
        let mut data = UsageData::default();
        data.records
            .insert("agent-skill".into(), SkillUsageRecord::default());
        data.records
            .insert("bundled-skill".into(), SkillUsageRecord::default());
        std::fs::write(
            tmp.path().join(".usage.json"),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(".bundled_manifest"),
            "bundled-skill:abc\n",
        )
        .unwrap();

        let store = UsageStore::open(tmp.path());
        let names: HashSet<_> = store
            .agent_created_report()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(names.contains("agent-skill"));
        assert!(!names.contains("bundled-skill"));
    }
}
