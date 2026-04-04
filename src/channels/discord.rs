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

/// Discord Gateway intents:
///   GUILDS          = 1 << 0  = 1
///   GUILD_MESSAGES  = 1 << 9  = 512
///   DIRECT_MESSAGES = 1 << 12 = 4096
///   MESSAGE_CONTENT = 1 << 15 = 32768
pub const DISCORD_INTENTS: u64 = 1 + 512 + 4096 + 32768; // 37377

/// Discord channel using the Gateway (WebSocket) for events and REST API for
/// sending messages. Hand-rolled with `tokio-tungstenite` and `reqwest`.
pub struct DiscordChannel {
    bot_token: String,
    client: reqwest::Client,
    allowed_users: Vec<String>,
    /// Per-channel timestamp of the last edit, used for rate-limiting streaming deltas.
    last_edit: Arc<Mutex<HashMap<String, Instant>>>,
}

impl DiscordChannel {
    pub fn new(bot_token: String, allowed_users: Vec<String>) -> Self {
        Self {
            bot_token,
            client: reqwest::Client::new(),
            allowed_users,
            last_edit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Parse a Discord MESSAGE_CREATE dispatch payload into
    /// (author_id, channel_id, content, message_id, is_bot).
    pub fn parse_message_create(d: &Value) -> Option<(String, String, String, String, bool)> {
        let author = d.get("author")?;
        let author_id = author.get("id")?.as_str()?.to_string();
        let is_bot = author.get("bot").and_then(|v| v.as_bool()).unwrap_or(false);
        let channel_id = d.get("channel_id")?.as_str()?.to_string();
        let content = d.get("content")?.as_str()?.to_string();
        let message_id = d.get("id")?.as_str()?.to_string();
        Some((author_id, channel_id, content, message_id, is_bot))
    }

    /// Compute the intent value (exposed for testing).
    pub fn intents() -> u64 {
        DISCORD_INTENTS
    }

    fn rest_url(&self, path: &str) -> String {
        format!("https://discord.com/api/v10{}", path)
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let url = self.rest_url(&format!("/channels/{}/messages", message.recipient));
        let body = serde_json::json!({ "content": message.content });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Discord send message request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord send message returned {}: {}", status, text);
        }
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        // 1. Get gateway URL
        let gw_resp = self
            .client
            .get(self.rest_url("/gateway/bot"))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .context("Discord gateway/bot request failed")?;
        let gw_data: Value = gw_resp.json().await.context("Discord gateway/bot parse failed")?;
        let gateway_url = gw_data
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Discord gateway response missing 'url'"))?;

        let ws_url = format!("{}?v=10&encoding=json", gateway_url);

        // 2. Connect WebSocket
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .context("Discord WebSocket connect failed")?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        // 3. Receive Hello (op:10)
        let hello_msg = ws_rx
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("Discord WebSocket closed before Hello"))?
            .context("Discord WebSocket read error")?;
        let hello: Value = serde_json::from_str(
            hello_msg
                .to_text()
                .context("Discord Hello was not text")?,
        )
        .context("Discord Hello parse failed")?;
        let heartbeat_interval_ms = hello
            .get("d")
            .and_then(|d| d.get("heartbeat_interval"))
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Discord Hello missing heartbeat_interval"))?;

        // 4. Send Identify (op:2)
        let identify = serde_json::json!({
            "op": 2,
            "d": {
                "token": self.bot_token,
                "intents": DISCORD_INTENTS,
                "properties": {
                    "os": "linux",
                    "browser": "fennec",
                    "device": "fennec"
                }
            }
        });
        ws_tx
            .send(WsMessage::Text(identify.to_string().into()))
            .await
            .context("Discord send Identify failed")?;

        // 5. Event loop
        let mut sequence: Option<u64> = None;
        let mut heartbeat_interval =
            tokio::time::interval(tokio::time::Duration::from_millis(heartbeat_interval_ms));
        let mut _ack_received = true;

        loop {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    let hb = serde_json::json!({
                        "op": 1,
                        "d": sequence,
                    });
                    if ws_tx.send(WsMessage::Text(hb.to_string().into())).await.is_err() {
                        anyhow::bail!("Discord WebSocket closed while sending heartbeat");
                    }
                    _ack_received = false;
                }
                msg = ws_rx.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            anyhow::bail!("Discord WebSocket error: {}", e);
                        }
                        None => {
                            anyhow::bail!("Discord WebSocket closed unexpectedly");
                        }
                    };

                    if msg.is_close() {
                        anyhow::bail!("Discord WebSocket received close frame");
                    }

                    let text = match msg.to_text() {
                        Ok(t) => t,
                        Err(_) => continue,
                    };

                    let payload: Value = match serde_json::from_str(text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let op = payload.get("op").and_then(|v| v.as_u64()).unwrap_or(99);

                    // Track sequence number.
                    if let Some(s) = payload.get("s").and_then(|v| v.as_u64()) {
                        sequence = Some(s);
                    }

                    match op {
                        0 => {
                            // Dispatch
                            let event_type = payload.get("t").and_then(|v| v.as_str()).unwrap_or("");
                            if event_type == "MESSAGE_CREATE" {
                                if let Some(d) = payload.get("d") {
                                    if let Some((author_id, channel_id, content, _msg_id, is_bot)) =
                                        Self::parse_message_create(d)
                                    {
                                        if is_bot {
                                            continue;
                                        }
                                        if !self.allows_sender(&author_id) {
                                            tracing::debug!(
                                                "Discord: ignoring message from disallowed sender {}",
                                                author_id
                                            );
                                            continue;
                                        }

                                        let now = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs();

                                        let msg = InboundMessage {
                                            id: uuid::Uuid::new_v4().to_string(),
                                            sender: author_id,
                                            content,
                                            channel: "discord".to_string(),
                                            chat_id: channel_id,
                                            timestamp: now,
                                            reply_to: None,
                                            metadata: HashMap::new(),
                                        };

                                        if tx.send(msg).await.is_err() {
                                            tracing::info!(
                                                "Discord: inbound channel closed, stopping listener"
                                            );
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                        }
                        1 => {
                            // Heartbeat request: send heartbeat immediately
                            let hb = serde_json::json!({ "op": 1, "d": sequence });
                            let _ = ws_tx.send(WsMessage::Text(hb.to_string().into())).await;
                        }
                        7 => {
                            // Reconnect requested
                            anyhow::bail!("Discord Gateway requested reconnect (op:7)");
                        }
                        9 => {
                            // Invalid Session: sleep then re-identify
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            ws_tx
                                .send(WsMessage::Text(identify.to_string().into()))
                                .await
                                .context("Discord re-identify after invalid session failed")?;
                        }
                        11 => {
                            // Heartbeat ACK
                            _ack_received = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send_streaming_start(&self, chat_id: &str) -> Result<Option<String>> {
        let url = self.rest_url(&format!("/channels/{}/messages", chat_id));
        let body = serde_json::json!({ "content": "..." });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Discord streaming start failed")?;
        let data: Value = resp.json().await.context("Discord streaming start parse failed")?;
        let message_id = data.get("id").and_then(|v| v.as_str()).map(String::from);
        Ok(message_id)
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

        let url = self.rest_url(&format!(
            "/channels/{}/messages/{}",
            chat_id, message_id
        ));
        let body = serde_json::json!({ "content": full_text });
        self.client
            .patch(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Discord streaming delta failed")?;

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
        let url = self.rest_url(&format!(
            "/channels/{}/messages/{}",
            chat_id, message_id
        ));
        let body = serde_json::json!({ "content": full_text });
        self.client
            .patch(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .context("Discord streaming end failed")?;

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
