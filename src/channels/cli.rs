use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::bus::InboundMessage;

use super::traits::{Channel, SendMessage};

/// Interactive CLI channel that reads from stdin and writes to stdout.
pub struct CliChannel;

impl CliChannel {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Channel for CliChannel {
    fn name(&self) -> &str {
        "cli"
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        println!("{}", message.content);
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        // Spawn a blocking task to read stdin line by line.
        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let reader = stdin.lock();

            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        let trimmed = text.trim().to_string();

                        // Check for quit commands.
                        if trimmed == "/quit" || trimmed == "/exit" {
                            break;
                        }

                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        let msg = InboundMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            sender: "user".to_string(),
                            content: trimmed,
                            channel: "cli".to_string(),
                            chat_id: "cli_session".to_string(),
                            timestamp: now,
                            reply_to: None,
                            metadata: HashMap::new(),
                        };

                        // If the receiver is dropped, stop listening.
                        if tx.blocking_send(msg).is_err() {
                            break;
                        }

                        print!("You: ");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                    Err(_) => {
                        // EOF or read error — stop listening.
                        break;
                    }
                }
            }
        })
        .await?;

        Ok(())
    }
}
