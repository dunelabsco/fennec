//! Move skills to and from `<home>/skills/.archive/`.
//!
//! Archival is the curator's "delete" — it never removes content, only
//! relocates the skill so the loader stops walking it. The original
//! tree is preserved verbatim under `.archive/<name>/` so a future
//! `restore` can put it back.
//!
//! For collision safety, if `.archive/<name>/` already exists (e.g. an
//! earlier archive of a same-named skill), the new archive lands at
//! `.archive/<name>-<unixtime>/` so neither copy is lost.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::format::SkillLayout;

/// Result of an archive call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveResult {
    /// Final destination under `.archive/`.
    pub archived_to: PathBuf,
    /// Whether the destination was suffixed with a timestamp because a
    /// previous archive already lived at the un-suffixed path.
    pub collision_suffixed: bool,
}

/// Move a skill (flat or directory) into `<skills_root>/.archive/`.
///
/// On success, returns the final destination path. The on-disk source
/// — either the `.md` file or the whole directory — is moved by
/// `std::fs::rename` so this is fast and atomic when source and dest
/// share a filesystem. (When they don't, we fall back to copy + remove.)
pub fn archive(skills_root: &Path, name: &str, layout: &SkillLayout) -> Result<ArchiveResult> {
    let archive_root = skills_root.join(".archive");
    std::fs::create_dir_all(&archive_root)
        .with_context(|| format!("creating archive root {}", archive_root.display()))?;

    let mut dest = archive_root.join(name);
    let mut collision_suffixed = false;
    if dest.exists() {
        let suffix = chrono::Utc::now().timestamp();
        dest = archive_root.join(format!("{}-{}", name, suffix));
        collision_suffixed = true;
    }

    // For a flat skill we move the .md file; for a directory skill we
    // move the whole tree. To keep the archive's on-disk shape uniform
    // ("everything under .archive/<name>/ is a directory") we wrap a
    // flat skill in a `<name>/` directory containing the original
    // SKILL.md, mirroring the directory format. This means restore
    // can always treat the archive layout the same way.
    match layout {
        SkillLayout::Flat { file } => {
            std::fs::create_dir(&dest)
                .with_context(|| format!("mkdir {}", dest.display()))?;
            let target = dest.join("SKILL.md");
            move_path(file, &target).with_context(|| {
                format!(
                    "archiving flat skill {} → {}",
                    file.display(),
                    target.display()
                )
            })?;
        }
        SkillLayout::Directory { dir, .. } => {
            move_path(dir, &dest).with_context(|| {
                format!("archiving directory skill {} → {}", dir.display(), dest.display())
            })?;
        }
    }

    Ok(ArchiveResult {
        archived_to: dest,
        collision_suffixed,
    })
}

/// Restore a previously archived skill. Returns the path the skill is
/// restored to.
///
/// The restore always lands in directory format at
/// `<skills_root>/<name>/SKILL.md` (regardless of whether the original
/// was flat). This is intentional: once the curator has touched a
/// skill, promoting it to directory format keeps the option to add
/// supporting files later without another move.
pub fn restore(skills_root: &Path, name: &str) -> Result<PathBuf> {
    let archive_root = skills_root.join(".archive");
    let src = archive_root.join(name);
    if !src.is_dir() {
        anyhow::bail!(
            "no archived skill named {:?} at {}",
            name,
            src.display()
        );
    }
    let dest = skills_root.join(name);
    if dest.exists() {
        anyhow::bail!(
            "cannot restore {:?}: destination {} already exists",
            name,
            dest.display()
        );
    }
    move_path(&src, &dest).with_context(|| {
        format!("restoring {} → {}", src.display(), dest.display())
    })
}

/// List archived skill names (children of `.archive/`).
pub fn list_archived(skills_root: &Path) -> Vec<String> {
    let archive_root = skills_root.join(".archive");
    let entries = match std::fs::read_dir(&archive_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut names = Vec::new();
    for e in entries.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(n) = e.file_name().to_str() {
                if !n.starts_with('.') {
                    names.push(n.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

/// Move a path. Falls back to copy + remove when `rename` fails with
/// `ErrorKind::CrossesDevices` (Linux EXDEV) or any other error that
/// suggests the source and destination live on different filesystems.
fn move_path(src: &Path, dest: &Path) -> Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(dest.to_path_buf()),
        Err(_) => {
            // Cross-device or any other reason — copy then remove. We
            // accept the small window where both copies exist; the
            // alternative (failing the archive entirely) loses data.
            copy_recursive(src, dest)?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)?;
            } else {
                std::fs::remove_file(src)?;
            }
            Ok(dest.to_path_buf())
        }
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_file() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dest)?;
        return Ok(());
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let dst = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_recursive(&p, &dst)?;
        } else {
            std::fs::copy(&p, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_flat(root: &Path, name: &str, body: &str) -> SkillLayout {
        let p = root.join(format!("{}.md", name));
        std::fs::write(
            &p,
            format!("---\nname: {}\ndescription: x\n---\n{}\n", name, body),
        )
        .unwrap();
        SkillLayout::Flat { file: p }
    }

    fn write_directory(root: &Path, name: &str, body: &str) -> SkillLayout {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {}\ndescription: x\n---\n{}\n", name, body),
        )
        .unwrap();
        SkillLayout::Directory {
            dir,
            category: None,
        }
    }

    #[test]
    fn archive_flat_skill_wraps_in_directory() {
        let tmp = TempDir::new().unwrap();
        let layout = write_flat(tmp.path(), "foo", "body");
        let r = archive(tmp.path(), "foo", &layout).unwrap();
        assert_eq!(r.archived_to, tmp.path().join(".archive").join("foo"));
        assert!(!r.collision_suffixed);
        assert!(r.archived_to.join("SKILL.md").is_file());
        // Original file is gone.
        assert!(!tmp.path().join("foo.md").exists());
    }

    #[test]
    fn archive_directory_skill_moves_whole_tree() {
        let tmp = TempDir::new().unwrap();
        let layout = write_directory(tmp.path(), "foo", "body");
        // Add a supporting file.
        std::fs::create_dir_all(tmp.path().join("foo").join("references")).unwrap();
        std::fs::write(
            tmp.path().join("foo").join("references").join("a.md"),
            "ref",
        )
        .unwrap();

        let r = archive(tmp.path(), "foo", &layout).unwrap();
        assert!(r.archived_to.join("SKILL.md").is_file());
        assert!(
            r.archived_to.join("references").join("a.md").is_file(),
            "supporting files preserved"
        );
        assert!(!tmp.path().join("foo").exists(), "original directory gone");
    }

    #[test]
    fn collision_appends_timestamp() {
        let tmp = TempDir::new().unwrap();
        let layout1 = write_flat(tmp.path(), "foo", "first");
        let r1 = archive(tmp.path(), "foo", &layout1).unwrap();
        assert_eq!(r1.archived_to, tmp.path().join(".archive").join("foo"));
        assert!(!r1.collision_suffixed);

        let layout2 = write_flat(tmp.path(), "foo", "second");
        let r2 = archive(tmp.path(), "foo", &layout2).unwrap();
        assert!(r2.collision_suffixed, "second archive should suffix");
        assert!(
            r2.archived_to
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("foo-"))
                .unwrap_or(false),
            "suffixed name: {:?}",
            r2.archived_to
        );
    }

    #[test]
    fn restore_brings_skill_back() {
        let tmp = TempDir::new().unwrap();
        let layout = write_directory(tmp.path(), "foo", "body");
        archive(tmp.path(), "foo", &layout).unwrap();
        assert!(!tmp.path().join("foo").exists());

        let dest = restore(tmp.path(), "foo").unwrap();
        assert_eq!(dest, tmp.path().join("foo"));
        assert!(tmp.path().join("foo").join("SKILL.md").is_file());
        assert!(!tmp.path().join(".archive").join("foo").exists());
    }

    #[test]
    fn restore_flat_archive_lands_in_directory_format() {
        let tmp = TempDir::new().unwrap();
        let layout = write_flat(tmp.path(), "foo", "body");
        archive(tmp.path(), "foo", &layout).unwrap();

        let dest = restore(tmp.path(), "foo").unwrap();
        assert_eq!(dest, tmp.path().join("foo"));
        assert!(tmp.path().join("foo").join("SKILL.md").is_file());
        // Restore lands in directory format even for an originally-flat skill.
        assert!(!tmp.path().join("foo.md").exists());
    }

    #[test]
    fn restore_errors_when_destination_exists() {
        let tmp = TempDir::new().unwrap();
        // Archive a skill, then create something at the live name.
        let layout = write_directory(tmp.path(), "foo", "body");
        archive(tmp.path(), "foo", &layout).unwrap();
        std::fs::create_dir_all(tmp.path().join("foo")).unwrap();

        let err = restore(tmp.path(), "foo").unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "got: {}",
            err
        );
    }

    #[test]
    fn restore_errors_when_archive_missing() {
        let tmp = TempDir::new().unwrap();
        let err = restore(tmp.path(), "ghost").unwrap_err();
        assert!(err.to_string().contains("no archived skill"));
    }

    #[test]
    fn list_archived_returns_sorted_names() {
        let tmp = TempDir::new().unwrap();
        let l1 = write_flat(tmp.path(), "alpha", "x");
        let l2 = write_flat(tmp.path(), "gamma", "x");
        let l3 = write_flat(tmp.path(), "beta", "x");
        archive(tmp.path(), "alpha", &l1).unwrap();
        archive(tmp.path(), "gamma", &l2).unwrap();
        archive(tmp.path(), "beta", &l3).unwrap();
        let names = list_archived(tmp.path());
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn list_archived_empty_when_no_archive_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(list_archived(tmp.path()).is_empty());
    }
}
