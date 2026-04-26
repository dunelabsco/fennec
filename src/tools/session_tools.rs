use std::sync::Arc;

use async_trait::async_trait;

use crate::sessions::SessionStore;
use crate::tools::traits::{Tool, ToolResult};

// ---------------------------------------------------------------------------
// SessionSearchTool
// ---------------------------------------------------------------------------

/// Tool that searches past conversation history via FTS5.
pub struct SessionSearchTool {
    store: Arc<SessionStore>,
}

impl SessionSearchTool {
    pub fn new(store: Arc<SessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past conversation history across all sessions using full-text search"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args["query"].as_str().unwrap_or("").to_string();
        if query.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("No query provided".into()),
            });
        }

        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        match self.store.search(&query, limit).await {
            Ok(hits) => {
                if hits.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No matching messages found in session history.".into(),
                        error: None,
                    });
                }

                let mut output = format!("Found {} matching messages:\n\n", hits.len());
                for (i, hit) in hits.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. [session:{}] [{}] {}\n   timestamp: {} | score: {:.2}\n\n",
                        i + 1,
                        &hit.session_id[..8.min(hit.session_id.len())],
                        hit.role,
                        hit.content,
                        hit.timestamp,
                        hit.score,
                    ));
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
                error: Some(format!("Search failed: {}", e)),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// SessionListTool
// ---------------------------------------------------------------------------

/// Tool that lists recent sessions.
pub struct SessionListTool {
    store: Arc<SessionStore>,
}

impl SessionListTool {
    pub fn new(store: Arc<SessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SessionListTool {
    fn name(&self) -> &str {
        "session_list"
    }

    fn description(&self) -> &str {
        "List recent conversation sessions"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of sessions to return (default 10)"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        match self.store.list_sessions(limit).await {
            Ok(sessions) => {
                if sessions.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No sessions found.".into(),
                        error: None,
                    });
                }

                let mut output = format!("Found {} sessions:\n\n", sessions.len());
                for (i, s) in sessions.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. id: {}\n   channel: {}\n   started: {}\n   ended: {}\n   summary: {}\n\n",
                        i + 1,
                        s.id,
                        s.channel,
                        s.started_at,
                        s.ended_at.as_deref().unwrap_or("(active)"),
                        s.summary.as_deref().unwrap_or("(none)"),
                    ));
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
                error: Some(format!("List failed: {}", e)),
            }),
        }
    }
}
