use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

/// Email channel with IMAP polling for incoming mail and SMTP (via lettre) for
/// outgoing messages.
pub struct EmailChannel {
    imap_host: String,
    imap_port: u16,
    imap_user: String,
    imap_password: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_user: String,
    smtp_password: String,
    from_address: String,
    allowed_senders: Vec<String>,
    poll_interval_secs: u64,
}

impl EmailChannel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        imap_host: String,
        imap_port: u16,
        imap_user: String,
        imap_password: String,
        smtp_host: String,
        smtp_port: u16,
        smtp_user: String,
        smtp_password: String,
        from_address: String,
        allowed_senders: Vec<String>,
        poll_interval_secs: u64,
    ) -> Self {
        Self {
            imap_host,
            imap_port,
            imap_user,
            imap_password,
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_password,
            from_address,
            allowed_senders,
            poll_interval_secs: if poll_interval_secs == 0 {
                30
            } else {
                poll_interval_secs
            },
        }
    }

    /// Extract the plain-text sender email from a raw "From" header value.
    /// Handles formats like:
    ///   "Alice <alice@example.com>"
    ///   "alice@example.com"
    pub fn extract_sender_email(from: &str) -> String {
        if let Some(start) = from.find('<') {
            if let Some(end) = from.find('>') {
                return from[start + 1..end].trim().to_lowercase();
            }
        }
        from.trim().to_lowercase()
    }

    /// Extract the plain-text body from a raw email body string.
    /// This is a best-effort parser: it takes the first text/plain part
    /// or falls back to the raw body with tags stripped.
    pub fn extract_text_body(raw: &str) -> String {
        // Simple approach: just use the raw body, trimming common MIME artifacts.
        // A production implementation would use a proper MIME parser.
        let body = raw.trim();
        if body.is_empty() {
            return String::new();
        }
        body.to_string()
    }

    /// Connect to IMAP and fetch unseen messages. Returns a list of
    /// (sender, subject, body) tuples.
    async fn fetch_unseen(&self) -> Result<Vec<(String, String, String)>> {
        use futures::StreamExt;

        let tls = async_native_tls::TlsConnector::new();
        let imap_addr = (self.imap_host.as_str(), self.imap_port);

        let tcp_stream = tokio::net::TcpStream::connect(imap_addr)
            .await
            .context("IMAP TCP connect failed")?;

        let tls_stream = tls
            .connect(&self.imap_host, tcp_stream)
            .await
            .context("IMAP TLS connect failed")?;

        let client = async_imap::Client::new(tls_stream);
        let mut session = client
            .login(&self.imap_user, &self.imap_password)
            .await
            .map_err(|e| anyhow::anyhow!("IMAP login failed: {}", e.0))?;

        session
            .select("INBOX")
            .await
            .context("IMAP SELECT INBOX failed")?;

        // Search for unseen messages.
        let search_results = session
            .search("UNSEEN")
            .await
            .context("IMAP SEARCH UNSEEN failed")?;

        if search_results.is_empty() {
            let _ = session.logout().await;
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // Build sequence set from message sequence numbers.
        let seqs: Vec<String> = search_results.iter().map(|u| u.to_string()).collect();
        let seq_set = seqs.join(",");

        // Fetch full RFC822 message for each unseen message.
        let mut fetch_stream = session
            .fetch(&seq_set, "RFC822")
            .await
            .context("IMAP FETCH failed")?;

        while let Some(fetch_result) = fetch_stream.next().await {
            let fetch = match fetch_result {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("IMAP fetch error: {}", e);
                    continue;
                }
            };

            // async-imap Fetch::body() returns Option<&[u8]> for the RFC822 body.
            let raw_bytes = match fetch.body() {
                Some(b) => b,
                None => continue,
            };
            let raw_str = String::from_utf8_lossy(raw_bytes);

            // Simple header/body split on first blank line.
            let (header_part, body_part) = match raw_str.find("\r\n\r\n") {
                Some(pos) => (&raw_str[..pos], &raw_str[pos + 4..]),
                None => match raw_str.find("\n\n") {
                    Some(pos) => (&raw_str[..pos], &raw_str[pos + 2..]),
                    None => continue,
                },
            };

            let mut sender = String::new();
            let mut subject = String::new();

            for line in header_part.lines() {
                let lower = line.to_lowercase();
                if lower.starts_with("from:") {
                    sender = Self::extract_sender_email(&line[5..]);
                } else if lower.starts_with("subject:") {
                    subject = line[8..].trim().to_string();
                }
            }

            let body = Self::extract_text_body(body_part);

            if !sender.is_empty() && !body.is_empty() {
                results.push((sender, subject, body));
            }
        }
        drop(fetch_stream);

        // Mark fetched messages as seen.
        let _ = session
            .store(&seq_set, "+FLAGS (\\Seen)")
            .await;

        let _ = session.logout().await;

        Ok(results)
    }

    /// Send an email via SMTP using lettre.
    fn build_smtp_transport(
        &self,
    ) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>> {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::AsyncSmtpTransport;

        let creds = Credentials::new(self.smtp_user.clone(), self.smtp_password.clone());

        let transport = if self.smtp_port == 465 {
            AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&self.smtp_host)
                .context("SMTP relay setup failed")?
                .credentials(creds)
                .port(self.smtp_port)
                .build()
        } else {
            AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&self.smtp_host)
                .context("SMTP STARTTLS relay setup failed")?
                .credentials(creds)
                .port(self.smtp_port)
                .build()
        };

        Ok(transport)
    }
}

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        "email"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        use lettre::message::header::ContentType;
        use lettre::Message;
        use lettre::AsyncTransport;

        let email = Message::builder()
            .from(
                self.from_address
                    .parse()
                    .context("Invalid from_address for SMTP")?,
            )
            .to(message
                .recipient
                .parse()
                .context("Invalid recipient email address")?)
            .subject("Re: Fennec")
            .header(ContentType::TEXT_PLAIN)
            .body(message.content.clone())
            .context("Failed to build email message")?;

        let transport = self.build_smtp_transport()?;
        transport
            .send(email)
            .await
            .context("SMTP send failed")?;

        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        let interval = tokio::time::Duration::from_secs(self.poll_interval_secs);

        loop {
            match self.fetch_unseen().await {
                Ok(messages) => {
                    for (sender, subject, body) in messages {
                        if !self.allows_sender(&sender) {
                            tracing::debug!(
                                "Email: ignoring message from disallowed sender {}",
                                sender
                            );
                            continue;
                        }

                        let content = if subject.is_empty() {
                            body
                        } else {
                            format!("[{}] {}", subject, body)
                        };

                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        let msg = InboundMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            sender: sender.clone(),
                            content,
                            channel: "email".to_string(),
                            chat_id: sender,
                            timestamp: now,
                            reply_to: None,
                            metadata: HashMap::new(),
                        };

                        if tx.send(msg).await.is_err() {
                            tracing::info!("Email: inbound channel closed, stopping listener");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Email: IMAP poll error: {}", e);
                }
            }

            tokio::time::sleep(interval).await;
        }
    }

    fn allows_sender(&self, sender_id: &str) -> bool {
        if self.allowed_senders.is_empty() {
            return true;
        }
        if self.allowed_senders.iter().any(|u| u == "*") {
            return true;
        }
        let sender_lower = sender_id.to_lowercase();
        self.allowed_senders
            .iter()
            .any(|u| u.to_lowercase() == sender_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sender_email_with_angle_brackets() {
        assert_eq!(
            EmailChannel::extract_sender_email("Alice Smith <alice@example.com>"),
            "alice@example.com"
        );
    }

    #[test]
    fn test_extract_sender_email_plain() {
        assert_eq!(
            EmailChannel::extract_sender_email("alice@example.com"),
            "alice@example.com"
        );
    }

    #[test]
    fn test_extract_sender_email_uppercase() {
        assert_eq!(
            EmailChannel::extract_sender_email("BOB@Example.COM"),
            "bob@example.com"
        );
    }

    #[test]
    fn test_extract_text_body() {
        assert_eq!(
            EmailChannel::extract_text_body("  Hello world  "),
            "Hello world"
        );
        assert_eq!(EmailChannel::extract_text_body(""), "");
    }

    #[test]
    fn test_allows_sender_empty() {
        let ch = EmailChannel::new(
            "imap.example.com".to_string(),
            993,
            "user".to_string(),
            "pass".to_string(),
            "smtp.example.com".to_string(),
            587,
            "user".to_string(),
            "pass".to_string(),
            "bot@example.com".to_string(),
            vec![],
            30,
        );
        assert!(ch.allows_sender("anyone@example.com"));
    }

    #[test]
    fn test_allows_sender_wildcard() {
        let ch = EmailChannel::new(
            "imap.example.com".to_string(),
            993,
            "user".to_string(),
            "pass".to_string(),
            "smtp.example.com".to_string(),
            587,
            "user".to_string(),
            "pass".to_string(),
            "bot@example.com".to_string(),
            vec!["*".to_string()],
            30,
        );
        assert!(ch.allows_sender("anyone@example.com"));
    }

    #[test]
    fn test_allows_sender_specific() {
        let ch = EmailChannel::new(
            "imap.example.com".to_string(),
            993,
            "user".to_string(),
            "pass".to_string(),
            "smtp.example.com".to_string(),
            587,
            "user".to_string(),
            "pass".to_string(),
            "bot@example.com".to_string(),
            vec!["alice@example.com".to_string()],
            30,
        );
        assert!(ch.allows_sender("alice@example.com"));
        assert!(ch.allows_sender("Alice@Example.COM")); // case insensitive
        assert!(!ch.allows_sender("bob@example.com"));
    }
}
