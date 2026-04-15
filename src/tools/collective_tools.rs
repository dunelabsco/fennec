use std::sync::Arc;

use async_trait::async_trait;

use crate::collective::search::{CollectiveSearch, SearchConfidence};
use crate::collective::scrub;
use crate::collective::traits::{CollectiveLayer, OutcomeReport};
use crate::memory::experience::{Attempt, Experience, ExperienceContext};
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

// ---------------------------------------------------------------------------
// CollectivePublishTool
// ---------------------------------------------------------------------------

/// Tool that lets the agent publish an experience to the collective
/// intelligence network so other agents can benefit from it.
///
/// Before publishing, searches the collective for similar experiences.
/// If a high-confidence match exists, the publish is skipped to avoid
/// duplicating knowledge.
pub struct CollectivePublishTool {
    collective: Arc<dyn CollectiveLayer>,
    search: Arc<CollectiveSearch>,
}

impl CollectivePublishTool {
    pub fn new(collective: Arc<dyn CollectiveLayer>, search: Arc<CollectiveSearch>) -> Self {
        Self { collective, search }
    }
}

#[async_trait]
impl Tool for CollectivePublishTool {
    fn name(&self) -> &str {
        "collective_publish"
    }

    fn description(&self) -> &str {
        "Publish an experience to the collective intelligence network (Plurum). Use this after completing a non-trivial task to help other agents. Include what you tried, what worked, what didn't, and any gotchas."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What was the task? e.g. 'Sign up on shipz.ai'"
                },
                "domain": {
                    "type": "string",
                    "description": "Category: web-automation, coding, devops, debugging, etc."
                },
                "what_worked": {
                    "type": "string",
                    "description": "The solution that worked. Be specific — include URLs, endpoints, commands."
                },
                "dead_ends": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Things you tried that did NOT work. e.g. 'Tried POST to /api/register — got 405'"
                },
                "gotchas": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Non-obvious things other agents should know. e.g. 'Must include Content-Type header'"
                },
                "tools_used": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Which tools you used: web_fetch, shell, browser, etc."
                }
            },
            "required": ["goal", "what_worked"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let goal = args["goal"].as_str().unwrap_or("").to_string();
        if goal.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("goal is required".into()),
            });
        }

        let what_worked = args["what_worked"].as_str().unwrap_or("").to_string();
        if what_worked.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("what_worked is required".into()),
            });
        }

        // Dedup check: search collective for similar experiences before publishing.
        match self.search.search(&goal, 3).await {
            Ok(result) => {
                if matches!(result.confidence, SearchConfidence::High) {
                    if let Some(top) = result.experiences.first() {
                        tracing::info!(
                            existing_goal = %top.result.goal,
                            score = top.final_score,
                            "Skipping publish — similar experience already exists"
                        );
                        return Ok(ToolResult {
                            success: true,
                            output: format!(
                                "Skipped publishing — the collective already has a similar experience: \
                                 \"{}\" (score: {:.2}). No need to duplicate.",
                                top.result.goal, top.final_score
                            ),
                            error: None,
                        });
                    }
                }
            }
            Err(e) => {
                // Don't block publishing if search fails — just log and continue.
                tracing::warn!("Dedup search failed, publishing anyway: {}", e);
            }
        }

        let domain = args["domain"].as_str().unwrap_or("general").to_string();

        let dead_ends: Vec<String> = args["dead_ends"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let gotchas: Vec<String> = args["gotchas"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let tools_used: Vec<String> = args["tools_used"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Build attempts from dead_ends + what_worked
        let mut attempts = Vec::new();
        for de in &dead_ends {
            attempts.push(Attempt {
                action: de.clone(),
                outcome: "Failed".to_string(),
                dead_end: true,
                insight: de.clone(),
            });
        }
        attempts.push(Attempt {
            action: what_worked.clone(),
            outcome: "Success".to_string(),
            dead_end: false,
            insight: what_worked.clone(),
        });

        let experience = Experience {
            id: uuid::Uuid::new_v4().to_string(),
            goal: goal.clone(),
            context: ExperienceContext {
                tools_used,
                environment: String::new(),
                constraints: String::new(),
            },
            attempts,
            solution: Some(what_worked),
            gotchas,
            tags: vec![domain.clone()],
            confidence: 0.8,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        // Scrub secrets before publishing
        let scrubbed = scrub::scrub_experience(&experience);

        tracing::info!(goal = %goal, domain = %domain, "collective_publish: sending to Plurum");

        match self.collective.publish(&scrubbed).await {
            Ok(id) => {
                tracing::info!(id = %id, goal = %goal, "collective_publish: SUCCESS — experience published");
                Ok(ToolResult {
                    success: true,
                    output: format!("Experience published to collective! ID: {}. Other agents can now benefit from your knowledge about: {}", id, goal),
                    error: None,
                })
            }
            Err(e) => {
                tracing::error!(goal = %goal, error = %e, "collective_publish: FAILED");
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Publish failed: {}", e)),
                })
            }
        }
    }
}
