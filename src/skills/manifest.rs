//! Provenance manifests: the bundled-skill manifest and the hub lock.
//!
//! Both files live under `<home>/skills/` and answer one question:
//! "is this skill agent-created, or did it come from somewhere else?"
//! The usage sidecar and the curator both filter by this signal.
//!
//! # Bundled manifest (`<home>/skills/.bundled_manifest`)
//!
//! v2 format: one `name:hash` line per bundled skill. The hash is
//! a content fingerprint of the canonical bundled copy. When a sync
//! runs and the user's on-disk content matches the recorded hash, the
//! sync may safely update; when it does not, the user has customized
//! the skill and we leave it alone.
//!
//! v1 format (legacy): plain skill names, one per line, no hash. Auto-
//! migrated to v2 on next sync (the migration sets the hash to the
//! user's current content, which is the conservative choice — it
//! prevents the next sync from overwriting whatever they have).
//!
//! Lines starting with `#` are comments; blank lines are ignored.
//!
//! # Hub lock (`<home>/skills/.hub/lock.json`)
//!
//! Placeholder for the future skills-hub installer. Today the loader
//! reads it if present, but no Fennec code writes it. The file shape is:
//!
//! ```json
//! { "installed": { "skill-name": { ... } } }
//! ```
//!
//! Anything in `installed` is treated as hub-installed and excluded
//! from agent-created tracking.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// In-memory view of `<home>/skills/.bundled_manifest`.
///
/// Holds an owned map so it can be cloned cheaply across the loader,
/// the usage store, and the sync routine without re-reading the file.
#[derive(Debug, Clone, Default)]
pub struct BundledManifest {
    /// `name -> origin_hash`. Empty when the file is missing.
    entries: BTreeMap<String, String>,
    /// Where the manifest lives on disk. Used by `save`.
    path: Option<PathBuf>,
}

impl BundledManifest {
    /// Load from `<skills_root>/.bundled_manifest`. Returns an empty,
    /// path-bound manifest when the file is missing or unreadable;
    /// neither condition is fatal because most users start with no
    /// bundled-sync state.
    pub fn load(skills_root: &Path) -> Self {
        let path = skills_root.join(".bundled_manifest");
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                return Self {
                    entries: BTreeMap::new(),
                    path: Some(path),
                };
            }
        };

        let mut entries = BTreeMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // v2: `name:hash`. v1: `name` alone — the hash slot is
            // filled with empty string and a future sync will baseline.
            if let Some((name, hash)) = line.split_once(':') {
                let n = name.trim();
                let h = hash.trim();
                if !n.is_empty() {
                    entries.insert(n.to_string(), h.to_string());
                }
            } else {
                entries.insert(line.to_string(), String::new());
            }
        }

        Self {
            entries,
            path: Some(path),
        }
    }

    /// Construct without a backing file. Useful in tests and for
    /// callers that want to ask "what would provenance look like with
    /// these explicit bundled names?"
    pub fn from_entries(entries: BTreeMap<String, String>) -> Self {
        Self {
            entries,
            path: None,
        }
    }

    /// True if `name` is recorded as bundled.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Origin hash (empty string for v1 entries that haven't been
    /// upgraded yet). `None` when the skill is not in the manifest.
    pub fn origin_hash(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(String::as_str)
    }

    /// Insert or update an entry. Does not write to disk.
    pub fn set(&mut self, name: impl Into<String>, hash: impl Into<String>) {
        self.entries.insert(name.into(), hash.into());
    }

    /// Remove an entry. Does not write to disk.
    pub fn remove(&mut self, name: &str) -> Option<String> {
        self.entries.remove(name)
    }

    /// Snapshot of every recorded `(name, hash)` pair.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Atomic write to the manifest's bound path. Returns `Err` if the
    /// manifest was constructed without one (`from_entries`).
    pub fn save(&self) -> std::io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "manifest has no on-disk path; load it from a file or use save_to",
            )
        })?;
        self.save_to(path)
    }

    /// Atomic write to an explicit path.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut body =
            String::from("# Fennec bundled-skill manifest. Format: name:origin_hash\n");
        for (name, hash) in &self.entries {
            if hash.is_empty() {
                body.push_str(name);
                body.push('\n');
            } else {
                body.push_str(name);
                body.push(':');
                body.push_str(hash);
                body.push('\n');
            }
        }

        let tmp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Number of recorded skills.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the manifest is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// In-memory view of `<home>/skills/.hub/lock.json`. Everything in
/// `installed` is treated as hub-managed and excluded from agent-
/// created tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubLock {
    /// Map of installed skill name -> opaque metadata (left as
    /// `serde_json::Value` because the hub installer owns the schema).
    #[serde(default)]
    pub installed: HashMap<String, serde_json::Value>,
}

impl HubLock {
    /// Load from `<skills_root>/.hub/lock.json`. Returns an empty
    /// lock if missing, malformed, or unreadable.
    pub fn load(skills_root: &Path) -> Self {
        let path = skills_root.join(".hub").join("lock.json");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        match serde_json::from_slice::<HubLock>(&bytes) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "hub lock unreadable; treating as empty"
                );
                Self::default()
            }
        }
    }

    /// True if `name` is present in `installed`.
    pub fn contains(&self, name: &str) -> bool {
        self.installed.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let m = BundledManifest::load(tmp.path());
        assert!(m.is_empty());
        assert!(!m.contains("anything"));
    }

    #[test]
    fn parses_v2_lines() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".bundled_manifest"),
            "# header comment\nfoo:abc\nbar:def\n\n# trailing comment\n",
        )
        .unwrap();
        let m = BundledManifest::load(tmp.path());
        assert_eq!(m.len(), 2);
        assert!(m.contains("foo"));
        assert_eq!(m.origin_hash("foo"), Some("abc"));
        assert_eq!(m.origin_hash("bar"), Some("def"));
        assert!(!m.contains("nope"));
    }

    #[test]
    fn parses_v1_lines_with_empty_hash() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".bundled_manifest"), "foo\nbar\n").unwrap();
        let m = BundledManifest::load(tmp.path());
        assert!(m.contains("foo"));
        assert_eq!(m.origin_hash("foo"), Some(""));
    }

    #[test]
    fn save_round_trips() {
        let tmp = TempDir::new().unwrap();
        let mut m = BundledManifest::load(tmp.path());
        m.set("foo", "abc");
        m.set("bar", "");
        m.save().unwrap();

        let m2 = BundledManifest::load(tmp.path());
        assert_eq!(m2.origin_hash("foo"), Some("abc"));
        assert_eq!(m2.origin_hash("bar"), Some(""));
    }

    #[test]
    fn save_fails_without_path() {
        let m = BundledManifest::from_entries(BTreeMap::from([("x".into(), "h".into())]));
        assert!(m.save().is_err());
    }

    #[test]
    fn remove_works() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".bundled_manifest"),
            "foo:abc\nbar:def\n",
        )
        .unwrap();
        let mut m = BundledManifest::load(tmp.path());
        let prev = m.remove("foo");
        assert_eq!(prev.as_deref(), Some("abc"));
        assert!(!m.contains("foo"));
        m.save().unwrap();
        let m2 = BundledManifest::load(tmp.path());
        assert!(!m2.contains("foo"));
        assert!(m2.contains("bar"));
    }

    #[test]
    fn hub_lock_missing_is_empty() {
        let tmp = TempDir::new().unwrap();
        let l = HubLock::load(tmp.path());
        assert!(!l.contains("anything"));
    }

    #[test]
    fn hub_lock_reads_installed_keys() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".hub")).unwrap();
        std::fs::write(
            tmp.path().join(".hub").join("lock.json"),
            r#"{"installed":{"foo":{"source":"github"}}}"#,
        )
        .unwrap();
        let l = HubLock::load(tmp.path());
        assert!(l.contains("foo"));
        assert!(!l.contains("bar"));
    }

    #[test]
    fn hub_lock_malformed_treated_as_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".hub")).unwrap();
        std::fs::write(tmp.path().join(".hub").join("lock.json"), b"not json").unwrap();
        let l = HubLock::load(tmp.path());
        assert!(!l.contains("anything"));
    }
}
