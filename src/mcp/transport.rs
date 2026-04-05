use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::types::{JsonRpcRequest, JsonRpcResponse};

/// Trait for sending JSON-RPC requests to an MCP server.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a JSON-RPC request and return the response result value.
    async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value>;

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<()>;
}

/// Transport that communicates with an MCP server over a child process's
/// stdin/stdout using newline-delimited JSON-RPC.
pub struct StdioTransport {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    stdout: Arc<Mutex<BufReader<tokio::process::ChildStdout>>>,
    #[allow(dead_code)]
    child: Arc<Mutex<Child>>,
    next_id: AtomicU64,
}

impl StdioTransport {
    /// Spawn a subprocess and create a transport communicating over its
    /// stdin/stdout.
    pub async fn new(command: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("spawning MCP server: {} {:?}", command, args))?;

        let stdin = child
            .stdin
            .take()
            .context("failed to capture child stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("failed to capture child stdout")?;

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            child: Arc::new(Mutex::new(child)),
            next_id: AtomicU64::new(1),
        })
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);

        let mut payload = serde_json::to_string(&request)
            .context("serialising JSON-RPC request")?;
        payload.push('\n');

        // Write request.
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(payload.as_bytes())
                .await
                .context("writing to MCP server stdin")?;
            stdin.flush().await.context("flushing MCP server stdin")?;
        }

        // Read response line.
        let mut line = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            stdout
                .read_line(&mut line)
                .await
                .context("reading from MCP server stdout")?;
        }

        let response: JsonRpcResponse = serde_json::from_str(line.trim())
            .context("parsing JSON-RPC response from MCP server")?;

        if let Some(err) = response.error {
            anyhow::bail!("MCP JSON-RPC error ({}): {}", err.code, err.message);
        }

        response
            .result
            .context("MCP response missing result field")
    }

    async fn send_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);

        let mut payload = serde_json::to_string(&request)
            .context("serialising JSON-RPC notification")?;
        payload.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(payload.as_bytes())
            .await
            .context("writing notification to MCP server stdin")?;
        stdin.flush().await.context("flushing MCP server stdin")?;

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
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.into(),
            next_id: AtomicU64::new(1),
        }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value> {
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

        if let Some(err) = response.error {
            anyhow::bail!("MCP JSON-RPC error ({}): {}", err.code, err.message);
        }

        response
            .result
            .context("MCP HTTP response missing result field")
    }

    async fn send_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);

        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("sending HTTP JSON-RPC notification to MCP server")?;

        Ok(())
    }
}
