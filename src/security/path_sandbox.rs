//! Local-filesystem access sandbox for agent tools.
//!
//! Tools that take a file path from the LLM (`read_file`, `write_file`,
//! `list_dir`, `edit_file`, `glob`, `grep`, `pdf_read` local branch,
//! `image_info` local branch, `vision_describe` local branch,
//! `transcribe_audio`) were previously unconstrained — a prompt-injected
//! agent could ask for `~/.ssh/id_rsa` and ship it off-device via whichever
//! tool's output channel happened to be convenient.
//!
//! This module provides a single primitive, [`PathSandbox::check`], which:
//!
//! 1. **Canonicalizes** the input path (resolving symlinks where possible).
//!    Symlinks pointing at a forbidden file therefore can't bypass the
//!    check by being planted in a "benign" directory. When the path
//!    doesn't exist yet (write case), the deepest existing ancestor is
//!    canonicalized and the non-existing tail is appended literally.
//! 2. Runs the canonical path through the `security.forbidden_paths`
//!    denylist from config. Each denied entry is matched **by path
//!    components**, not by raw substring. So `.ssh` rejects `~/.ssh/id_rsa`
//!    but does *not* reject `~/projects/.sshare/notes.md` (the substring
//!    `.ssh` appears inside `.sshare` but it isn't a complete component).
//!    Patterns starting with `/` are anchored to the filesystem root;
//!    patterns without a leading slash match anywhere as a contiguous
//!    component run.
//!
//!    We started with naive `String::contains` here, which is what flagged
//!    legitimate paths like `/home/user/letsencrypt/...` (substring "etc"
//!    inside "letsencrypt") and `~/Documents/.ssh-tools-doc.txt`. Component
//!    matching keeps the same denials we want without those false positives.
//!
//! The sandbox is a *denylist*, not an allowlist root. Agents are often
//! asked to touch `/tmp/...` or `~/Downloads/...`, so requiring a
//! configured workspace root would break legitimate workflows. The
//! denylist catches the known-sensitive paths we've identified and is
//! easy to extend via config.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Shared denylist-based path sandbox.
///
/// Cheap to clone (wraps a `Vec<String>`), shareable via `Arc` when tools
/// want to hold one. The empty sandbox is a no-op and suitable as a
/// `Default` for tests / unconfigured setups.
#[derive(Debug, Clone, Default)]
pub struct PathSandbox {
    /// Component-aware denial patterns. Each entry is a path-like string
    /// (e.g. `.ssh`, `.config/gcloud`, `/etc`, `.fennec/config.toml`).
    /// Matched component-by-component against the canonical path —
    /// patterns starting with `/` are anchored to the filesystem root,
    /// patterns without a leading slash match a contiguous component run
    /// anywhere in the path. macOS uses case-insensitive comparison
    /// because HFS+ is case-insensitive by default.
    denied: Vec<String>,
}

impl PathSandbox {
    /// Create a sandbox from a list of denylist patterns (typically
    /// `SecurityConfig::forbidden_paths`).
    pub fn new(denied: Vec<String>) -> Self {
        Self { denied }
    }

    /// An empty sandbox — accepts any path. Intended for tests and for
    /// tools that haven't had a real sandbox wired in yet.
    pub fn empty() -> Self {
        Self {
            denied: Vec::new(),
        }
    }

    /// Validate a path. Returns the canonical form if acceptable; returns
    /// an error if the path resolves (or would resolve) to a denylisted
    /// location.
    pub fn check(&self, path: &Path) -> Result<PathBuf> {
        let canonical = resolve_best_effort(path).with_context(|| {
            format!("resolving path: {}", path.display())
        })?;

        for pat in &self.denied {
            if path_matches_pattern(&canonical, pat) {
                bail!(
                    "path matches forbidden pattern '{}': {}",
                    pat,
                    canonical.display()
                );
            }
        }
        Ok(canonical)
    }

    /// True if the sandbox has no denied patterns — useful for conditional
    /// fast paths in tests.
    pub fn is_empty(&self) -> bool {
        self.denied.is_empty()
    }
}

/// Component-aware match: does `path` contain a contiguous run of
/// components equal to the components of `pattern`? Patterns starting with
/// `/` are anchored to the root of the path; otherwise the run can start
/// at any position.
///
/// We compare via `OsStr` to stay correct on platforms with non-UTF-8 path
/// components, falling back to case-insensitive comparison via
/// `to_string_lossy` on macOS (HFS+ default is case-insensitive).
fn path_matches_pattern(path: &Path, pattern: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if pat_parts.is_empty() {
        return false;
    }
    let anchored = pattern.starts_with('/');

    let path_parts: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            // RootDir/Prefix/CurDir/ParentDir are not real components —
            // canonicalization should have eliminated CurDir/ParentDir; we
            // skip the rest because they aren't matched against names.
            _ => None,
        })
        .collect();

    if path_parts.len() < pat_parts.len() {
        return false;
    }

    let max_start = if anchored {
        0
    } else {
        path_parts.len() - pat_parts.len()
    };

    for start in 0..=max_start {
        if window_matches(&path_parts[start..start + pat_parts.len()], &pat_parts) {
            return true;
        }
    }
    false
}

/// Compare a window of path components against a pattern's parts.
fn window_matches(window: &[&std::ffi::OsStr], pat_parts: &[&str]) -> bool {
    for (a, b) in window.iter().zip(pat_parts.iter()) {
        let matches = a.to_str() == Some(*b) || {
            #[cfg(target_os = "macos")]
            {
                a.to_string_lossy().to_lowercase() == b.to_lowercase()
            }
            #[cfg(not(target_os = "macos"))]
            {
                false
            }
        };
        if !matches {
            return false;
        }
    }
    true
}

/// Canonicalize `path` if it exists. If it doesn't, walk up to find the
/// deepest existing ancestor, canonicalize that, and append the remaining
/// non-existing tail literally. This lets writes to new files still
/// benefit from symlink resolution on their existing parent directories.
fn resolve_best_effort(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    if absolute.exists() {
        return Ok(std::fs::canonicalize(&absolute)?);
    }

    // Walk up looking for an existing ancestor.
    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = absolute.as_path();
    loop {
        if cursor.exists() {
            let mut result = std::fs::canonicalize(cursor)?;
            for seg in tail.iter().rev() {
                result.push(seg);
            }
            return Ok(result);
        }
        match (cursor.file_name(), cursor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                cursor = parent;
            }
            _ => {
                // Ran out of ancestors without finding one that exists.
                // Return the absolute path as-is; the denylist check will
                // still run on it.
                return Ok(absolute);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> PathSandbox {
        PathSandbox::new(vec![
            "/etc".to_string(),
            ".ssh".to_string(),
            ".fennec/.secret_key".to_string(),
        ])
    }

    #[test]
    fn empty_sandbox_accepts_everything() {
        let s = PathSandbox::empty();
        assert!(s.check(Path::new("/")).is_ok());
    }

    #[test]
    fn rejects_denied_substring() {
        let s = sandbox();
        let r = s.check(Path::new("/etc/passwd"));
        assert!(r.is_err());
        let err = r.unwrap_err().to_string();
        assert!(err.contains("/etc"), "err: {}", err);
    }

    #[test]
    fn rejects_dotssh_substring() {
        let tmp = tempfile::tempdir().unwrap();
        let ssh = tmp.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        let id = ssh.join("id_rsa");
        std::fs::write(&id, b"fake").unwrap();

        let s = sandbox();
        let r = s.check(&id);
        assert!(r.is_err());
    }

    #[test]
    fn accepts_benign_path() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();
        let s = sandbox();
        let resolved = s.check(&f).unwrap();
        assert_eq!(resolved, f.canonicalize().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn rejects_symlink_to_denied() {
        // Plant a symlink in a benign dir that targets a denied path.
        // Canonicalization should resolve the symlink and trip the deny
        // list.
        let tmp = tempfile::tempdir().unwrap();
        let ssh_dir = tmp.path().join(".ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        let real = ssh_dir.join("id_rsa");
        std::fs::write(&real, b"fake").unwrap();

        let link = tmp.path().join("benign.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let s = sandbox();
        let r = s.check(&link);
        assert!(r.is_err(), "symlink to .ssh should be rejected");
    }

    #[test]
    fn write_path_to_nonexistent_file_under_denied_dir() {
        // For a write to a not-yet-existing file whose PARENT is denied,
        // the sandbox must still catch it via the tail-append logic.
        let s = PathSandbox::new(vec!["/etc".to_string()]);
        let r = s.check(Path::new("/etc/new-file-we-are-writing.txt"));
        assert!(r.is_err());
    }

    /// The old substring match rejected `/home/user/letsencrypt/notes`
    /// because the substring "etc" appeared in "letsencrypt". Component
    /// matching gets it right.
    #[test]
    fn substring_etc_inside_letsencrypt_is_not_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("letsencrypt");
        std::fs::create_dir(&dir).unwrap();
        let f = dir.join("notes.txt");
        std::fs::write(&f, b"hi").unwrap();

        let s = PathSandbox::new(vec!["/etc".to_string()]);
        assert!(
            s.check(&f).is_ok(),
            "letsencrypt directory must not be confused with /etc"
        );
    }

    /// Same shape: `.ssh` should not match `.sshare`.
    #[test]
    fn substring_ssh_inside_sshare_is_not_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".sshare");
        std::fs::create_dir(&dir).unwrap();
        let f = dir.join("notes.md");
        std::fs::write(&f, b"hi").unwrap();

        let s = PathSandbox::new(vec![".ssh".to_string()]);
        assert!(
            s.check(&f).is_ok(),
            ".sshare must not be confused with .ssh"
        );
    }

    /// `.config/gcloud` should match the directory but NOT `.config/gcloud-tools`
    /// or `.config/foo/gcloud`.
    #[test]
    fn config_gcloud_pattern_is_component_anchored() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join(".config");
        std::fs::create_dir(&cfg).unwrap();

        // True positive: .config/gcloud/auth.json
        let real = cfg.join("gcloud");
        std::fs::create_dir(&real).unwrap();
        let f = real.join("auth.json");
        std::fs::write(&f, b"hi").unwrap();
        let s = PathSandbox::new(vec![".config/gcloud".to_string()]);
        assert!(s.check(&f).is_err());

        // False positive case: .config/gcloud-tool/notes.txt — different
        // component, must not match.
        let other = cfg.join("gcloud-tool");
        std::fs::create_dir(&other).unwrap();
        let f2 = other.join("notes.txt");
        std::fs::write(&f2, b"hi").unwrap();
        assert!(s.check(&f2).is_ok(), "gcloud-tool != gcloud as a component");
    }

    /// Anchored patterns (starting with `/`) only match from the root.
    #[test]
    fn root_anchored_pattern_only_matches_from_root() {
        let s = PathSandbox::new(vec!["/etc".to_string()]);
        // /etc/passwd matches.
        assert!(s.check(Path::new("/etc/passwd")).is_err());
        // /home/user/etc/notes does NOT match because /etc is anchored.
        let tmp = tempfile::tempdir().unwrap();
        let etc_under_home = tmp.path().join("etc");
        std::fs::create_dir(&etc_under_home).unwrap();
        let f = etc_under_home.join("notes.txt");
        std::fs::write(&f, b"hi").unwrap();
        assert!(s.check(&f).is_ok(), "/etc anchored pattern must not match etc/ deeper in tree");
    }

    #[test]
    fn resolve_best_effort_handles_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let non = tmp.path().join("does/not/exist/yet.txt");
        let r = resolve_best_effort(&non).unwrap();
        assert!(r.ends_with("does/not/exist/yet.txt"));
        // The existing portion (tmp.path()) should be canonicalized.
        assert!(r.starts_with(tmp.path().canonicalize().unwrap()));
    }
}
