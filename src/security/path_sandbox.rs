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
//!    denylist from config. Each denied entry is a substring match against
//!    the canonical path string (e.g. `.ssh`, `.gnupg`, `.aws`,
//!    `.fennec/config.toml`, `.fennec/.secret_key`).
//!
//! The sandbox is a *denylist*, not an allowlist root. Agents are often
//! asked to touch `/tmp/...` or `~/Downloads/...`, so requiring a
//! configured workspace root would break legitimate workflows. The
//! denylist catches the known-sensitive paths we've identified and is
//! easy to extend via config.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Shared denylist-based path sandbox.
///
/// Cheap to clone (wraps a `Vec<String>`), shareable via `Arc` when tools
/// want to hold one. The empty sandbox is a no-op and suitable as a
/// `Default` for tests / unconfigured setups.
#[derive(Debug, Clone, Default)]
pub struct PathSandbox {
    /// Substrings in the canonicalized path that cause a reject. Matched
    /// case-sensitively on Unix / Linux; on macOS we also match
    /// case-insensitively because HFS+ is case-insensitive by default.
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

        let canonical_str = canonical.to_string_lossy();
        for pat in &self.denied {
            if canonical_str.contains(pat.as_str()) {
                bail!(
                    "path matches forbidden pattern '{}': {}",
                    pat,
                    canonical.display()
                );
            }
            #[cfg(target_os = "macos")]
            {
                if canonical_str
                    .to_lowercase()
                    .contains(&pat.to_lowercase())
                    && !canonical_str.contains(pat.as_str())
                {
                    bail!(
                        "path matches forbidden pattern '{}' (case-insensitive): {}",
                        pat,
                        canonical.display()
                    );
                }
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
