use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::Arc;

use super::transport::{HttpTransport, StdioTransport, Transport};
use super::types::{McpContent, McpToolResult, McpToolSpec};

/// Max bytes we accept for a tool's description. A malicious MCP server
/// could otherwise advertise a tool with a 100 KB "description" that the
/// agent would inline into its tool-spec prompt, burning context or
/// smuggling prompt-injection payloads.
const MAX_TOOL_DESCRIPTION_BYTES: usize = 4096;

/// Client for communicating with an MCP (Model Context Protocol) server.
pub struct McpClient {
    transport: Arc<dyn Transport>,
    /// Tools discovered during initialization.
    tools: Vec<McpToolSpec>,
    /// Short identifier for this server instance, used to namespace the
    /// tool names we expose to the agent (see `namespaced_tool_name`).
    /// For stdio transport this is derived from the command path; for
    /// HTTP it's the URL host.
    server_label: String,
}

impl McpClient {
    /// Connect to an MCP server over a child process's stdin/stdout.
    ///
    /// Spawns the given command, sends the `initialize` handshake, and
    /// discovers available tools.
    pub async fn connect_stdio(command: &str, args: &[&str]) -> Result<Self> {
        let label = derive_stdio_label(command);
        let transport = StdioTransport::new(command, args).await?;
        let transport: Arc<dyn Transport> = Arc::new(transport);
        Self::initialize(transport, label).await
    }

    /// Connect to an MCP server over HTTP.
    pub async fn connect_http(url: &str) -> Result<Self> {
        let label = derive_http_label(url);
        let transport: Arc<dyn Transport> = Arc::new(HttpTransport::new(url)?);
        Self::initialize(transport, label).await
    }

    /// Perform the MCP `initialize` handshake and discover tools.
    async fn initialize(transport: Arc<dyn Transport>, server_label: String) -> Result<Self> {
        let init_params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "fennec",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let init_result = transport
            .send_request("initialize", Some(init_params))
            .await
            .context("MCP initialize handshake failed")?;

        // Tools capability is technically required for a server that
        // advertises tools, but some servers skip publishing it. Warn
        // rather than fail so a mostly-compliant server still works.
        if init_result
            .get("capabilities")
            .and_then(|c| c.get("tools"))
            .is_none()
        {
            tracing::warn!(
                "[mcp:{}] server did not advertise 'tools' capability; tools/list may fail",
                server_label
            );
        }

        transport
            .send_notification("notifications/initialized", None)
            .await
            .context("sending initialized notification")?;

        let tools_result = transport
            .send_request("tools/list", None)
            .await
            .context("listing MCP tools")?;

        let tools = Self::parse_tool_list(&tools_result, &server_label)?;

        Ok(Self {
            transport,
            tools,
            server_label,
        })
    }

    /// List all tools exposed by the MCP server.
    pub fn list_tools(&self) -> &[McpToolSpec] {
        &self.tools
    }

    /// The label used to namespace tool names from this server. See
    /// [`Self::namespaced_tool_name`].
    pub fn server_label(&self) -> &str {
        &self.server_label
    }

    /// Compose the namespaced Fennec tool name for a given MCP tool name.
    ///
    /// MCP servers are an external trust boundary: the audit flagged
    /// that a hostile or compromised server could advertise a tool
    /// called `read_file` or `shell` that would shadow Fennec's native
    /// tool with the same name. Namespacing forces every MCP tool into
    /// the `mcp_<label>_<name>` prefix so the agent's tool registry
    /// can't be silently hijacked.
    pub fn namespaced_tool_name(&self, tool_name: &str) -> String {
        format!(
            "mcp_{}_{}",
            sanitize_identifier(&self.server_label),
            sanitize_identifier(tool_name)
        )
    }

    /// Refresh the tool list from the server.
    pub async fn refresh_tools(&mut self) -> Result<&[McpToolSpec]> {
        let tools_result = self
            .transport
            .send_request("tools/list", None)
            .await
            .context("refreshing MCP tool list")?;

        self.tools = Self::parse_tool_list(&tools_result, &self.server_label)?;
        Ok(&self.tools)
    }

    /// Call a tool on the MCP server by its original (un-namespaced) name.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResult> {
        let params = json!({
            "name": name,
            "arguments": arguments
        });

        let result = self
            .transport
            .send_request("tools/call", Some(params))
            .await
            .with_context(|| format!("calling MCP tool '{}'", name))?;

        Self::parse_tool_result(&result)
    }

    /// Send the shutdown notification to the MCP server.
    ///
    /// Note: the MCP spec does not define a `shutdown` RPC — client
    /// shutdown is expected to happen via transport close. This method
    /// is kept as a best-effort hint for servers that happen to listen
    /// for it, and because dropping the transport will kill the child
    /// process via `kill_on_drop`.
    pub async fn shutdown(&self) -> Result<()> {
        self.transport
            .send_notification("shutdown", None)
            .await
            .context("sending MCP shutdown notification")
    }

    /// Parse the `tools/list` response into a vector of [`McpToolSpec`].
    ///
    /// The `server_label` is used only for log context when a tool spec
    /// triggers a warning (e.g. an oversized description).
    fn parse_tool_list(result: &Value, server_label: &str) -> Result<Vec<McpToolSpec>> {
        let tools_array = result
            .get("tools")
            .and_then(|t| t.as_array())
            .context("MCP tools/list response missing 'tools' array")?;

        let mut specs = Vec::with_capacity(tools_array.len());
        for tool in tools_array {
            let name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .context("MCP tool missing 'name'")?
                .to_string();
            let raw_description = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let description = if raw_description.len() > MAX_TOOL_DESCRIPTION_BYTES {
                tracing::warn!(
                    "[mcp:{}] tool '{}' has an oversized description ({} bytes); truncating",
                    server_label,
                    name,
                    raw_description.len()
                );
                truncate_at_char_boundary(raw_description, MAX_TOOL_DESCRIPTION_BYTES)
            } else {
                raw_description.to_string()
            };
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or(json!({"type": "object"}));

            specs.push(McpToolSpec {
                name,
                description,
                input_schema,
            });
        }

        Ok(specs)
    }

    /// Parse a `tools/call` response into an [`McpToolResult`].
    fn parse_tool_result(result: &Value) -> Result<McpToolResult> {
        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        let content = if let Some(content_array) = result.get("content").and_then(|c| c.as_array())
        {
            content_array
                .iter()
                .map(|block| {
                    let type_ = block
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("text")
                        .to_string();
                    let text = block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string());
                    McpContent { type_, text }
                })
                .collect()
        } else {
            Vec::new()
        };

        Ok(McpToolResult { content, is_error })
    }
}

/// Best-effort label for a stdio command — used for tool-name namespacing
/// and log prefixes. For `/usr/local/bin/mcp-server-filesystem` the label
/// is `mcp-server-filesystem`.
fn derive_stdio_label(command: &str) -> String {
    std::path::Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command)
        .to_string()
}

/// Best-effort label for an HTTP MCP endpoint — uses the host portion of
/// the URL if parseable, otherwise a literal "http".
fn derive_http_label(url: &str) -> String {
    // A tiny URL-host parser — avoids pulling `url` as a direct dep just
    // for a label. Accepts `http://host/...`, `https://host:port/...`, etc.
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = after_scheme
        .split(|c: char| c == '/' || c == ':' || c == '?' || c == '#')
        .next()
        .unwrap_or("http");
    if host.is_empty() {
        "http".to_string()
    } else {
        host.to_string()
    }
}

/// Replace every non-`[a-zA-Z0-9_]` char with `_` so the namespaced tool
/// name passes OpenAI's / Anthropic's `^[a-zA-Z0-9_-]{1,64}$` regex. Keeps
/// `-` as-is because OpenAI accepts it; upstream tool names commonly
/// contain hyphens.
fn sanitize_identifier(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Truncate `s` at or below `max_bytes`, stepping back to a UTF-8 char
/// boundary if the cut would split a multibyte character. Appends an
/// ellipsis marker.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_list() {
        let result = json!({
            "tools": [
                {
                    "name": "read_file",
                    "description": "Read a file from disk",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"}
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "write_file",
                    "description": "Write a file to disk",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "content": {"type": "string"}
                        }
                    }
                }
            ]
        });

        let tools = McpClient::parse_tool_list(&result, "test").unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[0].description, "Read a file from disk");
        assert_eq!(tools[1].name, "write_file");
    }

    #[test]
    fn test_parse_tool_result_success() {
        let result = json!({
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "isError": false
        });

        let tool_result = McpClient::parse_tool_result(&result).unwrap();
        assert!(!tool_result.is_error);
        assert_eq!(tool_result.content.len(), 1);
        assert_eq!(tool_result.content[0].type_, "text");
        assert_eq!(
            tool_result.content[0].text.as_deref(),
            Some("Hello, world!")
        );
    }

    #[test]
    fn test_parse_tool_result_error() {
        let result = json!({
            "content": [
                {"type": "text", "text": "Something went wrong"}
            ],
            "isError": true
        });

        let tool_result = McpClient::parse_tool_result(&result).unwrap();
        assert!(tool_result.is_error);
        assert_eq!(tool_result.content.len(), 1);
    }

    #[test]
    fn test_parse_tool_list_empty() {
        let result = json!({"tools": []});
        let tools = McpClient::parse_tool_list(&result, "test").unwrap();
        assert!(tools.is_empty());
    }

    /// Regression: a malicious server advertising a 100 KB description
    /// must not inject it verbatim into the agent's tool catalog.
    #[test]
    fn oversized_description_is_truncated() {
        let huge = "A".repeat(MAX_TOOL_DESCRIPTION_BYTES + 1000);
        let result = json!({
            "tools": [
                { "name": "x", "description": huge, "inputSchema": {} }
            ]
        });
        let tools = McpClient::parse_tool_list(&result, "test").unwrap();
        assert_eq!(tools.len(), 1);
        assert!(
            tools[0].description.len() <= MAX_TOOL_DESCRIPTION_BYTES + 64,
            "description not truncated (got {} bytes)",
            tools[0].description.len()
        );
        assert!(tools[0].description.contains("truncated"));
    }

    /// Regression: description truncation at a multibyte char boundary
    /// must not panic.
    #[test]
    fn oversized_description_with_multibyte_chars_does_not_panic() {
        // Fill with 3-byte CJK chars — guaranteed to land at a
        // non-boundary byte if we naively slice at MAX.
        let huge = "日".repeat(MAX_TOOL_DESCRIPTION_BYTES);
        let result = json!({
            "tools": [
                { "name": "x", "description": huge, "inputSchema": {} }
            ]
        });
        let tools = McpClient::parse_tool_list(&result, "test").unwrap();
        assert!(tools[0].description.contains("truncated"));
    }

    #[test]
    fn sanitize_identifier_keeps_alphanumeric_and_dash() {
        assert_eq!(sanitize_identifier("read_file"), "read_file");
        assert_eq!(sanitize_identifier("read-file"), "read-file");
        assert_eq!(sanitize_identifier("abc123"), "abc123");
    }

    #[test]
    fn sanitize_identifier_replaces_other_chars() {
        assert_eq!(sanitize_identifier("a:b:c"), "a_b_c");
        assert_eq!(sanitize_identifier("a b c"), "a_b_c");
        assert_eq!(sanitize_identifier("a.b/c"), "a_b_c");
        assert_eq!(sanitize_identifier("a\"b;c"), "a_b_c");
    }

    #[test]
    fn derive_http_label_extracts_host() {
        assert_eq!(derive_http_label("http://example.com/"), "example.com");
        assert_eq!(derive_http_label("https://mcp.example.com:8080/rpc"), "mcp.example.com");
        assert_eq!(derive_http_label("https://1.2.3.4:9/path?x=1"), "1.2.3.4");
        assert_eq!(derive_http_label("not-a-url"), "not-a-url");
    }

    #[test]
    fn derive_stdio_label_strips_path() {
        assert_eq!(
            derive_stdio_label("/usr/local/bin/mcp-server-fs"),
            "mcp-server-fs"
        );
        assert_eq!(derive_stdio_label("mcp-server-fs"), "mcp-server-fs");
    }
}
