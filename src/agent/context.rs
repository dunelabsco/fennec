use chrono::Utc;

/// Builds the system prompt for the agent, combining identity, datetime,
/// memory context, and available tool names.
pub struct SystemPromptBuilder {
    identity_name: String,
    identity_persona: String,
}

impl SystemPromptBuilder {
    /// Create a new builder with the given identity name and persona.
    pub fn new(name: impl Into<String>, persona: impl Into<String>) -> Self {
        Self {
            identity_name: name.into(),
            identity_persona: persona.into(),
        }
    }

    /// Build the full system prompt string.
    ///
    /// - `memory_context`: recent memory entries formatted as strings.
    /// - `tool_names`: names of the tools available to the agent.
    pub fn build(&self, memory_context: &[String], tool_names: &[String]) -> String {
        let mut parts = Vec::new();

        // Identity
        parts.push(format!(
            "You are {}, {}",
            self.identity_name, self.identity_persona
        ));

        // Current datetime
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        parts.push(format!("Current datetime: {now}"));

        // Memory context
        if !memory_context.is_empty() {
            parts.push("Relevant memories:".to_string());
            for mem in memory_context {
                parts.push(format!("- {mem}"));
            }
        }

        // Available tools
        if !tool_names.is_empty() {
            parts.push(format!(
                "Available tools: {}",
                tool_names.join(", ")
            ));
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_basic() {
        let builder = SystemPromptBuilder::new("Fennec", "a helpful AI assistant");
        let prompt = builder.build(&[], &[]);
        assert!(prompt.contains("You are Fennec, a helpful AI assistant"));
        assert!(prompt.contains("Current datetime:"));
    }

    #[test]
    fn test_build_with_memory_and_tools() {
        let builder = SystemPromptBuilder::new("Fennec", "a helpful AI assistant");
        let memory = vec!["[user_name] Alice".to_string()];
        let tools = vec!["shell".to_string(), "read_file".to_string()];
        let prompt = builder.build(&memory, &tools);

        assert!(prompt.contains("Relevant memories:"));
        assert!(prompt.contains("[user_name] Alice"));
        assert!(prompt.contains("Available tools: shell, read_file"));
    }
}
