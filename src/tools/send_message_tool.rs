
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::channels::traits::{Channel, SendMessage};
use crate::channels::ChannelMapHandle;

use super::traits::{Tool, ToolResult};

/// Tool that lets the agent proactively send a message to a specific channel
/// and chat. Useful for proactive outreach or multi-channel communication.
pub struct SendMessageTool {
    channels: ChannelMapHandle,
}

impl SendMessageTool {
    pub fn new(channels: ChannelMapHandle) -> Self {
        Self { channels }
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a message to a specific channel and chat. Use to proactively reach out."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "The channel to send the message through (e.g. 'telegram', 'discord', 'slack')"
                },
                "chat_id": {
                    "type": "string",
                    "description": "The chat/conversation ID to send the message to"
                },
                "message": {
                    "type": "string",
                    "description": "The message content to send"
                }
            },
            "required": ["channel", "chat_id", "message"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let channel_name = match args.get("channel").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: channel".to_string()),
                });
            }
        };

        let chat_id = match args.get("chat_id").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: chat_id".to_string()),
                });
            }
        };

        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: message".to_string()),
                });
            }
        };

        // Look up the channel.
        let channel = {
            let map = self.channels.read();
            map.get(&channel_name).cloned()
        };

        let channel = match channel {
            Some(ch) => ch,
            None => {
                // List available channels for the error message.
                let available: Vec<String> = {
                    let map = self.channels.read();
                    map.keys().cloned().collect()
                };
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "channel '{}' not found. Available channels: {}",
                        channel_name,
                        if available.is_empty() {
                            "none".to_string()
                        } else {
                            available.join(", ")
                        }
                    )),
                });
            }
        };

        // Send the message using the channel's send method.
        let send_msg = SendMessage::new(&message, &chat_id);
        match channel.send(&send_msg).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!(
                    "Message sent to {} (chat {})",
                    channel_name, chat_id
                ),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("failed to send message: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::new_channel_map;

    #[test]
    fn test_send_message_tool_spec() {
        let channels = new_channel_map();
        let tool = SendMessageTool::new(channels);
        assert_eq!(tool.name(), "send_message");
        let spec = tool.spec();
        assert_eq!(spec.name, "send_message");
        let props = spec.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("channel"));
        assert!(props.contains_key("chat_id"));
        assert!(props.contains_key("message"));
    }

    #[tokio::test]
    async fn test_missing_channel() {
        let channels = new_channel_map();
        let tool = SendMessageTool::new(channels);
        let result = tool
            .execute(json!({
                "channel": "nonexistent",
                "chat_id": "123",
                "message": "hello"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_missing_params() {
        let channels = new_channel_map();
        let tool = SendMessageTool::new(channels);

        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("channel"));
    }
}
