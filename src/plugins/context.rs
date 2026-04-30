//! The handle a plugin uses to register its contributions.
//!
//! Each call to [`Plugin::register`](super::Plugin::register) receives a
//! fresh `&mut PluginContext`. Plugins call methods on it to register
//! tools (and, in later phases, hooks, channels, providers, CLI
//! subcommands). After `register` returns, the registry drains the
//! context's collected contributions and consumes them.

use crate::tools::traits::Tool;

/// Mutable handle passed to a plugin's `register` method.
///
/// In this initial phase the only registration surface is
/// [`PluginContext::register_tool`]. Subsequent phases will add:
///
/// - `register_hook(...)` for lifecycle observation
/// - `register_channel(...)` for new messaging platforms
/// - `register_memory_provider(...)` for the exclusive-category model
/// - `register_cli_command(...)` for `fennec <plugin> <subcmd>`
///
/// The deliberate ordering is "tools first" because that surface is
/// already proven (every existing built-in tool implements [`Tool`])
/// and slips into the agent without any other plumbing changes.
pub struct PluginContext {
    tools: Vec<Box<dyn Tool>>,
}

impl PluginContext {
    pub(super) fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool implementation. The tool's
    /// [`Tool::name`](crate::tools::traits::Tool::name) is what the LLM
    /// will see in the tool list and call by.
    ///
    /// Plugins are responsible for returning unique tool names. The
    /// registry doesn't currently de-duplicate across plugins; if two
    /// plugins both register a tool called `"echo"`, the agent's tool
    /// registry will reject the second one at agent-build time.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Drain the collected tool registrations. Called by the registry
    /// after a plugin's `register` completes.
    pub(super) fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools
    }
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}
