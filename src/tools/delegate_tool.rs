use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::agent::subagent::SubagentManager;
use crate::memory::traits::Memory;
use crate::providers::traits::Provider;
use crate::tools::traits::{Tool, ToolResult};

/// Tool that lets the main agent delegate a task to an isolated subagent.
///
/// The subagent runs synchronously within the tool call (blocks until done).
pub struct DelegateTool {
    provider: Arc<dyn Provider>,
    memory: Arc<dyn Memory>,
    available_tools: Vec<Arc<dyn Tool>>,
}

impl DelegateTool {
    /// Create a new delegate tool.
    ///
    /// `available_tools` is the set of tools the subagent is allowed to use.
    /// Typically these are read-only tools (read_file, list_dir, shell for safe
    /// commands).
    pub fn new(
        provider: Arc<dyn Provider>,
        memory: Arc<dyn Memory>,
        available_tools: Vec<Arc<dyn Tool>>,
    ) -> Self {
        Self {
            provider,
            memory,
            available_tools,
        }
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to a subagent that runs in isolation with a limited tool set. \
         The subagent executes synchronously and returns its result."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task description for the subagent to execute"
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of tool names the subagent should use (defaults to all available read-only tools)"
                }
            },
            "required": ["task"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let task = match args.get("task").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: task".to_string()),
                });
            }
        };

        // Determine which tools to give the subagent.
        let requested_tools: Option<Vec<String>> = args
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let tools: Vec<Box<dyn Tool>> = if let Some(ref names) = requested_tools {
            self.available_tools
                .iter()
                .filter(|t| names.contains(&t.name().to_string()))
                .map(|t| Box::new(ArcToolWrapper(Arc::clone(t))) as Box<dyn Tool>)
                .collect()
        } else {
            // Default: give all available tools (typically read-only).
            self.available_tools
                .iter()
                .map(|t| Box::new(ArcToolWrapper(Arc::clone(t))) as Box<dyn Tool>)
                .collect()
        };

        if tools.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "no matching tools available for the subagent".to_string(),
                ),
            });
        }

        let manager = SubagentManager::new(
            Arc::clone(&self.provider),
            Arc::clone(&self.memory),
        );

        let result = manager.spawn(task, tools, 10).await?;

        Ok(ToolResult {
            success: result.success,
            output: result.output,
            error: if result.success {
                None
            } else {
                Some("subagent execution failed".to_string())
            },
        })
    }
}

/// Wrapper that implements `Tool` by forwarding to an `Arc<dyn Tool>`.
///
/// This allows us to create `Box<dyn Tool>` from `Arc<dyn Tool>` references
/// without cloning the underlying tool.
struct ArcToolWrapper(Arc<dyn Tool>);

#[async_trait]
impl Tool for ArcToolWrapper {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.0.parameters_schema()
    }

    fn is_read_only(&self) -> bool {
        self.0.is_read_only()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        self.0.execute(args).await
    }
}
