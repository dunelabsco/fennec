//! Image generation tool — generates an image via OpenAI's Images API
//! (DALL-E 3) and saves it to disk.
//!
//! Works independently of the main provider: the tool pulls an OpenAI key
//! from either the provider config (when provider is "openai") or the
//! `OPENAI_API_KEY` environment variable. Users with Anthropic as their
//! primary provider can still generate images as long as they have a
//! second OpenAI key configured.

use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{Tool, ToolResult};

/// Where generated images are written.
pub fn default_output_dir(fennec_home: &std::path::Path) -> PathBuf {
    fennec_home.join("generated_images")
}

pub struct ImageGenTool {
    api_key: String,
    output_dir: PathBuf,
    model: String,
    client: reqwest::Client,
}

impl ImageGenTool {
    /// Build the tool if an OpenAI key is available.
    ///
    /// Resolution order: `config_api_key` (when non-empty) then
    /// `OPENAI_API_KEY` env var. Returns None if no key is found.
    pub fn new_with_key(
        api_key: String,
        output_dir: PathBuf,
        model: Option<String>,
    ) -> Option<Self> {
        if api_key.is_empty() {
            return None;
        }
        let client = super::http::shared_client();
        Some(Self {
            api_key,
            output_dir,
            model: model.unwrap_or_else(|| "dall-e-3".to_string()),
            client,
        })
    }

    /// Resolve an OpenAI key from config + env, returning None if neither
    /// source has one.
    pub fn resolve_openai_key(config_provider_name: &str, config_api_key: &str) -> String {
        if config_provider_name.eq_ignore_ascii_case("openai") && !config_api_key.is_empty() {
            return config_api_key.to_string();
        }
        std::env::var("OPENAI_API_KEY").unwrap_or_default()
    }
}

#[async_trait]
impl Tool for ImageGenTool {
    fn name(&self) -> &str {
        "image_generate"
    }

    fn description(&self) -> &str {
        "Generate an image from a text prompt (DALL-E 3). The image is saved \
         to disk; the returned text includes the file path. Use for mockups, \
         diagrams, illustrations, concept art, memes. Be specific about \
         subject, style, and composition."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What to generate. Be specific: subject, style, mood, composition."
                },
                "size": {
                    "type": "string",
                    "enum": ["1024x1024", "1792x1024", "1024x1792"],
                    "description": "Image dimensions. Default 1024x1024 (square)."
                },
                "quality": {
                    "type": "string",
                    "enum": ["standard", "hd"],
                    "description": "Quality level. 'hd' is slower and costs more. Default 'standard'."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: prompt".to_string()),
                });
            }
        };
        let size = args
            .get("size")
            .and_then(|v| v.as_str())
            .filter(|s| matches!(*s, "1024x1024" | "1792x1024" | "1024x1792"))
            .unwrap_or("1024x1024")
            .to_string();
        let quality = args
            .get("quality")
            .and_then(|v| v.as_str())
            .filter(|s| matches!(*s, "standard" | "hd"))
            .unwrap_or("standard")
            .to_string();

        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "n": 1,
            "size": size,
            "quality": quality,
            "response_format": "b64_json"
        });

        let resp = match self
            .client
            .post("https://api.openai.com/v1/images/generations")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .timeout(std::time::Duration::from_secs(120))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("OpenAI request failed: {}", e)),
                });
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("OpenAI images API error ({}): {}", status, body)),
            });
        }

        let parsed: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to parse OpenAI response: {}", e)),
                });
            }
        };

        let b64 = match extract_b64(&parsed) {
            Some(x) => x,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("no b64_json in response: {}", parsed)),
                });
            }
        };

        let bytes = match decode_b64(&b64) {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("base64 decode failed: {}", e)),
                });
            }
        };

        let path = match write_image(&self.output_dir, &bytes).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to save image: {}", e)),
                });
            }
        };

        let revised_prompt = parsed
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|e| e.get("revised_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let output = if revised_prompt.is_empty() {
            format!("Image saved to {}", path.display())
        } else {
            format!(
                "Image saved to {}\nRevised prompt used by DALL-E: {}",
                path.display(),
                revised_prompt
            )
        };

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

fn extract_b64(v: &Value) -> Option<String> {
    v.get("data")?
        .as_array()?
        .first()?
        .get("b64_json")?
        .as_str()
        .map(String::from)
}

fn decode_b64(b64: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    B64.decode(b64).context("base64 decode")
}

async fn write_image(dir: &std::path::Path, bytes: &[u8]) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
    let path = dir.join(format!("{}.png", ts));
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_prefers_config_when_provider_is_openai() {
        let k = ImageGenTool::resolve_openai_key("openai", "sk-from-config");
        assert_eq!(k, "sk-from-config");
    }

    #[test]
    fn resolve_key_falls_back_to_env_for_non_openai_providers() {
        // We can't easily set/unset env vars safely in Rust 2024 tests
        // (unsafe + race hazards), so we just verify the config path is NOT
        // used when provider isn't openai.
        let k = ImageGenTool::resolve_openai_key("anthropic", "sk-anthropic-key");
        // If no OPENAI_API_KEY set in the test env, we get empty string.
        // If it IS set, we get whatever is there. Either way, not the
        // anthropic config key.
        assert_ne!(k, "sk-anthropic-key");
    }

    #[test]
    fn new_with_key_returns_none_for_empty_key() {
        assert!(
            ImageGenTool::new_with_key(String::new(), PathBuf::from("/tmp"), None).is_none()
        );
    }

    #[test]
    fn new_with_key_builds_for_valid_key() {
        let t = ImageGenTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
        );
        assert!(t.is_some());
    }

    #[test]
    fn default_model_is_dalle3() {
        let t = ImageGenTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
        )
        .unwrap();
        assert_eq!(t.model, "dall-e-3");
    }

    #[test]
    fn custom_model_overrides_default() {
        let t = ImageGenTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            Some("custom-model".to_string()),
        )
        .unwrap();
        assert_eq!(t.model, "custom-model");
    }

    #[test]
    fn default_output_dir_under_fennec_home() {
        let p = default_output_dir(std::path::Path::new("/home/user/.fennec"));
        assert_eq!(p, PathBuf::from("/home/user/.fennec/generated_images"));
    }

    #[test]
    fn extract_b64_happy_path() {
        let v = json!({
            "data": [{"b64_json": "aGVsbG8="}]
        });
        assert_eq!(extract_b64(&v), Some("aGVsbG8=".to_string()));
    }

    #[test]
    fn extract_b64_missing_data() {
        let v = json!({});
        assert_eq!(extract_b64(&v), None);
    }

    #[test]
    fn extract_b64_empty_array() {
        let v = json!({"data": []});
        assert_eq!(extract_b64(&v), None);
    }

    #[test]
    fn decode_b64_valid() {
        let bytes = decode_b64("aGVsbG8=").unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn decode_b64_invalid_returns_err() {
        assert!(decode_b64("!!!not base64").is_err());
    }

    #[tokio::test]
    async fn write_image_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"fake png bytes";
        let path = write_image(tmp.path(), bytes).await.unwrap();
        assert!(path.exists());
        let read_back = tokio::fs::read(&path).await.unwrap();
        assert_eq!(read_back, bytes);
    }

    #[tokio::test]
    async fn execute_rejects_missing_prompt() {
        let t = ImageGenTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
        )
        .unwrap();
        let result = t.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("missing required parameter"));
    }

    #[tokio::test]
    async fn execute_rejects_empty_prompt() {
        let t = ImageGenTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
        )
        .unwrap();
        let result = t.execute(json!({"prompt": ""})).await.unwrap();
        assert!(!result.success);
    }
}
