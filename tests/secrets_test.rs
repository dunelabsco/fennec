use fennec::security::SecretStore;
use tempfile::TempDir;

#[test]
fn encrypt_decrypt_roundtrip() {
    let dir = TempDir::new().expect("tempdir");
    let store = SecretStore::new(dir.path().to_path_buf()).expect("new store");

    let original = "super-secret-api-key-12345";
    let encrypted = store.encrypt(original).expect("encrypt");
    assert!(encrypted.starts_with("enc2:"));
    assert_ne!(encrypted, original);

    let decrypted = store.decrypt(&encrypted).expect("decrypt");
    assert_eq!(decrypted, original);
}

#[test]
fn plaintext_passthrough() {
    let dir = TempDir::new().expect("tempdir");
    let store = SecretStore::new(dir.path().to_path_buf()).expect("new store");

    let plain = "not-encrypted-at-all";
    let result = store.decrypt(plain).expect("decrypt plain");
    assert_eq!(result, plain);
}

#[test]
fn different_encryptions_differ() {
    let dir = TempDir::new().expect("tempdir");
    let store = SecretStore::new(dir.path().to_path_buf()).expect("new store");

    let text = "same plaintext";
    let enc1 = store.encrypt(text).expect("encrypt 1");
    let enc2 = store.encrypt(text).expect("encrypt 2");

    // Different nonces should produce different ciphertexts.
    assert_ne!(enc1, enc2);

    // Both should still decrypt to the original.
    assert_eq!(store.decrypt(&enc1).expect("dec1"), text);
    assert_eq!(store.decrypt(&enc2).expect("dec2"), text);
}

#[test]
fn key_persistence_across_instances() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();

    let store1 = SecretStore::new(path.clone()).expect("store 1");
    let encrypted = store1.encrypt("persistent secret").expect("encrypt");

    // Drop and recreate — key should be loaded from disk.
    drop(store1);
    let store2 = SecretStore::new(path).expect("store 2");
    let decrypted = store2.decrypt(&encrypted).expect("decrypt");
    assert_eq!(decrypted, "persistent secret");
}

#[test]
fn wrong_key_fails() {
    let dir1 = TempDir::new().expect("tempdir1");
    let dir2 = TempDir::new().expect("tempdir2");

    let store1 = SecretStore::new(dir1.path().to_path_buf()).expect("store 1");
    let store2 = SecretStore::new(dir2.path().to_path_buf()).expect("store 2");

    let encrypted = store1.encrypt("secret data").expect("encrypt");

    // Decrypting with a different key should fail.
    let result = store2.decrypt(&encrypted);
    assert!(result.is_err());
}
