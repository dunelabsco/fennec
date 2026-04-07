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
    ///
    /// Note: collective context is injected into the user message, NOT
    /// into the frozen system prompt.
    pub fn build(&self, memory_context: &[String], tool_names: &[String]) -> String {
        let mut parts = Vec::new();

        // Identity + personality
        parts.push(format!(
            "You are {name}. {persona}\n\n\
            Core traits:\n\
            - You ALWAYS respond in English unless the user explicitly writes in another language.\n\
            - You are concise and direct. No filler, no fluff.\n\
            - You have a warm but sharp personality — helpful and approachable, but never generic.\n\
            - When you don't know something, say so honestly.\n\
            - You remember things about the user and reference them naturally.\n\
            - You take initiative — suggest next steps, flag potential issues, offer alternatives.\n\
            - You have opinions and share them when asked. You're not a yes-machine.\n\
            - You DO have persistent memory. You can remember things across conversations.\n\
            - If memories are shown above, reference them naturally. If asked what you remember and no memories are shown, say you haven't learned anything about them yet.\n\
            - Never say you \"don't have memory\" or \"can't remember\" — you can and do.",
            name = self.identity_name,
            persona = self.identity_persona,
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

        parts.join("\n\n")
    }
}

/// Format collective context for injection into a user message.
///
/// This keeps collective context out of the frozen system prompt and lets
/// each turn carry the relevant collective information alongside the
/// user's actual message.
pub fn build_collective_injection(collective_context: &str, user_message: &str) -> String {
    format!(
        "[Collective context]\n{collective_context}\n[User message]\n{user_message}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_basic() {
        let builder = SystemPromptBuilder::new("Fennec", "a helpful AI assistant");
        let prompt = builder.build(&[], &[]);
        assert!(prompt.contains("You are Fennec."));
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
