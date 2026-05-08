//! Slash commands for the TUI.
//!
//! The user types `/foo args` in the input box; the TUI's submit
//! path detects the leading `/` and routes here instead of
//! dispatching the text as a chat message.
//!
//! Commands live in this module rather than in the existing CLI
//! channel because they're TUI-specific (some open modal
//! overlays, some change the renderer's state, some are pure
//! agent operations). Implementations land in subsequent commits;
//! this file establishes the registry shape so callers compile.

use std::collections::HashMap;

use anyhow::Result;

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
}

/// Trait every slash command implements.
pub trait CommandHandler: Send + Sync {
    /// Command name without the leading slash (e.g. `"clear"`).
    fn name(&self) -> &'static str;
    /// One-line summary for `/help`.
    fn help(&self) -> &'static str;
    /// Run the command.
    fn execute(&self, args: &str) -> Result<CommandOutcome>;
}

/// Registry of installed commands.
pub struct CommandRegistry {
    handlers: HashMap<&'static str, Box<dyn CommandHandler>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Box<dyn CommandHandler>) {
        self.handlers.insert(handler.name(), handler);
    }

    pub fn get(&self, name: &str) -> Option<&dyn CommandHandler> {
        self.handlers.get(name).map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.handlers.keys().copied().collect();
        v.sort();
        v
    }

    /// Look up `name` and run with `args`. Returns
    /// `CommandOutcome::Unknown(name)` if no handler exists.
    pub fn dispatch(&self, name: &str, args: &str) -> Result<CommandOutcome> {
        match self.get(name) {
            Some(h) => h.execute(args),
            None => Ok(CommandOutcome::Unknown(name.to_string())),
        }
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
        fn execute(&self, args: &str) -> Result<CommandOutcome> {
            Ok(CommandOutcome::Text(args.to_string()))
        }
    }

    #[test]
    fn registry_dispatches_known_command() {
        let mut r = CommandRegistry::new();
        r.register(Box::new(EchoCommand));
        let outcome = r.dispatch("echo", "hello").unwrap();
        match outcome {
            CommandOutcome::Text(s) => assert_eq!(s, "hello"),
            other => panic!("unexpected outcome: {:?}", other),
        }
    }

    #[test]
    fn registry_returns_unknown_for_missing_command() {
        let r = CommandRegistry::new();
        match r.dispatch("missing", "").unwrap() {
            CommandOutcome::Unknown(name) => assert_eq!(name, "missing"),
            other => panic!("unexpected outcome: {:?}", other),
        }
    }
}
