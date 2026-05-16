//! Curator-internal tools.
//!
//! The curator's tool loop exposes three tools to the LLM:
//!
//!   - `skills_list` — read-only summary of every agent-created
//!     skill plus its usage record. Cheap and idempotent.
//!   - `skill_view` — wrapped `load_skill` from the existing
//!     [`crate::tools::SkillsTool`] (we don't redefine it here).
//!   - `skill_manage` — the existing
//!     [`crate::tools::SkillManageTool`].
//!
//! Only `skills_list` is novel; this file owns it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::skills::{
    BundledManifest, HubLock, SkillsLoader, UsageStore, format::SkillProvenance,
};
use crate::tools::traits::{Tool, ToolResult};

/// Tool that returns a JSON list of every agent-created skill on
/// disk along with its usage record. Used by the curator to plan
/// consolidation work.
pub struct SkillsListTool {
    skills_root: PathBuf,
    usage: Arc<UsageStore>,
}

impl SkillsListTool {
    pub fn new(skills_root: PathBuf, usage: Arc<UsageStore>) -> Self {
        Self {
            skills_root,
            usage,
        }
    }
}

#[async_trait]
impl Tool for SkillsListTool {
    fn name(&self) -> &str {
        "skills_list"
    }

    fn description(&self) -> &str {
        "List every agent-created skill with its description, usage counters, lifecycle state, \
         and pinned flag. Bundled and hub-installed skills are excluded. Read-only — call this \
         once at the start of curator review to see the full landscape."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<ToolResult> {
        let bundled = BundledManifest::load(&self.skills_root);
        let hub = HubLock::load(&self.skills_root);
        let skills = match SkillsLoader::load_with_provenance(
            &self.skills_root,
            Some(&bundled),
            Some(&hub),
            Some(&self.usage),
        ) {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("could not load skills: {}", e)),
                });
            }
        };

        let mut entries = Vec::new();
        for skill in skills {
            if !matches!(skill.provenance, SkillProvenance::AgentCreated) {
                continue;
            }
            let record = self.usage.get(&skill.name).unwrap_or_default();
            let layout_kind = skill
                .layout
                .as_ref()
                .map(|l| if l.supports_subfiles() { "directory" } else { "flat" })
                .unwrap_or("unknown");
            entries.push(json!({
                "name": skill.name,
                "description": skill.description,
                "layout": layout_kind,
                "category": skill.layout.as_ref().and_then(|l| l.category()).map(String::from),
                "state": skill.state.as_str(),
                "pinned": skill.pinned,
                "use_count": record.use_count,
                "view_count": record.view_count,
                "patch_count": record.patch_count,
                "last_used_at": record.last_used_at,
                "last_patched_at": record.last_patched_at,
                "created_at": record.created_at,
            }));
        }

        let payload = json!({
            "count": entries.len(),
            "skills": entries,
        });
        Ok(ToolResult {
            success: true,
            output: payload.to_string(),
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::format::SkillState;
    use tempfile::TempDir;

    fn write_skill(root: &std::path::Path, name: &str, body: &str) {
        std::fs::write(
            root.join(format!("{}.md", name)),
            format!("---\nname: {}\ndescription: x\n---\n{}\n", name, body),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn lists_only_agent_created() {
        let tmp = TempDir::new().unwrap();
        // Two skills on disk. Mark `bundled-foo` as bundled.
        write_skill(tmp.path(), "agent-foo", "body");
        write_skill(tmp.path(), "bundled-foo", "body");
        std::fs::write(tmp.path().join(".bundled_manifest"), "bundled-foo:abc\n").unwrap();

        let usage = Arc::new(UsageStore::open(tmp.path()));
        let t = SkillsListTool::new(tmp.path().to_path_buf(), Arc::clone(&usage));
        let r = t.execute(json!({})).await.unwrap();
        assert!(r.success);
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["count"], json!(1));
        let names: Vec<&str> = payload["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["agent-foo"]);
    }

    #[tokio::test]
    async fn surfaces_usage_record_fields() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "alpha", "body");
        let usage = Arc::new(UsageStore::open(tmp.path()));
        usage.bump_use("alpha");
        usage.bump_use("alpha");
        usage.bump_view("alpha");
        usage.set_state("alpha", SkillState::Stale);

        let t = SkillsListTool::new(tmp.path().to_path_buf(), Arc::clone(&usage));
        let r = t.execute(json!({})).await.unwrap();
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        let entry = &payload["skills"][0];
        assert_eq!(entry["use_count"], json!(2));
        assert_eq!(entry["view_count"], json!(1));
        assert_eq!(entry["state"], json!("stale"));
        assert_eq!(entry["pinned"], json!(false));
    }

    #[tokio::test]
    async fn empty_root_returns_empty_list() {
        let tmp = TempDir::new().unwrap();
        let usage = Arc::new(UsageStore::open(tmp.path()));
        let t = SkillsListTool::new(tmp.path().to_path_buf(), Arc::clone(&usage));
        let r = t.execute(json!({})).await.unwrap();
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["count"], json!(0));
    }

    #[test]
    fn tool_is_read_only() {
        let tmp = TempDir::new().unwrap();
        let usage = Arc::new(UsageStore::open(tmp.path()));
        let t = SkillsListTool::new(tmp.path().to_path_buf(), usage);
        assert!(t.is_read_only());
    }
}
