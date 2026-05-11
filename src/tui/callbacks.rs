//! Bridge from `AgentCallbacks` to the TUI's app state.
//!
//! The agent loop runs on its own task; the TUI renderer reads
//! from `App` on the main thread. To bridge them, the agent's
//! callbacks push events into a channel; the TUI's tick handler
//! drains the channel and applies the events to `App`.
//!
//! The wiring of this bridge into the agent loop happens in a
//! follow-up commit. This file holds the type so the rest of the
//! TUI can compile and reason about it.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::agent::callbacks::{
    AgentCallbacks, ApprovalRequest, ClarifyRequest, SecretRequest, SubagentComplete,
    SubagentSpawn, ToolComplete, ToolProgress, ToolStart,
};

use super::app::App;

/// Events emitted by the agent that the TUI's event loop drains
/// into `App` mutations.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolStart(ToolStart),
    ToolProgress(ToolProgress),
    ToolComplete(ToolComplete),
    Status(String),
    TurnStart(String),
    TurnComplete(String),
    /// New sub-agent spawned via the `delegate` tool. Carries the
    /// id, optional parent_id (for nested spawns), and the goal
    /// the parent passed.
    SubagentSpawn(SubagentSpawn),
    /// Sub-agent's main loop began — separate from spawn so the
    /// overlay can show the queued → running transition.
    SubagentStart(String),
    /// Streaming text delta from a sub-agent.
    SubagentText { id: String, delta: String },
    /// Streaming reasoning delta from a sub-agent.
    SubagentThinking { id: String, delta: String },
    /// A tool started inside a sub-agent.
    SubagentTool { id: String, start: ToolStart },
    /// Free-text progress note from a sub-agent.
    SubagentProgress { id: String, note: String },
    /// Sub-agent finished — final event for that branch.
    SubagentComplete(SubagentComplete),
}

/// Sender side of the bridge — held by the `AgentCallbacks` impl.
pub type EventSender = mpsc::UnboundedSender<TuiEvent>;
/// Receiver side — owned by the TUI's drain loop.
pub type EventReceiver = mpsc::UnboundedReceiver<TuiEvent>;

/// `AgentCallbacks` implementation that just forwards every event
/// onto a channel. Non-blocking — even the human-in-the-loop
/// prompts default to the trait's "auto-approve / cancel"
/// behavior for now (modal handling lands in F1-2).
pub struct TuiBridge {
    tx: EventSender,
    /// Held so the bridge can opt to mutate app state directly
    /// for events that don't need to round-trip the channel
    /// (e.g. status updates that should land instantly).
    pub app: Arc<Mutex<App>>,
}

impl TuiBridge {
    pub fn new(app: Arc<Mutex<App>>) -> (Self, EventReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, app }, rx)
    }
}

impl AgentCallbacks for TuiBridge {
    fn on_text_delta(&self, delta: &str) {
        let _ = self.tx.send(TuiEvent::TextDelta(delta.to_string()));
    }

    fn on_reasoning_delta(&self, delta: &str) {
        let _ = self.tx.send(TuiEvent::ReasoningDelta(delta.to_string()));
    }

    fn on_tool_start(&self, start: ToolStart) {
        let _ = self.tx.send(TuiEvent::ToolStart(start));
    }

    fn on_tool_progress(&self, progress: ToolProgress) {
        let _ = self.tx.send(TuiEvent::ToolProgress(progress));
    }

    fn on_tool_complete(&self, complete: ToolComplete) {
        let _ = self.tx.send(TuiEvent::ToolComplete(complete));
    }

    fn on_status(&self, message: &str) {
        let _ = self.tx.send(TuiEvent::Status(message.to_string()));
    }

    fn on_turn_start(&self, prompt: &str) {
        let _ = self.tx.send(TuiEvent::TurnStart(prompt.to_string()));
    }

    fn on_turn_complete(&self, summary: &str) {
        let _ = self.tx.send(TuiEvent::TurnComplete(summary.to_string()));
    }

    fn on_approval_request(&self, _request: ApprovalRequest) -> bool {
        // F1-2: render a modal and block until the user resolves
        // it. For now, deny by default — safer than auto-approve
        // when the TUI is the active frontend.
        false
    }

    fn on_clarify_request(&self, _request: ClarifyRequest) -> Option<String> {
        None
    }

    fn on_secret_request(&self, _request: SecretRequest) -> Option<String> {
        None
    }

    fn on_subagent_spawn(&self, spawn: SubagentSpawn) {
        let _ = self.tx.send(TuiEvent::SubagentSpawn(spawn));
    }

    fn on_subagent_start(&self, id: &str) {
        let _ = self.tx.send(TuiEvent::SubagentStart(id.to_string()));
    }

    fn on_subagent_text(&self, id: &str, delta: &str) {
        let _ = self.tx.send(TuiEvent::SubagentText {
            id: id.to_string(),
            delta: delta.to_string(),
        });
    }

    fn on_subagent_thinking(&self, id: &str, delta: &str) {
        let _ = self.tx.send(TuiEvent::SubagentThinking {
            id: id.to_string(),
            delta: delta.to_string(),
        });
    }

    fn on_subagent_tool(&self, id: &str, start: ToolStart) {
        let _ = self.tx.send(TuiEvent::SubagentTool {
            id: id.to_string(),
            start,
        });
    }

    fn on_subagent_progress(&self, id: &str, note: &str) {
        let _ = self.tx.send(TuiEvent::SubagentProgress {
            id: id.to_string(),
            note: note.to_string(),
        });
    }

    fn on_subagent_complete(&self, complete: SubagentComplete) {
        let _ = self.tx.send(TuiEvent::SubagentComplete(complete));
    }
}
