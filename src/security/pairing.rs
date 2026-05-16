use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::fs::write_secure;

/// Guards DM access via a pairing code flow with brute-force lockout.
///
/// A 6-digit numeric code is generated server-side. Clients present the code
/// to receive a token (`fc_<64-hex>`). The token hash is stored; raw tokens
/// are never persisted.
///
/// Persistence covers everything that needs to survive a restart:
/// - `allowed_users` — IDs the operator has explicitly allowed.
/// - `paired_token_hashes` — without this, every paired client has to
///   re-pair after a restart.
/// - `failed_attempts` — without this, an attacker who scripts the
///   pairing endpoint can bounce the process after every 5 wrong codes
///   to reset the lockout. We persist `(count, since_epoch_secs)` so
///   the lockout window survives. `Instant` is monotonic and not
///   serializable, so we convert to/from `SystemTime` at the persistence
///   boundary; the in-memory representation stays `Instant` for the
///   monotonic-clock guarantees.
pub struct PairingGuard {
    code: Option<String>,
    allowed_users: HashSet<String>,
    paired_token_hashes: HashSet<String>,
    failed_attempts: HashMap<String, (u32, Instant)>,
    persist_path: Option<PathBuf>,
    max_failures: u32,
    lockout_secs: u64,
}

impl PairingGuard {
    /// Create a new `PairingGuard`, optionally loading persisted allowed-users
    /// from `persist_path`.
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        let mut guard = Self {
            code: None,
            allowed_users: HashSet::new(),
            paired_token_hashes: HashSet::new(),
            failed_attempts: HashMap::new(),
            persist_path,
            max_failures: 5,
            lockout_secs: 300,
        };
        // Best-effort load; if the file doesn't exist yet that's fine.
        let _ = guard.load();
        guard
    }

    /// Generate a fresh 6-digit pairing code (with leading zeros).
    pub fn generate_code(&mut self) -> String {
        let mut rng = rand::rng();
        let num: u32 = rng.random_range(0..1_000_000);
        let code = format!("{num:06}");
        self.code = Some(code.clone());
        code
    }

    /// Verify a pairing code presented by `client_id`.
    ///
    /// On success, returns an `fc_<64-hex>` token whose SHA-256 hash is stored
    /// internally. On failure, increments the failed-attempt counter for this
    /// client and returns an error. After `max_failures` consecutive failures
    /// within `lockout_secs` the client is locked out.
    pub fn verify_code(&mut self, client_id: &str, input: &str) -> Result<String> {
        // Check lockout.
        if let Some((count, since)) = self.failed_attempts.get(client_id) {
            if *count >= self.max_failures {
                let elapsed = since.elapsed().as_secs();
                if elapsed < self.lockout_secs {
                    anyhow::bail!(
                        "locked out: too many failed attempts (try again in {}s)",
                        self.lockout_secs - elapsed
                    );
                }
                // Lockout expired — reset counter.
                self.failed_attempts.remove(client_id);
            }
        }

        // Hash the input unconditionally so timing does not distinguish
        // "no code generated" from "wrong code" cases.
        let input_hash = Sha256::digest(input.as_bytes());

        let matched = match self.code.as_deref() {
            Some(expected) => {
                let expected_hash = Sha256::digest(expected.as_bytes());
                // Constant-time compare on the 32-byte digests — short-circuiting
                // equality on the hex String would leak timing.
                bool::from(input_hash.as_slice().ct_eq(expected_hash.as_slice()))
            }
            None => {
                // Dummy compare against a fixed digest keeps the timing
                // profile aligned with the Some branch. The result is
                // ignored; we still return the distinct error below.
                let dummy = Sha256::digest(b"");
                let _ = input_hash.as_slice().ct_eq(dummy.as_slice());
                return Err(anyhow::anyhow!("no pairing code has been generated"));
            }
        };

        if !matched {
            let entry = self
                .failed_attempts
                .entry(client_id.to_string())
                .or_insert((0, Instant::now()));
            entry.0 += 1;
            // Reset the clock on each failure.
            entry.1 = Instant::now();
            // Persist failed attempts so a process restart doesn't reset
            // the lockout counter — without this, an attacker who can
            // bounce the process clears the brute-force window.
            let _ = self.save();
            anyhow::bail!("invalid pairing code");
        }

        // Success — generate token.
        let mut rng = rand::rng();
        let mut token_bytes = [0u8; 32];
        rng.fill(&mut token_bytes);
        let token = format!("fc_{}", hex::encode(token_bytes));

        // Store the hash of the token, not the token itself.
        let token_hash = sha256_hex(&token);
        self.paired_token_hashes.insert(token_hash);

        // Clear the code so it can't be reused.
        self.code = None;

        // Clear failed attempts for this client.
        self.failed_attempts.remove(client_id);

        // Persist the new token hash + cleared attempts so paired
        // clients survive a restart.
        let _ = self.save();

        Ok(token)
    }

    /// Check whether a token is authorized (by comparing its hash against
    /// stored hashes).
    pub fn is_authorized(&self, token: &str) -> bool {
        let hash = sha256_hex(token);
        self.paired_token_hashes.contains(&hash)
    }

    /// Add a user ID to the allowed set and persist.
    pub fn add_allowed_user(&mut self, user_id: &str) {
        self.allowed_users.insert(user_id.to_string());
        let _ = self.save();
    }

    /// Check whether `user_id` is allowed. The wildcard `"*"` permits everyone.
    pub fn is_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.contains("*") || self.allowed_users.contains(user_id)
    }

    /// Persist the full guard state to a JSON file.
    ///
    /// We use `write_secure` (0600 perms) because the file contains
    /// token hashes. The hashes are SHA-256 over fresh 32-byte tokens
    /// so they're not directly recoverable, but the file is still
    /// security-adjacent — it should not be world-readable.
    pub fn save(&self) -> Result<()> {
        let path = self
            .persist_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no persist path configured"))?;

        // Convert in-memory `Instant` timestamps to wall-clock seconds
        // for serialization. `Instant` is monotonic (better for
        // in-memory comparisons) but not portable across restarts; we
        // store the elapsed-vs-now offset as a SystemTime epoch.
        let now_inst = Instant::now();
        let now_sys_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let failed_attempts: Vec<serde_json::Value> = self
            .failed_attempts
            .iter()
            .map(|(client, (count, since))| {
                let since_secs_ago = now_inst.saturating_duration_since(*since).as_secs();
                let since_epoch = now_sys_epoch.saturating_sub(since_secs_ago);
                serde_json::json!({
                    "client": client,
                    "count": count,
                    "since_epoch_secs": since_epoch,
                })
            })
            .collect();

        let data = serde_json::json!({
            "allowed_users": self.allowed_users.iter().collect::<Vec<_>>(),
            "paired_token_hashes": self.paired_token_hashes.iter().collect::<Vec<_>>(),
            "failed_attempts": failed_attempts,
        });
        let json = serde_json::to_string_pretty(&data).context("serializing pairing state")?;
        write_secure(path, json.as_bytes())
            .with_context(|| format!("writing pairing state to {}", path.display()))?;
        Ok(())
    }

    /// Load the full guard state from the JSON file.
    pub fn load(&mut self) -> Result<()> {
        let path = self
            .persist_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no persist path configured"))?;
        if !path.exists() {
            return Ok(());
        }
        let json = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let data: serde_json::Value =
            serde_json::from_str(&json).context("parsing pairing state JSON")?;

        if let Some(users) = data.get("allowed_users").and_then(|v| v.as_array()) {
            self.allowed_users = users
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }

        if let Some(hashes) = data
            .get("paired_token_hashes")
            .and_then(|v| v.as_array())
        {
            self.paired_token_hashes = hashes
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }

        if let Some(attempts) = data
            .get("failed_attempts")
            .and_then(|v| v.as_array())
        {
            // Convert persisted (epoch_secs) back into `Instant` by
            // taking the offset from "now" in both clocks. If the
            // persisted timestamp is in the future relative to now
            // (clock skew between processes / timezone weirdness),
            // anchor at "now" so the lockout window starts fresh
            // rather than being interpreted as a negative duration.
            let now_inst = Instant::now();
            let now_sys_epoch = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            for entry in attempts {
                let client = entry.get("client").and_then(|v| v.as_str());
                let count = entry.get("count").and_then(|v| v.as_u64());
                let since_epoch = entry.get("since_epoch_secs").and_then(|v| v.as_u64());
                if let (Some(client), Some(count), Some(since_epoch)) =
                    (client, count, since_epoch)
                {
                    let since_secs_ago = now_sys_epoch.saturating_sub(since_epoch);
                    let since = now_inst
                        .checked_sub(Duration::from_secs(since_secs_ago))
                        .unwrap_or(now_inst);
                    self.failed_attempts
                        .insert(client.to_string(), (count as u32, since));
                }
            }
        }
        Ok(())
    }
}

/// Compute the hex-encoded SHA-256 digest of `input`.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_is_6_digits() {
        let mut guard = PairingGuard::new(None);
        let code = guard.generate_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn verify_without_code_returns_distinct_error() {
        // When no code has been generated, the error should still be the
        // specific "no pairing code" message (preserving the existing
        // contract) — the timing of the work that produced it is now
        // equalized via a dummy compare, but the user-visible behavior
        // is unchanged.
        let mut guard = PairingGuard::new(None);
        let err = guard.verify_code("x", "123456").unwrap_err().to_string();
        assert!(
            err.contains("no pairing code"),
            "expected 'no pairing code' message, got: {err}"
        );
    }

    #[test]
    fn verify_rejects_wrong_code_with_correct_code_set() {
        let mut guard = PairingGuard::new(None);
        let code = guard.generate_code();
        // Pick a deterministically-different wrong code.
        let wrong = if code == "000000" { "000001" } else { "000000" };
        let err = guard
            .verify_code("x", wrong)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("invalid pairing code"),
            "expected 'invalid pairing code' message, got: {err}"
        );
    }

    /// Persistence round-trip: paired token hashes and failed-attempt
    /// counters must survive a restart. Without this, every paired client
    /// has to re-pair on restart and an attacker who can bounce the
    /// process resets the brute-force lockout window.
    #[test]
    fn save_load_round_trips_full_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pairing.json");

        // First instance: pair a client AND rack up some failed attempts
        // for a different client.
        let token = {
            let mut g = PairingGuard::new(Some(path.clone()));
            let code = g.generate_code();
            let success_token = g.verify_code("alice", &code).unwrap();
            // Now generate a new code and fail it once for "bob".
            let new_code = g.generate_code();
            // Pick a string that's deterministically NOT the new code.
            let wrong = if new_code == "000000" { "000001" } else { "000000" };
            let _ = g.verify_code("bob", wrong);
            g.add_allowed_user("alice");
            success_token
        };

        // Second instance: should see alice's token still authorized
        // and bob's failed-attempt counter persisted.
        let g2 = PairingGuard::new(Some(path));
        assert!(g2.is_authorized(&token), "alice's token must survive restart");
        assert!(g2.is_allowed("alice"), "alice must still be in allowed_users");
        assert!(
            g2.failed_attempts.contains_key("bob"),
            "bob's failed-attempt counter must survive restart"
        );
    }
}
