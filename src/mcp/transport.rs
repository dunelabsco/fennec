use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

const HTTP_TIMEOUT_SECS: u64 = 30;

/// Trait for sending JSON-RPC requests to an MCP server.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a JSON-RPC request and return the response result value.
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value>;

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()>;
}

/// stdin + stdout held together so a single mutex serializes the full
/// write-then-read roundtrip. Previously each half had its own mutex,
/// which meant two concurrent `send_request`s could both write, then
/// race for the stdout lock — and the wrong task could read the wrong
/// reply.
struct StdioIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Transport that communicates with an MCP server over a child process's
/// stdin/stdout using newline-delimited JSON-RPC.
pub struct StdioTransport {
    io: Arc<Mutex<StdioIo>>,
    /// Keeps the child handle alive so `kill_on_drop(true)` fires when the
    /// transport is dropped. The stderr handle is taken out during
    /// construction and consumed by a logger task; only the bare child
    /// remains here.
    _child: Arc<Mutex<Child>>,
    next_id: AtomicU64,
}

impl StdioTransport {
    /// Spawn a subprocess and create a transport communicating over its
    /// stdin/stdout.
    ///
    /// The child is spawned with `kill_on_drop(true)` so dropping the
    /// transport terminates the server process instead of leaking a
    /// zombie. stderr was previously discarded to `/dev/null`, hiding
    /// every server-side error; it's now streamed to `tracing::warn!`
    /// under the `[mcp:<label>]` prefix.
    pub async fn new(command: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning MCP server: {} {:?}", command, args))?;

        let stdin = child.stdin.take().context("failed to capture child stdin")?;
        let stdout = child.stdout.take().context("failed to capture child stdout")?;
        let stderr = child.stderr.take().context("failed to capture child stderr")?;

        let label = derive_stdio_label(command);
        spawn_stderr_logger(label, stderr);

        let io = StdioIo {
            stdin,
            stdout: BufReader::new(stdout),
        };

        Ok(Self {
            io: Arc::new(Mutex::new(io)),
            _child: Arc::new(Mutex::new(child)),
            next_id: AtomicU64::new(1),
        })
    }
}

/// Best-effort label for a stdio command — used only in stderr log
/// prefixes. For `/usr/local/bin/mcp-server-filesystem` the label is
/// `mcp-server-filesystem`.
fn derive_stdio_label(command: &str) -> String {
    std::path::Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command)
        .to_string()
}

/// Spawn a background task that reads every line the child writes to
/// stderr and logs it at WARN. The task exits when stderr closes (child
/// exit).
fn spawn_stderr_logger(label: String, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::warn!("[mcp:{}] {}", label, line);
        }
    });
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let mut payload =
            serde_json::to_string(&request).context("serialising JSON-RPC request")?;
        payload.push('\n');

        // Hold the io lock for the whole write-read roundtrip. This
        // serializes concurrent requests but guarantees that a reply is
        // read by the same task that wrote the matching request.
        let mut io = self.io.lock().await;
        io.stdin
            .write_all(payload.as_bytes())
            .await
            .context("writing to MCP server stdin")?;
        io.stdin.flush().await.context("flushing MCP server stdin")?;

        // Read lines until we see a valid JSON-RPC response whose id
        // matches ours. Skip non-JSON lines (misbehaving servers that log
        // to stdout instead of stderr) and skip responses with mismatched
        // ids (stale replies from a prior request that timed out etc.)
        // with a debug log, so a poorly-behaved server can't desync the
        // stream.
        loop {
            let mut line = String::new();
            let n = io
                .stdout
                .read_line(&mut line)
                .await
                .context("reading from MCP server stdout")?;
            if n == 0 {
                anyhow::bail!("MCP server closed stdout before responding (id {})", id);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response: JsonRpcResponse = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(
                        "MCP server emitted non-JSON-RPC stdout line ({}): {}",
                        e,
                        trimmed
                    );
                    continue;
                }
            };
            if response.id.as_u64() != Some(id) {
                tracing::debug!(
                    "MCP response with unexpected id {:?} while waiting for {}; skipping",
                    response.id,
                    id
                );
                continue;
            }
            if let Some(err) = response.error {
                anyhow::bail!("MCP JSON-RPC error ({}): {}", err.code, err.message);
            }
            return response
                .result
                .context("MCP response missing result field");
        }
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        // Emit a proper JSON-RPC notification (no `id` field). The old
        // implementation reused JsonRpcRequest and consumed an id from the
        // counter, which (a) is a spec violation and (b) caused some
        // servers to send an unsolicited response that the next
        // `send_request` would read as if it were its own reply.
        let notif = JsonRpcNotification::new(method, params);
        let mut payload =
            serde_json::to_string(&notif).context("serialising JSON-RPC notification")?;
        payload.push('\n');

        let mut io = self.io.lock().await;
        io.stdin
            .write_all(payload.as_bytes())
            .await
            .context("writing notification to MCP server stdin")?;
        io.stdin.flush().await.context("flushing MCP server stdin")?;
        Ok(())
    }
}

/// Transport that communicates with an MCP server over HTTP POST.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    next_id: AtomicU64,
}

impl HttpTransport {
    /// Create a new HTTP transport pointing at the given URL.
    ///
    /// The reqwest client has a 30s overall timeout so a hung server
    /// can't make the call hang indefinitely (the old
    /// `reqwest::Client::new()` had no timeout).
    pub fn new(url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .expect("build reqwest client for MCP HTTP transport");
        Self {
            client,
            url: url.into(),
            next_id: AtomicU64::new(1),
        }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);

        let http_response = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("sending HTTP JSON-RPC request to MCP server")?;

        let status = http_response.status();
        let body = http_response
            .text()
            .await
            .context("reading MCP HTTP response body")?;

        if !status.is_success() {
            anyhow::bail!("MCP HTTP error ({}): {}", status, body);
        }

        let response: JsonRpcResponse = serde_json::from_str(&body)
            .context("parsing JSON-RPC response from MCP HTTP server")?;

        if response.id.as_u64() != Some(id) {
            tracing::debug!(
                "MCP HTTP response id {:?} does not match request id {}",
                response.id,
                id
            );
            // HTTP is strictly request/response (single round trip), so a
            // mismatched id is a server bug — surface it rather than loop.
        }

        if let Some(err) = response.error {
            anyhow::bail!("MCP JSON-RPC error ({}): {}", err.code, err.message);
        }

        response
            .result
            .context("MCP HTTP response missing result field")
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);

        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&notif)
            .send()
            .await
            .context("sending HTTP JSON-RPC notification to MCP server")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_serializes_without_id_field() {
        // Regression: the old send_notification serialized a
        // JsonRpcRequest with an `id` field, which is a spec violation.
        let n = JsonRpcNotification::new("notifications/initialized", None);
        let s = serde_json::to_string(&n).unwrap();
        assert!(!s.contains("\"id\""), "notification must not carry id: {}", s);
        assert!(s.contains("\"method\":\"notifications/initialized\""));
    }

    #[test]
    fn request_serializes_with_id_field() {
        let r = JsonRpcRequest::new(7, "tools/list", None);
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"id\":7"));
        assert!(s.contains("\"method\":\"tools/list\""));
    }

    #[test]
    fn derive_stdio_label_strips_path() {
        assert_eq!(derive_stdio_label("/usr/local/bin/mcp-server-fs"), "mcp-server-fs");
        assert_eq!(derive_stdio_label("mcp-server-fs"), "mcp-server-fs");
        assert_eq!(derive_stdio_label("./tools/server"), "server");
    }
}
