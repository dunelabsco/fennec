use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::traits::{Tool, ToolResult};

/// Safe git commands that are allowed to be executed.
const SAFE_COMMANDS: &[&str] = &[
    "status", "diff", "log", "branch", "show", "blame", "stash",
    "rev-parse", "describe", "shortlog", "tag", "ls-files", "ls-tree",
    "cat-file", "reflog",
];

/// Dangerous git sub-commands or flags that are explicitly blocked.
const BLOCKED_PATTERNS: &[&str] = &[
    "push", "reset --hard", "clean -f", "clean -fd", "clean -fx",
    "checkout .", "restore .", "branch -D", "push --force",
    "push -f", "rebase", "merge", "pull", "fetch", "clone",
    "remote add", "remote set-url", "config",
];

/// A tool that runs safe, read-only git commands.
pub struct GitTool;

impl GitTool {
    pub fn new() -> Self {
        Self
    }

    /// Extract the git sub-command (first word) from a command string.
    fn extract_subcommand(command: &str) -> &str {
        command.split_whitespace().next().unwrap_or("")
    }

    /// Check if a command is safe to run.
    fn is_safe(command: &str) -> bool {
        let trimmed = command.trim();

        // Block explicitly dangerous patterns.
        for pattern in BLOCKED_PATTERNS {
            if trimmed.starts_with(pattern) || trimmed == *pattern {
                return false;
            }
        }

        // Check the sub-command against the safe list.
        let subcmd = Self::extract_subcommand(trimmed);
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
         Dangerous commands like push, reset --hard, and clean are blocked."
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

        if !Self::is_safe(command) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "blocked: git command '{}' is not in the safe list",
                    command
                )),
            });
        }

        let full_command = format!("git {command}");
        let timeout = tokio::time::Duration::from_secs(30);
        let result = tokio::time::timeout(timeout, async {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&full_command)
                .output()
                .await
        })
        .await;

        match result {
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("git command timed out after 30s".to_string()),
            }),
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("failed to spawn git: {e}")),
            }),
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let combined = if stderr.is_empty() {
                    stdout.to_string()
                } else if stdout.is_empty() {
                    stderr.to_string()
                } else {
                    format!("{stdout}\n--- stderr ---\n{stderr}")
                };

                // Truncate very large outputs.
                let truncated = if combined.len() > 50_000 {
                    format!(
                        "{}\n\n... [truncated, total {} bytes]",
                        &combined[..50_000],
                        combined.len()
                    )
                } else {
                    combined
                };

                Ok(ToolResult {
                    success: output.status.success(),
                    output: truncated,
                    error: if output.status.success() {
                        None
                    } else {
                        Some(format!(
                            "exit code: {}",
                            output.status.code().unwrap_or(-1)
                        ))
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands() {
        assert!(GitTool::is_safe("status"));
        assert!(GitTool::is_safe("diff"));
        assert!(GitTool::is_safe("log --oneline -10"));
        assert!(GitTool::is_safe("branch -a"));
        assert!(GitTool::is_safe("show HEAD"));
        assert!(GitTool::is_safe("blame src/main.rs"));
        assert!(GitTool::is_safe("stash list"));
    }

    #[test]
    fn test_blocked_commands() {
        assert!(!GitTool::is_safe("push"));
        assert!(!GitTool::is_safe("push origin main"));
        assert!(!GitTool::is_safe("push --force"));
        assert!(!GitTool::is_safe("reset --hard"));
        assert!(!GitTool::is_safe("clean -f"));
        assert!(!GitTool::is_safe("clean -fd"));
        assert!(!GitTool::is_safe("checkout ."));
        assert!(!GitTool::is_safe("branch -D feature"));
        assert!(!GitTool::is_safe("rebase main"));
        assert!(!GitTool::is_safe("merge feature"));
        assert!(!GitTool::is_safe("pull"));
        assert!(!GitTool::is_safe("config user.name"));
    }

    #[test]
    fn test_empty_command() {
        assert!(!GitTool::is_safe(""));
    }
}
