//! Trust-policy matrix that turns a `Verdict` into an
//! [`InstallDecision`].
//!
//! The same `Caution` verdict is treated differently depending on
//! where the skill came from:
//!
//! ```text
//!                     safe       caution    dangerous
//! Builtin           allow      allow      allow
//! Trusted           allow      allow      block
//! Community         allow      block      block
//! AgentCreated      allow      allow      ask
//! ```
//!
//! `Builtin` are skills shipped with the Fennec binary. `Trusted` is
//! the small set of vetted external publishers (currently empty, but
//! the slot is here for the future hub installer). `Community` is
//! everything else from the hub. `AgentCreated` is anything written
//! by `skill_manage` at the agent's own initiative.
//!
//! `Ask` decisions surface to the user through the install flow;
//! agent-created writes treat `Ask` as a refusal because the agent
//! has no human in the immediate loop.

use super::Verdict;

/// Origin of a skill — the policy axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    /// Shipped with Fennec itself.
    Builtin,
    /// External publisher on the small, curated trust list.
    Trusted,
    /// External publisher, no special trust.
    Community,
    /// Written by the agent through `skill_manage`.
    AgentCreated,
}

/// What the install pipeline should do given a `(trust, verdict,
/// force)` triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallDecision {
    /// Proceed with the install / write.
    Allow,
    /// Surface to the user with this rationale; only proceed on
    /// affirmative confirmation. For agent-created writes (no
    /// interactive user), treat this as `Block`.
    Ask(String),
    /// Refuse outright with this rationale. `force=true` overrides
    /// `Block` to `Allow` (the operator opts into the risk).
    Block(String),
}

/// Apply the policy matrix. `force=true` upgrades a `Block` to
/// `Allow` for the same reason a hub user might pass `--force` —
/// the operator is taking the risk knowingly.
pub fn should_allow_install(
    trust: TrustLevel,
    verdict: Verdict,
    force: bool,
) -> InstallDecision {
    let raw = match (trust, verdict) {
        // Builtin: anything in the binary is, by construction, what
        // the operator already trusts.
        (TrustLevel::Builtin, _) => InstallDecision::Allow,

        // Trusted: dangerous still blocks because trust ≠ infallible
        // (a compromised upstream or a rogue maintainer).
        (TrustLevel::Trusted, Verdict::Safe) => InstallDecision::Allow,
        (TrustLevel::Trusted, Verdict::Caution) => InstallDecision::Allow,
        (TrustLevel::Trusted, Verdict::Dangerous) => {
            InstallDecision::Block("trusted publisher but skill is Dangerous".into())
        }

        // Community: caution is enough to gate. Operator can `--force`.
        (TrustLevel::Community, Verdict::Safe) => InstallDecision::Allow,
        (TrustLevel::Community, Verdict::Caution) => InstallDecision::Block(
            "community skill flagged as Caution; use --force to install anyway".into(),
        ),
        (TrustLevel::Community, Verdict::Dangerous) => InstallDecision::Block(
            "community skill flagged as Dangerous; use --force to install anyway".into(),
        ),

        // AgentCreated: caution is allowed (the agent can self-correct
        // on the next turn), but anything Dangerous prompts. With no
        // interactive user, the caller treats `Ask` as block.
        (TrustLevel::AgentCreated, Verdict::Safe) => InstallDecision::Allow,
        (TrustLevel::AgentCreated, Verdict::Caution) => InstallDecision::Allow,
        (TrustLevel::AgentCreated, Verdict::Dangerous) => InstallDecision::Ask(
            "agent-written skill flagged as Dangerous; confirm before writing".into(),
        ),
    };

    if force {
        match raw {
            InstallDecision::Block(_) => InstallDecision::Allow,
            other => other,
        }
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_passes_every_verdict() {
        for v in [Verdict::Safe, Verdict::Caution, Verdict::Dangerous] {
            assert_eq!(
                should_allow_install(TrustLevel::Builtin, v, false),
                InstallDecision::Allow
            );
        }
    }

    #[test]
    fn trusted_blocks_dangerous() {
        assert_eq!(
            should_allow_install(TrustLevel::Trusted, Verdict::Safe, false),
            InstallDecision::Allow
        );
        assert_eq!(
            should_allow_install(TrustLevel::Trusted, Verdict::Caution, false),
            InstallDecision::Allow
        );
        assert!(matches!(
            should_allow_install(TrustLevel::Trusted, Verdict::Dangerous, false),
            InstallDecision::Block(_)
        ));
    }

    #[test]
    fn community_blocks_caution_and_dangerous() {
        assert_eq!(
            should_allow_install(TrustLevel::Community, Verdict::Safe, false),
            InstallDecision::Allow
        );
        assert!(matches!(
            should_allow_install(TrustLevel::Community, Verdict::Caution, false),
            InstallDecision::Block(_)
        ));
        assert!(matches!(
            should_allow_install(TrustLevel::Community, Verdict::Dangerous, false),
            InstallDecision::Block(_)
        ));
    }

    #[test]
    fn agent_created_asks_on_dangerous() {
        assert_eq!(
            should_allow_install(TrustLevel::AgentCreated, Verdict::Safe, false),
            InstallDecision::Allow
        );
        assert_eq!(
            should_allow_install(TrustLevel::AgentCreated, Verdict::Caution, false),
            InstallDecision::Allow
        );
        assert!(matches!(
            should_allow_install(TrustLevel::AgentCreated, Verdict::Dangerous, false),
            InstallDecision::Ask(_)
        ));
    }

    #[test]
    fn force_upgrades_block_to_allow() {
        assert_eq!(
            should_allow_install(TrustLevel::Community, Verdict::Dangerous, true),
            InstallDecision::Allow
        );
        assert_eq!(
            should_allow_install(TrustLevel::Trusted, Verdict::Dangerous, true),
            InstallDecision::Allow
        );
    }

    #[test]
    fn force_does_not_change_ask() {
        // `Ask` is a different signal — needs interactive confirm, not
        // an opt-out. `force=true` skips the block path but still
        // surfaces an Ask.
        assert!(matches!(
            should_allow_install(TrustLevel::AgentCreated, Verdict::Dangerous, true),
            InstallDecision::Ask(_)
        ));
    }

    #[test]
    fn force_does_not_change_allow() {
        assert_eq!(
            should_allow_install(TrustLevel::Builtin, Verdict::Safe, true),
            InstallDecision::Allow
        );
    }
}
