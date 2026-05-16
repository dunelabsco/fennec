//! Idle-gate scheduler for automatic curator runs.
//!
//! The runner doesn't fire on every agent boot — that would be both
//! wasteful (LLM calls cost tokens) and disruptive (curator
//! consolidation can delete or rename skills the user is about to
//! reach for). Instead, the gate checks three things:
//!
//!   1. The user hasn't paused automatic runs (`paused` flag).
//!   2. Enough wall-clock time has passed since the last run
//!      (`interval_hours`, default seven days).
//!   3. The agent is currently idle (`min_idle_hours` since the last
//!      tool call), if the caller has an idle measurement.
//!
//! Manual runs (`fennec curator run`) bypass the gate entirely.

use chrono::{DateTime, Duration, Utc};

use super::state::CuratorState;

/// Knobs for the curator schedule. Defaults match the upstream
/// reference and are intended to feel "background-y": the curator
/// should run rarely enough that you forget about it.
#[derive(Debug, Clone, Copy)]
pub struct CuratorScheduleConfig {
    /// Curator is enabled overall. When false, automatic runs never
    /// fire (manual `fennec curator run` still works as long as the
    /// CLI command is wired). Default true.
    pub enabled: bool,
    /// Wall-clock hours between automatic runs. Default 168 (one
    /// week). The gate compares against `state.last_run_at`.
    pub interval_hours: u64,
    /// Minimum idle hours since the last tool call before an
    /// automatic run is allowed. Set to 0 to disable the idle gate.
    /// Default 2.
    pub min_idle_hours: f64,
}

impl Default for CuratorScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: 168,
            min_idle_hours: 2.0,
        }
    }
}

/// Decide whether the curator should run automatically right now.
/// Manual runs ignore this and call the runner directly.
///
/// `idle_seconds` is the duration since the last tool-call activity,
/// or `None` if the caller has no measurement (the idle gate is
/// then ignored).
pub fn should_auto_run(
    config: &CuratorScheduleConfig,
    state: &CuratorState,
    idle_seconds: Option<f64>,
    now: DateTime<Utc>,
) -> AutoRunDecision {
    if !config.enabled {
        return AutoRunDecision::Skip(SkipReason::Disabled);
    }
    if state.paused {
        return AutoRunDecision::Skip(SkipReason::Paused);
    }
    if let Some(last) = state.last_run_at {
        let elapsed = now - last;
        let interval = Duration::hours(config.interval_hours as i64);
        if elapsed < interval {
            return AutoRunDecision::Skip(SkipReason::TooSoon { elapsed, interval });
        }
    }
    if let Some(idle) = idle_seconds {
        let needed = config.min_idle_hours * 3600.0;
        if idle < needed {
            return AutoRunDecision::Skip(SkipReason::NotIdle {
                observed_seconds: idle,
                required_seconds: needed,
            });
        }
    }
    AutoRunDecision::Run
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutoRunDecision {
    /// Gate passed — caller may invoke the runner.
    Run,
    /// Gate refused — surfaced for `fennec curator status`.
    Skip(SkipReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    Disabled,
    Paused,
    TooSoon {
        elapsed: Duration,
        interval: Duration,
    },
    NotIdle {
        observed_seconds: f64,
        required_seconds: f64,
    },
}

impl SkipReason {
    /// One-line human-readable form for status output.
    pub fn as_human_string(&self) -> String {
        match self {
            SkipReason::Disabled => "curator disabled in config".into(),
            SkipReason::Paused => "curator paused".into(),
            SkipReason::TooSoon { elapsed, interval } => format!(
                "{:.1}h since last run; interval is {:.1}h",
                elapsed.num_minutes() as f64 / 60.0,
                interval.num_minutes() as f64 / 60.0
            ),
            SkipReason::NotIdle {
                observed_seconds,
                required_seconds,
            } => format!(
                "agent is busy ({:.1}m idle; {:.1}m required)",
                observed_seconds / 60.0,
                required_seconds / 60.0
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn disabled_skips() {
        let cfg = CuratorScheduleConfig {
            enabled: false,
            ..Default::default()
        };
        let state = CuratorState::default();
        assert!(matches!(
            should_auto_run(&cfg, &state, None, now()),
            AutoRunDecision::Skip(SkipReason::Disabled)
        ));
    }

    #[test]
    fn paused_skips() {
        let cfg = CuratorScheduleConfig::default();
        let state = CuratorState {
            paused: true,
            ..Default::default()
        };
        assert!(matches!(
            should_auto_run(&cfg, &state, None, now()),
            AutoRunDecision::Skip(SkipReason::Paused)
        ));
    }

    #[test]
    fn too_soon_skips() {
        let cfg = CuratorScheduleConfig::default();
        let n = now();
        let state = CuratorState {
            last_run_at: Some(n - Duration::hours(1)),
            ..Default::default()
        };
        let d = should_auto_run(&cfg, &state, None, n);
        assert!(matches!(d, AutoRunDecision::Skip(SkipReason::TooSoon { .. })));
    }

    #[test]
    fn first_run_passes_interval_gate() {
        let cfg = CuratorScheduleConfig::default();
        let state = CuratorState::default(); // last_run_at = None
        // No idle measurement, so the idle gate is bypassed.
        assert_eq!(should_auto_run(&cfg, &state, None, now()), AutoRunDecision::Run);
    }

    #[test]
    fn interval_satisfied_runs() {
        let cfg = CuratorScheduleConfig::default();
        let n = now();
        let state = CuratorState {
            last_run_at: Some(n - Duration::hours(200)), // > 168
            ..Default::default()
        };
        assert_eq!(should_auto_run(&cfg, &state, None, n), AutoRunDecision::Run);
    }

    #[test]
    fn busy_agent_skips() {
        let cfg = CuratorScheduleConfig {
            min_idle_hours: 2.0,
            ..Default::default()
        };
        let state = CuratorState::default();
        // 30 minutes idle, need 2 hours.
        let d = should_auto_run(&cfg, &state, Some(30.0 * 60.0), now());
        assert!(matches!(d, AutoRunDecision::Skip(SkipReason::NotIdle { .. })));
    }

    #[test]
    fn idle_agent_runs() {
        let cfg = CuratorScheduleConfig {
            min_idle_hours: 2.0,
            ..Default::default()
        };
        let state = CuratorState::default();
        // 3 hours idle.
        assert_eq!(
            should_auto_run(&cfg, &state, Some(3.0 * 3600.0), now()),
            AutoRunDecision::Run
        );
    }

    #[test]
    fn idle_zero_disables_idle_gate() {
        let cfg = CuratorScheduleConfig {
            min_idle_hours: 0.0,
            ..Default::default()
        };
        let state = CuratorState::default();
        // Even 0 seconds idle should pass.
        assert_eq!(
            should_auto_run(&cfg, &state, Some(0.0), now()),
            AutoRunDecision::Run
        );
    }
}
