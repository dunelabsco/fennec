use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::traits::{Tool, ToolResult};

/// A single task item in the ephemeral todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String, // "pending", "in_progress", "completed", "cancelled"
}

/// In-memory store for todo items (ephemeral per session).
struct TodoStore {
    items: Vec<TodoItem>,
}

impl TodoStore {
    fn new() -> Self {
        Self { items: Vec::new() }
    }
}

/// Tool for managing an ephemeral, in-memory task list for multi-step work.
pub struct TodoTool {
    store: Arc<Mutex<TodoStore>>,
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(TodoStore::new())),
        }
    }

    /// Format the current todo list for display.
    fn format_list(items: &[TodoItem]) -> String {
        if items.is_empty() {
            return "No tasks in the list.".to_string();
        }

        let mut out = String::new();
        for item in items {
            let marker = match item.status.as_str() {
                "in_progress" => "[>]",
                "completed" => "[x]",
                "cancelled" => "[-]",
                _ => "[ ]", // pending
            };
            out.push_str(&format!("{} {}: {}\n", marker, item.status, item.content));
        }
        out
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage a task list for multi-step work. Call with no parameters to read. Call with todos array to create/update."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Array of todo items to create or update",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for the task"
                            },
                            "content": {
                                "type": "string",
                                "description": "Description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current status of the task"
                            }
                        },
                        "required": ["id", "content", "status"]
                    }
                },
                "merge": {
                    "type": "boolean",
                    "description": "If true, merge with existing list (update by id, append new). If false, replace the entire list. Defaults to false.",
                    "default": false
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let todos = args.get("todos").and_then(|v| v.as_array());

        match todos {
            None => {
                // No todos parameter — return current list.
                let store = self.store.lock().unwrap();
                let output = Self::format_list(&store.items);
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Some(todo_array) => {
                let merge = args
                    .get("merge")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Parse the incoming todo items.
                let new_items: Vec<TodoItem> = todo_array
                    .iter()
                    .filter_map(|v| {
                        let id = v.get("id")?.as_str()?.to_string();
                        let content = v.get("content")?.as_str()?.to_string();
                        let status = v
                            .get("status")
                            .and_then(|s| s.as_str())
                            .unwrap_or("pending")
                            .to_string();
                        Some(TodoItem {
                            id,
                            content,
                            status,
                        })
                    })
                    .collect();

                let mut store = self.store.lock().unwrap();

                if merge {
                    // Merge: update existing items by id, append new ones.
                    for new_item in &new_items {
                        if let Some(existing) = store.items.iter_mut().find(|i| i.id == new_item.id)
                        {
                            existing.content = new_item.content.clone();
                            existing.status = new_item.status.clone();
                        } else {
                            store.items.push(new_item.clone());
                        }
                    }
                } else {
                    // Replace entire list.
                    store.items = new_items;
                }

                let output = Self::format_list(&store.items);
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_empty_list() {
        let tool = TodoTool::new();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "No tasks in the list.");
    }

    #[tokio::test]
    async fn test_create_todos() {
        let tool = TodoTool::new();
        let result = tool
            .execute(json!({
                "todos": [
                    {"id": "1", "content": "Do first thing", "status": "pending"},
                    {"id": "2", "content": "Do second thing", "status": "in_progress"}
                ]
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("[ ] pending: Do first thing"));
        assert!(result.output.contains("[>] in_progress: Do second thing"));
    }

    #[tokio::test]
    async fn test_replace_list() {
        let tool = TodoTool::new();

        // Create initial list.
        tool.execute(json!({
            "todos": [{"id": "1", "content": "Original", "status": "pending"}]
        }))
        .await
        .unwrap();

        // Replace list (merge=false by default).
        let result = tool
            .execute(json!({
                "todos": [{"id": "2", "content": "Replacement", "status": "completed"}]
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.output.contains("Original"));
        assert!(result.output.contains("[x] completed: Replacement"));
    }

    #[tokio::test]
    async fn test_merge_todos() {
        let tool = TodoTool::new();

        // Create initial list.
        tool.execute(json!({
            "todos": [
                {"id": "1", "content": "First", "status": "pending"},
                {"id": "2", "content": "Second", "status": "pending"}
            ]
        }))
        .await
        .unwrap();

        // Merge: update one, add one.
        let result = tool
            .execute(json!({
                "todos": [
                    {"id": "1", "content": "First (updated)", "status": "completed"},
                    {"id": "3", "content": "Third", "status": "in_progress"}
                ],
                "merge": true
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("[x] completed: First (updated)"));
        assert!(result.output.contains("[ ] pending: Second"));
        assert!(result.output.contains("[>] in_progress: Third"));
    }

    #[tokio::test]
    async fn test_read_after_create() {
        let tool = TodoTool::new();

        tool.execute(json!({
            "todos": [{"id": "1", "content": "My task", "status": "pending"}]
        }))
        .await
        .unwrap();

        // Read with empty args.
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("[ ] pending: My task"));
    }
}
