use fennec::security::PairingGuard;

#[test]
fn test_code_is_6_digits() {
    let mut guard = PairingGuard::new(None);
    for _ in 0..20 {
        let code = guard.generate_code();
        assert_eq!(code.len(), 6, "code should be 6 chars: {code}");
        assert!(
            code.chars().all(|c| c.is_ascii_digit()),
            "code should be all digits: {code}"
        );
    }
}

#[test]
fn test_verification_succeeds_with_correct_code() {
    let mut guard = PairingGuard::new(None);
    let code = guard.generate_code();

    let token = guard
        .verify_code("user_1", &code)
        .expect("correct code should succeed");

    assert!(token.starts_with("fc_"), "token should start with fc_");
    assert_eq!(
        token.len(),
        3 + 64,
        "token should be fc_ + 64 hex chars"
    );
    assert!(
        guard.is_authorized(&token),
        "token should be authorized after pairing"
    );
}

#[test]
fn test_verification_fails_with_wrong_code() {
    let mut guard = PairingGuard::new(None);
    let _code = guard.generate_code();

    let result = guard.verify_code("user_2", "000000");
    // It could accidentally match if the generated code is "000000", but that's 1 in 10^6.
    // If it fails (expected), check the error.
    if result.is_err() {
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid pairing code"),
            "error should mention invalid code, got: {err}"
        );
    }
}

#[test]
fn test_lockout_after_max_failures() {
    let mut guard = PairingGuard::new(None);
    let _code = guard.generate_code();

    // Fail 5 times.
    for i in 0..5 {
        let result = guard.verify_code("bad_user", "999999");
        if result.is_err() {
            // Expected failure.
        } else {
            // Extremely unlikely random match; skip.
            return;
        }
        if i < 4 {
            // Not yet locked out — error should say "invalid", not "locked out".
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("invalid"),
                "attempt {} should be 'invalid', got: {}",
                i + 1,
                err
            );
        }
    }

    // 6th attempt should be locked out.
    let result = guard.verify_code("bad_user", "999999");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("locked out"),
        "should be locked out after 5 failures, got: {err}"
    );
}

#[test]
fn test_allowed_user_basic() {
    let mut guard = PairingGuard::new(None);

    assert!(!guard.is_allowed("alice"));

    guard.add_allowed_user("alice");
    assert!(guard.is_allowed("alice"));
    assert!(!guard.is_allowed("bob"));
}

#[test]
fn test_allowed_user_wildcard() {
    let mut guard = PairingGuard::new(None);

    guard.add_allowed_user("*");
    assert!(guard.is_allowed("alice"));
    assert!(guard.is_allowed("bob"));
    assert!(guard.is_allowed("anyone_at_all"));
}

#[test]
fn test_unauthorized_token() {
    let guard = PairingGuard::new(None);
    assert!(!guard.is_authorized("fc_bogus"));
    assert!(!guard.is_authorized("random_string"));
}

#[test]
fn test_persistence_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pairing.json");

    // Create guard, add users, save.
    {
        let mut guard = PairingGuard::new(Some(path.clone()));
        guard.add_allowed_user("alice");
        guard.add_allowed_user("bob");
        guard.save().unwrap();
    }

    // Create new guard from same path — should load users.
    {
        let guard = PairingGuard::new(Some(path.clone()));
        assert!(guard.is_allowed("alice"));
        assert!(guard.is_allowed("bob"));
        assert!(!guard.is_allowed("charlie"));
    }
}

#[test]
fn test_code_not_reusable_after_success() {
    let mut guard = PairingGuard::new(None);
    let code = guard.generate_code();

    // First verification succeeds.
    let token = guard.verify_code("user_a", &code).unwrap();
    assert!(guard.is_authorized(&token));

    // Second attempt with the same code should fail (code cleared).
    let result = guard.verify_code("user_b", &code);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("no pairing code"),
        "should say no code generated, got: {err}"
    );
}

#[test]
fn test_persist_path_none_save_returns_error() {
    let guard = PairingGuard::new(None);
    let result = guard.save();
    assert!(result.is_err());
}
