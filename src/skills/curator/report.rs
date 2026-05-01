//! Run-report writer: `run.json` (machine-readable) and
//! `REPORT.md` (human-readable) under
//! `<logs_root>/curator/<YYYYMMDD-HHMMSS>/`.
//!
//! Reports are immutable: each run gets its own directory; nothing
//! gets overwritten. If two runs land in the same second (rare but
//! possible during testing), the second appends a `-2` suffix.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::skills::lifecycle::TransitionCounts;

use super::tool_loop::ToolLoopOutcome;

/// Aggregated record of a single run. Serialized verbatim into
/// `run.json` and rendered into `REPORT.md`.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub started_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub auto_transitions: TransitionCounts,
    pub llm_outcome: Option<ToolLoopOutcome>,
}

// `ToolLoopOutcome` doesn't derive Serialize on its own (it owns
// `RecordedToolCall`s which DO serialize); wrap it in a serializer.
impl Serialize for ToolLoopOutcome {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = ser.serialize_struct("ToolLoopOutcome", 4)?;
        state.serialize_field("final_summary", &self.final_summary)?;
        state.serialize_field("tool_calls", &self.tool_calls)?;
        state.serialize_field("iterations", &self.iterations)?;
        state.serialize_field("hit_iteration_cap", &self.hit_iteration_cap)?;
        state.end()
    }
}

/// Write the report. Returns the directory where it landed.
pub fn write_report(logs_root: &Path, report: &RunReport) -> Result<PathBuf> {
    let dir = pick_report_dir(logs_root, report.started_at)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating report dir {}", dir.display()))?;

    // run.json
    let json = serde_json::to_vec_pretty(report)
        .context("serializing run.json")?;
    std::fs::write(dir.join("run.json"), json)
        .with_context(|| format!("writing {}/run.json", dir.display()))?;

    // REPORT.md
    let md = render_markdown(report);
    std::fs::write(dir.join("REPORT.md"), md)
        .with_context(|| format!("writing {}/REPORT.md", dir.display()))?;

    Ok(dir)
}

/// Pick a unique directory under `<logs_root>/curator/`. Default
/// path is `<ts>` (e.g. `20260501-153000`); if it already exists
/// (concurrent test runs), append `-2`, `-3`, etc.
fn pick_report_dir(logs_root: &Path, ts: DateTime<Utc>) -> Result<PathBuf> {
    let curator_dir = logs_root.join("curator");
    let base = curator_dir.join(ts.format("%Y%m%d-%H%M%S").to_string());
    if !base.exists() {
        return Ok(base);
    }
    for suffix in 2..1000 {
        let candidate = curator_dir.join(format!(
            "{}-{}",
            ts.format("%Y%m%d-%H%M%S"),
            suffix
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "could not find unique report dir under {}",
        curator_dir.display()
    );
}

fn render_markdown(report: &RunReport) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# Curator run {}\n\n",
        report.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str(&format!(
        "Duration: {:.2}s\n\n",
        report.duration_seconds
    ));
    s.push_str("## Auto-transitions\n\n");
    let a = &report.auto_transitions;
    s.push_str(&format!("- checked: {}\n", a.checked));
    s.push_str(&format!("- marked stale: {}\n", a.marked_stale));
    s.push_str(&format!("- archived: {}\n", a.archived));
    s.push_str(&format!("- reactivated: {}\n", a.reactivated));
    if !a.archived_names.is_empty() {
        s.push_str("\n### Archived this run\n\n");
        for name in &a.archived_names {
            s.push_str(&format!("- {}\n", name));
        }
    }

    if let Some(llm) = &report.llm_outcome {
        s.push_str("\n## LLM consolidation pass\n\n");
        s.push_str(&format!("- iterations: {}\n", llm.iterations));
        s.push_str(&format!("- tool calls: {}\n", llm.tool_calls.len()));
        if llm.hit_iteration_cap {
            s.push_str("- **hit iteration cap**\n");
        }

        // Per-tool tally.
        let mut tally: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for tc in &llm.tool_calls {
            *tally.entry(tc.name.as_str()).or_insert(0) += 1;
        }
        if !tally.is_empty() {
            s.push_str("\n### Tool calls\n\n");
            for (name, count) in tally {
                s.push_str(&format!("- {}: {}\n", name, count));
            }
        }

        s.push_str("\n### Summary\n\n");
        s.push_str(&llm.final_summary);
        s.push('\n');
    } else {
        s.push_str("\n## LLM consolidation pass\n\n");
        s.push_str("Skipped — no auxiliary client configured.\n");
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::curator::tool_loop::RecordedToolCall;
    use tempfile::TempDir;

    fn empty_counts() -> TransitionCounts {
        TransitionCounts::default()
    }

    #[test]
    fn writes_run_json_and_report_md() {
        let tmp = TempDir::new().unwrap();
        let report = RunReport {
            started_at: Utc::now(),
            duration_seconds: 1.5,
            auto_transitions: empty_counts(),
            llm_outcome: None,
        };
        let dir = write_report(tmp.path(), &report).unwrap();
        assert!(dir.join("run.json").is_file());
        assert!(dir.join("REPORT.md").is_file());
    }

    #[test]
    fn run_json_round_trips() {
        let tmp = TempDir::new().unwrap();
        let report = RunReport {
            started_at: Utc::now(),
            duration_seconds: 2.0,
            auto_transitions: TransitionCounts {
                checked: 5,
                marked_stale: 1,
                archived: 1,
                reactivated: 0,
                archived_names: vec!["foo".into()],
            },
            llm_outcome: Some(ToolLoopOutcome {
                final_summary: "Reviewed 5 skills.".into(),
                tool_calls: vec![RecordedToolCall {
                    name: "skills_list".into(),
                    arguments: serde_json::json!({}),
                    success: true,
                    result: "{count: 5}".into(),
                }],
                iterations: 2,
                hit_iteration_cap: false,
            }),
        };
        let dir = write_report(tmp.path(), &report).unwrap();
        let raw = std::fs::read_to_string(dir.join("run.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["auto_transitions"]["archived"], 1);
        assert_eq!(v["llm_outcome"]["iterations"], 2);
        assert_eq!(v["llm_outcome"]["tool_calls"][0]["name"], "skills_list");
    }

    #[test]
    fn markdown_includes_archived_names_and_summary() {
        let tmp = TempDir::new().unwrap();
        let report = RunReport {
            started_at: Utc::now(),
            duration_seconds: 1.0,
            auto_transitions: TransitionCounts {
                checked: 3,
                marked_stale: 0,
                archived: 1,
                reactivated: 0,
                archived_names: vec!["lonely-skill".into()],
            },
            llm_outcome: Some(ToolLoopOutcome {
                final_summary: "Merged docker-build into docker.".into(),
                tool_calls: vec![],
                iterations: 1,
                hit_iteration_cap: false,
            }),
        };
        let dir = write_report(tmp.path(), &report).unwrap();
        let md = std::fs::read_to_string(dir.join("REPORT.md")).unwrap();
        assert!(md.contains("Curator run"));
        assert!(md.contains("lonely-skill"));
        assert!(md.contains("Merged docker-build"));
    }

    #[test]
    fn dir_collision_appends_suffix() {
        let tmp = TempDir::new().unwrap();
        let ts = Utc::now();
        let report = RunReport {
            started_at: ts,
            duration_seconds: 0.0,
            auto_transitions: empty_counts(),
            llm_outcome: None,
        };
        let d1 = write_report(tmp.path(), &report).unwrap();
        let d2 = write_report(tmp.path(), &report).unwrap();
        assert_ne!(d1, d2);
        assert!(d2.to_string_lossy().contains("-2"));
    }

    #[test]
    fn no_llm_renders_skipped_block() {
        let tmp = TempDir::new().unwrap();
        let report = RunReport {
            started_at: Utc::now(),
            duration_seconds: 0.5,
            auto_transitions: empty_counts(),
            llm_outcome: None,
        };
        let dir = write_report(tmp.path(), &report).unwrap();
        let md = std::fs::read_to_string(dir.join("REPORT.md")).unwrap();
        assert!(md.contains("Skipped"));
    }

    #[test]
    fn hit_cap_renders_warning() {
        let tmp = TempDir::new().unwrap();
        let report = RunReport {
            started_at: Utc::now(),
            duration_seconds: 60.0,
            auto_transitions: empty_counts(),
            llm_outcome: Some(ToolLoopOutcome {
                final_summary: "ran out".into(),
                tool_calls: vec![],
                iterations: 30,
                hit_iteration_cap: true,
            }),
        };
        let dir = write_report(tmp.path(), &report).unwrap();
        let md = std::fs::read_to_string(dir.join("REPORT.md")).unwrap();
        assert!(md.contains("hit iteration cap"));
    }
}
