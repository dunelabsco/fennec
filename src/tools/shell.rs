use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::json;

use super::traits::{Tool, ToolResult};

/// A shell command execution tool with allowlist and forbidden path checks.
pub struct ShellTool {
    allowlist: Vec<String>,
    forbidden_paths: Vec<String>,
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new(allowlist: Vec<String>, forbidden_paths: Vec<String>, timeout_secs: u64) -> Self {
        Self {
            allowlist,
            forbidden_paths,
            timeout_secs,
        }
    }

    /// Extract the base command name from a shell command string.
    fn extract_command_name(command: &str) -> &str {
        command.split_whitespace().next().unwrap_or("")
    }

    /// Check if the command is in the allowlist.
    fn is_allowed(&self, command: &str) -> bool {
        let cmd_name = Self::extract_command_name(command);
        self.allowlist.iter().any(|a| a == cmd_name)
    }

    /// Check if the command references any forbidden paths.
    fn has_forbidden_path(&self, command: &str) -> Option<&str> {
        for fp in &self.forbidden_paths {
            if command.contains(fp.as_str()) {
                return Some(fp);
            }
        }
        None
    }

    /// Truncate output that exceeds the limit, keeping head + tail.
    fn truncate_output(output: &str, max_len: usize) -> String {
        if output.len() <= max_len {
            return output.to_string();
        }
        let half = max_len / 2;
        let head = &output[..half];
        let tail = &output[output.len() - half..];
        format!("{head}\n\n... [truncated {len} chars] ...\n\n{tail}", len = output.len() - max_len)
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Only allowlisted commands are permitted."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: command"))?;

        // Check allowlist.
        if !self.is_allowed(command) {
            let cmd_name = Self::extract_command_name(command);
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("command not allowed: {cmd_name}")),
            });
        }

        // Check forbidden paths.
        if let Some(path) = self.has_forbidden_path(command) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("forbidden path in command: {path}")),
            });
        }

        // Execute the command with a timeout.
        let timeout = tokio::time::Duration::from_secs(self.timeout_secs);
        let result = tokio::time::timeout(timeout, async {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .await
        })
        .await;

        match result {
            Err(_) => {
                // Timeout.
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("command timed out after {}s", self.timeout_secs)),
                })
            }
            Ok(Err(e)) => {
                bail!("failed to spawn command: {e}");
            }
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

                let truncated = Self::truncate_output(&combined, 10_000);

                Ok(ToolResult {
                    success: output.status.success(),
                    output: truncated,
                    error: if output.status.success() {
                        None
                    } else {
                        Some(format!("exit code: {}", output.status.code().unwrap_or(-1)))
                    },
                })
            }
        }
    }
}
