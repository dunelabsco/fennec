//! Structural integrity checks for skill directories.
//!
//! These run before the regex pass and catch shapes the regex pass
//! can't see — too many files, oversized blobs, binary extensions,
//! symlink escapes, executable-bit creep on non-script content.
//! Each finding has a stable `rule` name so a config can disable it
//! individually.

use std::path::{Path, PathBuf};

use super::patterns::Category;
use super::{Finding, GuardConfig, Location, Severity};

/// Soft limit on how many files a skill directory can contain. A
/// well-shaped skill is `SKILL.md` plus a handful of `references/`,
/// `templates/`, `scripts/`, `assets/` files. Anything past this
/// suggests a dump rather than a curated skill.
pub const MAX_FILES: usize = 50;

/// Hard limit on total skill size (bytes, post-walk).
pub const MAX_TOTAL_SIZE: u64 = 1 * 1024 * 1024;

/// Per-file size cap. Anything larger gets flagged so reviewers
/// notice; this is independent of the loader's size limit because
/// the guard runs before the loader.
pub const MAX_INDIVIDUAL_FILE: u64 = 256 * 1024;

/// File extensions that should never appear in a skill. Anything
/// containing a binary like this is a critical finding regardless
/// of whether the rest of the directory looks clean.
const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "msi", "dmg", "deb", "rpm", "apk",
    "ipa", "bin",
];

/// Specific structural issues, used in `Finding.rule` to identify
/// which check fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralIssue {
    TooManyFiles,
    OversizedSkill,
    OversizedFile,
    BinaryFile,
    SymlinkEscape,
    BrokenSymlink,
    UnexpectedExecutable,
}

impl StructuralIssue {
    pub fn rule_name(self) -> &'static str {
        match self {
            StructuralIssue::TooManyFiles => "struct_too_many_files",
            StructuralIssue::OversizedSkill => "struct_oversized_skill",
            StructuralIssue::OversizedFile => "struct_oversized_file",
            StructuralIssue::BinaryFile => "struct_binary_file",
            StructuralIssue::SymlinkEscape => "struct_symlink_escape",
            StructuralIssue::BrokenSymlink => "struct_broken_symlink",
            StructuralIssue::UnexpectedExecutable => "struct_unexpected_executable",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            StructuralIssue::TooManyFiles => Severity::Medium,
            StructuralIssue::OversizedSkill => Severity::High,
            StructuralIssue::OversizedFile => Severity::Medium,
            StructuralIssue::BinaryFile => Severity::Critical,
            StructuralIssue::SymlinkEscape => Severity::Critical,
            StructuralIssue::BrokenSymlink => Severity::Medium,
            StructuralIssue::UnexpectedExecutable => Severity::Medium,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            StructuralIssue::TooManyFiles => "skill directory contains an unusual number of files",
            StructuralIssue::OversizedSkill => "skill directory total size exceeds the cap",
            StructuralIssue::OversizedFile => "single file exceeds the per-file cap",
            StructuralIssue::BinaryFile => "skill directory contains a binary executable",
            StructuralIssue::SymlinkEscape => "symlink points outside the skill directory",
            StructuralIssue::BrokenSymlink => "broken symlink inside the skill directory",
            StructuralIssue::UnexpectedExecutable => {
                "non-script file has the executable bit set"
            }
        }
    }
}

/// Walk the skill directory and emit findings for every structural
/// issue. Returns an empty vec when the directory is missing — the
/// regex pass will handle the in-memory content separately.
pub fn scan_dir(skill_dir: &Path, config: &GuardConfig) -> Vec<Finding> {
    if !skill_dir.is_dir() {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let mut file_count = 0usize;
    let mut total_size: u64 = 0;
    walk(skill_dir, skill_dir, &mut |entry| {
        let path = entry.path();
        match entry.kind {
            EntryKind::File { size, executable } => {
                file_count += 1;
                total_size = total_size.saturating_add(size);
                if size > MAX_INDIVIDUAL_FILE {
                    push_struct(
                        &mut findings,
                        config,
                        StructuralIssue::OversizedFile,
                        &path,
                        format!("{} bytes (cap {})", size, MAX_INDIVIDUAL_FILE),
                    );
                }
                if let Some(ext) = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                {
                    if BINARY_EXTENSIONS.contains(&ext.as_str()) {
                        push_struct(
                            &mut findings,
                            config,
                            StructuralIssue::BinaryFile,
                            &path,
                            format!(".{} extension", ext),
                        );
                    }
                }
                if executable && !is_under_scripts(skill_dir, &path) {
                    push_struct(
                        &mut findings,
                        config,
                        StructuralIssue::UnexpectedExecutable,
                        &path,
                        String::new(),
                    );
                }
            }
            EntryKind::SymlinkOutside => {
                push_struct(
                    &mut findings,
                    config,
                    StructuralIssue::SymlinkEscape,
                    &path,
                    String::new(),
                );
            }
            EntryKind::SymlinkBroken => {
                push_struct(
                    &mut findings,
                    config,
                    StructuralIssue::BrokenSymlink,
                    &path,
                    String::new(),
                );
            }
        }
    });

    if file_count > MAX_FILES {
        push_struct(
            &mut findings,
            config,
            StructuralIssue::TooManyFiles,
            skill_dir,
            format!("{} files (limit {})", file_count, MAX_FILES),
        );
    }
    if total_size > MAX_TOTAL_SIZE {
        push_struct(
            &mut findings,
            config,
            StructuralIssue::OversizedSkill,
            skill_dir,
            format!("{} bytes (cap {})", total_size, MAX_TOTAL_SIZE),
        );
    }
    findings
}

fn push_struct(
    out: &mut Vec<Finding>,
    config: &GuardConfig,
    issue: StructuralIssue,
    path: &Path,
    snippet: String,
) {
    let rule_name = issue.rule_name();
    if config.disabled_rules.iter().any(|r| r == rule_name) {
        return;
    }
    out.push(Finding {
        category: Category::PathTraversal,
        severity: issue.severity(),
        rule: rule_name.to_string(),
        description: issue.description().to_string(),
        location: Location {
            path: PathBuf::from(path),
            line: None,
        },
        snippet: super::sanitize_snippet(&snippet),
    });
}

fn is_under_scripts(skill_root: &Path, file: &Path) -> bool {
    let scripts = skill_root.join("scripts");
    file.starts_with(&scripts)
}

struct WalkedEntry {
    path: PathBuf,
    kind: EntryKind,
}

impl WalkedEntry {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

enum EntryKind {
    File { size: u64, executable: bool },
    SymlinkOutside,
    SymlinkBroken,
}

fn walk(root: &Path, dir: &Path, on_entry: &mut dyn FnMut(WalkedEntry)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let lstat = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = lstat.file_type();
        if ft.is_symlink() {
            // Resolve and check escape.
            match std::fs::read_link(&p) {
                Ok(target) => {
                    let absolute = if target.is_absolute() {
                        target
                    } else {
                        p.parent()
                            .map(|parent| parent.join(&target))
                            .unwrap_or(target)
                    };
                    let canonical_root = root
                        .canonicalize()
                        .unwrap_or_else(|_| root.to_path_buf());
                    let canonical_target = absolute
                        .canonicalize()
                        .unwrap_or(absolute.clone());
                    if !canonical_target.starts_with(&canonical_root) {
                        on_entry(WalkedEntry {
                            path: p.clone(),
                            kind: EntryKind::SymlinkOutside,
                        });
                    } else if !canonical_target.exists() {
                        on_entry(WalkedEntry {
                            path: p.clone(),
                            kind: EntryKind::SymlinkBroken,
                        });
                    }
                }
                Err(_) => {
                    on_entry(WalkedEntry {
                        path: p.clone(),
                        kind: EntryKind::SymlinkBroken,
                    });
                }
            }
            continue;
        }
        if ft.is_dir() {
            walk(root, &p, on_entry);
            continue;
        }
        if ft.is_file() {
            let executable = is_executable(&lstat);
            on_entry(WalkedEntry {
                path: p.clone(),
                kind: EntryKind::File {
                    size: lstat.len(),
                    executable,
                },
            });
        }
    }
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    mode & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_dir_yields_no_findings() {
        let tmp = TempDir::new().unwrap();
        assert!(scan_dir(tmp.path(), &GuardConfig::default()).is_empty());
    }

    #[test]
    fn missing_dir_yields_no_findings() {
        assert!(scan_dir(Path::new("/nonexistent/path"), &GuardConfig::default()).is_empty());
    }

    #[test]
    fn single_skill_md_is_clean() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: x\ndescription: y\n---\nbody\n",
        )
        .unwrap();
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(f.is_empty(), "got: {:?}", f);
    }

    #[test]
    fn binary_extension_flagged() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("payload.exe"), b"MZ\x90\x00").unwrap();
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(
            f.iter().any(|x| x.rule == "struct_binary_file"),
            "expected binary-file finding, got {:?}",
            f
        );
        assert_eq!(
            f.iter()
                .find(|x| x.rule == "struct_binary_file")
                .unwrap()
                .severity,
            Severity::Critical
        );
    }

    #[test]
    fn oversized_file_flagged() {
        let tmp = TempDir::new().unwrap();
        let big = vec![b'a'; (MAX_INDIVIDUAL_FILE + 1) as usize];
        std::fs::write(tmp.path().join("big.md"), &big).unwrap();
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(f.iter().any(|x| x.rule == "struct_oversized_file"));
    }

    #[cfg(unix)]
    #[test]
    fn unexpected_executable_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("note.md");
        std::fs::write(&p, "body").unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(f.iter().any(|x| x.rule == "struct_unexpected_executable"));
    }

    #[cfg(unix)]
    #[test]
    fn executable_under_scripts_is_allowed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("scripts")).unwrap();
        let p = tmp.path().join("scripts").join("run.sh");
        std::fs::write(&p, "#!/bin/sh\necho hi\n").unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(
            !f.iter().any(|x| x.rule == "struct_unexpected_executable"),
            "scripts/ exec is fine, got {:?}",
            f
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_outside_skill_flagged() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside.md");
        std::fs::write(&outside, "secret").unwrap();
        let skill_root = tmp.path().join("skill");
        std::fs::create_dir(&skill_root).unwrap();
        std::os::unix::fs::symlink(&outside, skill_root.join("leak")).unwrap();

        let f = scan_dir(&skill_root, &GuardConfig::default());
        assert!(
            f.iter().any(|x| x.rule == "struct_symlink_escape"),
            "got {:?}",
            f
        );
    }

    #[test]
    fn too_many_files_flagged() {
        let tmp = TempDir::new().unwrap();
        for i in 0..(MAX_FILES + 5) {
            std::fs::write(tmp.path().join(format!("f{}.md", i)), "x").unwrap();
        }
        let f = scan_dir(tmp.path(), &GuardConfig::default());
        assert!(f.iter().any(|x| x.rule == "struct_too_many_files"));
    }

    #[test]
    fn disabled_rule_skips_only_that_rule() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("x.exe"), b"MZ").unwrap();
        std::fs::write(
            tmp.path().join("big.md"),
            vec![b'a'; (MAX_INDIVIDUAL_FILE + 1) as usize],
        )
        .unwrap();
        let cfg = GuardConfig {
            disabled_rules: vec!["struct_binary_file".into()],
            ..Default::default()
        };
        let f = scan_dir(tmp.path(), &cfg);
        let rules: Vec<_> = f.iter().map(|x| x.rule.as_str()).collect();
        assert!(!rules.contains(&"struct_binary_file"));
        assert!(rules.contains(&"struct_oversized_file"));
    }
}
