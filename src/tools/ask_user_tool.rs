use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::channels::traits::{Channel, SendMessage};
use crate::channels::ChannelMapHandle;

use super::traits::{Tool, ToolResult};

/// A tool that asks the user a question and waits for their response.
///
/// This allows the agent to request clarification or approval mid-task by
/// sending a message through the originating channel and blocking until the
/// user replies (or a timeout expires).
pub struct AskUserTool {
    channels: ChannelMapHandle,
    default_channel: String,
}

impl AskUserTool {
    /// Create a new `AskUserTool`.
    ///
    /// `channels` is the shared channel map populated by the gateway.
    /// `default_channel` is the channel name to use when no specific channel is
    /// available (e.g. `"telegram"`).
    pub fn new(channels: ChannelMapHandle, default_channel: String) -> Self {
        Self {
            channels,
            default_channel,
        }
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. Use when you need clarification or approval before proceeding."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "choices": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of choices to present to the user"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for a response in seconds (default 300)"
                }
            },
            "required": ["question"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let question = match args.get("question").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: question".to_string()),
                });
            }
        };

        let choices: Vec<String> = args
            .get("choices")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300);

        // Format the question with choices if provided.
        let formatted = if choices.is_empty() {
            format!("\u{2753} {question}")
        } else {
            let mut msg = format!("\u{2753} {question}\n");
            for (i, choice) in choices.iter().enumerate() {
                msg.push_str(&format!("  {}. {}\n", i + 1, choice));
            }
            msg.push_str("\nReply with your choice (number or text):");
            msg
        };

        // Get the channel.
        let channel = {
            let map = self.channels.read();
            map.get(&self.default_channel).cloned()
        };

        let channel = match channel {
            Some(ch) => ch,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "channel '{}' not available for ask_user",
                        self.default_channel
                    )),
                });
            }
        };

        // Send the question.
        // Note: we need a chat_id but the tool doesn't know it directly.
        // We send to a placeholder recipient; the actual routing is handled
        // by the channel's send method. For channels like Telegram the
        // recipient needs to be a chat_id, which we don't have in the tool
        // context. For now we send using a broadcast-style approach.
        let send_msg = SendMessage::new(&formatted, "user");
        if let Err(e) = channel.send(&send_msg).await {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("failed to send question: {e}")),
            });
        }

        // Create a listener to wait for the user's response.
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let listener_handle = tokio::spawn({
            let channel = channel.clone();
            async move {
                let _ = channel.listen(tx).await;
            }
        });

        // Wait for a response with timeout.
        let duration = tokio::time::Duration::from_secs(timeout_secs);
        let result = match tokio::time::timeout(duration, rx.recv()).await {
            Ok(Some(msg)) => {
                let response = msg.content.clone();
                // If choices were provided, try to match the response.
                if !choices.is_empty() {
                    if let Ok(idx) = response.trim().parse::<usize>() {
                        if idx >= 1 && idx <= choices.len() {
                            Ok(ToolResult {
                                success: true,
                                output: choices[idx - 1].clone(),
                                error: None,
                            })
                        } else {
                            Ok(ToolResult {
                                success: true,
                                output: response,
                                error: None,
                            })
                        }
                    } else {
                        Ok(ToolResult {
                            success: true,
                            output: response,
                            error: None,
                        })
                    }
                } else {
                    Ok(ToolResult {
                        success: true,
                        output: response,
                        error: None,
                    })
                }
            }
            Ok(None) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("channel closed before receiving a response".to_string()),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "timed out waiting for user response after {timeout_secs}s"
                )),
            }),
        };

        // Abort the listener task.
        listener_handle.abort();

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::new_channel_map;

    #[test]
    fn test_ask_user_tool_spec() {
        let channels = new_channel_map();
        let tool = AskUserTool::new(channels, "test".to_string());
        assert_eq!(tool.name(), "ask_user");
        assert!(tool.is_read_only());
        let spec = tool.spec();
        assert_eq!(spec.name, "ask_user");
        let props = spec.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("question"));
        assert!(props.contains_key("choices"));
        assert!(props.contains_key("timeout_secs"));
    }
}
