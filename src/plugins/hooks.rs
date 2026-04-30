//! Lifecycle hooks for plugins.
//!
//! Plugins observe — and in some cases influence — the agent's
//! lifecycle by registering callbacks at defined points.
//!
//! # Hook points
//!
//! | Hook | Type | Fires |
//! |---|---|---|
//! | [`PreToolCallHook`] | Action | before each tool call |
//! | [`PostToolCallHook`] | Action | after each tool call |
//! | [`PreLlmCallHook`] | Observer | before each provider.chat call |
//! | [`PostLlmCallHook`] | Observer | after each provider.chat call |
//! | [`OnSessionStartHook`] | Observer | first turn of a session |
//! | [`OnSessionEndHook`] | Observer | when [`Agent::clear_history`] ends a session |
//!
//! # HookAction (tool hooks only)
//!
//! Tool hooks return an action so plugins can influence flow:
//!
//! - [`PreToolCallAction::Continue`] — proceed normally
//! - [`PreToolCallAction::Skip`] — abort the call. The tool result
//!   becomes `"[skipped by plugin: <reason>]"` so the LLM sees
//!   something coherent.
//! - [`PreToolCallAction::Rewrite`] — replace the tool's arguments.
//!   Subsequent hooks see the rewritten args.
//! - [`PostToolCallAction::Continue`] — pass output through unchanged
//! - [`PostToolCallAction::Rewrite`] — replace the output / success
//!
//! LLM and session hooks are deliberately observer-only — "skip the
//! LLM call mid-turn" or "skip session-start" don't have coherent
//! semantics, so the return type is `()`.
//!
//! # Multi-plugin ordering
//!
//! Hooks fire in plugin registration order. For tool hooks:
//!
//! - First [`Skip`](PreToolCallAction::Skip) wins (subsequent hooks
//!   not fired).
//! - [`Rewrite`](PreToolCallAction::Rewrite) actions chain — each
//!   subsequent hook sees the previous one's modifications.
//!
//! For observer hooks (LLM, session): all registered hooks fire,
//! none can abort.
//!
//! # Threading
//!
//! Hook callbacks must be `Send + Sync + 'static`. The agent calls
//! them synchronously from the tool/LLM/session paths. Hooks that
//! need async work spawn their own tasks; the agent loop does not
//! await them.
//!
//! # Panic isolation
//!
//! Each hook invocation is wrapped in `std::panic::catch_unwind`,
//! so a panicking hook does NOT abort the agent turn or block the
//! rest of the hook chain. The panic is logged and treated as
//! `Continue` (so the tool runs normally) for safety.

use std::sync::Arc;

use serde_json::Value;

// ---------------------------------------------------------------------------
// HookKind enum (used by the registry / context for kind-aware operations)
// ---------------------------------------------------------------------------

/// Hook kind discriminant. Used in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    PreToolCall,
    PostToolCall,
    PreLlmCall,
    PostLlmCall,
    OnSessionStart,
    OnSessionEnd,
}

// ---------------------------------------------------------------------------
// Tool hook events + actions
// ---------------------------------------------------------------------------

/// Event passed to `pre_tool_call` hooks.
#[derive(Debug)]
pub struct PreToolCallEvent<'a> {
    pub tool_name: &'a str,
    /// Effective arguments — if a previous hook returned a `Rewrite`,
    /// this is the rewritten value. The hook chain composes.
    pub args: &'a Value,
}

/// Action a `pre_tool_call` hook returns.
#[derive(Debug, Clone)]
pub enum PreToolCallAction {
    /// Proceed unchanged. Default for hooks with no opinion.
    Continue,
    /// Abort the tool call. The agent emits a synthetic tool result
    /// `"[skipped by plugin: <reason>]"` so the LLM sees coherent
    /// state.
    Skip { reason: String },
    /// Replace the tool's arguments. Subsequent hooks see the new
    /// args via [`PreToolCallEvent::args`].
    Rewrite { args: Value },
}

/// Event passed to `post_tool_call` hooks.
#[derive(Debug)]
pub struct PostToolCallEvent<'a> {
    pub tool_name: &'a str,
    pub args: &'a Value,
    /// Effective output — if a previous post-hook rewrote, this is
    /// the rewritten string.
    pub output: &'a str,
    /// Effective success — same chaining as `output`.
    pub success: bool,
}

/// Action a `post_tool_call` hook returns.
#[derive(Debug, Clone)]
pub enum PostToolCallAction {
    Continue,
    /// Replace the tool's output and success flag. Subsequent
    /// post-hooks see the new values.
    Rewrite { output: String, success: bool },
}

// ---------------------------------------------------------------------------
// LLM hook events (observers)
// ---------------------------------------------------------------------------

/// Event passed to `pre_llm_call` hooks. The full message list is
/// JSON-serialised once per call (cached in the registry to avoid
/// re-serialising for each hook).
#[derive(Debug)]
pub struct PreLlmCallEvent<'a> {
    /// JSON-encoded list of messages being sent to the provider.
    /// Stable shape: `[{"role": "...", "content": "..."}]`.
    pub messages_json: &'a str,
}

/// Event passed to `post_llm_call` hooks.
#[derive(Debug)]
pub struct PostLlmCallEvent<'a> {
    /// JSON-encoded provider response.
    pub response_json: &'a str,
}

// ---------------------------------------------------------------------------
// Session hook events (observers)
// ---------------------------------------------------------------------------

/// Event passed to `on_session_start` and `on_session_end` hooks.
#[derive(Debug)]
pub struct SessionEvent<'a> {
    /// Stable session identifier. For agents constructed without an
    /// explicit session id, the default is `"default"`.
    pub session_id: &'a str,
}

// ---------------------------------------------------------------------------
// Hook callback type aliases
// ---------------------------------------------------------------------------

pub type PreToolCallHook =
    Arc<dyn Fn(&PreToolCallEvent) -> PreToolCallAction + Send + Sync + 'static>;
pub type PostToolCallHook =
    Arc<dyn Fn(&PostToolCallEvent) -> PostToolCallAction + Send + Sync + 'static>;
pub type PreLlmCallHook = Arc<dyn Fn(&PreLlmCallEvent) + Send + Sync + 'static>;
pub type PostLlmCallHook = Arc<dyn Fn(&PostLlmCallEvent) + Send + Sync + 'static>;
pub type OnSessionStartHook = Arc<dyn Fn(&SessionEvent) + Send + Sync + 'static>;
pub type OnSessionEndHook = Arc<dyn Fn(&SessionEvent) + Send + Sync + 'static>;

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Collection of lifecycle hooks, owned by the agent and queried at
/// each lifecycle point.
#[derive(Default)]
pub struct HookRegistry {
    pre_tool: Vec<PreToolCallHook>,
    post_tool: Vec<PostToolCallHook>,
    pre_llm: Vec<PreLlmCallHook>,
    post_llm: Vec<PostLlmCallHook>,
    on_session_start: Vec<OnSessionStartHook>,
    on_session_end: Vec<OnSessionEndHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_pre_tool(&mut self, hook: PreToolCallHook) {
        self.pre_tool.push(hook);
    }
    pub fn register_post_tool(&mut self, hook: PostToolCallHook) {
        self.post_tool.push(hook);
    }
    pub fn register_pre_llm(&mut self, hook: PreLlmCallHook) {
        self.pre_llm.push(hook);
    }
    pub fn register_post_llm(&mut self, hook: PostLlmCallHook) {
        self.post_llm.push(hook);
    }
    pub fn register_on_session_start(&mut self, hook: OnSessionStartHook) {
        self.on_session_start.push(hook);
    }
    pub fn register_on_session_end(&mut self, hook: OnSessionEndHook) {
        self.on_session_end.push(hook);
    }

    /// Resolution returned from `fire_pre_tool` after running the
    /// hook chain. The agent acts on this:
    ///
    /// - [`PreToolResolution::Continue`] — run the tool with the
    ///   (possibly rewritten) args.
    /// - [`PreToolResolution::Skip`] — emit a synthetic skipped
    ///   result; do not run the tool.
    pub fn fire_pre_tool(
        &self,
        tool_name: &str,
        original_args: &Value,
    ) -> PreToolResolution {
        // Effective args carry forward through the hook chain. If a
        // hook returns Rewrite, subsequent hooks see the new args.
        let mut effective: Value = original_args.clone();
        for hook in &self.pre_tool {
            let event = PreToolCallEvent {
                tool_name,
                args: &effective,
            };
            let action = match invoke_with_panic_isolation(hook, &event, "pre_tool_call") {
                Some(a) => a,
                // Panic → treat as Continue so the tool still runs.
                None => PreToolCallAction::Continue,
            };
            match action {
                PreToolCallAction::Continue => {}
                PreToolCallAction::Skip { reason } => {
                    return PreToolResolution::Skip { reason };
                }
                PreToolCallAction::Rewrite { args } => {
                    effective = args;
                }
            }
        }
        PreToolResolution::Continue {
            effective_args: effective,
        }
    }

    /// Fire the post-tool chain. Returns the effective output +
    /// success, possibly rewritten by one or more hooks.
    pub fn fire_post_tool(
        &self,
        tool_name: &str,
        args: &Value,
        original_output: &str,
        original_success: bool,
    ) -> PostToolResolution {
        let mut effective_output = original_output.to_string();
        let mut effective_success = original_success;
        for hook in &self.post_tool {
            let event = PostToolCallEvent {
                tool_name,
                args,
                output: &effective_output,
                success: effective_success,
            };
            let action = match invoke_with_panic_isolation(hook, &event, "post_tool_call") {
                Some(a) => a,
                None => PostToolCallAction::Continue,
            };
            match action {
                PostToolCallAction::Continue => {}
                PostToolCallAction::Rewrite { output, success } => {
                    effective_output = output;
                    effective_success = success;
                }
            }
        }
        PostToolResolution {
            output: effective_output,
            success: effective_success,
        }
    }

    /// Fire all `pre_llm_call` hooks. Observers; no return value.
    pub fn fire_pre_llm(&self, messages_json: &str) {
        let event = PreLlmCallEvent { messages_json };
        for hook in &self.pre_llm {
            invoke_observer(hook, &event, "pre_llm_call");
        }
    }

    /// Fire all `post_llm_call` hooks. Observers; no return value.
    pub fn fire_post_llm(&self, response_json: &str) {
        let event = PostLlmCallEvent { response_json };
        for hook in &self.post_llm {
            invoke_observer(hook, &event, "post_llm_call");
        }
    }

    /// Fire all `on_session_start` hooks.
    pub fn fire_on_session_start(&self, session_id: &str) {
        let event = SessionEvent { session_id };
        for hook in &self.on_session_start {
            invoke_observer(hook, &event, "on_session_start");
        }
    }

    /// Fire all `on_session_end` hooks.
    pub fn fire_on_session_end(&self, session_id: &str) {
        let event = SessionEvent { session_id };
        for hook in &self.on_session_end {
            invoke_observer(hook, &event, "on_session_end");
        }
    }

    /// Total number of registered hooks across all kinds.
    pub fn count(&self) -> usize {
        self.pre_tool.len()
            + self.post_tool.len()
            + self.pre_llm.len()
            + self.post_llm.len()
            + self.on_session_start.len()
            + self.on_session_end.len()
    }
}

/// Outcome of `fire_pre_tool`. The agent dispatches on this:
/// `Continue` runs the tool with the resolved args; `Skip` emits a
/// synthetic skipped result.
#[derive(Debug)]
pub enum PreToolResolution {
    Continue { effective_args: Value },
    Skip { reason: String },
}

/// Outcome of `fire_post_tool`. The agent uses these as the final
/// (output, success) values pushed into the conversation history.
#[derive(Debug)]
pub struct PostToolResolution {
    pub output: String,
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Panic isolation helpers
// ---------------------------------------------------------------------------

/// Invoke a hook that returns an `Action`, isolating panics.
/// Returns `None` on panic (caller falls back to `Continue`-equivalent).
fn invoke_with_panic_isolation<E, A, F: ?Sized>(
    hook: &Arc<F>,
    event: &E,
    kind: &'static str,
) -> Option<A>
where
    F: Fn(&E) -> A + Send + Sync + 'static,
{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(event)));
    match result {
        Ok(action) => Some(action),
        Err(_) => {
            tracing::warn!(hook = %kind, "Plugin hook panicked; isolating and continuing");
            None
        }
    }
}

/// Invoke an observer hook (return type `()`), isolating panics.
fn invoke_observer<E, F: ?Sized>(hook: &Arc<F>, event: &E, kind: &'static str)
where
    F: Fn(&E) + Send + Sync + 'static,
{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(event)));
    if result.is_err() {
        tracing::warn!(hook = %kind, "Plugin hook panicked; isolating and continuing");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    /// Empty registry should fire no callbacks and report zero hooks.
    #[test]
    fn empty_registry_fires_nothing() {
        let r = HookRegistry::new();
        let v = json!({});
        match r.fire_pre_tool("test", &v) {
            PreToolResolution::Continue { effective_args } => {
                assert_eq!(effective_args, v);
            }
            _ => panic!("expected Continue"),
        }
        let post = r.fire_post_tool("test", &v, "ok", true);
        assert_eq!(post.output, "ok");
        assert!(post.success);
        r.fire_pre_llm("[]");
        r.fire_post_llm("{}");
        r.fire_on_session_start("default");
        r.fire_on_session_end("default");
        assert_eq!(r.count(), 0);
    }

    /// Multiple pre-tool hooks all fire in registration order.
    #[test]
    fn multiple_pre_tool_hooks_fire_in_order() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::<usize>::new()));
        let mut r = HookRegistry::new();
        for i in 0..3 {
            let order = Arc::clone(&order);
            r.register_pre_tool(Arc::new(move |_event| {
                order.lock().push(i);
                PreToolCallAction::Continue
            }));
        }
        let v = json!({"k": "v"});
        match r.fire_pre_tool("x", &v) {
            PreToolResolution::Continue { .. } => {}
            _ => panic!("expected Continue"),
        }
        assert_eq!(*order.lock(), vec![0, 1, 2]);
    }

    /// First `Skip` wins; subsequent hooks do not fire.
    #[test]
    fn first_skip_wins_and_short_circuits() {
        let after_count = Arc::new(AtomicUsize::new(0));
        let mut r = HookRegistry::new();
        r.register_pre_tool(Arc::new(|_event| PreToolCallAction::Continue));
        r.register_pre_tool(Arc::new(|_event| PreToolCallAction::Skip {
            reason: "denied by audit plugin".to_string(),
        }));
        let after = Arc::clone(&after_count);
        r.register_pre_tool(Arc::new(move |_event| {
            after.fetch_add(1, Ordering::SeqCst);
            PreToolCallAction::Continue
        }));

        let v = json!({});
        match r.fire_pre_tool("danger", &v) {
            PreToolResolution::Skip { reason } => {
                assert_eq!(reason, "denied by audit plugin");
            }
            _ => panic!("expected Skip"),
        }
        assert_eq!(
            after_count.load(Ordering::SeqCst),
            0,
            "hooks after Skip must not fire"
        );
    }

    /// `Rewrite` actions chain — each subsequent hook sees the
    /// previous hook's modifications.
    #[test]
    fn pre_tool_rewrites_chain() {
        let mut r = HookRegistry::new();
        // Hook 1: append "/A" to args.tag.
        r.register_pre_tool(Arc::new(|event| {
            let mut args = event.args.clone();
            let tag = args
                .get("tag")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            args["tag"] = json!(format!("{tag}/A"));
            PreToolCallAction::Rewrite { args }
        }));
        // Hook 2: append "/B".
        r.register_pre_tool(Arc::new(|event| {
            let mut args = event.args.clone();
            let tag = args
                .get("tag")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            args["tag"] = json!(format!("{tag}/B"));
            PreToolCallAction::Rewrite { args }
        }));

        let v = json!({"tag": "start"});
        match r.fire_pre_tool("t", &v) {
            PreToolResolution::Continue { effective_args } => {
                assert_eq!(effective_args["tag"], "start/A/B");
            }
            _ => panic!("expected Continue with rewritten args"),
        }
    }

    /// Post-tool rewrites also chain, modifying both output and
    /// success.
    #[test]
    fn post_tool_rewrites_chain() {
        let mut r = HookRegistry::new();
        r.register_post_tool(Arc::new(|event| PostToolCallAction::Rewrite {
            output: format!("{}-A", event.output),
            success: event.success,
        }));
        r.register_post_tool(Arc::new(|event| PostToolCallAction::Rewrite {
            output: format!("{}-B", event.output),
            success: !event.success,
        }));
        let v = json!({});
        let resolution = r.fire_post_tool("t", &v, "raw", true);
        assert_eq!(resolution.output, "raw-A-B");
        assert!(!resolution.success);
    }

    /// Observer hooks (LLM, session) all fire and cannot abort.
    #[test]
    fn observer_hooks_all_fire() {
        let llm_pre = Arc::new(AtomicUsize::new(0));
        let llm_post = Arc::new(AtomicUsize::new(0));
        let sess_start = Arc::new(AtomicUsize::new(0));
        let sess_end = Arc::new(AtomicUsize::new(0));
        let mut r = HookRegistry::new();
        for _ in 0..3 {
            let c = Arc::clone(&llm_pre);
            r.register_pre_llm(Arc::new(move |_event| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        for _ in 0..2 {
            let c = Arc::clone(&llm_post);
            r.register_post_llm(Arc::new(move |_event| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        let c = Arc::clone(&sess_start);
        r.register_on_session_start(Arc::new(move |_event| {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        let c = Arc::clone(&sess_end);
        r.register_on_session_end(Arc::new(move |_event| {
            c.fetch_add(1, Ordering::SeqCst);
        }));

        r.fire_pre_llm("[]");
        r.fire_post_llm("{}");
        r.fire_on_session_start("s1");
        r.fire_on_session_end("s1");

        assert_eq!(llm_pre.load(Ordering::SeqCst), 3);
        assert_eq!(llm_post.load(Ordering::SeqCst), 2);
        assert_eq!(sess_start.load(Ordering::SeqCst), 1);
        assert_eq!(sess_end.load(Ordering::SeqCst), 1);
    }

    /// A panicking hook is isolated; subsequent hooks still fire.
    #[test]
    fn panicking_hook_is_isolated() {
        let after_count = Arc::new(AtomicUsize::new(0));
        let mut r = HookRegistry::new();
        r.register_pre_tool(Arc::new(|_event| {
            panic!("intentional test panic");
        }));
        let after = Arc::clone(&after_count);
        r.register_pre_tool(Arc::new(move |_event| {
            after.fetch_add(1, Ordering::SeqCst);
            PreToolCallAction::Continue
        }));

        let v = json!({});
        match r.fire_pre_tool("t", &v) {
            PreToolResolution::Continue { .. } => {}
            _ => panic!("expected Continue (panic should not Skip)"),
        }
        assert_eq!(after_count.load(Ordering::SeqCst), 1);
    }
}
