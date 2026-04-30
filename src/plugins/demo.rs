//! A trivial bundled plugin that exists for one reason: to validate
//! that the plugin scaffold round-trips end-to-end (inventory
//! registration → manifest validation → context registration → tool
//! reaches the agent).
//!
//! It's compiled into every Fennec build but stays dormant unless the
//! operator explicitly adds `"echo-demo"` to `[plugins].enabled` in
//! `config.toml`. Default behaviour is byte-identical to the
//! pre-plugin Fennec.
//!
//! When enabled, the plugin contributes a single `echo` tool that
//! returns whatever `text` argument is passed in. Used as a smoke
//! test for `fennec doctor` plugin coverage and as a worked example
//! of the bundled-plugin authoring pattern.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::traits::{Tool, ToolResult};

use super::context::PluginContext;
use super::manifest::{PluginKind, PluginManifest};
use super::traits::{Plugin, PluginEntry};

struct EchoPlugin;

impl Plugin for EchoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("echo-demo", env!("CARGO_PKG_VERSION"))
            .with_description("Smoke-test plugin that echoes its input. Off by default.")
            .with_author("Fennec")
            .with_kind(PluginKind::Standalone)
    }

    fn register(&self, ctx: &mut PluginContext) -> Result<()> {
        ctx.register_tool(Box::new(EchoTool));
        Ok(())
    }
}

inventory::submit! { PluginEntry { plugin: &EchoPlugin } }

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo the input text back unchanged. Provided by the bundled \
         echo-demo plugin; only available when 'echo-demo' is in \
         [plugins].enabled."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to echo back."
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        Ok(ToolResult {
            success: true,
            output: text.to_string(),
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

    #[tokio::test]
    async fn echo_returns_input_unchanged() {
        let t = EchoTool;
        let r = t
            .execute(json!({"text": "hello world"}))
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.output, "hello world");
        assert!(r.error.is_none());
    }

    #[tokio::test]
    async fn echo_handles_missing_text() {
        let t = EchoTool;
        let r = t.execute(json!({})).await.unwrap();
        // Empty `text` is not an error — the LLM is free to pass an
        // empty string. The tool simply echoes nothing back.
        assert!(r.success);
        assert_eq!(r.output, "");
    }
}
