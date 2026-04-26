//! Ask the user a question and wait for their reply.
//!
//! The flow:
//!
//!   1. The tool reads the *current turn's* `(channel, chat_id)` from
//!      the shared [`TurnOriginHandle`] populated by the gateway.
//!   2. It sends the formatted question through that channel using the
//!      origin's `chat_id` as the recipient.
//!   3. It registers a one-shot expectation in the shared
//!      [`PendingReplies`] map keyed on the same `(channel, chat_id)`,
//!      then awaits on the receiver with a timeout.
//!   4. The gateway's inbound dispatch checks `PendingReplies` *before*
//!      forwarding to the agent. A reply from the same chat is delivered
//!      through the oneshot and is not turned into a fresh agent turn.
//!
//! The previous implementation spawned a second `channel.listen(tx)`
//! task to capture the reply. On Telegram (and other long-poll
//! channels) that task raced the gateway's own poller for `getUpdates`
//! offsets — whichever poller called the API first stole the message.
//! Replies disappeared randomly. Routing through `PendingReplies` is
//! cooperative with the gateway and avoids the race.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::bus::{PendingReplies, TurnOrigin, TurnOriginHandle};
use crate::channels::traits::{Channel, SendMessage};
use crate::channels::ChannelMapHandle;

use super::traits::{Tool, ToolResult};

/// Maximum wait the LLM is allowed to request, regardless of the
/// `timeout_secs` argument. Bounds resource use on a forgotten
/// `ask_user` call (e.g. the user closes the app and never replies).
const MAX_TIMEOUT_SECS: u64 = 30 * 60;

pub struct AskUserTool {
    channels: ChannelMapHandle,
    origin: TurnOriginHandle,
    pending: PendingReplies,
}

impl AskUserTool {
    pub fn new(
        channels: ChannelMapHandle,
        origin: TurnOriginHandle,
        pending: PendingReplies,
    ) -> Self {
        Self {
            channels,
            origin,
            pending,
        }
    }

    fn current_origin(&self) -> Option<TurnOrigin> {
        self.origin.lock().ok().and_then(|guard| guard.clone())
    }

    fn lookup_channel(&self, name: &str) -> Option<Arc<dyn Channel>> {
        let map = self.channels.read();
        map.get(name).cloned()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question through the channel they messaged you on, and \
         wait for their next reply. Use when you need clarification or approval \
         before proceeding. Returns the user's reply text, or times out."
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
                    "description": "Optional list of choices. If supplied, a numeric reply (1, 2, ...) is mapped to the corresponding choice text."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for a reply, in seconds. Default 300, max 1800."
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
            Some(q) if !q.is_empty() => q.to_string(),
            _ => {
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

        let requested_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300);
        let timeout_secs = requested_secs.min(MAX_TIMEOUT_SECS);

        // Resolve the origin. Without a known (channel, chat_id) we have
        // nowhere to send the question and nothing to wait on.
        let origin = match self.current_origin() {
            Some(o) => o,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "ask_user has no current turn origin — this tool can only \
                         be used inside an agent turn triggered by an inbound \
                         channel message"
                            .to_string(),
                    ),
                });
            }
        };

        let channel = match self.lookup_channel(&origin.channel) {
            Some(ch) => ch,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "channel '{}' not registered; cannot ask user",
                        origin.channel
                    )),
                });
            }
        };

        // Format question with choices, if any.
        let formatted = format_question(&question, &choices);

        // Register the wait BEFORE sending the question. Otherwise a fast
        // user could reply between send and register and we'd miss the
        // delivery (the gateway would forward to the agent loop, which
        // would treat it as a fresh turn).
        let receiver = self.pending.register(origin.clone());

        let send_msg = SendMessage::new(&formatted, &origin.chat_id);
        if let Err(e) = channel.send(&send_msg).await {
            // Sending failed; clear the registration so the next
            // legitimate inbound from this chat reaches the agent.
            self.pending.cancel(&origin);
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("failed to send question: {e}")),
            });
        }

        // Wait for either the reply or the timeout. Either way, clear
        // the registration on exit so the gateway resumes normal flow.
        let outcome = tokio::time::timeout(Duration::from_secs(timeout_secs), receiver).await;

        match outcome {
            Ok(Ok(msg)) => {
                let reply = msg.content;
                let resolved = if choices.is_empty() {
                    reply
                } else {
                    resolve_choice(&reply, &choices)
                };
                Ok(ToolResult {
                    success: true,
                    output: resolved,
                    error: None,
                })
            }
            Ok(Err(_)) => {
                // The sender was dropped without a value — the
                // PendingReplies registration was clobbered by another
                // ask_user call (rare; see register's docs).
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("ask_user reply channel closed unexpectedly".to_string()),
                })
            }
            Err(_) => {
                // Timed out. Clear the pending entry so the next inbound
                // from this chat doesn't get silently consumed.
                self.pending.cancel(&origin);
                Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "timed out waiting for user reply after {timeout_secs}s"
                    )),
                })
            }
        }
    }
}

/// Render the question with optional numbered choices appended.
fn format_question(question: &str, choices: &[String]) -> String {
    if choices.is_empty() {
        format!("\u{2753} {question}")
    } else {
        let mut msg = format!("\u{2753} {question}\n");
        for (i, c) in choices.iter().enumerate() {
            msg.push_str(&format!("  {}. {}\n", i + 1, c));
        }
        msg.push_str("\nReply with the number of your choice, or type your own:");
        msg
    }
}

/// If `reply` parses as a 1-based index into `choices`, return that
/// choice's text. Otherwise return `reply` unchanged so freeform
/// answers still work.
fn resolve_choice(reply: &str, choices: &[String]) -> String {
    if let Ok(idx) = reply.trim().parse::<usize>() {
        if (1..=choices.len()).contains(&idx) {
            return choices[idx - 1].clone();
        }
    }
    reply.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{new_turn_origin, InboundMessage};
    use crate::channels::new_channel_map;
    use std::collections::HashMap;

    #[test]
    fn formats_question_without_choices() {
        let f = format_question("ready?", &[]);
        assert!(f.contains("ready?"));
        assert!(!f.contains('\n'));
    }

    #[test]
    fn formats_question_with_choices() {
        let f = format_question("pick one", &["A".into(), "B".into()]);
        assert!(f.contains("1. A"));
        assert!(f.contains("2. B"));
    }

    #[test]
    fn resolve_choice_maps_number_to_text() {
        let choices = vec!["red".to_string(), "blue".to_string()];
        assert_eq!(resolve_choice("1", &choices), "red");
        assert_eq!(resolve_choice("  2  ", &choices), "blue");
    }

    #[test]
    fn resolve_choice_passes_through_freeform() {
        let choices = vec!["red".to_string(), "blue".to_string()];
        assert_eq!(resolve_choice("green", &choices), "green");
        assert_eq!(resolve_choice("3", &choices), "3"); // out of range
    }

    /// Without a turn origin set, the tool refuses cleanly.
    #[tokio::test]
    async fn refuses_without_origin() {
        let channels = new_channel_map();
        let origin = new_turn_origin();
        let pending = PendingReplies::new();
        let tool = AskUserTool::new(channels, origin, pending);
        let r = tool.execute(json!({"question": "hi"})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("origin"));
    }

    /// With an origin set but the channel not registered, refuses cleanly.
    #[tokio::test]
    async fn refuses_when_channel_missing() {
        let channels = new_channel_map();
        let origin = new_turn_origin();
        *origin.lock().unwrap() = Some(TurnOrigin {
            channel: "nonexistent".into(),
            chat_id: "1".into(),
        });
        let pending = PendingReplies::new();
        let tool = AskUserTool::new(channels, origin, pending);
        let r = tool.execute(json!({"question": "hi"})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("not registered"));
    }

    /// Timeout enforcement: a tiny timeout fires and returns an error.
    #[tokio::test]
    async fn times_out_when_no_reply_arrives() {
        // Register a channel that always succeeds on send.
        struct OkChannel;
        #[async_trait]
        impl Channel for OkChannel {
            fn name(&self) -> &str {
                "test"
            }
            async fn send(&self, _msg: &SendMessage) -> anyhow::Result<()> {
                Ok(())
            }
            async fn listen(
                &self,
                _tx: tokio::sync::mpsc::Sender<InboundMessage>,
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let channels = new_channel_map();
        channels
            .write()
            .insert("test".to_string(), Arc::new(OkChannel));
        let origin = new_turn_origin();
        *origin.lock().unwrap() = Some(TurnOrigin {
            channel: "test".into(),
            chat_id: "1".into(),
        });
        let pending = PendingReplies::new();
        let tool = AskUserTool::new(channels, origin, pending.clone());

        let r = tool
            .execute(json!({"question": "hi", "timeout_secs": 1}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("timed out"));
        // Pending entry must be cleaned up after timeout.
        assert_eq!(pending.len(), 0);
    }

    /// Happy path: a reply delivered via PendingReplies returns success.
    #[tokio::test]
    async fn returns_reply_when_delivered() {
        struct OkChannel;
        #[async_trait]
        impl Channel for OkChannel {
            fn name(&self) -> &str {
                "test"
            }
            async fn send(&self, _msg: &SendMessage) -> anyhow::Result<()> {
                Ok(())
            }
            async fn listen(
                &self,
                _tx: tokio::sync::mpsc::Sender<InboundMessage>,
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let channels = new_channel_map();
        channels
            .write()
            .insert("test".to_string(), Arc::new(OkChannel));
        let origin_handle = new_turn_origin();
        let origin = TurnOrigin {
            channel: "test".into(),
            chat_id: "1".into(),
        };
        *origin_handle.lock().unwrap() = Some(origin.clone());
        let pending = PendingReplies::new();

        let tool = AskUserTool::new(channels, origin_handle, pending.clone());

        // Spawn the tool execution; meanwhile, deliver a reply.
        let pending_for_delivery = pending.clone();
        let origin_for_delivery = origin.clone();
        tokio::spawn(async move {
            // Small wait so the tool registers first.
            tokio::time::sleep(Duration::from_millis(50)).await;
            pending_for_delivery.take_and_deliver(
                &origin_for_delivery,
                InboundMessage {
                    id: "x".into(),
                    sender: "u".into(),
                    content: "yes please".into(),
                    channel: "test".into(),
                    chat_id: "1".into(),
                    timestamp: 0,
                    reply_to: None,
                    metadata: HashMap::new(),
                },
            );
        });

        let r = tool
            .execute(json!({"question": "ok?", "timeout_secs": 5}))
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.output, "yes please");
    }
}
