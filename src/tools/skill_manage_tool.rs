//! The `skill_manage` tool — the agent's CRUD interface for skills.
//!
//! Wraps the operations in [`crate::skills::manage`] in the JSON
//! envelope the LLM sees. Six actions are exposed:
//!
//!   - `create`   — write a new skill (always directory format).
//!   - `edit`     — full SKILL.md rewrite.
//!   - `patch`    — fuzzy find-and-replace inside SKILL.md or a
//!                  supporting file.
//!   - `delete`   — archive an agent-created skill (refuses bundled
//!                  and hub-installed).
//!   - `write_file` — create/overwrite a supporting file under
//!                    `references/`, `templates/`, `scripts/`, or
//!                    `assets/`. Promotes flat skills to directory
//!                    format on first call.
//!   - `remove_file` — delete a supporting file.
//!
//! Provenance and lifecycle state are populated from the loader
//! before dispatch so `delete` can refuse bundled skills, and `patch`
//! can flag pinned skills (today no action is blocked on `pinned`,
//! but the metadata is included for future use).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::skills::{
    BundledManifest, HubLock, Skill, SkillsLoader, UsageStore,
    manage::{self, ManageError, ManageOutcome},
};
use crate::tools::traits::{Tool, ToolResult};

/// The `skill_manage` tool. Owns the skills root path and the usage
/// store so dispatch can refresh lifecycle metadata and update
/// counters atomically with each mutation.
pub struct SkillManageTool {
    skills_root: PathBuf,
    usage: Arc<UsageStore>,
}

impl SkillManageTool {
    pub fn new(skills_root: PathBuf, usage: Arc<UsageStore>) -> Self {
        Self {
            skills_root,
            usage,
        }
    }

    /// Reload skills from disk with full provenance + state populated,
    /// then return the skill matching `name`. Returns `None` if no
    /// such skill exists. The manifest and hub lock are re-read on
    /// every call so an out-of-band sync is reflected immediately.
    fn find_target(&self, name: &str) -> Option<Skill> {
        let bundled = BundledManifest::load(&self.skills_root);
        let hub = HubLock::load(&self.skills_root);
        let skills = SkillsLoader::load_with_provenance(
            &self.skills_root,
            Some(&bundled),
            Some(&hub),
            Some(&self.usage),
        )
        .ok()?;
        skills.into_iter().find(|s| s.name == name)
    }

    /// Snapshot of all skills (for create-collision checks).
    fn snapshot(&self) -> Vec<Skill> {
        let bundled = BundledManifest::load(&self.skills_root);
        let hub = HubLock::load(&self.skills_root);
        SkillsLoader::load_with_provenance(
            &self.skills_root,
            Some(&bundled),
            Some(&hub),
            Some(&self.usage),
        )
        .unwrap_or_default()
    }

    fn ok(&self, outcome: ManageOutcome) -> ToolResult {
        let mut payload = json!({
            "success": true,
            "message": outcome.message,
            "path": outcome.primary_path.display().to_string(),
        });
        if outcome.migrated_to_directory {
            payload["migrated_to_directory"] = json!(true);
        }
        ToolResult {
            success: true,
            output: payload.to_string(),
            error: None,
        }
    }

    fn err(&self, e: ManageError) -> ToolResult {
        let category = match &e {
            ManageError::InvalidArgument(_) => "invalid_argument",
            ManageError::Conflict(_) => "conflict",
            ManageError::Io(_) => "io",
            ManageError::Other(_) => "other",
        };
        let payload = json!({
            "success": false,
            "error": e.to_string(),
            "category": category,
        });
        ToolResult {
            success: false,
            output: payload.to_string(),
            error: Some(e.to_string()),
        }
    }

    fn missing_arg(&self, arg: &str) -> ToolResult {
        let msg = format!("missing required argument: {}", arg);
        ToolResult {
            success: false,
            output: json!({"success": false, "error": msg, "category": "invalid_argument"})
                .to_string(),
            error: Some(msg),
        }
    }

    fn unknown_action(&self, action: &str) -> ToolResult {
        let msg = format!(
            "unknown action {:?}; valid actions: create, edit, patch, delete, write_file, remove_file",
            action
        );
        ToolResult {
            success: false,
            output: json!({"success": false, "error": msg, "category": "invalid_argument"})
                .to_string(),
            error: Some(msg),
        }
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn description(&self) -> &str {
        "Create, edit, patch, or delete the agent's local skills. Use this to \
         persist behaviours you want to keep across sessions. Skills written here \
         live in the user's local skills directory and are not visible to anyone \
         else. Bundled and hub-installed skills can be edited or patched but not \
         deleted (use `fennec skills reset --restore` to revert local edits). \
         Six actions are supported: create, edit, patch, delete, write_file, remove_file."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "edit", "patch", "delete", "write_file", "remove_file"],
                    "description": "Which CRUD action to perform."
                },
                "name": {
                    "type": "string",
                    "description": "The skill's machine-readable name (lowercase letters, digits, dot/underscore/hyphen; max 64 chars)."
                },
                "content": {
                    "type": "string",
                    "description": "Full SKILL.md content for `create` and `edit` (including YAML frontmatter)."
                },
                "category": {
                    "type": "string",
                    "description": "Optional single-segment category folder for `create` (e.g. \"productivity\")."
                },
                "old_string": {
                    "type": "string",
                    "description": "For `patch`: the text to find. Whitespace-tolerant; LLM diffs with indent drift still apply."
                },
                "new_string": {
                    "type": "string",
                    "description": "For `patch`: the replacement text."
                },
                "file_path": {
                    "type": "string",
                    "description": "For `patch` (optional), `write_file`, `remove_file`: path inside the skill directory under references/, templates/, scripts/, or assets/."
                },
                "file_content": {
                    "type": "string",
                    "description": "For `write_file`: the file body to write."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "For `patch`: replace every match instead of erroring on ambiguity. Default false."
                }
            },
            "required": ["action", "name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => return Ok(self.missing_arg("action")),
        };
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return Ok(self.missing_arg("name")),
        };

        let result = match action.as_str() {
            "create" => self.do_create(&name, &args),
            "edit" => self.do_edit(&name, &args),
            "patch" => self.do_patch(&name, &args),
            "delete" => self.do_delete(&name),
            "write_file" => self.do_write_file(&name, &args),
            "remove_file" => self.do_remove_file(&name, &args),
            other => return Ok(self.unknown_action(other)),
        };
        Ok(result)
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl SkillManageTool {
    fn do_create(&self, name: &str, args: &Value) -> ToolResult {
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return self.missing_arg("content"),
        };
        let category = args.get("category").and_then(|v| v.as_str());
        let snapshot = self.snapshot();
        match manage::create(&self.skills_root, name, content, category, &snapshot) {
            Ok(outcome) => self.ok(outcome),
            Err(e) => self.err(e),
        }
    }

    fn do_edit(&self, name: &str, args: &Value) -> ToolResult {
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return self.missing_arg("content"),
        };
        let target = match self.find_target(name) {
            Some(s) => s,
            None => {
                return self.err(ManageError::Conflict(format!(
                    "skill {:?} not found",
                    name
                )));
            }
        };
        match manage::edit(&self.skills_root, name, content, &target) {
            Ok(outcome) => {
                self.usage.bump_patch(name);
                self.ok(outcome)
            }
            Err(e) => self.err(e),
        }
    }

    fn do_patch(&self, name: &str, args: &Value) -> ToolResult {
        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return self.missing_arg("old_string"),
        };
        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return self.missing_arg("new_string"),
        };
        let file_path = args.get("file_path").and_then(|v| v.as_str());
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let target = match self.find_target(name) {
            Some(s) => s,
            None => {
                return self.err(ManageError::Conflict(format!(
                    "skill {:?} not found",
                    name
                )));
            }
        };
        match manage::patch(
            &self.skills_root,
            name,
            old_string,
            new_string,
            file_path,
            replace_all,
            &target,
        ) {
            Ok(outcome) => {
                self.usage.bump_patch(name);
                self.ok(outcome)
            }
            Err(e) => self.err(e),
        }
    }

    fn do_delete(&self, name: &str) -> ToolResult {
        let target = match self.find_target(name) {
            Some(s) => s,
            None => {
                return self.err(ManageError::Conflict(format!(
                    "skill {:?} not found",
                    name
                )));
            }
        };
        match manage::delete(&self.skills_root, name, &target, &self.usage) {
            Ok((outcome, _)) => self.ok(outcome),
            Err(e) => self.err(e),
        }
    }

    fn do_write_file(&self, name: &str, args: &Value) -> ToolResult {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return self.missing_arg("file_path"),
        };
        let file_content = match args.get("file_content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return self.missing_arg("file_content"),
        };
        let target = match self.find_target(name) {
            Some(s) => s,
            None => {
                return self.err(ManageError::Conflict(format!(
                    "skill {:?} not found",
                    name
                )));
            }
        };
        match manage::write_file(
            &self.skills_root,
            name,
            file_path,
            file_content,
            &target,
        ) {
            Ok(outcome) => {
                self.usage.bump_patch(name);
                self.ok(outcome)
            }
            Err(e) => self.err(e),
        }
    }

    fn do_remove_file(&self, name: &str, args: &Value) -> ToolResult {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return self.missing_arg("file_path"),
        };
        let target = match self.find_target(name) {
            Some(s) => s,
            None => {
                return self.err(ManageError::Conflict(format!(
                    "skill {:?} not found",
                    name
                )));
            }
        };
        match manage::remove_file(&self.skills_root, name, file_path, &target) {
            Ok(outcome) => {
                self.usage.bump_patch(name);
                self.ok(outcome)
            }
            Err(e) => self.err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tool(tmp: &TempDir) -> SkillManageTool {
        let usage = Arc::new(UsageStore::open(tmp.path()));
        SkillManageTool::new(tmp.path().to_path_buf(), usage)
    }

    fn skill_md(name: &str, body: &str) -> String {
        format!("---\nname: {}\ndescription: ok\n---\n{}\n", name, body)
    }

    #[tokio::test]
    async fn create_via_tool() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        let body = skill_md("foo", "Body");
        let r = t
            .execute(json!({
                "action": "create",
                "name": "foo",
                "content": body,
            }))
            .await
            .unwrap();
        assert!(r.success, "got {:?}", r);
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["success"], json!(true));
        assert!(tmp.path().join("foo").join("SKILL.md").is_file());
    }

    #[tokio::test]
    async fn create_with_category() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        let body = skill_md("foo", "Body");
        let r = t
            .execute(json!({
                "action": "create",
                "name": "foo",
                "content": body,
                "category": "productivity",
            }))
            .await
            .unwrap();
        assert!(r.success);
        assert!(
            tmp.path()
                .join("productivity")
                .join("foo")
                .join("SKILL.md")
                .is_file()
        );
    }

    #[tokio::test]
    async fn create_returns_invalid_argument_for_bad_name() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        let body = skill_md("Foo", "Body");
        let r = t
            .execute(json!({
                "action": "create",
                "name": "Foo",
                "content": body,
            }))
            .await
            .unwrap();
        assert!(!r.success);
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["category"], json!("invalid_argument"));
    }

    #[tokio::test]
    async fn edit_existing_skill() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        // Create first.
        t.execute(json!({
            "action": "create",
            "name": "foo",
            "content": skill_md("foo", "Initial"),
        }))
        .await
        .unwrap();
        // Edit.
        let r = t
            .execute(json!({
                "action": "edit",
                "name": "foo",
                "content": skill_md("foo", "Edited"),
            }))
            .await
            .unwrap();
        assert!(r.success);
        let content =
            std::fs::read_to_string(tmp.path().join("foo").join("SKILL.md")).unwrap();
        assert!(content.contains("Edited"));
        // Patch counter bumped.
        let r = UsageStore::open(tmp.path()).get("foo").unwrap();
        assert_eq!(r.patch_count, 1);
    }

    #[tokio::test]
    async fn patch_via_tool() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        t.execute(json!({
            "action": "create",
            "name": "foo",
            "content": skill_md("foo", "Initial body"),
        }))
        .await
        .unwrap();
        let r = t
            .execute(json!({
                "action": "patch",
                "name": "foo",
                "old_string": "Initial body",
                "new_string": "Patched body",
            }))
            .await
            .unwrap();
        assert!(r.success, "got {:?}", r);
        let content =
            std::fs::read_to_string(tmp.path().join("foo").join("SKILL.md")).unwrap();
        assert!(content.contains("Patched body"));
    }

    #[tokio::test]
    async fn delete_archives_skill() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        t.execute(json!({
            "action": "create",
            "name": "foo",
            "content": skill_md("foo", "X"),
        }))
        .await
        .unwrap();
        let r = t
            .execute(json!({"action": "delete", "name": "foo"}))
            .await
            .unwrap();
        assert!(r.success);
        assert!(tmp.path().join(".archive").join("foo").is_dir());
        assert!(!tmp.path().join("foo").exists());
    }

    #[tokio::test]
    async fn write_file_creates_supporting_file() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        t.execute(json!({
            "action": "create",
            "name": "foo",
            "content": skill_md("foo", "X"),
        }))
        .await
        .unwrap();
        let r = t
            .execute(json!({
                "action": "write_file",
                "name": "foo",
                "file_path": "references/api.md",
                "file_content": "REF\n",
            }))
            .await
            .unwrap();
        assert!(r.success);
        assert!(
            tmp.path()
                .join("foo")
                .join("references")
                .join("api.md")
                .is_file()
        );
    }

    #[tokio::test]
    async fn remove_file_via_tool() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        t.execute(json!({
            "action": "create",
            "name": "foo",
            "content": skill_md("foo", "X"),
        }))
        .await
        .unwrap();
        t.execute(json!({
            "action": "write_file",
            "name": "foo",
            "file_path": "references/note.md",
            "file_content": "x",
        }))
        .await
        .unwrap();
        let r = t
            .execute(json!({
                "action": "remove_file",
                "name": "foo",
                "file_path": "references/note.md",
            }))
            .await
            .unwrap();
        assert!(r.success);
        assert!(
            !tmp.path()
                .join("foo")
                .join("references")
                .join("note.md")
                .exists()
        );
    }

    #[tokio::test]
    async fn unknown_action_returns_error() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        let r = t
            .execute(json!({"action": "frobnicate", "name": "foo"}))
            .await
            .unwrap();
        assert!(!r.success);
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["category"], json!("invalid_argument"));
    }

    #[tokio::test]
    async fn missing_action_returns_error() {
        let tmp = TempDir::new().unwrap();
        let t = tool(&tmp);
        let r = t.execute(json!({"name": "foo"})).await.unwrap();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn delete_refuses_bundled_skill() {
        let tmp = TempDir::new().unwrap();
        // Pre-seed a flat skill named "bundled-foo" + bundled manifest.
        std::fs::write(
            tmp.path().join("bundled-foo.md"),
            skill_md("bundled-foo", "X"),
        )
        .unwrap();
        std::fs::write(tmp.path().join(".bundled_manifest"), "bundled-foo:abc\n").unwrap();
        let t = tool(&tmp);

        let r = t
            .execute(json!({"action": "delete", "name": "bundled-foo"}))
            .await
            .unwrap();
        assert!(!r.success);
        let payload: Value = serde_json::from_str(&r.output).unwrap();
        assert_eq!(payload["category"], json!("conflict"));
        // Skill still on disk.
        assert!(tmp.path().join("bundled-foo.md").is_file());
    }
}
