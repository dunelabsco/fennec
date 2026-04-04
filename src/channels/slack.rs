use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::SinkExt;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

/// Slack channel using Socket Mode (WebSocket) for events and Web API for
/// sending messages. Hand-rolled with `tokio-tungstenite` and `reqwest`.
pub struct SlackChannel {
    bot_token: String,
    app_token: String,
    client: reqwest::Client,
    allowed_users: Vec<String>,
    /// Per-channel timestamp of the last edit, used for rate-limiting streaming deltas.
    last_edit: Arc<Mutex<HashMap<String, Instant>>>,
}

impl SlackChannel {
    pub fn new(bot_token: String, app_token: String, allowed_users: Vec<String>) -> Self {
        Self {
            bot_token,
            app_token,
            client: reqwest::Client::new(),
            allowed_users,
            last_edit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Parse a Slack Socket Mode events_api envelope. Returns
    /// (envelope_id, user, text, channel) if the envelope is a valid
    /// non-bot message event.
    pub fn parse_event_envelope(payload: &Value) -> Option<(String, String, String, String)> {
        let envelope_type = payload.get("type")?.as_str()?;
        if envelope_type != "events_api" {
            return None;
        }
        let envelope_id = payload.get("envelope_id")?.as_str()?.to_string();

        let event = payload.get("payload")?.get("event")?;
        let event_type = event.get("type")?.as_str()?;
        if event_type != "message" {
            return None;
        }

        // Skip bot messages (they have a bot_id field).
        if event.get("bot_id").is_some() {
            return None;
        }
        // Also skip message subtypes (edits, joins, etc.) — we only want plain messages.
        if event.get("subtype").is_some() {
            return None;
        }

        let user = event.get("user")?.as_str()?.to_string();
        let text = event.get("text")?.as_str()?.to_string();
        let channel = event.get("channel")?.as_str()?.to_string();

        Some((envelope_id, user, text, channel))
    }

    /// Build the acknowledgement JSON for a Socket Mode envelope.
    pub fn ack_envelope(envelope_id: &str) -> String {
        serde_json::json!({ "envelope_id": envelope_id }).to_string()
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let body = serde_json::json!({
            "channel": message.recipient,
            "text": message.content,
        });
        let resp = self
            .client
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Slack chat.postMessage request failed")?;
        let data: Value = resp.json().await.context("Slack chat.postMessage parse failed")?;
        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err = data.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
            anyhow::bail!("Slack chat.postMessage error: {}", err);
        }
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        // 1. Open a Socket Mode connection
        let open_resp = self
            .client
            .post("https://slack.com/api/apps.connections.open")
            .header("Authorization", format!("Bearer {}", self.app_token))
            .send()
            .await
            .context("Slack apps.connections.open request failed")?;
        let open_data: Value = open_resp
            .json()
            .await
            .context("Slack apps.connections.open parse failed")?;
        if open_data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err = open_data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Slack apps.connections.open error: {}", err);
        }
        let ws_url = open_data
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Slack connection response missing 'url'"))?;

        // 2. Connect WebSocket
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .context("Slack WebSocket connect failed")?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        // 3. Message loop
        while let Some(msg) = ws_rx.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    anyhow::bail!("Slack WebSocket error: {}", e);
                }
            };

            if msg.is_close() {
                anyhow::bail!("Slack WebSocket received close frame");
            }

            let text = match msg.to_text() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let payload: Value = match serde_json::from_str(text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Check for disconnect envelope
            if payload.get("type").and_then(|v| v.as_str()) == Some("disconnect") {
                tracing::info!("Slack: received disconnect envelope, will reconnect");
                anyhow::bail!("Slack Socket Mode disconnect received");
            }

            // Try to parse as an events_api message
            if let Some((envelope_id, user, content, channel)) =
                Self::parse_event_envelope(&payload)
            {
                // Send acknowledgement immediately
                let ack = Self::ack_envelope(&envelope_id);
                let _ = ws_tx.send(WsMessage::Text(ack.into())).await;

                if !self.allows_sender(&user) {
                    tracing::debug!("Slack: ignoring message from disallowed sender {}", user);
                    continue;
                }

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let msg = InboundMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender: user,
                    content,
                    channel: "slack".to_string(),
                    chat_id: channel,
                    timestamp: now,
                    reply_to: None,
                    metadata: HashMap::new(),
                };

                if tx.send(msg).await.is_err() {
                    tracing::info!("Slack: inbound channel closed, stopping listener");
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send_streaming_start(&self, chat_id: &str) -> Result<Option<String>> {
        let body = serde_json::json!({
            "channel": chat_id,
            "text": "...",
        });
        let resp = self
            .client
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Slack streaming start failed")?;
        let data: Value = resp.json().await.context("Slack streaming start parse failed")?;
        // Slack returns the message timestamp as `ts` which serves as the message ID.
        let ts = data.get("ts").and_then(|v| v.as_str()).map(String::from);
        Ok(ts)
    }

    async fn send_streaming_delta(
        &self,
        chat_id: &str,
        message_id: &str,
        full_text: &str,
    ) -> Result<()> {
        // Rate-limit: skip if last edit was <300ms ago.
        {
            let map = self.last_edit.lock();
            if let Some(last) = map.get(chat_id) {
                if last.elapsed().as_millis() < 300 {
                    return Ok(());
                }
            }
        }

        let body = serde_json::json!({
            "channel": chat_id,
            "ts": message_id,
            "text": full_text,
        });
        self.client
            .post("https://slack.com/api/chat.update")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Slack streaming delta failed")?;

        {
            let mut map = self.last_edit.lock();
            map.insert(chat_id.to_string(), Instant::now());
        }

        Ok(())
    }

    async fn send_streaming_end(
        &self,
        chat_id: &str,
        message_id: &str,
        full_text: &str,
    ) -> Result<()> {
        let body = serde_json::json!({
            "channel": chat_id,
            "ts": message_id,
            "text": full_text,
        });
        self.client
            .post("https://slack.com/api/chat.update")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Slack streaming end failed")?;

        {
            let mut map = self.last_edit.lock();
            map.remove(chat_id);
        }

        Ok(())
    }

    fn allows_sender(&self, sender_id: &str) -> bool {
        if self.allowed_users.is_empty() {
            return true;
        }
        if self.allowed_users.iter().any(|u| u == "*") {
            return true;
        }
        self.allowed_users.iter().any(|u| u == sender_id)
    }
}
