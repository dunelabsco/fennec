//! Claude Code CLI delegation tool.
//!
//! Wraps the locally-installed `claude` binary so the Fennec agent can
//! delegate heavy coding work (multi-file refactors, feature implementation,
//! debugging deep call graphs) to Claude Code and get a single text result
//! back.
//!
//! Requires the user to have the Claude Code CLI installed and authenticated
//! on this machine. If `claude` is not on PATH, the tool disables itself at
//! startup rather than failing at call time.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::traits::{Tool, ToolResult};

/// Default per-call timeout. Claude Code sessions can take a while on big
/// tasks; default is generous but not infinite.
const DEFAULT_TIMEOUT_SECS: u64 = 600;

pub struct ClaudeCodeCliTool {
    /// Absolute path to the `claude` binary (or just "claude" if on PATH).
    binary: String,
    timeout: Duration,
}

impl ClaudeCodeCliTool {
    /// Return the tool only if `claude` is discoverable on PATH (or at the
    /// explicit override path). Returns None otherwise so wiring can skip.
    pub fn detect() -> Option<Self> {
        // Allow override via env var — useful when installed somewhere weird.
        if let Ok(path) = std::env::var("FENNEC_CLAUDE_BINARY") {
            if !path.is_empty() && PathBuf::from(&path).is_file() {
                return Some(Self {
                    binary: path,
                    timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                });
            }
        }
        if path_contains_binary("claude") {
            return Some(Self {
                binary: "claude".to_string(),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            });
        }
        None
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs.max(1));
        self
    }
}

fn path_contains_binary(bin: &str) -> bool {
    let paths = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    std::env::split_paths(&paths).any(|d| d.join(bin).is_file())
}

#[async_trait]
impl Tool for ClaudeCodeCliTool {
    fn name(&self) -> &str {
        "claude_code"
    }

    fn description(&self) -> &str {
        "Delegate a task to the Claude Code CLI installed on this host. \
         Claude Code has powerful multi-file code-editing abilities. Use \
         for: refactors across many files, new feature implementation, \
         deep debugging, test writing. One-shot: the prompt you give runs \
         non-interactively and the final text result comes back. Requires \
         `claude` to be installed and authenticated on this machine."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The task to delegate — be specific about files, goals, and constraints."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional directory to run claude in (defaults to current working dir)."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: prompt".to_string()),
                });
            }
        };
        let working_dir = args.get("working_dir").and_then(|v| v.as_str());

        let mut cmd = Command::new(&self.binary);
        cmd.arg("--print") // non-interactive mode
            .arg(&prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if let Some(d) = working_dir {
            cmd.current_dir(d);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to spawn {}: {}", self.binary, e)),
                });
            }
        };

        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(Ok(status)) => {
                let mut out = String::new();
                let mut err = String::new();
                stdout.read_to_string(&mut out).await.ok();
                stderr.read_to_string(&mut err).await.ok();
                let success = status.success();
                let mut payload = out.trim_end().to_string();
                if !err.trim().is_empty() {
                    payload.push_str("\n\n--- claude stderr ---\n");
                    payload.push_str(err.trim_end());
                }
                Ok(ToolResult {
                    success,
                    output: payload,
                    error: if success {
                        None
                    } else {
                        Some(format!("claude exited with {}", status))
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("waiting for claude failed: {}", e)),
            }),
            Err(_elapsed) => {
                let _ = child.kill().await;
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "claude timed out after {}s",
                        self.timeout.as_secs()
                    )),
                })
            }
        }
    }

    fn is_read_only(&self) -> bool {
        // claude-code edits files. Not read-only.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_contains_binary_positive_for_sh() {
        #[cfg(unix)]
        assert!(path_contains_binary("sh"));
    }

    #[test]
    fn path_contains_binary_negative_for_made_up() {
        assert!(!path_contains_binary("fennec-xyz-does-not-exist"));
    }

    #[test]
    fn detect_returns_some_or_none_without_panic() {
        // Host may or may not have claude installed — just verify call is safe.
        let _ = ClaudeCodeCliTool::detect();
    }

    #[test]
    fn with_timeout_overrides_default() {
        let t = ClaudeCodeCliTool {
            binary: "claude".to_string(),
            timeout: Duration::from_secs(60),
        }
        .with_timeout(123);
        assert_eq!(t.timeout.as_secs(), 123);
    }

    #[test]
    fn with_timeout_min_one_second() {
        let t = ClaudeCodeCliTool {
            binary: "claude".to_string(),
            timeout: Duration::from_secs(60),
        }
        .with_timeout(0);
        assert_eq!(t.timeout.as_secs(), 1);
    }

    #[tokio::test]
    async fn execute_rejects_missing_prompt() {
        let t = ClaudeCodeCliTool {
            binary: "/bin/echo".to_string(), // harmless fallback binary
            timeout: Duration::from_secs(5),
        };
        let r = t.execute(json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("prompt"));
    }

    #[tokio::test]
    async fn execute_spawns_binary_and_returns_output() {
        // Sanity: use /bin/echo as a stand-in for `claude` to exercise the
        // spawn + read path without requiring claude to be installed.
        let t = ClaudeCodeCliTool {
            binary: "/bin/echo".to_string(),
            timeout: Duration::from_secs(5),
        };
        let r = t
            .execute(json!({"prompt": "hello"}))
            .await
            .unwrap();
        assert!(r.success, "error: {:?}", r.error);
        // echo prints its args; --print "hello" becomes "--print hello".
        assert!(r.output.contains("hello"));
    }

    // Note: timeout behavior is exercised by code_exec_tool's tests; we skip
    // a flaky version here because the stand-in binary (/bin/sleep) rejects
    // the `--print` arg we always pass and exits before the timeout fires.

    #[test]
    fn is_read_only_false() {
        let t = ClaudeCodeCliTool {
            binary: "claude".to_string(),
            timeout: Duration::from_secs(10),
        };
        assert!(!t.is_read_only());
    }
}
