//! Top-level orchestration for one curator run.
//!
//! A run is two phases:
//!
//!   1. **Auto-transitions** (no LLM): walk every agent-created
//!      skill, move stale ones to `Stale`, archive ones past the
//!      archive cutoff, reactivate any `Stale` skills that have been
//!      used recently.
//!   2. **LLM consolidation** (optional): if the auxiliary client
//!      has a `Curator` task configured, run the tool loop. The
//!      sub-conversation can call `skills_list`, `skill_view`
//!      (`load_skill`), and `skill_manage`. When it produces a
//!      final summary, we capture the full tool-call trace into the
//!      run report.
//!
//! The result is written to
//! `<home>/logs/curator/<YYYYMMDD-HHMMSS>/{run.json, REPORT.md}` and
//! the state file is updated with the new `last_run_at`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::providers::AuxiliaryClient;
use crate::skills::{
    BundledManifest, HubLock, SkillsLoader, UsageStore, lifecycle, manage::ManageOutcome,
};
use crate::tools::{SkillManageTool, SkillsTool, traits::Tool};

use super::prompt::CURATOR_SYSTEM_PROMPT;
use super::report::{RunReport, write_report};
use super::state::CuratorStateStore;
use super::tool_loop::{ToolLoopConfig, ToolLoopOutcome, run as run_tool_loop};
use super::tools::SkillsListTool;

/// Inputs to a single curator run.
pub struct RunContext {
    /// Where the user's skills live (`<home>/skills/`).
    pub skills_root: PathBuf,
    /// Where to write per-run reports (`<home>/logs/curator/`).
    pub logs_root: PathBuf,
    /// Usage sidecar — used by auto-transitions and the runner's
    /// audit summary.
    pub usage: Arc<UsageStore>,
    /// State file holder. The runner writes to it at the end of the
    /// run.
    pub state: Arc<CuratorStateStore>,
    /// Lifecycle thresholds (stale/archive days). Defaults are
    /// 30/90.
    pub lifecycle: lifecycle::LifecycleConfig,
    /// When `Some`, the runner attempts the LLM consolidation pass.
    /// When `None`, only the auto-transition phase runs.
    pub aux: Option<Arc<AuxiliaryClient>>,
    /// Tool-loop tunables. Used only when `aux` is `Some`.
    pub tool_loop_config: ToolLoopConfig,
    /// Initial user message handed to the LLM. The system prompt
    /// always comes from [`CURATOR_SYSTEM_PROMPT`]; this is the
    /// per-run kickoff. Default: "Begin curator review."
    pub initial_user_message: String,
}

impl RunContext {
    /// Lightweight constructor for callers that already have the
    /// stores ready and want defaults for everything else.
    pub fn new(
        skills_root: PathBuf,
        logs_root: PathBuf,
        usage: Arc<UsageStore>,
        state: Arc<CuratorStateStore>,
    ) -> Self {
        Self {
            skills_root,
            logs_root,
            usage,
            state,
            lifecycle: lifecycle::LifecycleConfig::default(),
            aux: None,
            tool_loop_config: ToolLoopConfig::default(),
            initial_user_message: "Begin curator review.".into(),
        }
    }
}

/// Outcome handed back to the CLI / status command.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub started_at: chrono::DateTime<Utc>,
    pub duration_seconds: f64,
    pub auto_transitions: lifecycle::TransitionCounts,
    pub llm_outcome: Option<ToolLoopOutcome>,
    pub report_dir: Option<PathBuf>,
    pub one_line_summary: String,
}

/// Run the curator end-to-end. Errors are propagated for the auto
/// phase but the LLM phase swallows its own errors and surfaces them
/// in the report (the auto phase is more important and shouldn't be
/// gated on a flaky provider).
pub async fn run_curator(ctx: &RunContext) -> Result<RunSummary> {
    let started_at = Utc::now();
    let start_instant = std::time::Instant::now();

    // ---------- Phase 1: auto-transitions ----------
    let bundled = BundledManifest::load(&ctx.skills_root);
    let hub = HubLock::load(&ctx.skills_root);
    let skills_before = SkillsLoader::load_with_provenance(
        &ctx.skills_root,
        Some(&bundled),
        Some(&hub),
        Some(&ctx.usage),
    )
    .context("loading skills for curator auto phase")?;

    let auto_counts = lifecycle::apply_automatic_transitions(
        &ctx.skills_root,
        &skills_before,
        &ctx.usage,
        ctx.lifecycle,
        Utc::now(),
    );

    // ---------- Phase 2: LLM consolidation (optional) ----------
    let llm_outcome = if let Some(aux) = ctx.aux.as_ref() {
        let tools = build_curator_tools(&ctx.skills_root, Arc::clone(&ctx.usage));
        match run_tool_loop(
            aux,
            CURATOR_SYSTEM_PROMPT,
            &ctx.initial_user_message,
            &tools,
            ctx.tool_loop_config,
        )
        .await
        {
            Ok(o) => Some(o),
            Err(e) => {
                tracing::warn!(error = %e, "curator LLM phase failed; continuing");
                Some(ToolLoopOutcome {
                    final_summary: format!("[llm phase error] {}", e),
                    tool_calls: Vec::new(),
                    iterations: 0,
                    hit_iteration_cap: false,
                })
            }
        }
    } else {
        None
    };

    let duration_seconds = start_instant.elapsed().as_secs_f64();

    // ---------- Phase 3: report ----------
    let report = RunReport {
        started_at,
        duration_seconds,
        auto_transitions: auto_counts.clone(),
        llm_outcome: llm_outcome.clone(),
    };
    let report_dir = match write_report(&ctx.logs_root, &report) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "curator report write failed");
            None
        }
    };

    // ---------- Phase 4: state update ----------
    let one_line = build_one_line_summary(&auto_counts, &llm_outcome);
    if let Err(e) = ctx
        .state
        .record_run(started_at, duration_seconds, one_line.clone(), report_dir.clone())
    {
        tracing::warn!(error = %e, "curator state update failed");
    }

    Ok(RunSummary {
        started_at,
        duration_seconds,
        auto_transitions: auto_counts,
        llm_outcome,
        report_dir,
        one_line_summary: one_line,
    })
}

/// Build the tool set the curator's LLM gets to call. `skills_list`
/// is curator-internal; `load_skill` and `skill_manage` reuse the
/// existing tool implementations.
fn build_curator_tools(
    skills_root: &Path,
    usage: Arc<UsageStore>,
) -> Vec<Arc<dyn Tool>> {
    // Pre-load skills for the SkillsTool so `load_skill` works
    // against the same snapshot the curator started with.
    let bundled = BundledManifest::load(skills_root);
    let hub = HubLock::load(skills_root);
    let skills = SkillsLoader::load_with_provenance(
        skills_root,
        Some(&bundled),
        Some(&hub),
        Some(&usage),
    )
    .unwrap_or_default();
    let view_tool: Arc<dyn Tool> = Arc::new(SkillsTool::with_usage(
        skills,
        Arc::clone(&usage),
    ));
    let manage_tool: Arc<dyn Tool> = Arc::new(SkillManageTool::new(
        skills_root.to_path_buf(),
        Arc::clone(&usage),
    ));
    let list_tool: Arc<dyn Tool> = Arc::new(SkillsListTool::new(
        skills_root.to_path_buf(),
        Arc::clone(&usage),
    ));
    vec![list_tool, view_tool, manage_tool]
}

fn build_one_line_summary(
    counts: &lifecycle::TransitionCounts,
    llm: &Option<ToolLoopOutcome>,
) -> String {
    let auto = format!(
        "auto: {} checked, {} marked stale, {} archived, {} reactivated",
        counts.checked, counts.marked_stale, counts.archived, counts.reactivated,
    );
    match llm {
        Some(o) => format!(
            "{}; llm: {} tool calls over {} iterations{}",
            auto,
            o.tool_calls.len(),
            o.iterations,
            if o.hit_iteration_cap {
                " (hit cap)"
            } else {
                ""
            },
        ),
        None => format!("{}; llm: skipped (no aux client configured)", auto),
    }
}

/// Stub used when calling delete from inside the curator runner.
/// (Currently unused — kept here to make the public surface honest;
/// the actual deletions happen through `skill_manage`.)
#[allow(dead_code)]
fn _unused_marker(_: ManageOutcome) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn auto_only_run_writes_report_and_updates_state() {
        let tmp = TempDir::new().unwrap();
        let skills_root = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();
        let logs_root = tmp.path().join("logs");

        let usage = Arc::new(UsageStore::open(&skills_root));
        let state = Arc::new(CuratorStateStore::open(&skills_root));
        let ctx = RunContext::new(
            skills_root.clone(),
            logs_root.clone(),
            Arc::clone(&usage),
            Arc::clone(&state),
        );

        let summary = run_curator(&ctx).await.unwrap();
        assert!(summary.report_dir.is_some());
        // State got recorded.
        let snap = state.snapshot();
        assert!(snap.last_run_at.is_some());
        assert!(snap.run_count >= 1);
        // Report directory exists.
        assert!(summary.report_dir.unwrap().is_dir());
    }

    #[tokio::test]
    async fn empty_collection_yields_empty_summary_no_panics() {
        let tmp = TempDir::new().unwrap();
        let skills_root = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();
        let logs_root = tmp.path().join("logs");
        let usage = Arc::new(UsageStore::open(&skills_root));
        let state = Arc::new(CuratorStateStore::open(&skills_root));
        let ctx = RunContext::new(skills_root, logs_root, Arc::clone(&usage), Arc::clone(&state));

        let s = run_curator(&ctx).await.unwrap();
        assert_eq!(s.auto_transitions.checked, 0);
        assert!(s.llm_outcome.is_none());
        assert!(s.one_line_summary.contains("auto: 0 checked"));
        assert!(s.one_line_summary.contains("skipped"));
    }

    #[tokio::test]
    async fn run_with_no_aux_skips_llm_phase() {
        let tmp = TempDir::new().unwrap();
        let skills_root = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();
        // Plant one agent-created skill.
        std::fs::write(
            skills_root.join("alpha.md"),
            "---\nname: alpha\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        let logs_root = tmp.path().join("logs");
        let usage = Arc::new(UsageStore::open(&skills_root));
        let state = Arc::new(CuratorStateStore::open(&skills_root));
        let ctx = RunContext::new(skills_root, logs_root, Arc::clone(&usage), Arc::clone(&state));

        let s = run_curator(&ctx).await.unwrap();
        assert_eq!(s.auto_transitions.checked, 1);
        assert!(s.llm_outcome.is_none());
    }
}
