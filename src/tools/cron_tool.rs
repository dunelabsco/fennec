use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cron::jobs::{parse_schedule, CronJob, JobStore};

use super::traits::{Tool, ToolResult};

/// Origin information captured from the current message context so that
/// cron-fired results can be routed back to the correct channel/chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronOrigin {
    pub channel: String,
    pub chat_id: String,
}

/// LLM-callable tool for creating, listing, and removing scheduled tasks.
pub struct CronTool {
    store_path: PathBuf,
    default_origin: Arc<Mutex<Option<CronOrigin>>>,
}

impl CronTool {
    /// Create a new `CronTool`.
    ///
    /// * `store_path` - path to the JSON file backing the job store.
    /// * `default_origin` - shared origin that the gateway sets before each
    ///   agent turn so the tool knows where to route cron results.
    pub fn new(store_path: PathBuf, default_origin: Arc<Mutex<Option<CronOrigin>>>) -> Self {
        Self {
            store_path,
            default_origin,
        }
    }

    /// Get a clone of the shared origin arc (for the gateway to hold).
    pub fn origin_handle(&self) -> Arc<Mutex<Option<CronOrigin>>> {
        Arc::clone(&self.default_origin)
    }

    /// Load the job store from disk.
    fn load_store(&self) -> Result<JobStore> {
        let mut store = JobStore::new(self.store_path.clone());
        store.load()?;
        Ok(store)
    }

    /// Normalise a schedule string: if it lacks the "every " prefix, add it.
    fn normalise_schedule(schedule: &str) -> String {
        let trimmed = schedule.trim();
        if trimmed.starts_with("every ") {
            trimmed.to_string()
        } else {
            format!("every {}", trimmed)
        }
    }

    fn execute_create(&self, prompt: Option<&str>, schedule: Option<&str>) -> Result<ToolResult> {
        let prompt = match prompt {
            Some(p) if !p.is_empty() => p,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: prompt".to_string()),
                });
            }
        };

        let schedule_raw = match schedule {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: schedule".to_string()),
                });
            }
        };

        let schedule_str = Self::normalise_schedule(schedule_raw);
        let interval_secs = match parse_schedule(&schedule_str) {
            Some(s) => s,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "invalid schedule '{}'. Use formats like '5m', '1h', 'every 30m', 'every 1h'.",
                        schedule_raw
                    )),
                });
            }
        };

        // Read origin from shared state.
        let origin = self.default_origin.lock().unwrap().clone();

        let job_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let now = Utc::now();
        let next_run = now + chrono::Duration::seconds(interval_secs as i64);

        let job = CronJob {
            id: job_id.clone(),
            name: prompt.chars().take(60).collect::<String>(),
            schedule: schedule_str.clone(),
            command: prompt.to_string(),
            enabled: true,
            last_run: Some(now.to_rfc3339()),
            origin_channel: origin.as_ref().map(|o| o.channel.clone()),
            origin_chat_id: origin.as_ref().map(|o| o.chat_id.clone()),
        };

        let mut store = self.load_store()?;
        store.add_job(job);
        store.save()?;

        let output = format!(
            "Scheduled job created.\n  ID: {}\n  Schedule: {}\n  Next run: {}\n  Prompt: {}",
            job_id,
            schedule_str,
            next_run.format("%Y-%m-%d %H:%M:%S UTC"),
            prompt,
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    fn execute_list(&self) -> Result<ToolResult> {
        let store = self.load_store()?;
        let jobs = store.list_jobs();

        if jobs.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No scheduled jobs.".to_string(),
                error: None,
            });
        }

        let mut lines = Vec::new();
        for job in jobs {
            let status = if job.enabled { "enabled" } else { "disabled" };
            let origin = match (&job.origin_channel, &job.origin_chat_id) {
                (Some(ch), Some(cid)) => format!(" -> {}:{}", ch, cid),
                _ => String::new(),
            };
            lines.push(format!(
                "- [{}] {} | {} | {}{}\n  Prompt: {}",
                job.id, status, job.schedule, job.name, origin, job.command,
            ));
        }

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    fn execute_remove(&self, job_id: Option<&str>) -> Result<ToolResult> {
        let job_id = match job_id {
            Some(id) if !id.is_empty() => id,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: job_id".to_string()),
                });
            }
        };

        let mut store = self.load_store()?;
        if store.remove_job(job_id) {
            store.save()?;
            Ok(ToolResult {
                success: true,
                output: format!("Job '{}' removed.", job_id),
                error: None,
            })
        } else {
            Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Job '{}' not found.", job_id)),
            })
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cronjob"
    }

    fn description(&self) -> &str {
        "Create, list, or remove scheduled tasks. Use this when the user asks you to remind them, schedule something, or do something later."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "remove"],
                    "description": "The action to perform"
                },
                "prompt": {
                    "type": "string",
                    "description": "What to do when the job fires (for create)"
                },
                "schedule": {
                    "type": "string",
                    "description": "When to fire: '5m', '1h', 'every 30m', 'every 1h' (for create)"
                },
                "job_id": {
                    "type": "string",
                    "description": "Job ID to remove (for remove)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: action".to_string()),
                });
            }
        };

        match action {
            "create" => {
                let prompt = args.get("prompt").and_then(|v| v.as_str());
                let schedule = args.get("schedule").and_then(|v| v.as_str());
                self.execute_create(prompt, schedule)
            }
            "list" => self.execute_list(),
            "remove" => {
                let job_id = args.get("job_id").and_then(|v| v.as_str());
                self.execute_remove(job_id)
            }
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "unknown action '{}'. Use 'create', 'list', or 'remove'.",
                    other
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tool(dir: &TempDir) -> CronTool {
        let path = dir.path().join("cron_jobs.json");
        let origin = Arc::new(Mutex::new(Some(CronOrigin {
            channel: "telegram".to_string(),
            chat_id: "12345".to_string(),
        })));
        CronTool::new(path, origin)
    }

    #[tokio::test]
    async fn test_create_and_list() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool
            .execute(json!({
                "action": "create",
                "prompt": "Remind me to drink water",
                "schedule": "every 30m"
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Scheduled job created"));

        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Remind me to drink water"));
        assert!(result.output.contains("telegram:12345"));
    }

    #[tokio::test]
    async fn test_create_bare_duration() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool
            .execute(json!({
                "action": "create",
                "prompt": "Check the oven",
                "schedule": "5m"
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("every 5m"));
    }

    #[tokio::test]
    async fn test_create_and_remove() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool
            .execute(json!({
                "action": "create",
                "prompt": "Do something",
                "schedule": "1h"
            }))
            .await
            .unwrap();
        assert!(result.success);

        // Extract job ID from output.
        let id_line = result
            .output
            .lines()
            .find(|l| l.contains("ID:"))
            .unwrap();
        let job_id = id_line.split("ID:").nth(1).unwrap().trim();

        let result = tool
            .execute(json!({"action": "remove", "job_id": job_id}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("removed"));

        // List should be empty now.
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.output.contains("No scheduled jobs"));
    }

    #[tokio::test]
    async fn test_remove_nonexistent() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool
            .execute(json!({"action": "remove", "job_id": "nope"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_invalid_schedule() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool
            .execute(json!({
                "action": "create",
                "prompt": "test",
                "schedule": "whenever"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("invalid schedule"));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let dir = TempDir::new().unwrap();
        let tool = make_tool(&dir);

        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("action"));
    }
}
