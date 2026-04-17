//! Voice tools — speech-to-text (Whisper) and text-to-speech via OpenAI.
//!
//! Two independent tools live here because they share the OpenAI key
//! resolution logic:
//!
//! - `TranscribeAudioTool` — upload an audio file, get a text transcript.
//! - `TextToSpeechTool`    — generate an audio file from text.
//!
//! Both pull the OpenAI key from `OPENAI_API_KEY` env or from the provider
//! config when the primary provider is `openai`. Wiring is conditional on
//! key availability (Copy the image_gen pattern).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{Tool, ToolResult};

/// Where synthesized speech is written by default.
pub fn default_tts_output_dir(fennec_home: &Path) -> PathBuf {
    fennec_home.join("generated_audio")
}

/// Resolve the OpenAI key shared between voice tools. Same logic as the
/// image generation tool — openai config key first, then OPENAI_API_KEY env.
pub fn resolve_openai_key(provider_name: &str, config_api_key: &str) -> String {
    if provider_name.eq_ignore_ascii_case("openai") && !config_api_key.is_empty() {
        return config_api_key.to_string();
    }
    std::env::var("OPENAI_API_KEY").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// TranscribeAudioTool (Whisper)
// ---------------------------------------------------------------------------

pub struct TranscribeAudioTool {
    api_key: String,
    client: reqwest::Client,
    model: String,
}

impl TranscribeAudioTool {
    pub fn new_with_key(api_key: String, model: Option<String>) -> Option<Self> {
        if api_key.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("build reqwest client for transcribe");
        Some(Self {
            api_key,
            client,
            model: model.unwrap_or_else(|| "whisper-1".to_string()),
        })
    }
}

#[async_trait]
impl Tool for TranscribeAudioTool {
    fn name(&self) -> &str {
        "transcribe_audio"
    }

    fn description(&self) -> &str {
        "Transcribe an audio file to text using OpenAI Whisper. Accepts a \
         local file path (mp3, wav, m4a, ogg, webm, flac). Returns the \
         transcript. Useful for voice notes, meeting recordings, podcasts."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "audio_path": {
                    "type": "string",
                    "description": "Local path to the audio file."
                },
                "language": {
                    "type": "string",
                    "description": "Optional ISO-639-1 code (e.g. 'en', 'es') to help Whisper. Auto-detected if omitted."
                }
            },
            "required": ["audio_path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let audio_path = match args.get("audio_path").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: audio_path".to_string()),
                });
            }
        };
        let language = args.get("language").and_then(|v| v.as_str()).unwrap_or("");

        let bytes = match tokio::fs::read(&audio_path).await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to read audio file: {}", e)),
                });
            }
        };

        let file_name = Path::new(&audio_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3")
            .to_string();

        let file_part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str("application/octet-stream")
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new()));

        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .part("file", file_part);
        if !language.is_empty() {
            form = form.text("language", language.to_string());
        }

        let resp = match self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("request failed: {}", e)),
                });
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Whisper API error ({}): {}", status, body)),
            });
        }

        let parsed: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to parse response: {}", e)),
                });
            }
        };

        let text = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(ToolResult {
            success: !text.is_empty(),
            output: text,
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// TextToSpeechTool
// ---------------------------------------------------------------------------

pub struct TextToSpeechTool {
    api_key: String,
    client: reqwest::Client,
    default_model: String,
    default_voice: String,
    output_dir: PathBuf,
}

impl TextToSpeechTool {
    pub fn new_with_key(
        api_key: String,
        output_dir: PathBuf,
        model: Option<String>,
        voice: Option<String>,
    ) -> Option<Self> {
        if api_key.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("build reqwest client for tts");
        Some(Self {
            api_key,
            client,
            default_model: model.unwrap_or_else(|| "tts-1".to_string()),
            default_voice: voice.unwrap_or_else(|| "alloy".to_string()),
            output_dir,
        })
    }

    fn valid_voice(v: &str) -> bool {
        matches!(
            v,
            "alloy" | "echo" | "fable" | "onyx" | "nova" | "shimmer"
        )
    }
}

#[async_trait]
impl Tool for TextToSpeechTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }

    fn description(&self) -> &str {
        "Synthesize an audio file from text using OpenAI TTS. Saves mp3 to \
         disk and returns the path. Voices: alloy, echo, fable, onyx, nova, \
         shimmer. Models: tts-1 (fast), tts-1-hd (higher quality)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to synthesize. Max ~4096 chars per call."
                },
                "voice": {
                    "type": "string",
                    "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                    "description": "Voice to use. Defaults to alloy."
                },
                "model": {
                    "type": "string",
                    "enum": ["tts-1", "tts-1-hd"],
                    "description": "tts-1 is faster, tts-1-hd is higher quality."
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: text".to_string()),
                });
            }
        };
        let voice = args
            .get("voice")
            .and_then(|v| v.as_str())
            .filter(|v| Self::valid_voice(v))
            .unwrap_or(&self.default_voice)
            .to_string();
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|m| matches!(*m, "tts-1" | "tts-1-hd"))
            .unwrap_or(&self.default_model)
            .to_string();

        let body = json!({
            "model": model,
            "voice": voice,
            "input": text,
            "response_format": "mp3"
        });

        let resp = match self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("TTS request failed: {}", e)),
                });
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("TTS API error ({}): {}", status, err_body)),
            });
        }

        let bytes = match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to read audio bytes: {}", e)),
                });
            }
        };

        let path = match write_audio(&self.output_dir, &bytes).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to save audio: {}", e)),
                });
            }
        };

        Ok(ToolResult {
            success: true,
            output: format!("Audio saved to {}", path.display()),
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

async fn write_audio(dir: &Path, bytes: &[u8]) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
    let path = dir.join(format!("{}.mp3", ts));
    tokio::fs::write(&path, bytes).await.context("write audio")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_prefers_config_for_openai() {
        let k = resolve_openai_key("openai", "sk-cfg");
        assert_eq!(k, "sk-cfg");
    }

    #[test]
    fn resolve_key_ignores_config_for_non_openai() {
        let k = resolve_openai_key("anthropic", "sk-ant");
        assert_ne!(k, "sk-ant");
    }

    #[test]
    fn default_tts_output_under_fennec_home() {
        let p = default_tts_output_dir(Path::new("/home/user/.fennec"));
        assert_eq!(p, PathBuf::from("/home/user/.fennec/generated_audio"));
    }

    #[test]
    fn transcribe_none_for_empty_key() {
        assert!(TranscribeAudioTool::new_with_key(String::new(), None).is_none());
    }

    #[test]
    fn transcribe_builds_with_key() {
        let t = TranscribeAudioTool::new_with_key("sk-test".to_string(), None);
        assert!(t.is_some());
        assert_eq!(t.unwrap().model, "whisper-1");
    }

    #[test]
    fn transcribe_custom_model() {
        let t = TranscribeAudioTool::new_with_key(
            "sk-test".to_string(),
            Some("whisper-large".to_string()),
        )
        .unwrap();
        assert_eq!(t.model, "whisper-large");
    }

    #[tokio::test]
    async fn transcribe_rejects_missing_path() {
        let t = TranscribeAudioTool::new_with_key("sk-test".to_string(), None).unwrap();
        let r = t.execute(json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("audio_path"));
    }

    #[tokio::test]
    async fn transcribe_rejects_nonexistent_file() {
        let t = TranscribeAudioTool::new_with_key("sk-test".to_string(), None).unwrap();
        let r = t
            .execute(json!({"audio_path": "/nonexistent/path/to/file.mp3"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("failed to read"));
    }

    #[test]
    fn tts_none_for_empty_key() {
        assert!(
            TextToSpeechTool::new_with_key(String::new(), PathBuf::from("/tmp"), None, None)
                .is_none()
        );
    }

    #[test]
    fn tts_builds_with_key_and_defaults() {
        let t = TextToSpeechTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(t.default_model, "tts-1");
        assert_eq!(t.default_voice, "alloy");
    }

    #[test]
    fn tts_custom_defaults() {
        let t = TextToSpeechTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            Some("tts-1-hd".to_string()),
            Some("onyx".to_string()),
        )
        .unwrap();
        assert_eq!(t.default_model, "tts-1-hd");
        assert_eq!(t.default_voice, "onyx");
    }

    #[test]
    fn tts_valid_voice_accepts_all_six() {
        for v in ["alloy", "echo", "fable", "onyx", "nova", "shimmer"] {
            assert!(TextToSpeechTool::valid_voice(v), "{} should be valid", v);
        }
    }

    #[test]
    fn tts_invalid_voice_rejected() {
        assert!(!TextToSpeechTool::valid_voice("custom"));
        assert!(!TextToSpeechTool::valid_voice(""));
    }

    #[tokio::test]
    async fn tts_rejects_missing_text() {
        let t = TextToSpeechTool::new_with_key(
            "sk-test".to_string(),
            PathBuf::from("/tmp"),
            None,
            None,
        )
        .unwrap();
        let r = t.execute(json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("text"));
    }

    #[tokio::test]
    async fn write_audio_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"fake mp3";
        let p = write_audio(tmp.path(), bytes).await.unwrap();
        assert!(p.exists());
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("mp3"));
    }
}
