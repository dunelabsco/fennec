use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::Arc;

use super::transport::{HttpTransport, StdioTransport, Transport};
use super::types::{McpContent, McpToolResult, McpToolSpec};

/// Client for communicating with an MCP (Model Context Protocol) server.
pub struct McpClient {
    transport: Arc<dyn Transport>,
    /// Tools discovered during initialization.
    tools: Vec<McpToolSpec>,
}

impl McpClient {
    /// Connect to an MCP server over a child process's stdin/stdout.
    ///
    /// Spawns the given command, sends the `initialize` handshake, and
    /// discovers available tools.
    pub async fn connect_stdio(command: &str, args: &[&str]) -> Result<Self> {
        let transport = StdioTransport::new(command, args).await?;
        let transport: Arc<dyn Transport> = Arc::new(transport);
        Self::initialize(transport).await
    }

    /// Connect to an MCP server over HTTP.
    ///
    /// Sends the `initialize` handshake and discovers available tools.
    pub async fn connect_http(url: &str) -> Result<Self> {
        let transport: Arc<dyn Transport> = Arc::new(HttpTransport::new(url));
        Self::initialize(transport).await
    }

    /// Perform the MCP `initialize` handshake and discover tools.
    async fn initialize(transport: Arc<dyn Transport>) -> Result<Self> {
        // Send initialize request.
        let init_params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "fennec",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let _init_result = transport
            .send_request("initialize", Some(init_params))
            .await
            .context("MCP initialize handshake failed")?;

        // Send initialized notification.
        transport
            .send_notification("notifications/initialized", None)
            .await
            .context("sending initialized notification")?;

        // Discover tools.
        let tools_result = transport
            .send_request("tools/list", None)
            .await
            .context("listing MCP tools")?;

        let tools = Self::parse_tool_list(&tools_result)?;

        Ok(Self { transport, tools })
    }

    /// List all tools exposed by the MCP server.
    pub fn list_tools(&self) -> &[McpToolSpec] {
        &self.tools
    }

    /// Refresh the tool list from the server.
    pub async fn refresh_tools(&mut self) -> Result<&[McpToolSpec]> {
        let tools_result = self
            .transport
            .send_request("tools/list", None)
            .await
            .context("refreshing MCP tool list")?;

        self.tools = Self::parse_tool_list(&tools_result)?;
        Ok(&self.tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResult> {
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
    pub async fn shutdown(&self) -> Result<()> {
        self.transport
            .send_notification("shutdown", None)
            .await
            .context("sending MCP shutdown notification")
    }

    /// Parse the `tools/list` response into a vector of [`McpToolSpec`].
    fn parse_tool_list(result: &Value) -> Result<Vec<McpToolSpec>> {
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
            let description = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
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

        let tools = McpClient::parse_tool_list(&result).unwrap();
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
        let tools = McpClient::parse_tool_list(&result).unwrap();
        assert!(tools.is_empty());
    }
}
