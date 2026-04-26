//! Constant-time comparison helpers for authentication-sensitive checks.
//!
//! Short-circuiting `==` on `String`/`&[u8]` leaks timing that an adversary
//! with many request attempts can use to recover secrets byte-by-byte. These
//! wrappers over `subtle::ConstantTimeEq` give us a fennec-local API that
//! call sites can use without re-importing the crate everywhere.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Constant-time byte-slice equality.
///
/// Returns `true` iff `a` and `b` have equal length and equal contents. On
/// unequal length, returns `false` immediately but runs a dummy compare over
/// the shared prefix so the time spent is still dominated by a constant-time
/// pass rather than the length check.
pub fn ct_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        let shared = a.len().min(b.len());
        let _ = a[..shared].ct_eq(&b[..shared]);
        return false;
    }
    a.ct_eq(b).into()
}

/// Constant-time string equality via SHA-256 of both sides.
///
/// Hashing first means the compare runs over fixed-length 32-byte digests
/// regardless of the plaintext lengths of `a` and `b`, which is the right
/// primitive for comparing bearer tokens / shared secrets that may differ
/// in length.
pub fn ct_eq_hashed(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    ha.as_slice().ct_eq(hb.as_slice()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_bytes_equal() {
        assert!(ct_eq_bytes(b"hello", b"hello"));
        assert!(ct_eq_bytes(b"", b""));
        assert!(ct_eq_bytes(&[0u8; 32], &[0u8; 32]));
    }

    #[test]
    fn ct_eq_bytes_unequal() {
        assert!(!ct_eq_bytes(b"hello", b"hellx"));
        assert!(!ct_eq_bytes(b"abc", b"abcd"));
        assert!(!ct_eq_bytes(b"abcd", b"abc"));
        assert!(!ct_eq_bytes(b"", b"x"));
    }

    #[test]
    fn ct_eq_hashed_equal() {
        assert!(ct_eq_hashed("secret-token", "secret-token"));
        assert!(ct_eq_hashed("", ""));
    }

    #[test]
    fn ct_eq_hashed_unequal() {
        assert!(!ct_eq_hashed("a", "b"));
        assert!(!ct_eq_hashed("short", "muchlongerstring"));
        assert!(!ct_eq_hashed("Token-1234", "token-1234"));
    }
}
