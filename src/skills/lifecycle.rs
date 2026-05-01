//! Pure lifecycle transitions for agent-created skills.
//!
//! This is the no-LLM half of the curator. Given the current
//! collection of loaded skills and the usage sidecar, it decides
//! which skills move between `active`, `stale`, and `archived` based
//! on age. The LLM consolidation pass (in the curator module) runs
//! *after* these transitions so it sees a clean view of recently-
//! relevant skills.
//!
//! Rules:
//!
//!   - Bundled and hub-installed skills are never transitioned.
//!   - Pinned skills are never transitioned.
//!   - The staleness anchor is `last_used_at`, falling back to
//!     `created_at` when the skill has never been used.
//!   - If anchor is older than `archive_after_days`: move to `archived`
//!     and physically relocate the skill to `<root>/.archive/`.
//!   - Else if older than `stale_after_days`: move to `stale`.
//!   - Else if currently `stale` but anchor is recent: move back to
//!     `active`.
//!
//! Counts of every transition are returned so the curator can build a
//! report of what happened.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use super::archive;
use super::format::{SkillProvenance, SkillState};
use super::loader::Skill;
use super::usage::UsageStore;

/// Knobs for the lifecycle scheduler. Comes from the `[curator]`
/// section of `config.toml`. Defaults match the upstream reference.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleConfig {
    /// Days of inactivity before a skill is marked `stale`. Default 30.
    pub stale_after_days: u32,
    /// Days of inactivity before a skill is `archived` (moved to
    /// `.archive/`). Default 90. Must be `>= stale_after_days`; if it
    /// isn't, we silently widen `archive_after_days = stale_after_days`
    /// to keep the state machine consistent.
    pub archive_after_days: u32,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            stale_after_days: 30,
            archive_after_days: 90,
        }
    }
}

/// Outcome of one scheduler run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransitionCounts {
    /// Number of skills the scheduler considered (agent-created,
    /// non-pinned). Bundled/hub-installed/pinned skills are not counted.
    pub checked: usize,
    /// Skills that moved Active → Stale this run.
    pub marked_stale: usize,
    /// Skills that moved (any state) → Archived this run.
    pub archived: usize,
    /// Skills that were `Stale` but became `Active` again because the
    /// anchor moved into the recent window (e.g. the agent used the
    /// skill since the last scheduler run).
    pub reactivated: usize,
    /// Names of every archived skill, in the order processed. The
    /// curator's run report quotes this list.
    pub archived_names: Vec<String>,
}

/// Apply automatic transitions in-place against the usage store and
/// the on-disk skill collection.
///
/// `now` is parameterized so tests can pin the clock. In production,
/// pass `Utc::now()`.
pub fn apply_automatic_transitions(
    skills_root: &Path,
    skills: &[Skill],
    usage: &UsageStore,
    config: LifecycleConfig,
    now: DateTime<Utc>,
) -> TransitionCounts {
    let mut counts = TransitionCounts::default();

    let stale_days = config.stale_after_days as i64;
    let archive_days = config
        .archive_after_days
        .max(config.stale_after_days) as i64;
    let stale_cutoff = now - Duration::days(stale_days);
    let archive_cutoff = now - Duration::days(archive_days);

    for skill in skills {
        if skill.provenance != SkillProvenance::AgentCreated {
            continue;
        }
        if skill.pinned {
            continue;
        }
        counts.checked += 1;

        let record = usage.get(&skill.name);
        let anchor = record
            .as_ref()
            .map(|r| r.staleness_anchor())
            .unwrap_or(now);
        let current_state = record
            .as_ref()
            .map(|r| r.state)
            .unwrap_or(SkillState::Active);

        // archive_after_days takes precedence: if a skill is well past
        // the archive cutoff it skips a stale step and goes straight in.
        if anchor <= archive_cutoff {
            if let Some(layout) = skill.layout.as_ref() {
                match archive::archive(skills_root, &skill.name, layout) {
                    Ok(_) => {
                        usage.set_state(&skill.name, SkillState::Archived);
                        counts.archived += 1;
                        counts.archived_names.push(skill.name.clone());
                    }
                    Err(e) => {
                        tracing::warn!(
                            name = %skill.name,
                            error = %e,
                            "archive failed during lifecycle transition; leaving skill in place"
                        );
                    }
                }
            } else {
                tracing::debug!(
                    name = %skill.name,
                    "skipping archive: skill has no on-disk layout (in-memory only)"
                );
            }
            continue;
        }

        if anchor <= stale_cutoff {
            if current_state != SkillState::Stale {
                usage.set_state(&skill.name, SkillState::Stale);
                counts.marked_stale += 1;
            }
            continue;
        }

        // anchor is recent — reactivate if previously stale.
        if current_state == SkillState::Stale {
            usage.set_state(&skill.name, SkillState::Active);
            counts.reactivated += 1;
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::format::SkillLayout;
    use tempfile::TempDir;

    fn skill(name: &str, _root: &Path, layout: SkillLayout, provenance: SkillProvenance) -> Skill {
        Skill {
            name: name.to_string(),
            description: "x".into(),
            content: "y".into(),
            always: false,
            requirements: vec![],
            layout: Some(layout),
            provenance,
            state: SkillState::Active,
            pinned: false,
            ..Default::default()
        }
    }

    fn write_flat(root: &Path, name: &str) -> SkillLayout {
        let p = root.join(format!("{}.md", name));
        std::fs::write(
            &p,
            format!("---\nname: {}\ndescription: x\n---\nbody\n", name),
        )
        .unwrap();
        SkillLayout::Flat { file: p }
    }

    /// Tests use real `Utc::now()` for record stamping (since the
    /// usage store always calls `Utc::now()` internally) and offset
    /// the scheduler's "now" relative to it.
    fn t_now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn no_transitions_when_no_skills() {
        let tmp = TempDir::new().unwrap();
        let usage = UsageStore::open(tmp.path());
        let counts =
            apply_automatic_transitions(tmp.path(), &[], &usage, LifecycleConfig::default(), t_now());
        assert_eq!(counts, TransitionCounts::default());
    }

    #[test]
    fn recent_skill_stays_active() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");

        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            t_now(),
        );
        assert_eq!(counts.checked, 1);
        assert_eq!(counts.marked_stale, 0);
        assert_eq!(counts.archived, 0);
    }

    #[test]
    fn old_skill_marked_stale() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");

        // Pretend the scheduler is running 45 days after the bump.
        let later = t_now() + Duration::days(45);
        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            later,
        );
        assert_eq!(counts.checked, 1);
        assert_eq!(counts.marked_stale, 1);
        assert_eq!(counts.archived, 0);
        assert_eq!(usage.get("foo").unwrap().state, SkillState::Stale);
    }

    #[test]
    fn very_old_skill_archived() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");

        let later = t_now() + Duration::days(120);
        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            later,
        );
        assert_eq!(counts.archived, 1);
        assert_eq!(counts.archived_names, vec!["foo"]);
        assert_eq!(usage.get("foo").unwrap().state, SkillState::Archived);
        assert!(tmp.path().join(".archive").join("foo").is_dir());
        assert!(!tmp.path().join("foo.md").exists());
    }

    #[test]
    fn pinned_skill_never_transitions() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let mut s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        s.pinned = true;
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");
        usage.set_pinned("foo", true);

        let later = t_now() + Duration::days(120);
        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            later,
        );
        assert_eq!(counts.checked, 0);
        assert_eq!(counts.archived, 0);
        assert!(tmp.path().join("foo.md").exists());
    }

    #[test]
    fn bundled_skill_never_transitions() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::Bundled);
        let usage = UsageStore::open(tmp.path());

        let later = t_now() + Duration::days(120);
        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            later,
        );
        assert_eq!(counts.checked, 0);
        assert_eq!(counts.archived, 0);
        assert!(tmp.path().join("foo.md").exists());
    }

    #[test]
    fn stale_skill_reactivates_when_used_recently() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");
        usage.set_state("foo", SkillState::Stale);
        // Bump again — anchor refreshes to ~now, before stale cutoff.
        usage.bump_use("foo");

        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            t_now(),
        );
        assert_eq!(counts.reactivated, 1);
        assert_eq!(usage.get("foo").unwrap().state, SkillState::Active);
    }

    #[test]
    fn archive_widens_when_smaller_than_stale() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        usage.bump_use("foo");

        // Misconfigured: archive_after_days < stale_after_days. Without
        // widening, a 15-day-old skill (past raw archive=10) would
        // archive prematurely. The scheduler must widen archive to
        // match stale (=30) so a 15-day skill stays Active.
        let cfg = LifecycleConfig {
            stale_after_days: 30,
            archive_after_days: 10,
        };
        let later = t_now() + Duration::days(15);
        let counts = apply_automatic_transitions(tmp.path(), &[s], &usage, cfg, later);
        assert_eq!(counts.archived, 0, "widened cutoff must prevent premature archive");
        assert_eq!(counts.marked_stale, 0, "skill is not stale at 15 days under stale=30");
    }

    #[test]
    fn never_used_skill_uses_created_at_anchor() {
        let tmp = TempDir::new().unwrap();
        let l = write_flat(tmp.path(), "foo");
        let s = skill("foo", tmp.path(), l, SkillProvenance::AgentCreated);
        let usage = UsageStore::open(tmp.path());
        // Create a record without ever bumping use_count.
        usage.set_state("foo", SkillState::Active);

        // Long after creation: the anchor is `created_at`, well past
        // the archive cutoff, so the skill archives.
        let later = t_now() + Duration::days(120);
        let counts = apply_automatic_transitions(
            tmp.path(),
            &[s],
            &usage,
            LifecycleConfig::default(),
            later,
        );
        assert_eq!(counts.checked, 1);
        assert_eq!(counts.archived, 1);
    }
}
