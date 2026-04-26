//! Code execution tool — runs Python, Node.js, or Bash snippets in a
//! subprocess with a timeout.
//!
//! Not a true sandbox: code runs with the same privileges as the Fennec
//! process. Useful for data analysis, quick scripts, and calculations where
//! shell commands would be awkward. For untrusted input, use the shell
//! tool's allowlist or run Fennec inside a container.
//!
//! Value-add over the generic shell tool: the code is written to a temp
//! file (shell cmd-line escaping problems avoided), each language has a
//! dedicated runner, and stdout/stderr/exit_code are returned as distinct
//! fields in the output.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::proc_util::{
    run_with_timeout, scrub_sensitive_env, use_process_group,
};
use super::traits::{Tool, ToolResult};

/// Per-stream output cap. 1 MB of print-loop output is plenty for the LLM
/// to notice something is wrong; anything past this gets dropped.
const MAX_STDOUT_BYTES: usize = 1_000_000;
const MAX_STDERR_BYTES: usize = 200_000;

/// Which language runner to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Python,
    Node,
    Bash,
}

impl Language {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "python" | "py" | "python3" => Some(Self::Python),
            "node" | "javascript" | "js" | "nodejs" => Some(Self::Node),
            "bash" | "sh" | "shell" => Some(Self::Bash),
            _ => None,
        }
    }

    pub fn runner(&self) -> &'static str {
        match self {
            Self::Python => "python3",
            Self::Node => "node",
            Self::Bash => "bash",
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::Node => "js",
            Self::Bash => "sh",
        }
    }
}

pub struct CodeExecTool {
    /// Max execution time per call. Hard-killed if exceeded.
    timeout: Duration,
    /// Where to write temp code files (cleaned up after each run).
    temp_dir: PathBuf,
}

impl CodeExecTool {
    pub fn new(timeout_secs: u64, temp_dir: PathBuf) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs.max(1)),
            temp_dir,
        }
    }

    /// Write code to a temp file and return its path.
    async fn write_code(&self, code: &str, lang: Language) -> Result<PathBuf> {
        tokio::fs::create_dir_all(&self.temp_dir).await?;
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let path = self
            .temp_dir
            .join(format!("codeexec_{}.{}", ts, lang.file_extension()));
        tokio::fs::write(&path, code).await?;
        Ok(path)
    }

    /// Execute the code file with the language runner. Returns stdout,
    /// stderr, exit code, and a timed_out flag.
    ///
    /// Uses `proc_util::run_with_timeout` which drains stdout and stderr
    /// concurrently with the wait — avoids the pipe-buffer deadlock where
    /// a child that prints >64 KB blocks forever because `wait()` runs
    /// before anyone reads the pipe.
    ///
    /// Also:
    ///   - runs in its own process group so the timeout kill sends SIGKILL
    ///     to the whole subtree (bash backgrounds, python subprocess,
    ///     node worker threads) rather than just the direct child.
    ///   - scrubs known-sensitive env vars (API keys, bot tokens) so a
    ///     prompt-injected agent that writes `os.environ` can't exfiltrate.
    async fn run(&self, path: &std::path::Path, lang: Language) -> Result<ExecOutcome> {
        let mut cmd = Command::new(lang.runner());
        cmd.arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        scrub_sensitive_env(&mut cmd);
        use_process_group(&mut cmd);

        let child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", lang.runner()))?;

        let spawn = run_with_timeout(child, self.timeout, MAX_STDOUT_BYTES, MAX_STDERR_BYTES).await?;

        Ok(ExecOutcome {
            stdout: String::from_utf8_lossy(&spawn.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&spawn.stderr).into_owned(),
            exit_code: spawn.exit_code,
            timed_out: spawn.timed_out,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl ExecOutcome {
    pub fn format_output(&self, timeout_secs: u64) -> String {
        let mut lines = Vec::new();
        if self.timed_out {
            lines.push(format!("⏱ timed out after {}s", timeout_secs));
        } else if let Some(code) = self.exit_code {
            lines.push(format!("exit: {}", code));
        } else {
            lines.push("exit: unknown".to_string());
        }
        if !self.stdout.is_empty() {
            lines.push("--- stdout ---".to_string());
            lines.push(self.stdout.trim_end().to_string());
        }
        if !self.stderr.is_empty() {
            lines.push("--- stderr ---".to_string());
            lines.push(self.stderr.trim_end().to_string());
        }
        if self.stdout.is_empty() && self.stderr.is_empty() && !self.timed_out {
            lines.push("(no output)".to_string());
        }
        lines.join("\n")
    }
}

#[async_trait]
impl Tool for CodeExecTool {
    fn name(&self) -> &str {
        "code_exec"
    }

    fn description(&self) -> &str {
        "Execute a snippet of Python, Node.js, or Bash and return stdout, \
         stderr, and exit code. Each call runs in a fresh subprocess — no \
         state is shared between calls. Use for data analysis, calculations, \
         parsing, quick scripts. Not sandboxed: runs with Fennec's \
         privileges. For risky code, use shell with its allowlist instead."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The source code to run."
                },
                "language": {
                    "type": "string",
                    "enum": ["python", "node", "bash"],
                    "description": "Which runner to use."
                }
            },
            "required": ["code", "language"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let code = match args.get("code").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: code".to_string()),
                });
            }
        };
        let lang_str = match args.get("language").and_then(|v| v.as_str()) {
            Some(l) if !l.is_empty() => l,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: language".to_string()),
                });
            }
        };
        let lang = match Language::from_str(lang_str) {
            Some(l) => l,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "unsupported language: {} (expected python, node, or bash)",
                        lang_str
                    )),
                });
            }
        };

        let path = match self.write_code(&code, lang).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to write code: {}", e)),
                });
            }
        };

        let outcome = match self.run(&path, lang).await {
            Ok(o) => o,
            Err(e) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("execution failed: {}", e)),
                });
            }
        };

        // Clean up the temp file.
        let _ = tokio::fs::remove_file(&path).await;

        let success = !outcome.timed_out && outcome.exit_code == Some(0);
        Ok(ToolResult {
            success,
            output: outcome.format_output(self.timeout.as_secs()),
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_str_python_aliases() {
        assert_eq!(Language::from_str("python"), Some(Language::Python));
        assert_eq!(Language::from_str("py"), Some(Language::Python));
        assert_eq!(Language::from_str("python3"), Some(Language::Python));
        assert_eq!(Language::from_str("PYTHON"), Some(Language::Python));
    }

    #[test]
    fn language_from_str_node_aliases() {
        assert_eq!(Language::from_str("node"), Some(Language::Node));
        assert_eq!(Language::from_str("js"), Some(Language::Node));
        assert_eq!(Language::from_str("javascript"), Some(Language::Node));
        assert_eq!(Language::from_str("nodejs"), Some(Language::Node));
    }

    #[test]
    fn language_from_str_bash_aliases() {
        assert_eq!(Language::from_str("bash"), Some(Language::Bash));
        assert_eq!(Language::from_str("sh"), Some(Language::Bash));
        assert_eq!(Language::from_str("shell"), Some(Language::Bash));
    }

    #[test]
    fn language_from_str_unknown_returns_none() {
        assert_eq!(Language::from_str("ruby"), None);
        assert_eq!(Language::from_str(""), None);
    }

    #[test]
    fn language_runners() {
        assert_eq!(Language::Python.runner(), "python3");
        assert_eq!(Language::Node.runner(), "node");
        assert_eq!(Language::Bash.runner(), "bash");
    }

    #[test]
    fn language_extensions() {
        assert_eq!(Language::Python.file_extension(), "py");
        assert_eq!(Language::Node.file_extension(), "js");
        assert_eq!(Language::Bash.file_extension(), "sh");
    }

    #[test]
    fn format_outcome_success() {
        let o = ExecOutcome {
            stdout: "hello\n".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
        };
        let s = o.format_output(30);
        assert!(s.contains("exit: 0"));
        assert!(s.contains("--- stdout ---"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn format_outcome_timeout() {
        let o = ExecOutcome {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            timed_out: true,
        };
        let s = o.format_output(15);
        assert!(s.contains("timed out after 15s"));
    }

    #[test]
    fn format_outcome_nonzero_exit() {
        let o = ExecOutcome {
            stdout: String::new(),
            stderr: "ValueError\n".to_string(),
            exit_code: Some(1),
            timed_out: false,
        };
        let s = o.format_output(30);
        assert!(s.contains("exit: 1"));
        assert!(s.contains("--- stderr ---"));
        assert!(s.contains("ValueError"));
    }

    #[test]
    fn format_outcome_no_output() {
        let o = ExecOutcome {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
        };
        let s = o.format_output(30);
        assert!(s.contains("(no output)"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_code() {
        let t = CodeExecTool::new(10, std::env::temp_dir());
        let r = t.execute(json!({"language": "python"})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("code"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_language() {
        let t = CodeExecTool::new(10, std::env::temp_dir());
        let r = t.execute(json!({"code": "print(1)"})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("language"));
    }

    #[tokio::test]
    async fn execute_rejects_unknown_language() {
        let t = CodeExecTool::new(10, std::env::temp_dir());
        let r = t
            .execute(json!({"code": "x", "language": "ruby"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("unsupported"));
    }

    #[tokio::test]
    async fn execute_python_happy_path() {
        // Requires python3 in PATH. Skip gracefully if unavailable.
        if which("python3").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(15, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                "code": "print(2 + 2)",
                "language": "python"
            }))
            .await
            .unwrap();
        assert!(r.success, "output: {}", r.output);
        assert!(r.output.contains('4'));
    }

    #[tokio::test]
    async fn execute_bash_happy_path() {
        if which("bash").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(15, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                "code": "echo hi",
                "language": "bash"
            }))
            .await
            .unwrap();
        assert!(r.success, "output: {}", r.output);
        assert!(r.output.contains("hi"));
    }

    #[tokio::test]
    async fn execute_python_nonzero_exit() {
        if which("python3").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(15, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                "code": "import sys; sys.exit(2)",
                "language": "python"
            }))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("exit: 2"));
    }

    #[tokio::test]
    async fn execute_python_timeout_kills_process() {
        if which("python3").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(1, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                "code": "import time; time.sleep(30)",
                "language": "python"
            }))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("timed out"), "output: {}", r.output);
    }

    #[tokio::test]
    async fn write_code_creates_file_with_correct_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(10, tmp.path().to_path_buf());
        let path = t.write_code("print(1)", Language::Python).await.unwrap();
        assert!(path.exists());
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("py"));
    }

    /// Regression: a script that writes more than the pipe-buffer size
    /// (~64 KB on Linux) used to deadlock under the wait-then-read pattern,
    /// never exiting until the timeout fired. With concurrent drainage,
    /// the child completes normally and the output is captured (up to
    /// MAX_STDOUT_BYTES).
    #[tokio::test]
    async fn large_output_does_not_deadlock() {
        if which("python3").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(15, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                // Print 200 KB of output — well past any pipe buffer.
                "code": "import sys; sys.stdout.write('x' * 200_000)",
                "language": "python"
            }))
            .await
            .unwrap();
        assert!(r.success, "output: {}", r.output);
        assert!(!r.output.contains("timed out"));
    }

    /// Regression: sensitive env vars from the Fennec process must not
    /// leak into the subprocess. Prompt-injected code should NOT be able
    /// to read API keys via `os.environ`.
    #[tokio::test]
    async fn subprocess_does_not_inherit_sensitive_env() {
        if which("python3").is_none() {
            return;
        }
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-scrub-code-exec");
        }
        let tmp = tempfile::tempdir().unwrap();
        let t = CodeExecTool::new(10, tmp.path().to_path_buf());
        let r = t
            .execute(json!({
                "code": "import os; print(os.environ.get('ANTHROPIC_API_KEY', '<unset>'))",
                "language": "python"
            }))
            .await
            .unwrap();
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
        assert!(r.success, "output: {}", r.output);
        assert!(
            r.output.contains("<unset>"),
            "API key leaked to subprocess: {}",
            r.output
        );
        assert!(
            !r.output.contains("sk-ant-test-scrub-code-exec"),
            "secret leaked: {}",
            r.output
        );
    }

    /// Test helper: check if a binary exists on PATH.
    fn which(binary: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let p = dir.join(binary);
                if p.is_file() { Some(p) } else { None }
            })
        })
    }
}
