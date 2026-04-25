use anyhow::{Context, Result};
use serde::Deserialize;

use crate::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use crate::providers::traits::{ChatMessage, ChatRequest, Provider};

/// Extracts structured information from conversations and stores it in memory.
pub struct MemoryConsolidator {
    provider: Box<dyn Provider>,
}

/// JSON schema returned by the extraction prompt.
#[derive(Debug, Deserialize)]
struct ExtractionResult {
    daily_summary: String,
    #[serde(default)]
    core_facts: Vec<CoreFact>,
}

#[derive(Debug, Deserialize)]
struct CoreFact {
    key: String,
    content: String,
}

impl MemoryConsolidator {
    /// Create a consolidator that uses the given (cheap) provider for extraction.
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self { provider }
    }

    /// Analyse the most recent conversation messages and extract structured
    /// information, storing daily summaries and core facts into memory.
    pub async fn consolidate(
        &self,
        memory: &dyn Memory,
        conversation: &[ChatMessage],
        session_id: &str,
    ) -> Result<()> {
        // Take the last 10 messages (or fewer).
        let recent: Vec<&ChatMessage> = conversation.iter().rev().take(10).collect::<Vec<_>>();
        let recent: Vec<&&ChatMessage> = recent.iter().rev().collect();

        if recent.is_empty() {
            return Ok(());
        }

        // Build conversation text for the prompt.
        let mut conversation_text = String::new();
        for msg in &recent {
            let role = &msg.role;
            let content = msg.content.as_deref().unwrap_or("(no content)");
            conversation_text.push_str(&format!("{role}: {content}\n"));
        }

        let prompt = format!(
            "Analyze this conversation and extract structured information.\n\
             Return valid JSON with this exact format:\n\
             {{\"daily_summary\": \"Brief summary of what happened\", \"core_facts\": [{{\"key\": \"unique-key\", \"content\": \"The fact or preference\"}}]}}\n\
             Only include core_facts for genuinely new, durable information (user preferences, project decisions, important facts). \
             If nothing new was learned, return an empty core_facts array.\n\
             Conversation:\n{conversation_text}"
        );

        let messages = vec![ChatMessage::user(prompt)];
        let request = ChatRequest {
            system: None,
            messages: &messages,
            tools: None,
            max_tokens: 1024,
            temperature: 0.0,
            thinking_level: crate::agent::thinking::ThinkingLevel::Off,
        };

        let response = self
            .provider
            .chat(request)
            .await
            .context("consolidation provider call")?;

        let raw_text = response.content.unwrap_or_default();

        let timestamp = chrono::Utc::now().to_rfc3339();

        // Try to parse as JSON. On failure, store the raw text as a daily summary.
        match serde_json::from_str::<ExtractionResult>(&raw_text) {
            Ok(extraction) => {
                // Store daily summary.
                let summary_key = format!("daily-{session_id}-{timestamp}");
                let entry = MemoryEntry {
                    key: summary_key,
                    content: extraction.daily_summary,
                    category: MemoryCategory::Daily,
                    session_id: Some(session_id.to_string()),
                    ..MemoryEntry::default()
                };
                memory.store(entry).await.context("storing daily summary")?;

                // Store core facts.
                for fact in extraction.core_facts {
                    let entry = MemoryEntry {
                        key: fact.key,
                        content: fact.content,
                        category: MemoryCategory::Core,
                        session_id: Some(session_id.to_string()),
                        ..MemoryEntry::default()
                    };
                    memory.store(entry).await.context("storing core fact")?;
                }
            }
            Err(_) => {
                // Graceful fallback: store the raw response as a daily summary.
                let summary_key = format!("daily-{session_id}-{timestamp}");
                let entry = MemoryEntry {
                    key: summary_key,
                    content: raw_text,
                    category: MemoryCategory::Daily,
                    session_id: Some(session_id.to_string()),
                    ..MemoryEntry::default()
                };
                memory.store(entry).await.context("storing fallback summary")?;
            }
        }

        Ok(())
    }
}
