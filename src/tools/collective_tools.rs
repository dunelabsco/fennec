use std::sync::Arc;

use async_trait::async_trait;

use crate::collective::search::CollectiveSearch;
use crate::collective::traits::{CollectiveLayer, OutcomeReport};
use crate::tools::traits::{Tool, ToolResult};

// ---------------------------------------------------------------------------
// CollectiveSearchTool
// ---------------------------------------------------------------------------

/// Tool that lets the agent explicitly search the collective intelligence
/// network for experiences related to a query.
pub struct CollectiveSearchTool {
    search: Arc<CollectiveSearch>,
}

impl CollectiveSearchTool {
    pub fn new(search: Arc<CollectiveSearch>) -> Self {
        Self { search }
    }
}

#[async_trait]
impl Tool for CollectiveSearchTool {
    fn name(&self) -> &str {
        "collective_search"
    }

    fn description(&self) -> &str {
        "Search the collective intelligence network for experiences related to a query"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for"
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
        match self.search.search(&query, 5).await {
            Ok(result) => {
                if result.experiences.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No relevant experiences found in the collective.".into(),
                        error: None,
                    });
                }
                let mut output = format!(
                    "Found {} experiences (confidence: {:?}):\n\n",
                    result.experiences.len(),
                    result.confidence
                );
                for (i, r) in result.experiences.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. [{}] {}\n",
                        i + 1,
                        r.source_label(),
                        r.result.goal
                    ));
                    if let Some(ref sol) = r.result.solution {
                        output.push_str(&format!("   Solution: {}\n", sol));
                    }
                    if !r.result.gotchas.is_empty() {
                        output.push_str(&format!(
                            "   Gotchas: {}\n",
                            r.result.gotchas.join("; ")
                        ));
                    }
                    output.push_str(&format!(
                        "   Trust: {:.2} | Relevance: {:.2}\n\n",
                        r.result.trust_score, r.final_score
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
// CollectiveReportTool
// ---------------------------------------------------------------------------

/// Tool that lets the agent report the outcome of applying a collective
/// experience (success/failure feedback loop).
pub struct CollectiveReportTool {
    collective: Arc<dyn CollectiveLayer>,
}

impl CollectiveReportTool {
    pub fn new(collective: Arc<dyn CollectiveLayer>) -> Self {
        Self { collective }
    }
}

#[async_trait]
impl Tool for CollectiveReportTool {
    fn name(&self) -> &str {
        "collective_report"
    }

    fn description(&self) -> &str {
        "Report the outcome of applying a collective experience (success or failure)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "experience_id": {
                    "type": "string",
                    "description": "The ID of the experience to report on"
                },
                "success": {
                    "type": "boolean",
                    "description": "Whether the experience was successful"
                },
                "notes": {
                    "type": "string",
                    "description": "Optional notes about the outcome"
                }
            },
            "required": ["experience_id", "success"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let experience_id = args["experience_id"].as_str().unwrap_or("").to_string();
        if experience_id.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("No experience_id provided".into()),
            });
        }

        let success = args["success"].as_bool().unwrap_or(false);
        let notes = args["notes"].as_str().map(|s| s.to_string());

        let report = OutcomeReport {
            success,
            execution_time_ms: None,
            error_message: if success {
                None
            } else {
                notes.clone()
            },
            context_notes: notes,
        };

        match self.collective.report_outcome(&experience_id, &report).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!(
                    "Outcome reported for experience {}: {}",
                    experience_id,
                    if success { "success" } else { "failure" }
                ),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Report failed: {}", e)),
            }),
        }
    }
}
