//! Plugin-contributed CLI subcommands.
//!
//! Plugins can extend the `fennec` CLI by registering top-level
//! subcommands. After registration, `fennec <plugin-cmd> <args...>`
//! dispatches to the plugin's handler.
//!
//! # Discovery model
//!
//! Plugin CLI commands are declared as **metadata** so the host can
//! discover them at startup without instantiating the plugin:
//!
//! - Bundled plugins return a list of [`CliCommandSpec`] from
//!   [`Plugin::cli_commands`](super::traits::Plugin::cli_commands)
//!   (default empty). The trait method is called on a static
//!   reference — no plugin construction needed.
//! - WASM plugins declare commands in `plugin.toml` under
//!   `cli_commands = [{ name = "...", description = "..." }, ...]`.
//!   No `.wasm` instantiation needed at parse time.
//!
//! Both paths produce the same [`CliCommandSpec`] which the host
//! converts into a clap [`clap::Command`] subcommand at startup.
//!
//! # Dispatch model
//!
//! Bundled plugins ship a closure handler — the
//! [`PluginContext::register_cli_command`](super::context::PluginContext::register_cli_command)
//! call binds a closure to a plugin command name, and at dispatch
//! time the host looks up the closure and calls it with the parsed
//! args (everything after `fennec <plugin-cmd>`).
//!
//! WASM plugins implement the `cli-execute(name, args) -> s32`
//! export defined in `wit/plugin.wit`. The host calls that export
//! when one of the plugin's declared commands matches.
//!
//! # Collision handling
//!
//! Plugin commands cannot shadow built-in `fennec` subcommands
//! (`agent`, `gateway`, `onboard`, `login`, `doctor`, `status`).
//! Collisions are rejected at the discovery pass with a warn log;
//! the offending command is dropped, the rest of the plugin's
//! commands are still registered.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Static metadata for one plugin-contributed CLI subcommand.
/// The plugin's name is implicit (the registry knows which plugin
/// this came from); only the user-facing command name + description
/// live here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCommandSpec {
    /// Command name as it appears on the CLI (e.g. `"spotify"`).
    /// Must be unique across the agent's full command set. Same
    /// validation rules as plugin/profile names: ASCII alphanumeric
    /// + `-` + `_`, 1-32 chars.
    pub name: String,
    /// One-line description shown in `fennec --help` next to the
    /// subcommand entry.
    pub description: String,
}

/// Bundled-plugin CLI handler. Receives the args following
/// `fennec <plugin-cmd>` and returns a Unix-style exit code.
pub type CliCommandHandler =
    Arc<dyn Fn(Vec<String>) -> Result<i32> + Send + Sync + 'static>;

/// Set of names reserved by the built-in `fennec` CLI. Plugin
/// commands using any of these are rejected at registration to
/// avoid shadowing core functionality.
pub const RESERVED_COMMAND_NAMES: &[&str] = &[
    "agent", "gateway", "onboard", "login", "doctor", "status", "help",
    // Useful flags / shortcuts the user might type expecting them
    // to be subcommands.
    "version",
];

/// Validate a plugin CLI command name. Returns `Err` with a
/// human-readable reason if the name is rejected. Caller logs and
/// drops the offending entry.
pub fn validate_command_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("plugin CLI command name cannot be empty");
    }
    if name.len() > 32 {
        anyhow::bail!(
            "plugin CLI command name '{}' too long ({} chars; max 32)",
            name,
            name.len()
        );
    }
    for (i, ch) in name.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if !ok {
            anyhow::bail!(
                "plugin CLI command name '{}' contains invalid character '{}' at position {}; \
                 allowed: ASCII letters, digits, '-', '_'",
                name,
                ch,
                i
            );
        }
    }
    if RESERVED_COMMAND_NAMES.contains(&name) {
        anyhow::bail!(
            "plugin CLI command name '{}' clashes with a reserved built-in command",
            name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_simple_name() {
        validate_command_name("spotify").unwrap();
        validate_command_name("disk-cleanup").unwrap();
        validate_command_name("user_2").unwrap();
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_command_name("").is_err());
    }

    #[test]
    fn rejects_collision_with_builtin() {
        assert!(validate_command_name("agent").is_err());
        assert!(validate_command_name("doctor").is_err());
        assert!(validate_command_name("gateway").is_err());
    }

    #[test]
    fn rejects_path_traversal_or_special_chars() {
        assert!(validate_command_name("..").is_err());
        assert!(validate_command_name("/etc").is_err());
        assert!(validate_command_name("with space").is_err());
        assert!(validate_command_name("a;b").is_err());
    }

    #[test]
    fn rejects_overlong() {
        let long = "a".repeat(33);
        assert!(validate_command_name(&long).is_err());
        let limit = "a".repeat(32);
        assert!(validate_command_name(&limit).is_ok());
    }
}
