use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, Key, Nonce};

use super::fs::write_secure;

const NONCE_LEN: usize = 12;
const PREFIX: &str = "enc2:";

/// Encrypts and decrypts secrets using ChaCha20-Poly1305 (AEAD).
///
/// The encryption key is stored as hex in `<config_dir>/.secret_key` and is
/// generated on first use.
pub struct SecretStore {
    key: Key,
}

impl SecretStore {
    /// Create or load a `SecretStore` rooted at `config_dir`.
    ///
    /// If the key file does not exist, a new random key is generated and
    /// persisted with restrictive permissions (0600 on Unix).
    pub fn new(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("creating config dir {}", config_dir.display()))?;

        let key_path = config_dir.join(".secret_key");

        let key = if key_path.exists() {
            // Load existing key.
            let hex_str = std::fs::read_to_string(&key_path)
                .with_context(|| format!("reading key file {}", key_path.display()))?;
            let bytes = hex::decode(hex_str.trim())
                .context("decoding hex key")?;
            if bytes.len() != 32 {
                bail!("invalid key length: expected 32 bytes, got {}", bytes.len());
            }
            *Key::from_slice(&bytes)
        } else {
            // Generate a new key and write it via write_secure so the file
            // never exists on disk with umask-default (0644) permissions,
            // even for the brief window between create-and-chmod.
            let key = ChaCha20Poly1305::generate_key(&mut OsRng);

            let hex_str = hex::encode(key.as_slice());
            write_secure(&key_path, hex_str.as_bytes())
                .with_context(|| format!("writing key file {}", key_path.display()))?;

            key
        };

        Ok(Self { key })
    }

    /// Encrypt `plaintext` and return a prefixed hex string.
    ///
    /// Format: `enc2:<hex(nonce || ciphertext)>`
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let cipher = ChaCha20Poly1305::new(&self.key);
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);

        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("encryption failed: {}", e))?;

        // nonce || ciphertext
        let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        combined.extend_from_slice(nonce.as_slice());
        combined.extend_from_slice(&ciphertext);

        Ok(format!("{}{}", PREFIX, hex::encode(&combined)))
    }

    /// Decrypt a value previously encrypted with [`encrypt`](Self::encrypt).
    ///
    /// If the value does not start with the `enc2:` prefix it is returned
    /// as-is (plaintext passthrough). This is a backwards-compat convenience
    /// for configs where the user pasted a raw secret without running it
    /// through `encrypt` first — but it also means an attacker able to
    /// overwrite stored ciphertext with arbitrary plaintext makes
    /// decryption "succeed" with their payload. A warning is logged on the
    /// passthrough path so operators can notice and migrate the config.
    pub fn decrypt(&self, value: &str) -> Result<String> {
        if !value.starts_with(PREFIX) {
            tracing::warn!(
                "SecretStore::decrypt: value lacks '{}' prefix — returning as \
                 plaintext. This is fine for legacy configs where the secret \
                 was pasted directly, but a stored value that SHOULD be \
                 ciphertext is now unauthenticated. Run encrypt() over these \
                 values.",
                PREFIX
            );
            return Ok(value.to_string());
        }

        let hex_part = &value[PREFIX.len()..];
        let combined = hex::decode(hex_part).context("decoding hex ciphertext")?;

        if combined.len() < NONCE_LEN {
            bail!("ciphertext too short");
        }

        let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = ChaCha20Poly1305::new(&self.key);
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("decryption failed: {}", e))?;

        String::from_utf8(plaintext).context("decrypted bytes are not valid UTF-8")
    }
}
