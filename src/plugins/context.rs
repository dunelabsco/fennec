//! The handle a plugin uses to register its contributions.
//!
//! Each call to [`Plugin::register`](super::Plugin::register) receives a
//! fresh `&mut PluginContext`. Plugins call methods on it to register
//! tools, lifecycle hooks, and (in future phases) channels, providers,
//! and CLI subcommands. After `register` returns, the registry drains
//! the context and folds the contributions into the agent.

use crate::tools::traits::Tool;

use super::hooks::{
    OnSessionEndHook, OnSessionStartHook, PostLlmCallHook, PostToolCallHook,
    PreLlmCallHook, PreToolCallHook,
};

/// Mutable handle passed to a plugin's `register` method.
pub struct PluginContext {
    tools: Vec<Box<dyn Tool>>,
    pre_tool_hooks: Vec<PreToolCallHook>,
    post_tool_hooks: Vec<PostToolCallHook>,
    pre_llm_hooks: Vec<PreLlmCallHook>,
    post_llm_hooks: Vec<PostLlmCallHook>,
    on_session_start_hooks: Vec<OnSessionStartHook>,
    on_session_end_hooks: Vec<OnSessionEndHook>,
}

impl PluginContext {
    pub(super) fn new() -> Self {
        Self {
            tools: Vec::new(),
            pre_tool_hooks: Vec::new(),
            post_tool_hooks: Vec::new(),
            pre_llm_hooks: Vec::new(),
            post_llm_hooks: Vec::new(),
            on_session_start_hooks: Vec::new(),
            on_session_end_hooks: Vec::new(),
        }
    }

    /// Register an LLM-callable tool.
    ///
    /// Plugins are responsible for returning unique tool names. The
    /// agent's tool registry will reject duplicates at build time.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register a `pre_tool_call` hook. Returns a
    /// [`PreToolCallAction`](super::hooks::PreToolCallAction):
    /// `Continue`, `Skip(reason)`, or `Rewrite(args)`.
    pub fn register_pre_tool_hook(&mut self, hook: PreToolCallHook) {
        self.pre_tool_hooks.push(hook);
    }

    /// Register a `post_tool_call` hook. Returns a
    /// [`PostToolCallAction`](super::hooks::PostToolCallAction):
    /// `Continue` or `Rewrite(output, success)`.
    pub fn register_post_tool_hook(&mut self, hook: PostToolCallHook) {
        self.post_tool_hooks.push(hook);
    }

    /// Register a `pre_llm_call` observer hook.
    pub fn register_pre_llm_hook(&mut self, hook: PreLlmCallHook) {
        self.pre_llm_hooks.push(hook);
    }

    /// Register a `post_llm_call` observer hook.
    pub fn register_post_llm_hook(&mut self, hook: PostLlmCallHook) {
        self.post_llm_hooks.push(hook);
    }

    /// Register an `on_session_start` observer hook.
    pub fn register_on_session_start(&mut self, hook: OnSessionStartHook) {
        self.on_session_start_hooks.push(hook);
    }

    /// Register an `on_session_end` observer hook.
    pub fn register_on_session_end(&mut self, hook: OnSessionEndHook) {
        self.on_session_end_hooks.push(hook);
    }

    /// Drain everything the plugin contributed. Used by the registry.
    pub(super) fn into_parts(self) -> PluginContextParts {
        PluginContextParts {
            tools: self.tools,
            pre_tool_hooks: self.pre_tool_hooks,
            post_tool_hooks: self.post_tool_hooks,
            pre_llm_hooks: self.pre_llm_hooks,
            post_llm_hooks: self.post_llm_hooks,
            on_session_start_hooks: self.on_session_start_hooks,
            on_session_end_hooks: self.on_session_end_hooks,
        }
    }
}

/// Drained contents of a [`PluginContext`].
pub(super) struct PluginContextParts {
    pub tools: Vec<Box<dyn Tool>>,
    pub pre_tool_hooks: Vec<PreToolCallHook>,
    pub post_tool_hooks: Vec<PostToolCallHook>,
    pub pre_llm_hooks: Vec<PreLlmCallHook>,
    pub post_llm_hooks: Vec<PostLlmCallHook>,
    pub on_session_start_hooks: Vec<OnSessionStartHook>,
    pub on_session_end_hooks: Vec<OnSessionEndHook>,
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}
