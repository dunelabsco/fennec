//! Shared subprocess-hardening helpers for tools that spawn child processes
//! (shell, git, code_exec, claude_code_cli).
//!
//! Addresses three recurring hazards:
//!
//! 1. **Pipe-buffer deadlock**: the "wait then read_to_string" pattern hangs
//!    once the child writes past the pipe buffer (~64 KB on Linux), because
//!    the child blocks waiting for a reader and our reader is gated behind
//!    `wait`. `run_with_timeout` drains stdout and stderr concurrently with
//!    the wait, with a byte cap so large output doesn't OOM the process.
//!
//! 2. **Orphan grandchildren on timeout**: `Child::kill` sends SIGKILL to
//!    the direct child only, so anything it spawned (bash backgrounds, node
//!    workers, python subprocesses) survives. `use_process_group` + `kill_process_group`
//!    put the child in its own PGID and kill the whole group on timeout.
//!
//! 3. **Secret leakage via inherited env**: subprocesses inherit the
//!    Fennec process env, which includes API keys (`ANTHROPIC_API_KEY`,
//!    `OPENAI_API_KEY`, …) and bot tokens. `scrub_sensitive_env` removes a
//!    curated list before spawn.

use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// Captured outcome of a subprocess run.
#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Environment variables we never want a spawned subprocess to see.
///
/// Kept as a static list (rather than a prefix glob) so the set is
/// auditable — anything added here is a deliberate policy decision.
pub const SENSITIVE_ENV_VARS: &[&str] = &[
    // LLM providers
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "KIMI_API_KEY",
    "MOONSHOT_API_KEY",
    "OPENROUTER_API_KEY",
    "OLLAMA_API_KEY",
    // Fennec-internal
    "FENNEC_SECRET_KEY",
    // Messaging channel tokens
    "TELEGRAM_BOT_TOKEN",
    "DISCORD_BOT_TOKEN",
    "DISCORD_TOKEN",
    "SLACK_BOT_TOKEN",
    "SLACK_APP_TOKEN",
    "WHATSAPP_ACCESS_TOKEN",
    "WHATSAPP_APP_SECRET",
    // Collective
    "PLURUM_API_KEY",
    // Cloud providers (user's host creds)
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GCP_CREDENTIALS",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "AZURE_CLIENT_SECRET",
    // Source control
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
];

/// Remove `SENSITIVE_ENV_VARS` from a `tokio::process::Command` before spawn.
pub fn scrub_sensitive_env(cmd: &mut Command) {
    for var in SENSITIVE_ENV_VARS {
        cmd.env_remove(var);
    }
}

/// Put the spawned child into its own process group on Unix so
/// `kill_process_group` can later signal the whole subtree.
pub fn use_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd; // no-op
    }
}

/// Kill `child` and every process in its group (Unix). Falls back to
/// killing just the direct child on other platforms.
pub async fn kill_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: libc::killpg is thread-safe; pid is the child's PID,
            // and we called `use_process_group` before spawn so PGID == PID.
            // Sending SIGKILL to a process we started is always valid.
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

/// Wait for `child` while concurrently draining stdout and stderr, with
/// per-stream byte caps. On timeout, sends SIGKILL to the child's process
/// group (Unix) and returns whatever output was captured.
///
/// Caller is responsible for configuring the Command with
/// `Stdio::piped()` for stdout/stderr, otherwise the returned buffers
/// will be empty.
pub async fn run_with_timeout(
    mut child: Child,
    timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<SpawnOutcome> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_reader = tokio::spawn(read_bounded(stdout, max_stdout_bytes));
    let stderr_reader = tokio::spawn(read_bounded(stderr, max_stderr_bytes));

    let wait_result = tokio::time::timeout(timeout, child.wait()).await;

    let (timed_out, exit_code) = match wait_result {
        Ok(Ok(status)) => (false, status.code()),
        Ok(Err(e)) => {
            // Still join the drain tasks so they don't leak.
            let _ = stdout_reader.await;
            let _ = stderr_reader.await;
            return Err(anyhow::anyhow!("waiting for child failed: {}", e));
        }
        Err(_elapsed) => {
            kill_process_group(&mut child).await;
            // Reap the zombie so the tokio::process::Child is cleaned up;
            // also lets the pipe readers see EOF and exit.
            let _ = child.wait().await;
            (true, None)
        }
    };

    let stdout_bytes = stdout_reader.await.unwrap_or_default();
    let stderr_bytes = stderr_reader.await.unwrap_or_default();

    Ok(SpawnOutcome {
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        exit_code,
        timed_out,
    })
}

/// Drain `reader` into a `Vec<u8>` capped at `cap` bytes. Once the cap is
/// reached, additional bytes are read and discarded so the child's pipe
/// doesn't block — critical for the "wait without reading" deadlock fix.
async fn read_bounded<R: AsyncRead + Unpin>(reader: Option<R>, cap: usize) -> Vec<u8> {
    let Some(mut r) = reader else {
        return Vec::new();
    };
    let mut buf = Vec::with_capacity(cap.min(4096));
    let mut tmp = [0u8; 4096];
    loop {
        match r.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < cap {
                    let take = (cap - buf.len()).min(n);
                    buf.extend_from_slice(&tmp[..take]);
                }
                // Continue reading past the cap so the child can keep
                // writing without blocking — we just drop the excess.
            }
            Err(_) => break,
        }
    }
    buf
}

/// Truncate an output string at a char boundary (safe for any UTF-8 input),
/// keeping head + tail with an ellipsis when the total exceeds `max_len`
/// chars.
pub fn truncate_head_tail(output: &str, max_len: usize) -> String {
    let total_chars = output.chars().count();
    if total_chars <= max_len {
        return output.to_string();
    }
    let half = max_len / 2;
    // Head: first `half` chars.
    let head: String = output.chars().take(half).collect();
    // Tail: last `half` chars (skip the first total-half).
    let tail: String = output.chars().skip(total_chars - half).collect();
    let omitted = total_chars - (2 * half);
    format!(
        "{}\n\n... [truncated {} chars] ...\n\n{}",
        head, omitted, tail
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn sensitive_env_list_non_empty_and_uppercase() {
        assert!(!SENSITIVE_ENV_VARS.is_empty());
        for v in SENSITIVE_ENV_VARS {
            assert_eq!(
                v.to_uppercase(),
                *v,
                "env var names should be uppercase: {}",
                v
            );
        }
    }

    #[tokio::test]
    async fn run_with_timeout_happy_path() {
        // Use a short echo that exits immediately.
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo hello")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        use_process_group(&mut cmd);
        let child = cmd.spawn().expect("spawn sh");
        let outcome = run_with_timeout(child, Duration::from_secs(5), 1024, 1024)
            .await
            .unwrap();
        assert!(!outcome.timed_out);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(&outcome.stdout).trim(), "hello");
    }

    #[tokio::test]
    async fn run_with_timeout_kills_on_elapsed() {
        // Start a sleep that would outlive the timeout.
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        use_process_group(&mut cmd);
        let child = cmd.spawn().expect("spawn sh");
        let outcome = run_with_timeout(child, Duration::from_millis(200), 1024, 1024)
            .await
            .unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, None);
    }

    #[tokio::test]
    async fn run_with_timeout_drains_large_output_without_deadlock() {
        // Write > 64 KB to stdout; the old wait-then-read pattern would
        // deadlock here because the pipe buffer fills and the child blocks.
        // Concurrent drainage means the child completes normally.
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("head -c 200000 /dev/zero | tr '\\0' 'x'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        use_process_group(&mut cmd);
        let child = cmd.spawn().expect("spawn sh");
        let outcome = run_with_timeout(child, Duration::from_secs(10), 100_000, 1024)
            .await
            .unwrap();
        assert!(
            !outcome.timed_out,
            "should NOT time out — concurrent drainage prevents pipe block"
        );
        assert_eq!(outcome.exit_code, Some(0));
        // Captured bytes are capped at max_stdout_bytes.
        assert!(outcome.stdout.len() <= 100_000);
    }

    #[tokio::test]
    async fn scrub_sensitive_env_removes_known_vars() {
        // Poke a variable into the current process env, spawn a child,
        // confirm the child does not see it after scrub.
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-test-scrub");
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo \"${ANTHROPIC_API_KEY:-<unset>}\"")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        scrub_sensitive_env(&mut cmd);
        use_process_group(&mut cmd);
        let child = cmd.spawn().expect("spawn sh");
        let outcome = run_with_timeout(child, Duration::from_secs(5), 1024, 1024)
            .await
            .unwrap();
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
        assert_eq!(outcome.exit_code, Some(0));
        let got = String::from_utf8_lossy(&outcome.stdout);
        assert!(
            got.contains("<unset>"),
            "ANTHROPIC_API_KEY leaked into child: {:?}",
            got
        );
        assert!(
            !got.contains("sk-test-scrub"),
            "secret leaked into child: {:?}",
            got
        );
    }

    #[test]
    fn truncate_head_tail_preserves_short() {
        assert_eq!(truncate_head_tail("hello", 100), "hello");
    }

    #[test]
    fn truncate_head_tail_cuts_long() {
        let s = "a".repeat(100);
        let out = truncate_head_tail(&s, 20);
        assert!(out.contains("truncated"));
        assert!(out.starts_with("aaaa"));
    }

    #[test]
    fn truncate_head_tail_utf8_safe() {
        // 100 multibyte chars — byte-slicing would panic; char-aware
        // truncation must not.
        let s = "日本語".repeat(50);
        let out = truncate_head_tail(&s, 20);
        assert!(out.contains("truncated"));
    }
}
