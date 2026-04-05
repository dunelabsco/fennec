use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

/// WhatsApp channel using Meta's WhatsApp Cloud API.
///
/// Unlike Telegram (long-polling) or Discord/Slack (WebSocket), WhatsApp uses
/// webhooks. The `listen` method starts a mini HTTP server (axum) that receives
/// webhook POST requests from Meta.
///
/// Webhook verification: GET request with
///   `hub.mode=subscribe&hub.verify_token=...&hub.challenge=...`
/// returns the challenge value.
///
/// Message webhook: POST with messages array -> extract sender phone, message
/// text, create InboundMessage.
///
/// Sending: POST to
///   `https://graph.facebook.com/v21.0/{phone_number_id}/messages`
pub struct WhatsAppChannel {
    phone_number_id: String,
    access_token: String,
    verify_token: String,
    webhook_port: u16,
    client: reqwest::Client,
    allowed_users: Vec<String>,
}

impl WhatsAppChannel {
    pub fn new(
        phone_number_id: String,
        access_token: String,
        verify_token: String,
        webhook_port: u16,
        allowed_users: Vec<String>,
    ) -> Self {
        Self {
            phone_number_id,
            access_token,
            verify_token,
            webhook_port,
            client: reqwest::Client::new(),
            allowed_users,
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "https://graph.facebook.com/v21.0/{}{}",
            self.phone_number_id, path
        )
    }

    /// Parse a WhatsApp webhook payload into a list of (sender_phone, message_text, message_id).
    pub fn parse_webhook_messages(body: &Value) -> Vec<(String, String, String)> {
        let mut results = Vec::new();

        let entries = match body.get("entry").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return results,
        };

        for entry in entries {
            let changes = match entry.get("changes").and_then(|v| v.as_array()) {
                Some(arr) => arr,
                None => continue,
            };

            for change in changes {
                let value = match change.get("value") {
                    Some(v) => v,
                    None => continue,
                };

                let messages = match value.get("messages").and_then(|v| v.as_array()) {
                    Some(arr) => arr,
                    None => continue,
                };

                for message in messages {
                    let msg_type = message
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if msg_type != "text" {
                        continue;
                    }

                    let from = message
                        .get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let text = message
                        .get("text")
                        .and_then(|t| t.get("body"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let msg_id = message
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !from.is_empty() && !text.is_empty() {
                        results.push((from, text, msg_id));
                    }
                }
            }
        }

        results
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &str {
        "whatsapp"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": message.recipient,
            "type": "text",
            "text": {
                "body": message.content
            }
        });

        let resp = self
            .client
            .post(self.api_url("/messages"))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("WhatsApp send message request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("WhatsApp send message returned {}: {}", status, text);
        }
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        use axum::extract::{Query, State};
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct WebhookState {
            verify_token: String,
            allowed_users: Vec<String>,
            tx: tokio::sync::mpsc::Sender<InboundMessage>,
        }

        #[derive(serde::Deserialize)]
        struct VerifyParams {
            #[serde(rename = "hub.mode")]
            hub_mode: Option<String>,
            #[serde(rename = "hub.verify_token")]
            hub_verify_token: Option<String>,
            #[serde(rename = "hub.challenge")]
            hub_challenge: Option<String>,
        }

        // Webhook verification handler (GET)
        async fn verify_webhook(
            State(state): State<WebhookState>,
            Query(params): Query<VerifyParams>,
        ) -> axum::response::Response {
            use axum::http::StatusCode;
            use axum::response::IntoResponse;

            if params.hub_mode.as_deref() == Some("subscribe")
                && params.hub_verify_token.as_deref() == Some(&state.verify_token)
            {
                if let Some(challenge) = params.hub_challenge {
                    return (StatusCode::OK, challenge).into_response();
                }
            }
            (StatusCode::FORBIDDEN, "Verification failed").into_response()
        }

        // Message webhook handler (POST)
        async fn receive_webhook(
            State(state): State<WebhookState>,
            axum::Json(body): axum::Json<Value>,
        ) -> axum::http::StatusCode {
            let messages = WhatsAppChannel::parse_webhook_messages(&body);

            for (sender, text, _msg_id) in messages {
                // Check allowed users
                if !state.allowed_users.is_empty()
                    && !state.allowed_users.iter().any(|u| u == "*")
                    && !state.allowed_users.iter().any(|u| u == &sender)
                {
                    tracing::debug!(
                        "WhatsApp: ignoring message from disallowed sender {}",
                        sender
                    );
                    continue;
                }

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let msg = InboundMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender: sender.clone(),
                    content: text,
                    channel: "whatsapp".to_string(),
                    chat_id: sender,
                    timestamp: now,
                    reply_to: None,
                    metadata: HashMap::new(),
                };

                if state.tx.send(msg).await.is_err() {
                    tracing::info!("WhatsApp: inbound channel closed");
                    break;
                }
            }

            axum::http::StatusCode::OK
        }

        let state = WebhookState {
            verify_token: self.verify_token.clone(),
            allowed_users: self.allowed_users.clone(),
            tx,
        };

        let app = Router::new()
            .route("/webhook/whatsapp", get(verify_webhook))
            .route("/webhook/whatsapp", post(receive_webhook))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", self.webhook_port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .context("WhatsApp webhook server bind failed")?;

        tracing::info!("WhatsApp webhook listening on {}", addr);
        axum::serve(listener, app)
            .await
            .context("WhatsApp webhook server error")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_webhook_messages_valid() {
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "123456",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {
                            "display_phone_number": "15551234567",
                            "phone_number_id": "123456789"
                        },
                        "contacts": [{
                            "profile": {"name": "Alice"},
                            "wa_id": "15559876543"
                        }],
                        "messages": [{
                            "from": "15559876543",
                            "id": "wamid.abc123",
                            "timestamp": "1700000000",
                            "text": {"body": "Hello Fennec!"},
                            "type": "text"
                        }]
                    },
                    "field": "messages"
                }]
            }]
        });

        let messages = WhatsAppChannel::parse_webhook_messages(&payload);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, "15559876543");
        assert_eq!(messages[0].1, "Hello Fennec!");
        assert_eq!(messages[0].2, "wamid.abc123");
    }

    #[test]
    fn test_parse_webhook_messages_empty() {
        let payload = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "123456",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "statuses": []
                    },
                    "field": "messages"
                }]
            }]
        });

        let messages = WhatsAppChannel::parse_webhook_messages(&payload);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_parse_webhook_messages_non_text_skipped() {
        let payload = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "from": "15559876543",
                            "id": "wamid.img123",
                            "type": "image",
                            "image": {"id": "img-id"}
                        }]
                    }
                }]
            }]
        });

        let messages = WhatsAppChannel::parse_webhook_messages(&payload);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_allows_sender() {
        let ch = WhatsAppChannel::new(
            "123".to_string(),
            "token".to_string(),
            "verify".to_string(),
            9090,
            vec!["15551234567".to_string()],
        );
        assert!(ch.allows_sender("15551234567"));
        assert!(!ch.allows_sender("15559999999"));

        // Empty list allows all
        let ch2 = WhatsAppChannel::new(
            "123".to_string(),
            "token".to_string(),
            "verify".to_string(),
            9090,
            vec![],
        );
        assert!(ch2.allows_sender("anyone"));

        // Wildcard allows all
        let ch3 = WhatsAppChannel::new(
            "123".to_string(),
            "token".to_string(),
            "verify".to_string(),
            9090,
            vec!["*".to_string()],
        );
        assert!(ch3.allows_sender("anyone"));
    }
}
