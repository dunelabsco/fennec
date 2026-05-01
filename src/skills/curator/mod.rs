//! Skill curator — periodic consolidation of agent-created skills.
//!
//! See [`runner`] for the orchestration entry point and
//! [`scheduler`] for the idle-gate logic. The CLI surface
//! (`fennec curator status/run/pause/resume/pin/unpin/restore`) is
//! wired in `src/main.rs`.

pub mod prompt;
pub mod report;
pub mod runner;
pub mod scheduler;
pub mod state;
pub mod tool_loop;
pub mod tools;

pub use prompt::CURATOR_SYSTEM_PROMPT;
pub use report::{RunReport, write_report};
pub use runner::{RunContext, RunSummary, run_curator};
pub use scheduler::{
    AutoRunDecision, CuratorScheduleConfig, SkipReason, should_auto_run,
};
pub use state::{CuratorState, CuratorStateStore};
pub use tool_loop::{RecordedToolCall, ToolLoopConfig, ToolLoopOutcome};
pub use tools::SkillsListTool;
