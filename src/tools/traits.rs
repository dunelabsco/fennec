use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Describes a tool's interface for the LLM (name, description, JSON Schema for parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Async trait that all tools must implement.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Machine-readable tool name (e.g. "shell", "read_file").
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's input parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given JSON arguments.
    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult>;

    /// Whether this tool only reads data (no side effects).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Build a [`ToolSpec`] from this tool's metadata.
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}
