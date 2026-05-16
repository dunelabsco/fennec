//! [`Tool`] adapter that invokes a WASM-side tool implementation.
//!
//! For each tool spec returned by a plugin's `register` call, the
//! registry constructs one `WasmTool` and hands the resulting
//! `Box<dyn Tool>` to the agent builder. When the agent decides to
//! invoke the tool, `execute` serialises the arguments to JSON,
//! locks the plugin's instance, calls the plugin's `invoke`, and
//! deserialises the result.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::tools::traits::{Tool, ToolResult};

use super::runtime::{ToolSpecOwned, WasmPluginInstance};

/// Wraps a single WASM-provided tool exposed to the agent.
pub struct WasmTool {
    instance: Arc<WasmPluginInstance>,
    /// Tool name as the LLM sees it.
    name: String,
    /// One-line description shown to the LLM.
    description: String,
    /// Cached JSON-decoded parameter schema. Validated at construction
    /// (a plugin shipping invalid JSON would otherwise fail at every
    /// agent call).
    parameters_schema: Value,
}

impl WasmTool {
    /// Build a `WasmTool` from one tool spec returned by the plugin's
    /// `register()` method.
    ///
    /// Returns `Err` if the plugin's `parameters_schema_json` field is
    /// not valid JSON. We surface this at load time rather than
    /// per-call so a busted plugin fails fast.
    pub fn from_spec(instance: Arc<WasmPluginInstance>, spec: ToolSpecOwned) -> Result<Self> {
        let parameters_schema: Value = if spec.parameters_schema_json.trim().is_empty() {
            // Empty schema → object with no properties. Some plugins
            // may return `""` for tools that take no arguments.
            serde_json::json!({"type": "object", "properties": {}})
        } else {
            serde_json::from_str(&spec.parameters_schema_json)
                .map_err(|e| anyhow::anyhow!(
                    "plugin '{}' tool '{}' has invalid parameters_schema_json: {e}",
                    instance.plugin_name,
                    spec.name
                ))?
        };
        Ok(Self {
            instance,
            name: spec.name,
            description: spec.description,
            parameters_schema,
        })
    }
}

#[async_trait]
impl Tool for WasmTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters_schema.clone()
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let args_json = args.to_string();
        match self
            .instance
            .call_invoke(&self.name, &args_json)
            .await
        {
            Ok(result_json) => {
                // The plugin returns its result as a JSON-encoded
                // string. We pass it through as-is — the agent's
                // tool result handling will further parse.
                Ok(ToolResult {
                    success: true,
                    output: result_json,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}
