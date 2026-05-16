use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;

use crate::skills::{Skill, UsageStore};
use crate::tools::traits::{Tool, ToolResult};

/// A tool that lets the agent load a skill by name at runtime.
///
/// When a `UsageStore` is wired in, every successful `load_skill`
/// bumps the corresponding skill's `use_count` so the curator can
/// distinguish "actively used" from "never touched" agent-created
/// skills. Bundled and hub-installed skills are filtered out by the
/// store itself (provenance check).
pub struct SkillsTool {
    skills: Arc<Mutex<Vec<Skill>>>,
    usage: Option<Arc<UsageStore>>,
}

impl SkillsTool {
    /// Create a new `SkillsTool` with the given list of available skills.
    /// No usage tracking — usable in tests and contexts without a
    /// home directory.
    pub fn new(skills: Vec<Skill>) -> Self {
        Self {
            skills: Arc::new(Mutex::new(skills)),
            usage: None,
        }
    }

    /// Create a new `SkillsTool` that records usage to the given store.
    pub fn with_usage(skills: Vec<Skill>, usage: Arc<UsageStore>) -> Self {
        Self {
            skills: Arc::new(Mutex::new(skills)),
            usage: Some(usage),
        }
    }

    /// Replace the available skills list.
    pub fn set_skills(&self, skills: Vec<Skill>) {
        *self.skills.lock() = skills;
    }
}

#[async_trait]
impl Tool for SkillsTool {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "Load a skill by name to get its full content and instructions."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to load."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let skill_name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if skill_name.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: name".to_string()),
            });
        }

        let skills = self.skills.lock();
        match skills.iter().find(|s| s.name == skill_name) {
            Some(skill) => {
                let output = format!(
                    "# Skill: {}\n\n{}\n\n{}",
                    skill.name, skill.description, skill.content
                );
                // Bump the use counter best-effort. Provenance filter
                // inside UsageStore drops bundled/hub skills, so this
                // is safe to call unconditionally.
                if let Some(u) = self.usage.as_ref() {
                    u.bump_use(&skill.name);
                }
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            None => {
                let available: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Skill '{}' not found. Available skills: {}",
                        skill_name,
                        available.join(", ")
                    )),
                })
            }
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_existing_skill() {
        let skills = vec![Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "Do the thing.".to_string(),
            always: false,
            requirements: vec![],
            ..Default::default()
        }];
        let tool = SkillsTool::new(skills);

        let result = tool
            .execute(json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("test-skill"));
        assert!(result.output.contains("Do the thing."));
    }

    #[tokio::test]
    async fn test_load_missing_skill() {
        let tool = SkillsTool::new(vec![]);
        let result = tool
            .execute(json!({"name": "nonexistent"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_load_skill_missing_name() {
        let tool = SkillsTool::new(vec![]);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn load_bumps_usage_when_store_wired() {
        let tmp = tempfile::tempdir().unwrap();
        let usage = Arc::new(UsageStore::open(tmp.path()));
        let skills = vec![Skill {
            name: "alpha".into(),
            description: "x".into(),
            content: "body".into(),
            always: false,
            requirements: vec![],
            ..Default::default()
        }];
        let tool = SkillsTool::with_usage(skills, Arc::clone(&usage));

        let r = tool.execute(json!({"name": "alpha"})).await.unwrap();
        assert!(r.success);
        assert_eq!(usage.get("alpha").unwrap().use_count, 1);

        // Loading again increments.
        tool.execute(json!({"name": "alpha"})).await.unwrap();
        assert_eq!(usage.get("alpha").unwrap().use_count, 2);
    }

    #[tokio::test]
    async fn bundled_skill_load_does_not_bump_usage() {
        let tmp = tempfile::tempdir().unwrap();
        // Bundled manifest claims "alpha" is bundled, so usage tracker
        // filters writes to it.
        std::fs::write(tmp.path().join(".bundled_manifest"), "alpha:abc\n").unwrap();
        let usage = Arc::new(UsageStore::open(tmp.path()));

        let skills = vec![Skill {
            name: "alpha".into(),
            description: "x".into(),
            content: "body".into(),
            always: false,
            requirements: vec![],
            ..Default::default()
        }];
        let tool = SkillsTool::with_usage(skills, Arc::clone(&usage));
        tool.execute(json!({"name": "alpha"})).await.unwrap();
        assert!(usage.get("alpha").is_none(), "bundled skill must not be tracked");
    }
}
