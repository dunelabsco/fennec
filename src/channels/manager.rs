use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bus::{MessageBus, OutboundMessage};

use super::traits::{Channel, SendMessage};

/// Manages multiple channels, routing inbound messages to the bus and
/// dispatching outbound messages to the appropriate channel.
pub struct ChannelManager {
    channels: Vec<Arc<dyn Channel>>,
    channels_by_name: HashMap<String, Arc<dyn Channel>>,
    bus: MessageBus,
}

impl ChannelManager {
    /// Create a new `ChannelManager` with the given channels and message bus.
    pub fn new(channels: Vec<Arc<dyn Channel>>, bus: MessageBus) -> Self {
        let channels_by_name: HashMap<String, Arc<dyn Channel>> = channels
            .iter()
            .map(|ch| (ch.name().to_string(), Arc::clone(ch)))
            .collect();
        Self {
            channels,
            channels_by_name,
            bus,
        }
    }

    /// Spawn a supervised listener task for each channel.
    ///
    /// Each listener calls `channel.listen(inbound_sender)`. On error or crash
    /// the listener is restarted with exponential backoff (1s initial, doubled
    /// each time, capped at 60s, maximum 10 restarts).
    pub fn start_all(&self) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::with_capacity(self.channels.len());

        for ch in &self.channels {
            let channel = Arc::clone(ch);
            let bus = self.bus.clone();

            let handle = tokio::spawn(async move {
                // Crash-loop guard. We retry up to `max_restarts` times in
                // *quick succession* — but if a listener has been running
                // for `STABLE_RUN_SECS` before it failed, we treat it as
                // "had a stable connection that just dropped" and reset the
                // counter. Without this reset, channels whose normal
                // operation involves periodic reconnect (Slack Socket Mode
                // rotates its WebSocket roughly every 60 minutes by
                // design) would permanently die after ~`max_restarts`
                // hours regardless of how stable each individual run was.
                let initial_backoff_ms: u64 = 1_000;
                let max_backoff_ms: u64 = 60_000;
                let max_restarts: u32 = 10;
                const STABLE_RUN_SECS: u64 = 60;
                let mut backoff_ms: u64 = initial_backoff_ms;
                let mut restarts: u32 = 0;

                loop {
                    let tx = bus.inbound_sender();
                    let name = channel.name().to_string();
                    let started_at = std::time::Instant::now();

                    match channel.listen(tx).await {
                        Ok(()) => {
                            tracing::info!("Channel '{}' listener exited cleanly", name);
                            break;
                        }
                        Err(e) => {
                            // If the listener ran long enough to be
                            // considered "stable" before failing, treat
                            // this as a fresh fault — not part of a crash
                            // loop. Reset counters before counting this
                            // failure.
                            let was_stable = started_at.elapsed()
                                >= std::time::Duration::from_secs(STABLE_RUN_SECS);
                            if was_stable {
                                tracing::debug!(
                                    "Channel '{}' was stable for {}s; resetting crash-loop counters",
                                    name,
                                    started_at.elapsed().as_secs()
                                );
                                restarts = 0;
                                backoff_ms = initial_backoff_ms;
                            }

                            restarts += 1;
                            tracing::error!(
                                "Channel '{}' listener crashed (attempt {}/{}, ran {}s): {}",
                                name,
                                restarts,
                                max_restarts,
                                started_at.elapsed().as_secs(),
                                e
                            );

                            if restarts >= max_restarts {
                                tracing::error!(
                                    "Channel '{}' exceeded max restarts ({}) without a stable run, giving up",
                                    name,
                                    max_restarts,
                                );
                                break;
                            }

                            tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms))
                                .await;
                            backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
                        }
                    }
                }
            });

            handles.push(handle);
        }

        handles
    }

    /// Consume outbound messages from the receiver and dispatch them to the
    /// appropriate channel by name.
    ///
    /// This method takes ownership of the channel map snapshot so it does not
    /// keep the `MessageBus` sender alive, allowing the receiver to close
    /// naturally when all external senders are dropped.
    pub async fn dispatch_outbound(&self, mut outbound_rx: mpsc::Receiver<OutboundMessage>) {
        // Take a snapshot of channels so we don't hold &self across awaits
        // in a way that prevents the bus from being dropped.
        let channels = self.channels_by_name.clone();
        Self::dispatch_loop(&channels, &mut outbound_rx).await;
    }

    /// Spawn an outbound dispatch task that owns the receiver and a snapshot of
    /// the channel map. This avoids holding a reference to the manager (and its
    /// bus sender) so the receiver can close when all external senders drop.
    pub fn spawn_outbound_dispatch(
        &self,
        outbound_rx: mpsc::Receiver<OutboundMessage>,
    ) -> JoinHandle<()> {
        let channels = self.channels_by_name.clone();
        tokio::spawn(async move {
            let mut rx = outbound_rx;
            Self::dispatch_loop(&channels, &mut rx).await;
        })
    }

    async fn dispatch_loop(
        channels: &HashMap<String, Arc<dyn Channel>>,
        outbound_rx: &mut mpsc::Receiver<OutboundMessage>,
    ) {
        while let Some(msg) = outbound_rx.recv().await {
            if let Some(channel) = channels.get(&msg.channel) {
                let send_msg = SendMessage::new(&msg.content, &msg.chat_id)
                    .with_reply_to(msg.reply_to.clone())
                    .with_metadata(msg.metadata.clone())
                    .with_attachments(msg.attachments.clone());
                if let Err(e) = channel.send(&send_msg).await {
                    tracing::error!(
                        "Failed to send outbound message to channel '{}': {}",
                        msg.channel,
                        e
                    );
                }
            } else {
                tracing::warn!("No channel registered for name '{}'", msg.channel);
            }
        }
    }

    /// Look up a channel by name.
    pub fn get_channel(&self, name: &str) -> Option<Arc<dyn Channel>> {
        self.channels_by_name.get(name).cloned()
    }
}
