use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Guards DM access via a pairing code flow with brute-force lockout.
///
/// A 6-digit numeric code is generated server-side. Clients present the code
/// to receive a token (`fc_<64-hex>`). The token hash is stored; raw tokens
/// are never persisted.
pub struct PairingGuard {
    code: Option<String>,
    allowed_users: HashSet<String>,
    paired_token_hashes: HashSet<String>,
    failed_attempts: HashMap<String, (u32, std::time::Instant)>,
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
                .or_insert((0, std::time::Instant::now()));
            entry.0 += 1;
            // Reset the clock on each failure.
            entry.1 = std::time::Instant::now();
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

    /// Persist the allowed-users set to a JSON file.
    pub fn save(&self) -> Result<()> {
        let path = self
            .persist_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no persist path configured"))?;
        let data = serde_json::json!({
            "allowed_users": self.allowed_users.iter().collect::<Vec<_>>(),
        });
        let json = serde_json::to_string_pretty(&data).context("serializing allowed users")?;
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Load the allowed-users set from the JSON file.
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
            serde_json::from_str(&json).context("parsing allowed users JSON")?;
        if let Some(users) = data.get("allowed_users").and_then(|v| v.as_array()) {
            self.allowed_users = users
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
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
}
