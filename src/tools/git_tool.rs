//! Read-only git tool.
//!
//! Commands are shell-word tokenized and executed via direct `Command::new("git")`
//! (no `sh -c`), so shell metacharacters can't slip through the allowlist.
//!
//! In addition to the "safe subcommand" check, individual args are
//! inspected for known-bad flags that let git execute arbitrary code
//! (`-c alias.x=!…`, `--upload-pack=…`, `--receive-pack=…`, `--exec=…`,
//! `--config`, `--config-env`, `--get-urlmatch`). Combined with the
//! subcommand allowlist, these two checks turn git into a genuinely
//! read-only surface rather than a "read-only by convention" one.
//!
//! The child runs with a restricted env (`GIT_CONFIG_NOSYSTEM=1`,
//! `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_TERMINAL_PROMPT=0`) so it cannot
//! pick up host-wide or user-level config (which could alias commands or
//! enable hostile hook loading).

use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

use super::proc_util::{
    run_with_timeout, scrub_sensitive_env, truncate_head_tail, use_process_group,
};
use super::traits::{Tool, ToolResult};

const MAX_STDOUT_BYTES: usize = 2_000_000; // git log/show can be large
const MAX_STDERR_BYTES: usize = 100_000;
const MAX_OUTPUT_CHARS: usize = 50_000;
const TIMEOUT_SECS: u64 = 30;

/// Safe git sub-commands (read-only).
const SAFE_COMMANDS: &[&str] = &[
    "status", "diff", "log", "branch", "show", "blame", "stash",
    "rev-parse", "describe", "shortlog", "tag", "ls-files", "ls-tree",
    "cat-file", "reflog",
];

/// Flags that let git execute arbitrary code or change behavior in ways
/// that break the "read-only" guarantee. Matches both `--flag` and
/// `--flag=value` forms.
const BLOCKED_FLAG_PREFIXES: &[&str] = &[
    "--upload-pack",
    "--receive-pack",
    "--exec",
    "--config-env",
    "--get-urlmatch",
];

/// Characters that shouldn't appear in a git command we pass through —
/// shell metacharacters are not meaningful without `sh -c`, but their
/// presence still indicates someone is trying to smuggle something.
const FORBIDDEN_METACHARS: &[char] = &[';', '&', '|', '>', '<', '$', '`', '\n', '\r'];

/// A tool that runs safe, read-only git commands.
pub struct GitTool;

impl GitTool {
    pub fn new() -> Self {
        Self
    }

    fn find_metachar(command: &str) -> Option<char> {
        command.chars().find(|c| FORBIDDEN_METACHARS.contains(c))
    }

    /// Check that a single arg isn't a blocked flag.
    fn arg_is_blocked(arg: &str) -> Option<&'static str> {
        // `-c foo.bar=baz` — overrides git config per-command, can alias
        // commands to shell execution. Reject both the bare `-c` and
        // anything of the form `-c<something>`.
        if arg == "-c" || arg.starts_with("-c") && arg.len() > 2 && !arg.starts_with("--") {
            return Some("-c");
        }
        if arg == "--config" {
            return Some("--config");
        }
        for prefix in BLOCKED_FLAG_PREFIXES {
            if arg == *prefix || arg.starts_with(&format!("{}=", prefix)) {
                return Some(prefix);
            }
        }
        None
    }

    /// Check if the subcommand (argv[0] after `git`) is safe.
    fn is_safe_subcommand(subcmd: &str) -> bool {
        SAFE_COMMANDS.contains(&subcmd)
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Run safe, read-only git commands (status, diff, log, branch, show, blame, etc.). \
         Dangerous commands (push, reset --hard, clean) and arbitrary-code flags \
         (-c, --upload-pack, --exec) are blocked."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Git sub-command and arguments (e.g. 'status', 'diff', 'log --oneline -10')"
                }
            },
            "required": ["command"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c.trim(),
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: command".to_string()),
                });
            }
        };

        if let Some(ch) = Self::find_metachar(command) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "shell metacharacter '{}' is not permitted in git commands",
                    ch
                )),
            });
        }

        let argv = match shell_words::split(command) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("empty git command".to_string()),
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("could not parse git command: {}", e)),
                });
            }
        };

        // argv[0] is the subcommand (we prepend "git" later).
        let subcmd = &argv[0];
        if !Self::is_safe_subcommand(subcmd) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "blocked: git sub-command '{}' is not in the safe list",
                    subcmd
                )),
            });
        }

        // Inspect every arg for blocked flags (including the subcommand slot,
        // in case someone tries `-c alias.status=!sh` as the first token).
        for a in &argv {
            if let Some(bad) = Self::arg_is_blocked(a) {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "blocked: git arg '{}' can execute arbitrary code",
                        bad
                    )),
                });
            }
        }

        let mut cmd = Command::new("git");
        cmd.args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            // Harden git env: no system/global config, no interactive prompts,
            // no credential helper auto-invocation.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0");
        scrub_sensitive_env(&mut cmd);
        use_process_group(&mut cmd);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to spawn git: {}", e)),
                });
            }
        };

        let outcome = run_with_timeout(
            child,
            Duration::from_secs(TIMEOUT_SECS),
            MAX_STDOUT_BYTES,
            MAX_STDERR_BYTES,
        )
        .await?;

        if outcome.timed_out {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("git command timed out after {}s", TIMEOUT_SECS)),
            });
        }

        let stdout = String::from_utf8_lossy(&outcome.stdout);
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        let combined = if stderr.is_empty() {
            stdout.to_string()
        } else if stdout.is_empty() {
            stderr.to_string()
        } else {
            format!("{}\n--- stderr ---\n{}", stdout, stderr)
        };
        let truncated = truncate_head_tail(&combined, MAX_OUTPUT_CHARS);
        let success = outcome.exit_code == Some(0);

        Ok(ToolResult {
            success,
            output: truncated,
            error: if success {
                None
            } else {
                Some(format!("exit code: {}", outcome.exit_code.unwrap_or(-1)))
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_subcommands() {
        assert!(GitTool::is_safe_subcommand("status"));
        assert!(GitTool::is_safe_subcommand("log"));
        assert!(GitTool::is_safe_subcommand("diff"));
        assert!(!GitTool::is_safe_subcommand("push"));
        assert!(!GitTool::is_safe_subcommand("pull"));
        assert!(!GitTool::is_safe_subcommand("clone"));
        assert!(!GitTool::is_safe_subcommand(""));
    }

    #[test]
    fn arg_blocklist_matches_c_flag() {
        assert_eq!(GitTool::arg_is_blocked("-c"), Some("-c"));
        assert_eq!(
            GitTool::arg_is_blocked("-calias.x=!sh"),
            Some("-c"),
        );
    }

    #[test]
    fn arg_blocklist_matches_config() {
        assert_eq!(GitTool::arg_is_blocked("--config"), Some("--config"));
    }

    #[test]
    fn arg_blocklist_matches_upload_pack() {
        assert_eq!(
            GitTool::arg_is_blocked("--upload-pack"),
            Some("--upload-pack"),
        );
        assert_eq!(
            GitTool::arg_is_blocked("--upload-pack=evil-tool"),
            Some("--upload-pack"),
        );
    }

    #[test]
    fn arg_blocklist_matches_exec() {
        assert_eq!(
            GitTool::arg_is_blocked("--exec=sh"),
            Some("--exec"),
        );
    }

    #[test]
    fn arg_blocklist_ignores_benign_flags() {
        assert_eq!(GitTool::arg_is_blocked("--oneline"), None);
        assert_eq!(GitTool::arg_is_blocked("-a"), None);
        assert_eq!(GitTool::arg_is_blocked("-10"), None);
        assert_eq!(GitTool::arg_is_blocked("HEAD"), None);
    }

    #[tokio::test]
    async fn rejects_metachar() {
        let t = GitTool::new();
        let r = t
            .execute(json!({"command": "log; rm -rf /"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("metacharacter"));
    }

    #[tokio::test]
    async fn rejects_unsafe_subcommand() {
        let t = GitTool::new();
        let r = t
            .execute(json!({"command": "push origin main"}))
            .await
            .unwrap();
        assert!(!r.success);
        let err = r.error.unwrap();
        assert!(err.contains("not in the safe list"), "err: {}", err);
    }

    #[tokio::test]
    async fn rejects_c_flag_even_with_safe_subcommand() {
        let t = GitTool::new();
        let r = t
            .execute(json!({"command": "status -c alias.fetch=!sh"}))
            .await
            .unwrap();
        assert!(!r.success);
        let err = r.error.unwrap();
        assert!(err.contains("arbitrary code"), "err: {}", err);
    }

    #[tokio::test]
    async fn rejects_upload_pack_flag() {
        let t = GitTool::new();
        let r = t
            .execute(json!({"command": "log --upload-pack=evil HEAD"}))
            .await
            .unwrap();
        assert!(!r.success);
        let err = r.error.unwrap();
        assert!(err.contains("arbitrary code"), "err: {}", err);
    }

    #[test]
    fn find_metachar_hits() {
        assert_eq!(GitTool::find_metachar("log; echo"), Some(';'));
        assert_eq!(GitTool::find_metachar("log | grep"), Some('|'));
        assert_eq!(GitTool::find_metachar("log --oneline -10"), None);
    }
}

impl Default for GitTool {
    fn default() -> Self {
        Self::new()
    }
}
