//! Slash commands for the TUI.
//!
//! The user types `/foo args` in the input box; the TUI's submit
//! path detects the leading `/` and routes here instead of
//! dispatching the text as a chat message.
//!
//! Commands live in this module rather than in the existing CLI
//! channel because they're TUI-specific (some open modal
//! overlays, some change the renderer's state, some are pure
//! agent operations).

pub mod builtin;

use std::collections::HashMap;

use anyhow::Result;

use super::app::App;

/// Action a command wants performed on the agent itself. Surfaced
/// because slash commands run on the main thread under the
/// app's parking_lot mutex; agent calls are async + need their
/// own tokio mutex. The submit loop performs these actions
/// *after* the command handler returns and the app lock is
/// released.
#[derive(Debug, Clone)]
pub enum AgentAction {
    /// Clear the agent's history (start a fresh conversation
    /// without restarting the process).
    Clear,
    /// Replay the last user message as a fresh turn.
    Retry,
    /// Pop the last user-assistant exchange.
    Undo,
    /// Inject `message` as a steering note into the next turn.
    Steer(String),
    /// Send `prompt` as a regular turn (used by /background and
    /// similar commands that wrap a normal turn).
    Run(String),
    /// Render the `/usage` panel — the submit loop locks the
    /// agent, calls `Agent::token_usage`, and pushes a formatted
    /// system message into the chat scrollback.
    ShowUsage,
    /// Set or read the title of the currently active session.
    /// `None` means "show the current title". Empty string means
    /// "clear the title". A non-empty string sets the title.
    SessionTitle(Option<String>),
    /// Resume a saved session by id-or-title (Hermes' fallback
    /// matches by exact title when no id matches).
    SessionResume(String),
    /// Show the active model and a known-models list, or swap
    /// to a different model live (mid-turn requests are
    /// rejected, mirroring Hermes' `_apply_model_switch`).
    /// `None` payload means "show". `Some(name)` means "switch".
    SwitchModel(Option<String>),
    /// `/tools` actions. `None` lists every registered tool with
    /// its enabled/disabled status; `Some((true, names))` enables
    /// the listed names; `Some((false, names))` disables them.
    /// Persistence to config.toml + chat-history reset happen
    /// alongside the toggle in the submit loop, matching Hermes'
    /// tools.configure (server.py:6213-6280).
    ToolsToggle(Option<(bool, Vec<String>)>),
    /// Re-read `~/.fennec/.env` into the running process so
    /// changed env-only credentials (API keys, base URLs) take
    /// effect on the next provider call. Mirrors Hermes' reload.env.
    ReloadEnv,
    /// Rescan MCP servers configured for the running session.
    /// Hermes' reload.mcp shuts down + rediscovers; Fennec's
    /// agent doesn't currently boot any MCP clients, so this
    /// surfaces an honest "not yet wired" status instead of a
    /// silent no-op.
    ReloadMcp,
    /// Attach an image at the given path to the next user
    /// turn. The file is read + base64-encoded immediately so
    /// `/image` can return dimensions + token estimate;
    /// providers serialise it inline on the next turn.
    AttachImage(std::path::PathBuf),
}

/// Outcome of running a slash command. Drives the TUI's response
/// to the command — display text, status flash, modal, exit, etc.
#[derive(Debug, Clone)]
pub enum CommandOutcome {
    /// Display this text as a system message in the chat panel.
    Text(String),
    /// Show a transient status message (~3 seconds in the bottom
    /// hint area).
    Status(String),
    /// User asked to quit the TUI.
    Quit,
    /// Command not recognized.
    Unknown(String),
    /// Command was a no-op for now (placeholder while
    /// implementations land).
    NotImplemented(String),
    /// Side-effecting agent operation queued for the submit loop.
    Agent(AgentAction),
}

/// Trait every slash command implements. Handlers take a mutable
/// `App` reference so they can mutate UI state directly (toggle
/// flags, push system messages, etc.). For agent operations they
/// return an `AgentAction` outcome variant; the submit loop
/// applies it after releasing the app lock.
pub trait CommandHandler: Send + Sync {
    /// Command name without the leading slash (e.g. `"clear"`).
    fn name(&self) -> &'static str;
    /// One-line summary for `/help`.
    fn help(&self) -> &'static str;
    /// Aliases (e.g. `"new"` is also `"clear"`). Default: none.
    fn aliases(&self) -> &[&'static str] {
        &[]
    }
    /// Run the command.
    fn execute(&self, args: &str, app: &mut App) -> Result<CommandOutcome>;
}

/// Registry of installed commands.
pub struct CommandRegistry {
    handlers: HashMap<&'static str, Box<dyn CommandHandler>>,
    /// Aliases pointing at the primary command name.
    aliases: HashMap<&'static str, &'static str>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Box<dyn CommandHandler>) {
        let aliases: Vec<&'static str> = handler.aliases().to_vec();
        let primary = handler.name();
        self.handlers.insert(primary, handler);
        // Aliases share the same primary handler via a re-lookup
        // in `get`. We don't double-store boxes; the alias map
        // just remembers which primary to forward to.
        for a in aliases {
            self.aliases.insert(a, primary);
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn CommandHandler> {
        if let Some(h) = self.handlers.get(name) {
            return Some(h.as_ref());
        }
        if let Some(primary) = self.aliases.get(name) {
            return self.handlers.get(primary).map(|b| b.as_ref());
        }
        None
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.handlers.keys().copied().collect();
        v.sort();
        v
    }

    /// Look up `name` and run with `args`. Returns
    /// `CommandOutcome::Unknown(name)` if no handler exists.
    pub fn dispatch(
        &self,
        name: &str,
        args: &str,
        app: &mut App,
    ) -> Result<CommandOutcome> {
        match self.get(name) {
            Some(h) => h.execute(args, app),
            None => Ok(CommandOutcome::Unknown(name.to_string())),
        }
    }

    /// Build a registry pre-loaded with the F1-1 built-in
    /// command set. Called by the TUI on startup.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        builtin::register_all(&mut r);
        r
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a raw `/foo bar baz` line into `("foo", "bar baz")`.
/// Returns `None` if `s` doesn't start with `/`.
pub fn parse(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let body = s.strip_prefix('/')?;
    let split = body
        .find(char::is_whitespace)
        .unwrap_or(body.len());
    let (name, rest) = body.split_at(split);
    Some((name, rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_name_only() {
        assert_eq!(parse("/help"), Some(("help", "")));
    }

    #[test]
    fn parse_command_with_args() {
        assert_eq!(parse("/title my new session"), Some(("title", "my new session")));
    }

    #[test]
    fn parse_rejects_non_command() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn parse_strips_leading_whitespace() {
        assert_eq!(parse("   /clear"), Some(("clear", "")));
    }

    struct EchoCommand;
    impl CommandHandler for EchoCommand {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn help(&self) -> &'static str {
            "echo args back"
        }
        fn execute(&self, args: &str, _app: &mut App) -> Result<CommandOutcome> {
            Ok(CommandOutcome::Text(args.to_string()))
        }
    }

    #[test]
    fn registry_dispatches_known_command() {
        let mut r = CommandRegistry::new();
        r.register(Box::new(EchoCommand));
        let mut app = App::new();
        let outcome = r.dispatch("echo", "hello", &mut app).unwrap();
        match outcome {
            CommandOutcome::Text(s) => assert_eq!(s, "hello"),
            other => panic!("unexpected outcome: {:?}", other),
        }
    }

    #[test]
    fn registry_returns_unknown_for_missing_command() {
        let r = CommandRegistry::new();
        let mut app = App::new();
        match r.dispatch("missing", "", &mut app).unwrap() {
            CommandOutcome::Unknown(name) => assert_eq!(name, "missing"),
            other => panic!("unexpected outcome: {:?}", other),
        }
    }
}
