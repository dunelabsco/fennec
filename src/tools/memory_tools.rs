use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::memory::traits::{Memory, MemoryCategory, MemoryEntry};
use crate::tools::traits::{Tool, ToolResult};

// ---------------------------------------------------------------------------
// MemoryStoreTool
// ---------------------------------------------------------------------------

/// Tool that stores information in persistent memory.
pub struct MemoryStoreTool {
    memory: Arc<dyn Memory>,
}

impl MemoryStoreTool {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Save information to persistent memory. Use for facts, preferences, decisions worth remembering."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "A short, unique identifier for this memory entry (e.g. 'user_name', 'project_stack')"
                },
                "content": {
                    "type": "string",
                    "description": "The information to remember"
                },
                "category": {
                    "type": "string",
                    "description": "Category for the memory: 'core', 'daily', 'conversation', or a custom category",
                    "default": "core"
                }
            },
            "required": ["key", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if key.is_empty() || content.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Both 'key' and 'content' are required".to_string()),
            });
        }

        let category_str = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("core");

        let category = match category_str {
            "core" => MemoryCategory::Core,
            "daily" => MemoryCategory::Daily,
            "conversation" => MemoryCategory::Conversation,
            other => MemoryCategory::Custom(other.to_string()),
        };

        let entry = MemoryEntry {
            key: key.clone(),
            content: content.clone(),
            category,
            ..MemoryEntry::default()
        };

        match self.memory.store(entry).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!("Stored memory '{}' in category '{}'", key, category_str),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to store memory: {e}")),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryRecallTool
// ---------------------------------------------------------------------------

/// Tool that searches persistent memory.
pub struct MemoryRecallTool {
    memory: Arc<dyn Memory>,
}

impl MemoryRecallTool {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn description(&self) -> &str {
        "Search your memory for stored information."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find relevant memories"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("'query' is required".to_string()),
            });
        }

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        match self.memory.recall(&query, limit).await {
            Ok(entries) => {
                if entries.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No matching memories found.".to_string(),
                        error: None,
                    });
                }

                let mut output = String::new();
                for entry in &entries {
                    let cat = format!("{}", serde_json::to_string(&entry.category).unwrap_or_else(|_| "unknown".to_string()));
                    // Remove surrounding quotes from serialized category.
                    let cat = cat.trim_matches('"');
                    output.push_str(&format!("- [{}] {}: {}\n", cat, entry.key, entry.content));
                }

                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to search memory: {e}")),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryForgetTool
// ---------------------------------------------------------------------------

/// Tool that removes a specific entry from memory.
pub struct MemoryForgetTool {
    memory: Arc<dyn Memory>,
}

impl MemoryForgetTool {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn description(&self) -> &str {
        "Remove a specific entry from memory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key of the memory entry to remove"
                }
            },
            "required": ["key"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if key.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("'key' is required".to_string()),
            });
        }

        match self.memory.forget(&key).await {
            Ok(true) => Ok(ToolResult {
                success: true,
                output: format!("Removed memory '{}'", key),
                error: None,
            }),
            Ok(false) => Ok(ToolResult {
                success: true,
                output: format!("No memory found with key '{}'", key),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to forget memory: {e}")),
            }),
        }
    }
}
