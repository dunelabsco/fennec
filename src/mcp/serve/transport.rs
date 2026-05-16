//! Newline-delimited JSON-RPC over stdio for the server side.
//!
//! Each line on stdin is one JSON-RPC message (request or
//! notification). Each line on stdout is one JSON-RPC response. No
//! Content-Length framing — the line-delimited shape is what every
//! current MCP client (Claude Desktop, Cursor, Codex, the
//! `@modelcontextprotocol/server-*` family) actually speaks even
//! though the spec also defines a framed variant.
//!
//! This module is dispatch-loop only. It reads, parses, hands the
//! message off to a `Handler` you supply, and writes the response.
//! It does not know what tools exist or what state the server has.

use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::mcp::types::{
    JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};

/// What a server-side handler must implement. Implementations are
/// `Send + Sync` because the dispatch loop owns the handler across
/// async boundaries.
#[async_trait]
pub trait Handler: Send + Sync {
    /// Handle a request. Return either a result `Value` (becomes the
    /// `result` field of the response) or an error response shape.
    async fn handle_request(&self, method: &str, params: Option<Value>)
        -> Result<Value, JsonRpcError>;

    /// Handle a notification. Notifications never produce a response.
    /// The default implementation logs and discards — most server
    /// implementations only need to act on `notifications/initialized`,
    /// which the MCP spec sends after the handshake.
    async fn handle_notification(&self, method: &str, _params: Option<Value>) {
        tracing::debug!(method = method, "MCP server: ignoring notification");
    }
}

/// Run the stdio dispatch loop until stdin closes (the client
/// disconnects) or a fatal I/O error.
///
/// The loop is single-threaded and serial: one request in, one
/// response out, no concurrent dispatch. MCP clients don't pipeline
/// — they wait for our response before sending the next message —
/// and a serial loop avoids ordering surprises in the `tools/call`
/// stream.
pub async fn run_stdio<H: Handler + 'static>(handler: H) -> Result<()> {
    // Use the async stdin/stdout. Each line is one JSON-RPC message;
    // we never have to buffer more than that.
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("reading from MCP stdin")?;
        if n == 0 {
            // Client closed stdin — clean shutdown.
            tracing::debug!("MCP stdin closed; exiting dispatch loop");
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try to parse as a request first (has `id`); if that fails,
        // try as a notification (no `id`); if both fail, log and
        // continue. We don't want a malformed line to kill the loop.
        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
            let response = dispatch_request(&handler, req).await;
            write_response(&mut stdout, &response).await?;
        } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(trimmed) {
            handler
                .handle_notification(&notif.method, notif.params)
                .await;
            // No response.
        } else {
            tracing::warn!(
                "MCP server: could not parse stdin line as request or notification: {}",
                truncate_for_log(trimmed)
            );
        }
    }
}

async fn dispatch_request<H: Handler>(
    handler: &H,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let id = req.id.clone();
    match handler.handle_request(&req.method, req.params).await {
        Ok(result) => JsonRpcResponse::success(id, result),
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(e),
        },
    }
}

async fn write_response<W>(stdout: &mut W, response: &JsonRpcResponse) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let line = serde_json::to_string(response)
        .context("serializing JSON-RPC response")?;
    stdout
        .write_all(line.as_bytes())
        .await
        .context("writing JSON-RPC response")?;
    stdout
        .write_all(b"\n")
        .await
        .context("writing newline after JSON-RPC response")?;
    stdout
        .flush()
        .await
        .context("flushing stdout after JSON-RPC response")?;
    Ok(())
}

/// Synchronous variant for tests: read pre-supplied input lines and
/// write responses to a `Write`. Lets unit tests exercise the
/// dispatch loop without spinning up a real stdio process. Same
/// dispatch semantics as `run_stdio`.
#[cfg(test)]
pub fn run_sync<R, W, H>(reader: R, mut writer: W, handler: H) -> Result<()>
where
    R: BufRead,
    W: Write,
    H: Handler + 'static,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        for line in reader.lines() {
            let line = line.context("reading test input")?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
                let response = dispatch_request(&handler, req).await;
                let json = serde_json::to_string(&response)?;
                writeln!(writer, "{}", json).context("writing test output")?;
                writer.flush().context("flushing test output")?;
            } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(trimmed) {
                handler
                    .handle_notification(&notif.method, notif.params)
                    .await;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
}

/// Standard JSON-RPC error codes used by the dispatcher.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

fn truncate_for_log(s: &str) -> String {
    if s.len() <= 200 {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(200).collect();
        t.push_str("…[truncated]");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    /// A handler that records every method+params pair it sees and
    /// returns a canned response so the test can assert dispatch
    /// shape without testing real tools.
    struct RecordingHandler {
        seen: Arc<Mutex<Vec<(String, Option<Value>)>>>,
    }

    #[async_trait]
    impl Handler for RecordingHandler {
        async fn handle_request(
            &self,
            method: &str,
            params: Option<Value>,
        ) -> Result<Value, JsonRpcError> {
            self.seen.lock().unwrap().push((method.to_string(), params));
            if method == "fail/me" {
                return Err(JsonRpcError {
                    code: error_codes::METHOD_NOT_FOUND,
                    message: "no such method".into(),
                    data: None,
                });
            }
            Ok(json!({"ok": true, "method": method}))
        }
    }

    fn handler() -> (Arc<Mutex<Vec<(String, Option<Value>)>>>, RecordingHandler) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let h = RecordingHandler {
            seen: Arc::clone(&seen),
        };
        (seen, h)
    }

    #[test]
    fn round_trip_single_request() {
        let (seen, h) = handler();
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"hello","params":{"x":42}}
"#;
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        let line = std::str::from_utf8(&output).unwrap().trim();
        let response: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["ok"], true);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "hello");
    }

    #[test]
    fn handler_error_becomes_error_response() {
        let (_, h) = handler();
        let input = r#"{"jsonrpc":"2.0","id":7,"method":"fail/me"}
"#;
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        let line = std::str::from_utf8(&output).unwrap().trim();
        let response: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(response["error"]["message"], "no such method");
        assert!(response["result"].is_null());
    }

    #[test]
    fn notifications_produce_no_response() {
        let (seen, h) = handler();
        let input = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        // Notification path doesn't reach handle_request, so no
        // recording AND no output.
        assert!(seen.lock().unwrap().is_empty());
        assert!(output.is_empty());
    }

    #[test]
    fn malformed_line_is_skipped() {
        let (seen, h) = handler();
        let input = "not json at all\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x\"}\n";
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        // The bad line was skipped; the good one produced one response.
        assert_eq!(seen.lock().unwrap().len(), 1);
        let response_count = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .count();
        assert_eq!(response_count, 1);
    }

    #[test]
    fn id_round_trips_for_string_ids() {
        let (_, h) = handler();
        let input = r#"{"jsonrpc":"2.0","id":"abc-123","method":"hi"}
"#;
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        let line = std::str::from_utf8(&output).unwrap().trim();
        let response: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(response["id"], "abc-123");
    }

    #[test]
    fn empty_lines_between_requests_skip_cleanly() {
        let (seen, h) = handler();
        let input = "\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x\"}\n\n";
        let mut output = Vec::new();
        run_sync(Cursor::new(input), &mut output, h).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
