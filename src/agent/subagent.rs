use std::sync::Arc;

use anyhow::Result;

use crate::memory::traits::Memory;
use crate::providers::traits::Provider;
use crate::tools::traits::Tool;

use super::callbacks::{
    AgentCallbacks, ApprovalRequest, CallbacksHandle, ClarifyRequest, SecretRequest,
    ToolComplete, ToolProgress, ToolStart,
};
use super::AgentBuilder;

/// Result of a subagent task execution.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// The final text output from the subagent.
    pub output: String,
    /// Names of tools that the subagent invoked during execution.
    pub tools_used: Vec<String>,
    /// Whether the subagent completed successfully (did not exceed iteration cap
    /// or encounter an error).
    pub success: bool,
}

/// Manages spawning isolated subagent instances that run specific tasks.
pub struct SubagentManager {
    memory: Arc<dyn Memory>,
    provider: Arc<dyn Provider>,
    /// Optional handle to the parent's callback bridge so the
    /// TUI's spawn-tree can observe sub-agent lifecycle. None =
    /// no frontend / channels mode.
    callbacks: Option<CallbacksHandle>,
    /// Subagent id of the currently-executing sub-agent (if
    /// `spawn` was called from within another sub-agent's tool
    /// dispatch). When set, freshly-spawned sub-agents record
    /// it as their `parent_id`.
    current_parent_id: Option<String>,
}

impl SubagentManager {
    /// Create a new manager that shares the given provider and memory.
    pub fn new(provider: Arc<dyn Provider>, memory: Arc<dyn Memory>) -> Self {
        Self {
            memory,
            provider,
            callbacks: None,
            current_parent_id: None,
        }
    }

    /// Construct with a parent-callback handle so each spawned
    /// sub-agent's events flow through to the TUI's spawn-tree
    /// overlay (or any other frontend listening on
    /// `AgentCallbacks`).
    pub fn with_callbacks(mut self, callbacks: CallbacksHandle) -> Self {
        self.callbacks = Some(callbacks);
        self
    }

    /// Tag spawned sub-agents with `parent_id` so nested
    /// delegations form a tree in the TUI's overlay.
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.current_parent_id = Some(parent_id.into());
        self
    }

    /// Spawn a subagent to execute the given task.
    ///
    /// The subagent is constructed with a limited tool set and a maximum
    /// iteration cap. It runs synchronously (blocks until done) and returns
    /// the result.
    pub async fn spawn(
        &self,
        task: &str,
        tools: Vec<Box<dyn Tool>>,
        max_iterations: usize,
    ) -> Result<SubagentResult> {
        let max_iterations = if max_iterations == 0 {
            10
        } else {
            max_iterations
        };

        // Track which tools were provided so we can report them.
        let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        // Generate a stable id for this sub-agent so the parent's
        // callback handle can correlate its lifecycle events.
        let subagent_id = format!("sa_{}", uuid::Uuid::new_v4().simple());

        // Fire `on_subagent_spawn` before we start the actual
        // turn so the spawn-tree overlay shows the node as
        // queued/starting from the moment the parent decided
        // to delegate.
        if let Some(ref parent_cb) = self.callbacks {
            parent_cb.on_subagent_spawn(super::callbacks::SubagentSpawn {
                id: subagent_id.clone(),
                parent_id: self.current_parent_id.clone(),
                goal: task.to_string(),
            });
        }

        // Build the sub-agent. If we have a parent callback handle,
        // wrap it so that the sub-agent's standard
        // `on_text_delta` / `on_tool_start` / etc. calls route
        // back to the parent's `on_subagent_*` methods tagged
        // with this id.
        let mut builder = AgentBuilder::new()
            .provider(Arc::clone(&self.provider))
            .memory(Arc::clone(&self.memory))
            .tools(tools)
            .max_tool_iterations(max_iterations)
            .identity_name("Fennec-Subagent")
            .identity_persona("A focused sub-agent executing a delegated task.");
        if let Some(ref parent_cb) = self.callbacks {
            let wrapper: CallbacksHandle = Arc::new(SubagentCallbacks {
                parent: Arc::clone(parent_cb),
                subagent_id: subagent_id.clone(),
            });
            builder = builder.callbacks(wrapper);
        }
        let mut agent = builder.build()?;

        // Mark the sub-agent as "started running" right before
        // we hand it the prompt. on_subagent_complete fires
        // after the turn returns regardless of outcome.
        if let Some(ref parent_cb) = self.callbacks {
            parent_cb.on_subagent_start(&subagent_id);
        }
        let started = std::time::Instant::now();
        let outcome = agent.turn(task).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let (output, success) = match outcome {
            Ok(text) => (text, true),
            Err(e) => (format!("Subagent failed: {e}"), false),
        };
        if let Some(ref parent_cb) = self.callbacks {
            parent_cb.on_subagent_complete(super::callbacks::SubagentComplete {
                id: subagent_id.clone(),
                output: output.clone(),
                success,
                duration_ms,
                tools_used: tool_names.clone(),
            });
        }

        Ok(SubagentResult {
            output,
            tools_used: tool_names,
            success,
        })
    }
}

/// `AgentCallbacks` adapter that translates a sub-agent's
/// standard lifecycle events into the parent's `on_subagent_*`
/// methods, tagged with the sub-agent's id. Suppresses the
/// `on_turn_*` methods because the parent already learns about
/// the sub-agent's start/complete via dedicated calls in
/// [`SubagentManager::spawn`]. Approval / clarify / secret
/// prompts are denied by default — sub-agents shouldn't be
/// asking the user for things directly.
struct SubagentCallbacks {
    parent: CallbacksHandle,
    subagent_id: String,
}

impl AgentCallbacks for SubagentCallbacks {
    fn on_text_delta(&self, delta: &str) {
        self.parent.on_subagent_text(&self.subagent_id, delta);
    }

    fn on_reasoning_delta(&self, delta: &str) {
        self.parent.on_subagent_thinking(&self.subagent_id, delta);
    }

    fn on_tool_start(&self, start: ToolStart) {
        self.parent.on_subagent_tool(&self.subagent_id, start);
    }

    fn on_tool_progress(&self, _progress: ToolProgress) {
        // Sub-agent tool-progress isn't surfaced as a separate
        // overlay event today. If the spawn-tree later needs
        // mid-tool progress, plumb it through here.
    }

    fn on_tool_complete(&self, _complete: ToolComplete) {
        // Tool completion for sub-agent tools isn't surfaced
        // separately — the `on_subagent_complete` event
        // includes the tools_used list at the end. Detail-pane
        // rendering treats the tool list as a snapshot, not a
        // live stream of completes.
    }

    fn on_status(&self, message: &str) {
        // Status messages from inside the sub-agent surface as
        // progress notes in the overlay's detail pane.
        self.parent.on_subagent_progress(&self.subagent_id, message);
    }

    // Suppress turn boundaries — SubagentManager handles those
    // explicitly via on_subagent_start / on_subagent_complete.
    fn on_turn_start(&self, _prompt: &str) {}
    fn on_turn_complete(&self, _summary: &str) {}

    // Sub-agents don't get user prompts. Default to deny / None.
    fn on_approval_request(&self, _request: ApprovalRequest) -> bool {
        false
    }
    fn on_clarify_request(&self, _request: ClarifyRequest) -> Option<String> {
        None
    }
    fn on_secret_request(&self, _request: SecretRequest) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn test_subagent_result_debug() {
        let result = SubagentResult {
            output: "done".to_string(),
            tools_used: vec!["read_file".to_string()],
            success: true,
        };
        let dbg = format!("{:?}", result);
        assert!(dbg.contains("done"));
        assert!(dbg.contains("read_file"));
    }

    /// Captures every subagent-scoped call the parent receives so a
    /// test can assert that [`SubagentCallbacks`] re-tagged each
    /// generic event with the correct subagent id.
    #[derive(Default)]
    struct CapturingParent {
        text: Mutex<Vec<(String, String)>>,
        thinking: Mutex<Vec<(String, String)>>,
        tool: Mutex<Vec<(String, String)>>,
        progress: Mutex<Vec<(String, String)>>,
        turn_starts: Mutex<u32>,
        turn_completes: Mutex<u32>,
    }

    impl AgentCallbacks for CapturingParent {
        fn on_subagent_text(&self, id: &str, delta: &str) {
            self.text.lock().push((id.into(), delta.into()));
        }
        fn on_subagent_thinking(&self, id: &str, delta: &str) {
            self.thinking.lock().push((id.into(), delta.into()));
        }
        fn on_subagent_tool(&self, id: &str, start: ToolStart) {
            self.tool.lock().push((id.into(), start.name));
        }
        fn on_subagent_progress(&self, id: &str, note: &str) {
            self.progress.lock().push((id.into(), note.into()));
        }
        fn on_turn_start(&self, _prompt: &str) {
            *self.turn_starts.lock() += 1;
        }
        fn on_turn_complete(&self, _summary: &str) {
            *self.turn_completes.lock() += 1;
        }
    }

    #[test]
    fn subagent_callbacks_route_standard_events_to_parent_with_id() {
        let parent = Arc::new(CapturingParent::default());
        let wrapper = SubagentCallbacks {
            parent: parent.clone() as CallbacksHandle,
            subagent_id: "sa_test".to_string(),
        };

        wrapper.on_text_delta("hello");
        wrapper.on_reasoning_delta("thinking…");
        wrapper.on_tool_start(ToolStart {
            tool_id: "t1".into(),
            name: "read_file".into(),
            preview: "Cargo.toml".into(),
        });
        wrapper.on_status("compressing context");

        let text = parent.text.lock().clone();
        assert_eq!(text, vec![("sa_test".into(), "hello".into())]);
        let thinking = parent.thinking.lock().clone();
        assert_eq!(thinking, vec![("sa_test".into(), "thinking…".into())]);
        let tool = parent.tool.lock().clone();
        assert_eq!(tool, vec![("sa_test".into(), "read_file".into())]);
        let progress = parent.progress.lock().clone();
        assert_eq!(progress, vec![("sa_test".into(), "compressing context".into())]);
    }

    #[test]
    fn subagent_callbacks_suppress_turn_boundaries() {
        let parent = Arc::new(CapturingParent::default());
        let wrapper = SubagentCallbacks {
            parent: parent.clone() as CallbacksHandle,
            subagent_id: "sa_test".to_string(),
        };

        wrapper.on_turn_start("go");
        wrapper.on_turn_complete("done");

        assert_eq!(*parent.turn_starts.lock(), 0);
        assert_eq!(*parent.turn_completes.lock(), 0);
    }

    #[test]
    fn subagent_callbacks_deny_user_prompts_by_default() {
        let wrapper = SubagentCallbacks {
            parent: Arc::new(CapturingParent::default()) as CallbacksHandle,
            subagent_id: "sa_test".to_string(),
        };

        assert!(!wrapper.on_approval_request(ApprovalRequest {
            command: "rm -rf /".into(),
            description: "delete".into(),
        }));
        assert!(wrapper
            .on_clarify_request(ClarifyRequest {
                question: "really?".into(),
                options: vec![],
            })
            .is_none());
        assert!(wrapper
            .on_secret_request(SecretRequest {
                label: "GitHub PAT".into(),
            })
            .is_none());
    }
}
