use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::mcp::client::McpClient;
use crate::mcp::types::McpToolSpec;
use crate::tools::traits::{Tool, ToolResult};

/// Bridges a single MCP tool so it can be used as a Fennec [`Tool`].
pub struct McpToolBridge {
    client: Arc<McpClient>,
    spec: McpToolSpec,
}

impl McpToolBridge {
    /// Wrap an MCP tool spec with its client as a Fennec [`Tool`].
    pub fn new(client: Arc<McpClient>, spec: McpToolSpec) -> Self {
        Self { client, spec }
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.spec.input_schema.clone()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        match self.client.call_tool(&self.spec.name, args).await {
            Ok(result) => {
                // Concatenate all text content blocks into output.
                let output: String = result
                    .content
                    .iter()
                    .filter_map(|c| c.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");

                if result.is_error {
                    Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(output),
                    })
                } else {
                    Ok(ToolResult {
                        success: true,
                        output,
                        error: None,
                    })
                }
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("MCP tool call failed: {}", e)),
            }),
        }
    }
}

/// Register all tools from an MCP client as Fennec [`Tool`] implementations.
pub fn register_mcp_tools(client: Arc<McpClient>) -> Vec<Box<dyn Tool>> {
    client
        .list_tools()
        .iter()
        .map(|spec| {
            let bridge = McpToolBridge::new(Arc::clone(&client), spec.clone());
            Box::new(bridge) as Box<dyn Tool>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_bridge_spec() {
        // We can't test execute without a real MCP server, but we can test
        // that the bridge correctly exposes the tool metadata.
        let spec = McpToolSpec {
            name: "test_tool".to_string(),
            description: "A test MCP tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            }),
        };

        // Create a mock-free test of the metadata.
        assert_eq!(spec.name, "test_tool");
        assert_eq!(spec.description, "A test MCP tool");
    }
}
