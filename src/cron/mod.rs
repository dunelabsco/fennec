pub mod jobs;
pub use jobs::{parse_schedule, parse_schedule_kind, CronJob, JobStore, ScheduleKind};

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
    /// for due jobs (default: 30).
    pub fn new(store: JobStore, bus: MessageBus, check_interval_secs: Option<u64>) -> Self {
        Self {
            store,
            bus,
            check_interval_secs: check_interval_secs.unwrap_or(30),
        }
    }

    /// Run the scheduler loop. This blocks until the task is cancelled.
    ///
    /// The interval is configured with
    /// `MissedTickBehavior::Skip` so that a system suspend / resume (laptop
    /// sleep) doesn't produce a burst of back-to-back ticks catching up
    /// missed time — the scheduler just resumes on the next normal tick.
    /// With the default `Burst` behavior, every `"every 5m"` job whose
    /// `last_run` predates the sleep would refire rapidly on wake.
    pub async fn run(&mut self) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.check_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            self.tick().await;
        }
    }

    /// Execute a single scheduler tick: check all enabled jobs and fire those
    /// that are due.
    pub async fn tick(&mut self) {
        let now = Utc::now();
        let mut dirty = false;

        // Snapshot the state we need before we mutate. Holding an
        // iterator over store.list_jobs() while taking a &mut to the
        // store would require gymnastics; a small clone is fine.
        let jobs: Vec<(String, String, String, Option<String>, Option<String>, Option<String>)> =
            self.store
                .list_jobs()
                .iter()
                .filter(|j| j.enabled)
                .map(|j| {
                    (
                        j.id.clone(),
                        j.schedule.clone(),
                        j.command.clone(),
                        j.last_run.clone(),
                        j.origin_channel.clone(),
                        j.origin_chat_id.clone(),
                    )
                })
                .collect();

        for (id, schedule, command, last_run, origin_channel, origin_chat_id) in jobs {
            let kind = match parse_schedule_kind(&schedule) {
                Some(k) => k,
                None => {
                    tracing::warn!("Cron job '{}': invalid schedule '{}'", id, schedule);
                    continue;
                }
            };

            // Resolve the baseline timestamp to measure "elapsed since …"
            // against. If the job has never run (`last_run == None`), anchor
            // the baseline to NOW on first observation — recording it as the
            // synthetic last_run so the first real fire is an interval later.
            // The old code treated `last_run == None` as "fire immediately,"
            // which meant adding a new `"every 24h"` job caused it to fire
            // within 30 seconds instead of 24 hours later.
            let last_dt_opt: Option<chrono::DateTime<chrono::Utc>> = last_run
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            if last_dt_opt.is_none() {
                // Anchor and skip this tick.
                if let Some(job) = self.store.get_mut(&id) {
                    job.last_run = Some(now.to_rfc3339());
                }
                dirty = true;
                continue;
            }
            let last_dt = last_dt_opt.expect("just checked is_some");
            let elapsed = now
                .signed_duration_since(last_dt)
                .num_seconds()
                .max(0) as u64;

            let (is_due, is_one_shot) = match kind {
                ScheduleKind::Recurring { interval_secs } => (elapsed >= interval_secs, false),
                ScheduleKind::OneShot { delay_secs } => {
                    // For a one-shot, `last_run` is either None (anchor — not
                    // reachable here) or Some(when-we-anchored). The
                    // elapsed check against delay_secs gives the right
                    // "delay from creation" semantics.
                    (elapsed >= delay_secs, true)
                }
            };

            if !is_due {
                continue;
            }

            let msg = InboundMessage {
                id: uuid::Uuid::new_v4().to_string(),
                sender: format!("cron:{}", id),
                content: command,
                channel: origin_channel.clone().unwrap_or_else(|| "cron".to_string()),
                chat_id: origin_chat_id.clone().unwrap_or_else(|| format!("cron:{}", id)),
                timestamp: now.timestamp() as u64,
                reply_to: None,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("source".to_string(), "cron".to_string());
                    m.insert("cron_job_id".to_string(), id.clone());
                    m
                },
            };

            if let Err(e) = self.bus.publish_inbound(msg).await {
                tracing::error!("Cron job '{}': failed to publish message: {}", id, e);
                continue;
            }

            // Update last_run; for one-shot jobs, also disable so the next
            // tick skips them entirely instead of re-firing every tick
            // (the headline bug this branch fixes).
            if let Some(job) = self.store.get_mut(&id) {
                job.last_run = Some(now.to_rfc3339());
                if is_one_shot {
                    job.enabled = false;
                    tracing::debug!("Cron job '{}': one-shot fired, disabling", id);
                }
            }
            dirty = true;
        }

        if dirty {
            if let Err(e) = self.store.save() {
                tracing::error!("Failed to save job store after cron tick: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;

    fn job(id: &str, schedule: &str, last_run: Option<&str>) -> CronJob {
        CronJob {
            id: id.into(),
            name: id.into(),
            schedule: schedule.into(),
            command: "do a thing".into(),
            enabled: true,
            last_run: last_run.map(|s| s.to_string()),
            origin_channel: None,
            origin_chat_id: None,
        }
    }

    #[tokio::test]
    async fn newly_added_recurring_job_does_not_fire_immediately() {
        // Regression: the old tick() treated last_run=None as "fire now"
        // which meant a newly-added `"every 24h"` job fired within the
        // first 30-second scheduler tick.
        let tmp = tempfile::tempdir().unwrap();
        let mut store = JobStore::new(tmp.path().join("jobs.json"));
        store.add_job(job("j1", "every 24h", None));

        let (bus, mut rx) = MessageBus::new(16);
        let mut scheduler = CronScheduler::new(store, bus, Some(30));
        scheduler.tick().await;

        // No inbound message should have been published.
        assert!(
            rx.inbound_rx.try_recv().is_err(),
            "newly added job fired on first tick — regression"
        );

        // last_run must have been anchored so future ticks can compute
        // elapsed correctly.
        let anchored = scheduler
            .store
            .list_jobs()
            .iter()
            .find(|j| j.id == "j1")
            .unwrap()
            .last_run
            .clone();
        assert!(anchored.is_some(), "last_run must be anchored");
    }

    #[tokio::test]
    async fn recurring_job_fires_after_interval_elapsed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = JobStore::new(tmp.path().join("jobs.json"));
        // last_run 2 hours ago — every 1h schedule should fire.
        let old = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        store.add_job(job("j2", "every 1h", Some(&old)));

        let (bus, mut rx) = MessageBus::new(16);
        let mut scheduler = CronScheduler::new(store, bus, Some(30));
        scheduler.tick().await;

        let msg = rx
            .inbound_rx
            .try_recv()
            .expect("recurring job past its interval should fire");
        assert_eq!(msg.sender, "cron:j2");
    }

    #[tokio::test]
    async fn one_shot_disables_after_firing() {
        // Regression for the headline T2-B bug: `"5m"` used to fire on
        // every scheduler tick after the first because `elapsed >= 300`
        // stayed true forever. The scheduler must now set enabled=false
        // after the first fire.
        let tmp = tempfile::tempdir().unwrap();
        let mut store = JobStore::new(tmp.path().join("jobs.json"));
        // last_run 1 hour ago — one-shot "5m" should have fired long ago.
        let old = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        store.add_job(job("once", "5m", Some(&old)));

        let (bus, mut rx) = MessageBus::new(16);
        let mut scheduler = CronScheduler::new(store, bus, Some(30));
        scheduler.tick().await;

        // Fires once.
        let msg = rx.inbound_rx.try_recv().expect("one-shot fires once");
        assert_eq!(msg.sender, "cron:once");

        // After firing, the job must be disabled.
        let disabled = scheduler
            .store
            .list_jobs()
            .iter()
            .find(|j| j.id == "once")
            .unwrap();
        assert!(!disabled.enabled, "one-shot job must be disabled after firing");

        // Second tick: must NOT fire again.
        scheduler.tick().await;
        assert!(
            rx.inbound_rx.try_recv().is_err(),
            "one-shot re-fired — regression"
        );
    }

    #[tokio::test]
    async fn invalid_schedule_does_not_crash_scheduler() {
        // A job with an unparseable schedule should be logged and skipped,
        // not crash the whole tick.
        let tmp = tempfile::tempdir().unwrap();
        let mut store = JobStore::new(tmp.path().join("jobs.json"));
        store.add_job(job("bad", "every 0s", None));
        store.add_job(job("good", "every 1h", None));

        let (bus, _rx) = MessageBus::new(16);
        let mut scheduler = CronScheduler::new(store, bus, Some(30));
        // Must not panic. No assertion needed beyond "this returns".
        scheduler.tick().await;
    }
}
