pub mod jobs;
pub use jobs::{parse_schedule, CronJob, JobStore};

use std::collections::HashMap;

use chrono::Utc;

use crate::bus::{InboundMessage, MessageBus};

pub struct CronScheduler {
    store: JobStore,
    bus: MessageBus,
    check_interval_secs: u64,
}

impl CronScheduler {
    /// Create a new scheduler.
    ///
    /// `check_interval_secs` controls how often the scheduler wakes up to check
    /// for due jobs (default: 60).
    pub fn new(store: JobStore, bus: MessageBus, check_interval_secs: Option<u64>) -> Self {
        Self {
            store,
            bus,
            check_interval_secs: check_interval_secs.unwrap_or(60),
        }
    }

    /// Run the scheduler loop. This blocks until the task is cancelled.
    pub async fn run(&mut self) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.check_interval_secs));

        loop {
            interval.tick().await;
            self.tick().await;
        }
    }

    /// Execute a single scheduler tick: check all enabled jobs and fire those
    /// that are due.
    pub async fn tick(&mut self) {
        let now = Utc::now();
        let mut fired = false;

        let jobs: Vec<(String, String, String, Option<String>)> = self
            .store
            .list_jobs()
            .iter()
            .filter(|j| j.enabled)
            .map(|j| {
                (
                    j.id.clone(),
                    j.schedule.clone(),
                    j.command.clone(),
                    j.last_run.clone(),
                )
            })
            .collect();

        for (id, schedule, command, last_run) in jobs {
            let interval_secs = match parse_schedule(&schedule) {
                Some(s) => s,
                None => {
                    tracing::warn!("Cron job '{}': invalid schedule '{}'", id, schedule);
                    continue;
                }
            };

            let is_due = match &last_run {
                Some(last) => {
                    if let Ok(last_dt) = chrono::DateTime::parse_from_rfc3339(last) {
                        let elapsed = now
                            .signed_duration_since(last_dt)
                            .num_seconds()
                            .max(0) as u64;
                        elapsed >= interval_secs
                    } else {
                        // Unparseable last_run — treat as never run.
                        true
                    }
                }
                None => true, // Never run before.
            };

            if is_due {
                let msg = InboundMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender: format!("cron:{}", id),
                    content: command,
                    channel: "cron".to_string(),
                    chat_id: format!("cron:{}", id),
                    timestamp: now.timestamp() as u64,
                    reply_to: None,
                    metadata: HashMap::new(),
                };

                if let Err(e) = self.bus.publish_inbound(msg).await {
                    tracing::error!("Cron job '{}': failed to publish message: {}", id, e);
                    continue;
                }

                // Update last_run.
                if let Some(job) = self.store.get_mut(&id) {
                    job.last_run = Some(now.to_rfc3339());
                }
                fired = true;
            }
        }

        if fired {
            if let Err(e) = self.store.save() {
                tracing::error!("Failed to save job store after cron tick: {}", e);
            }
        }
    }
}
