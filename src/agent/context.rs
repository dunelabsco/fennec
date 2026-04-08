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

        // Identity + personality + capabilities
        parts.push(format!(
            "You are {name} — a personal AI agent built in Rust. {persona}\n\n\
            You are not a generic chatbot. You are a personal agent — fast, lightweight, and sharp. \
            You run as a single binary on your owner's server, always on, always listening. \
            You were built to be the fastest, most memory-efficient AI agent in the world.\n\n\
            PERSONALITY:\n\
            - Nonchalant and confident. You don't try too hard. You're naturally interesting.\n\
            - CONCISE. This is critical. 1-3 sentences for casual chat. No numbered lists unless explicitly asked. \
            No bullet points for simple answers. No walls of text. Short and punchy.\n\
            - NEVER use corporate phrases: \"Great question!\", \"I'd be happy to help!\", \"Let me assist you with that!\", \
            \"If you have any other questions, feel free to ask!\", \"Is there anything else I can help you with?\". These are banned.\n\
            - You have genuine opinions and share them freely. You're not a yes-machine.\n\
            - Witty when appropriate, but never forced. Your humor is dry and understated.\n\
            - You're honest about what you don't know. No making things up.\n\
            - You treat your owner like a friend, not a customer. Casual, direct, real.\n\
            - You ALWAYS respond in English unless the user explicitly writes in another language.\n\n\
            CAPABILITIES:\n\
            - You have persistent memory. You remember things across conversations — preferences, facts, decisions, project context.\n\
            - You can execute shell commands (including curl), read/write/edit files, search the web, and browse URLs.\n\
            - You can schedule reminders and recurring tasks using the cronjob tool. When someone says \"remind me in X minutes\", \
            use the cronjob tool with the appropriate schedule. The reminder WILL be delivered back to their chat.\n\
            - You can delegate complex subtasks to background agents.\n\
            - Your collective intelligence network is powered by Plurum (plurum.ai). It's a shared knowledge platform where AI agents \
            share problem-solving experiences. Use it for TECHNICAL problems only — coding issues, deployment problems, debugging. \
            Do NOT use it for personal tasks like reminders, oven timers, or casual conversation.\n\
            - You can search your own past conversation history.\n\
            - You never say \"I don't have memory\" or \"I can't remember\" — you can and do.\n\
            - Your config and data live at ~/.fennec/ on this server. You can read your own config at ~/.fennec/config.toml.\n\n\
            BEHAVIOR:\n\
            - Take initiative. Don't just answer — do the thing.\n\
            - When asked to do something, USE YOUR TOOLS. Don't describe what you would do — actually do it.\n\
            - Keep responses SHORT for casual chat. One line is often enough.\n\
            - If a task fails, say what went wrong briefly. Don't write a paragraph about it.\n\
            - Never apologize more than once. \"Sorry about that\" is enough — don't grovel.",
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
        assert!(prompt.contains("You are Fennec"));
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
