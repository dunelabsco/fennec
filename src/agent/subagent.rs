use std::sync::Arc;

use anyhow::Result;

use crate::memory::traits::Memory;
use crate::providers::traits::Provider;
use crate::tools::traits::Tool;

use super::AgentBuilder;

/// Result of a subagent task execution.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// The final text output from the subagent.
    pub output: String,
    /// Names of tools that the subagent invoked during execution.
    pub tools_used: Vec<String>,
    /// Whether the subagent completed successfully (did not exceed iteration cap
    /// or encounter an error).
    pub success: bool,
}

/// Manages spawning isolated subagent instances that run specific tasks.
pub struct SubagentManager {
    memory: Arc<dyn Memory>,
    provider: Arc<dyn Provider>,
}

impl SubagentManager {
    /// Create a new manager that shares the given provider and memory.
    pub fn new(provider: Arc<dyn Provider>, memory: Arc<dyn Memory>) -> Self {
        Self { memory, provider }
    }

    /// Spawn a subagent to execute the given task.
    ///
    /// The subagent is constructed with a limited tool set and a maximum
    /// iteration cap. It runs synchronously (blocks until done) and returns
    /// the result.
    pub async fn spawn(
        &self,
        task: &str,
        tools: Vec<Box<dyn Tool>>,
        max_iterations: usize,
    ) -> Result<SubagentResult> {
        let max_iterations = if max_iterations == 0 {
            10
        } else {
            max_iterations
        };

        // Track which tools were provided so we can report them.
        let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        let mut agent = AgentBuilder::new()
            .provider(Arc::clone(&self.provider))
            .memory(Arc::clone(&self.memory))
            .tools(tools)
            .max_tool_iterations(max_iterations)
            .identity_name("Fennec-Subagent")
            .identity_persona("A focused sub-agent executing a delegated task.")
            .build()?;

        match agent.turn(task).await {
            Ok(output) => Ok(SubagentResult {
                output,
                tools_used: tool_names,
                success: true,
            }),
            Err(e) => Ok(SubagentResult {
                output: format!("Subagent failed: {e}"),
                tools_used: tool_names,
                success: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_result_debug() {
        let result = SubagentResult {
            output: "done".to_string(),
            tools_used: vec!["read_file".to_string()],
            success: true,
        };
        let dbg = format!("{:?}", result);
        assert!(dbg.contains("done"));
        assert!(dbg.contains("read_file"));
    }
}
