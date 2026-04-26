use std::collections::HashMap;
use std::time::Duration;

use crate::bus::{InboundMessage, MessageBus};

/// Default heartbeat interval in seconds (30 minutes).
const DEFAULT_INTERVAL_SECS: u64 = 1800;

/// Default heartbeat prompt used when `~/.fennec/HEARTBEAT.md` does not exist.
const DEFAULT_HEARTBEAT_PROMPT: &str =
    "Check if there are any pending tasks, cron jobs, or inbox items to process.\n\
     If nothing needs attention, respond with [SILENT].";

/// A periodic heartbeat that wakes the agent to check for pending work.
///
/// Each tick publishes an [`InboundMessage`] on the `"heartbeat"` channel.
/// The main agent loop processes it like any other message. If the agent
/// response contains `[SILENT]`, the caller should suppress any outbound
/// reply.
pub struct HeartbeatService {
    interval_secs: u64,
    bus: MessageBus,
    heartbeat_prompt: String,
}

impl HeartbeatService {
    /// Create a new heartbeat service.
    ///
    /// The prompt is loaded from `~/.fennec/HEARTBEAT.md` if it exists,
    /// otherwise the built-in default is used. The caller may also supply a
    /// custom prompt directly via `prompt_override`.
    pub fn new(
        interval_secs: Option<u64>,
        bus: MessageBus,
        prompt_override: Option<String>,
    ) -> Self {
        let heartbeat_prompt = prompt_override.unwrap_or_else(|| {
            load_heartbeat_prompt().unwrap_or_else(|| DEFAULT_HEARTBEAT_PROMPT.to_string())
        });

        Self {
            interval_secs: interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS),
            bus,
            heartbeat_prompt,
        }
    }

    /// Run the heartbeat loop. This blocks until the task is cancelled.
    ///
    /// `MissedTickBehavior::Skip` prevents a rapid-fire catch-up after the
    /// machine wakes from sleep. With the default `Burst`, a laptop that
    /// was suspended for 8 hours would immediately fire 16 heartbeats
    /// back-to-back on wake — which in turn fires 16 LLM calls.
    pub async fn run(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(self.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            self.tick().await;
        }
    }

    /// Execute a single heartbeat tick: publish an inbound message.
    pub async fn tick(&self) {
        let msg = InboundMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender: "heartbeat".to_string(),
            content: self.heartbeat_prompt.clone(),
            channel: "heartbeat".to_string(),
            chat_id: "heartbeat".to_string(),
            timestamp: chrono::Utc::now().timestamp() as u64,
            reply_to: None,
            metadata: HashMap::new(),
        };

        if let Err(e) = self.bus.publish_inbound(msg).await {
            tracing::error!("Heartbeat: failed to publish inbound message: {e}");
        }
    }

    /// Check whether an agent response should be suppressed (i.e. contains
    /// the `[SILENT]` marker).
    pub fn is_silent_response(response: &str) -> bool {
        response.contains("[SILENT]")
    }

    /// The current heartbeat interval in seconds.
    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    /// The prompt that is sent on each heartbeat tick.
    pub fn prompt(&self) -> &str {
        &self.heartbeat_prompt
    }
}

/// Try to load the heartbeat prompt from `~/.fennec/HEARTBEAT.md`.
fn load_heartbeat_prompt() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".fennec").join("HEARTBEAT.md");
    std::fs::read_to_string(path).ok().and_then(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_interval() {
        assert_eq!(DEFAULT_INTERVAL_SECS, 1800);
    }

    #[test]
    fn test_is_silent_response() {
        assert!(HeartbeatService::is_silent_response(
            "Nothing to do. [SILENT]"
        ));
        assert!(HeartbeatService::is_silent_response("[SILENT]"));
        assert!(!HeartbeatService::is_silent_response(
            "There are pending tasks."
        ));
    }

    #[test]
    fn test_new_with_custom_prompt() {
        let (bus, _rx) = MessageBus::new(8);
        let svc = HeartbeatService::new(
            Some(60),
            bus,
            Some("custom prompt".to_string()),
        );
        assert_eq!(svc.interval_secs(), 60);
        assert_eq!(svc.prompt(), "custom prompt");
    }

    #[test]
    fn test_new_default_prompt() {
        let (bus, _rx) = MessageBus::new(8);
        let svc = HeartbeatService::new(None, bus, None);
        assert_eq!(svc.interval_secs(), DEFAULT_INTERVAL_SECS);
        // The prompt should be either the file-based one or the default.
        assert!(!svc.prompt().is_empty());
    }

    #[tokio::test]
    async fn test_tick_publishes_message() {
        let (bus, mut rx) = MessageBus::new(8);
        let svc = HeartbeatService::new(
            Some(60),
            bus,
            Some("test prompt".to_string()),
        );

        svc.tick().await;

        let msg = rx
            .inbound_rx
            .try_recv()
            .expect("should have received a message");
        assert_eq!(msg.channel, "heartbeat");
        assert_eq!(msg.content, "test prompt");
        assert_eq!(msg.sender, "heartbeat");
    }
}
