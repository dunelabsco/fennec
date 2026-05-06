//! Matrix end-to-end encryption support.
//!
//! Compiled only when the `matrix-e2ee` Cargo feature is on. Wraps
//! `matrix-sdk-crypto`'s `OlmMachine` and pairs it with a
//! `matrix-sdk-sqlite` `SqliteCryptoStore` for persistence. Performs
//! the standard Matrix outgoing-request loop:
//!
//! 1. After each `/sync`, feed the to-device events, device-list
//!    changes, and one-time-key counts into `receive_sync_changes`.
//! 2. Drain `machine.outgoing_requests()` and dispatch each
//!    (`/keys/upload`, `/keys/query`, `/keys/claim`,
//!    `/sendToDevice`, `/keys/signatures/upload`) over reqwest with
//!    the channel's bearer token.
//! 3. Parse each response back into the matching ruma `Response`
//!    type and call `mark_request_as_sent` so the machine clears
//!    its pending state.
//!
//! For encrypted rooms specifically:
//!
//! - On the **send** path: detect that the destination room is
//!   encrypted (an `m.room.encryption` state event was observed at
//!   some point), call `share_room_key` to generate a Megolm
//!   session for any newly-joined recipient devices, dispatch the
//!   resulting to-device requests, then `encrypt_room_event_raw`
//!   to wrap the plaintext content into an `m.room.encrypted`
//!   event before PUTting it.
//! - On the **receive** path: when an `m.room.encrypted` payload
//!   arrives, hand it to `decrypt_room_event` to recover the
//!   plaintext, then re-dispatch the recovered inner event through
//!   the normal handler.
//!
//! The crypto store lives at the directory configured via
//! `MatrixChannelEntry::crypto_store_dir`. The store is single-
//! writer; the `OlmMachine` wraps its own mutexes internally so
//! concurrent callers from the channel are safe.
//!
//! This module is the **foundation** layer: it owns the OlmMachine,
//! the dispatch helpers, and the encrypt/decrypt entry points. The
//! integration into `matrix.rs` (encrypt-on-send branch, decrypt-on-
//! receive branch, background drain task) lands separately so each
//! piece can be reviewed independently.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use reqwest::Client;
use serde_json::Value;

use matrix_sdk_crypto::{
    AttachmentDecryptor, AttachmentEncryptor, DecryptionSettings, EncryptionSettings,
    EncryptionSyncChanges, MediaEncryptionInfo, OlmMachine, TrustRequirement,
};
use matrix_sdk_crypto::types::events::room::encrypted::EncryptedEvent;
use matrix_sdk_crypto::types::requests::{AnyOutgoingRequest, OutgoingRequest};
use matrix_sdk_sqlite::SqliteCryptoStore;
use ruma::api::auth_scheme::SendAccessToken;
use ruma::api::client::sync::sync_events::DeviceLists;
use ruma::api::{IncomingResponse, MatrixVersion, SupportedVersions};
use ruma::events::AnyToDeviceEvent;
use ruma::exports::http;
use ruma::serde::Raw;
use ruma::{DeviceId, OneTimeKeyAlgorithm, OwnedUserId, RoomId, TransactionId};

/// Default per-request timeout for crypto HTTP calls. Crypto
/// requests are small JSON; 30s is generous.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Cheap-to-clone E2EE handle (internal `Arc`).
#[derive(Clone)]
pub struct MatrixCrypto {
    inner: Arc<MatrixCryptoInner>,
}

struct MatrixCryptoInner {
    machine: OlmMachine,
    /// Rooms known to be encrypted (an `m.room.encryption` state
    /// event was observed). Once added, never removed (Matrix has
    /// no "un-encrypt a room" operation).
    encrypted_rooms: RwLock<HashSet<String>>,
    /// HTTP client shared with the parent channel — same connection
    /// pool, same TLS config.
    http: Client,
    /// Homeserver base URL with no trailing slash.
    base_url: String,
    /// Bearer access token. Looked up via the parent channel each
    /// dispatch so token rotation is reflected.
    token_provider: Arc<dyn Fn() -> String + Send + Sync>,
}

/// Settings the parent channel passes when constructing the crypto
/// layer. Plain POD so the parent doesn't need to depend on ruma
/// types.
pub struct MatrixCryptoConfig {
    /// Homeserver base URL, e.g. `https://matrix.org`.
    pub homeserver: String,
    /// Bot user-id, e.g. `@fennec:matrix.org`.
    pub user_id: String,
    /// Stable device id; recommended even outside E2EE so the
    /// homeserver doesn't churn devices on each restart.
    pub device_id: Option<String>,
    /// Directory for the SqliteCryptoStore. Will be created if
    /// missing.
    pub crypto_store_dir: PathBuf,
    /// Optional passphrase for the at-rest sqlite encryption.
    /// `None` keeps the store unencrypted on disk (file-system
    /// permissions become the only sensitive-material guard).
    pub crypto_store_passphrase: Option<String>,
    /// Reused HTTP client.
    pub http: Client,
    /// Closure that returns the current bearer access token. The
    /// crypto layer calls this per-request so token rotation is
    /// observed.
    pub token_provider: Arc<dyn Fn() -> String + Send + Sync>,
}

impl MatrixCrypto {
    /// Open or create the crypto store and load an `OlmMachine`.
    pub async fn new(config: MatrixCryptoConfig) -> Result<Self> {
        let user: OwnedUserId = config
            .user_id
            .parse()
            .with_context(|| format!("invalid matrix user_id: {}", config.user_id))?;
        let device: ruma::OwnedDeviceId = match config.device_id {
            Some(d) if !d.is_empty() => d.into(),
            _ => DeviceId::new(),
        };

        let store = SqliteCryptoStore::open(
            &config.crypto_store_dir,
            config.crypto_store_passphrase.as_deref(),
        )
        .await
        .context("failed to open matrix crypto store")?;

        let machine = OlmMachine::with_store(&user, &device, store, None)
            .await
            .context("failed to construct matrix OlmMachine")?;

        Ok(Self {
            inner: Arc::new(MatrixCryptoInner {
                machine,
                encrypted_rooms: RwLock::new(HashSet::new()),
                http: config.http,
                base_url: config.homeserver.trim_end_matches('/').to_string(),
                token_provider: config.token_provider,
            }),
        })
    }

    /// Bot's authoritative user-id as known to the crypto store.
    pub fn user_id(&self) -> String {
        self.inner.machine.user_id().to_string()
    }

    /// Bot's device-id as known to the crypto store.
    pub fn device_id(&self) -> String {
        self.inner.machine.device_id().to_string()
    }

    /// Mark a room as encrypted. The parent channel calls this when
    /// it sees an `m.room.encryption` state event during sync.
    pub fn mark_encrypted(&self, room_id: &str) {
        self.inner
            .encrypted_rooms
            .write()
            .insert(room_id.to_string());
    }

    pub fn is_encrypted(&self, room_id: &str) -> bool {
        self.inner.encrypted_rooms.read().contains(room_id)
    }

    /// Feed a sync response's encryption-relevant fields into the
    /// machine. Call this **before** persisting the next-batch token
    /// since to-device events are ephemeral and re-running the same
    /// sync from a stored token won't re-deliver them.
    pub async fn process_sync_changes(&self, sync_response: &Value) -> Result<()> {
        let to_device_events = parse_to_device_events(sync_response);
        let device_lists = parse_device_lists(sync_response);
        let otk_counts = parse_otk_counts(sync_response);
        let unused_fallback = parse_unused_fallback(sync_response);
        let next_batch_token = sync_response
            .get("next_batch")
            .and_then(|v| v.as_str())
            .map(String::from);

        let changes = EncryptionSyncChanges {
            to_device_events,
            changed_devices: &device_lists,
            one_time_keys_counts: &otk_counts,
            unused_fallback_keys: unused_fallback.as_deref(),
            next_batch_token,
        };

        let settings = DecryptionSettings {
            sender_device_trust_requirement: TrustRequirement::Untrusted,
        };

        self.inner
            .machine
            .receive_sync_changes(changes, &settings)
            .await
            .context("matrix OlmMachine receive_sync_changes failed")?;
        Ok(())
    }

    /// Decrypt an `m.room.encrypted` event JSON value. Returns the
    /// recovered plaintext event JSON (suitable for re-dispatching
    /// through the regular `m.room.message` path) on success.
    pub async fn decrypt(&self, room_id: &str, raw_event: &Value) -> Result<Value> {
        let room: &RoomId = <&RoomId>::try_from(room_id)
            .with_context(|| format!("invalid matrix room_id: {}", room_id))?;
        let raw_string = serde_json::to_string(raw_event)
            .context("failed to re-serialize encrypted event")?;
        let raw: Raw<EncryptedEvent> =
            Raw::from_json(serde_json::value::RawValue::from_string(raw_string)?);
        let settings = DecryptionSettings {
            sender_device_trust_requirement: TrustRequirement::Untrusted,
        };
        let decrypted = self
            .inner
            .machine
            .decrypt_room_event(&raw, room, &settings)
            .await
            .context("matrix decrypt_room_event failed")?;
        let plain: Value = serde_json::from_str(decrypted.event.json().get())
            .context("decrypted event payload not valid JSON")?;
        Ok(plain)
    }

    /// Encrypt an outbound `m.room.message` content for an
    /// encrypted room. Before encrypting, shares a Megolm session
    /// with `recipient_user_ids` and dispatches the resulting
    /// to-device requests so recipient devices get the key.
    /// Returns the `m.room.encrypted` content suitable for
    /// `PUT /rooms/{roomId}/send/m.room.encrypted/{txnId}`.
    pub async fn encrypt_room_message(
        &self,
        room_id: &str,
        content: &Value,
        recipient_user_ids: &[String],
    ) -> Result<Value> {
        let room: &RoomId = <&RoomId>::try_from(room_id)
            .with_context(|| format!("invalid matrix room_id: {}", room_id))?;

        let users: Vec<OwnedUserId> = recipient_user_ids
            .iter()
            .filter_map(|s| OwnedUserId::try_from(s.as_str()).ok())
            .collect();

        let settings = EncryptionSettings::default();
        let to_device_reqs = self
            .inner
            .machine
            .share_room_key(room, users.iter().map(|u| u.as_ref()), settings)
            .await
            .context("matrix share_room_key failed")?;

        for req in to_device_reqs {
            // Share-key requests come back outside the normal
            // outgoing_requests() queue and must be dispatched
            // directly so recipient devices have the key before
            // the encrypted event arrives. We assign a fresh txn
            // id rather than the one inside the request because
            // share_room_key's requests aren't tracked by the
            // mark_request_as_sent flow.
            let txn = TransactionId::new();
            self.send_to_device_inner(&txn, &req).await?;
        }

        let raw_content_string = serde_json::to_string(content)
            .context("failed to serialize matrix content for encryption")?;
        let raw_content: Raw<ruma::events::AnyMessageLikeEventContent> =
            Raw::from_json(serde_json::value::RawValue::from_string(raw_content_string)?);

        let encrypted = self
            .inner
            .machine
            .encrypt_room_event_raw(room, "m.room.message", &raw_content)
            .await
            .context("matrix encrypt_room_event_raw failed")?;
        let json: Value = serde_json::from_str(encrypted.json().get())
            .context("encrypted output not valid JSON")?;
        Ok(json)
    }

    /// Decrypt an encrypted attachment. `file_json` is the
    /// `content.file` object from an inbound `m.image` / `m.file` /
    /// `m.audio` / `m.video` event in an encrypted room — it
    /// carries the AES key, IV, and SHA-256 hash that
    /// `AttachmentDecryptor` needs. `encrypted` is the raw
    /// ciphertext as fetched from the homeserver media endpoint.
    /// Returns the plaintext.
    pub fn decrypt_attachment(
        &self,
        file_json: &Value,
        encrypted: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let info: MediaEncryptionInfo = serde_json::from_value(file_json.clone())
            .context("matrix: encrypted file metadata not parseable")?;
        let mut input = std::io::Cursor::new(encrypted);
        let mut decryptor = AttachmentDecryptor::new(&mut input, info)
            .map_err(|e| anyhow::anyhow!("matrix attachment decrypt init failed: {e}"))?;
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decryptor, &mut out)
            .context("matrix attachment decrypt read failed")?;
        Ok(out)
    }

    /// Encrypt a plaintext attachment for an encrypted room.
    /// Returns the ciphertext (which gets uploaded to
    /// `/_matrix/media/v3/upload`) and the `MediaEncryptionInfo`
    /// the caller should serialize into the outbound event's
    /// `file` field alongside the resulting `mxc://` URL.
    pub fn encrypt_attachment(&self, plain: &[u8]) -> Result<(Vec<u8>, Value)> {
        let mut cursor = std::io::Cursor::new(plain);
        let mut encryptor = AttachmentEncryptor::new(&mut cursor);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut encryptor, &mut out)
            .context("matrix attachment encrypt read failed")?;
        let info = encryptor.finish();
        let info_json =
            serde_json::to_value(&info).context("matrix encryption info serialize failed")?;
        Ok((out, info_json))
    }

    /// Drain the OlmMachine's pending outgoing requests once. The
    /// parent channel calls this after each sync response and
    /// occasionally on a timer so requests don't pile up.
    pub async fn drain_outgoing_requests(&self) -> Result<usize> {
        let pending = self
            .inner
            .machine
            .outgoing_requests()
            .await
            .context("matrix OlmMachine outgoing_requests failed")?;
        let n = pending.len();
        for req in pending {
            if let Err(e) = self.dispatch_one(&req).await {
                tracing::warn!(
                    error = %e,
                    request_id = %req.request_id(),
                    "matrix crypto outgoing request failed"
                );
            }
        }
        Ok(n)
    }

    async fn dispatch_one(&self, req: &OutgoingRequest) -> Result<()> {
        match req.request() {
            AnyOutgoingRequest::KeysUpload(r) => self
                .dispatch_typed(req.request_id(), r.clone(), |resp| {
                    matrix_sdk_crypto::types::requests::AnyIncomingResponse::KeysUpload(resp)
                })
                .await
                .map(|_| ()),
            AnyOutgoingRequest::KeysQuery(r) => {
                // matrix-sdk-crypto's KeysQueryRequest is a flat
                // POD wrapper with the same fields as ruma's; map
                // them across by hand.
                let mut ruma_req = ruma::api::client::keys::get_keys::v3::Request::new();
                ruma_req.timeout = r.timeout;
                ruma_req.device_keys = r.device_keys.clone();
                self.dispatch_typed(req.request_id(), ruma_req, |resp| {
                    matrix_sdk_crypto::types::requests::AnyIncomingResponse::KeysQuery(resp)
                })
                .await
                .map(|_| ())
            }
            AnyOutgoingRequest::KeysClaim(r) => self
                .dispatch_typed(req.request_id(), r.clone(), |resp| {
                    matrix_sdk_crypto::types::requests::AnyIncomingResponse::KeysClaim(resp)
                })
                .await
                .map(|_| ()),
            AnyOutgoingRequest::ToDeviceRequest(r) => {
                self.send_to_device_inner(req.request_id(), r).await?;
                let resp = ruma::api::client::to_device::send_event_to_device::v3::Response::new();
                self.inner
                    .machine
                    .mark_request_as_sent(
                        req.request_id(),
                        matrix_sdk_crypto::types::requests::AnyIncomingResponse::ToDevice(&resp),
                    )
                    .await
                    .context("matrix mark_request_as_sent (to_device) failed")?;
                Ok(())
            }
            AnyOutgoingRequest::SignatureUpload(r) => self
                .dispatch_typed(req.request_id(), r.clone(), |resp| {
                    matrix_sdk_crypto::types::requests::AnyIncomingResponse::SignatureUpload(resp)
                })
                .await
                .map(|_| ()),
            AnyOutgoingRequest::RoomMessage(_) => {
                // Verification-via-room-message: the regular send
                // pathway already covers `m.room.message` events
                // (encrypted when needed). Skip — leaving these in
                // the queue is a no-op since the OlmMachine just
                // re-emits them on the next drain.
                Ok(())
            }
        }
    }

    /// Generic helper: serialize a ruma `OutgoingRequest`, send via
    /// reqwest, parse the response, and forward it to the
    /// OlmMachine via `mark_request_as_sent`.
    async fn dispatch_typed<R, F>(
        &self,
        request_id: &TransactionId,
        request: R,
        wrap_response: F,
    ) -> Result<R::IncomingResponse>
    where
        R: ruma::api::OutgoingRequest + Clone,
        for<'a> R::Authentication: ruma::api::auth_scheme::AuthScheme<
                Input<'a> = SendAccessToken<'a>,
            >,
        for<'a> R::PathBuilder: ruma::api::path_builder::PathBuilder<
                Input<'a> = std::borrow::Cow<'a, SupportedVersions>,
            >,
        for<'a> F: FnOnce(&'a R::IncomingResponse) -> matrix_sdk_crypto::types::requests::AnyIncomingResponse<'a>,
    {
        let token = (self.inner.token_provider)();
        let supported = supported_versions();
        let http_req: http::Request<Vec<u8>> = request.clone().try_into_http_request(
            &self.inner.base_url,
            SendAccessToken::Always(&token),
            std::borrow::Cow::Owned(supported),
        )?;
        let bytes = self.send_http(http_req).await?;
        let http_resp = http::Response::builder()
            .status(http::StatusCode::OK)
            .body(bytes)?;
        let response: R::IncomingResponse =
            <R::IncomingResponse as IncomingResponse>::try_from_http_response(http_resp)
                .map_err(|e| anyhow::anyhow!("matrix response parse failed: {e}"))?;
        self.inner
            .machine
            .mark_request_as_sent(request_id, wrap_response(&response))
            .await
            .context("matrix mark_request_as_sent failed")?;
        Ok(response)
    }

    /// Dispatch an `http::Request` via reqwest and return the
    /// response body as a Vec<u8>. Errors on non-2xx status.
    async fn send_http(&self, req: http::Request<Vec<u8>>) -> Result<Vec<u8>> {
        let (parts, body) = req.into_parts();
        let url = parts.uri.to_string();
        let mut builder = self.inner.http.request(parts.method, &url).body(body);
        for (name, value) in parts.headers.iter() {
            builder = builder.header(name, value);
        }
        let resp = builder
            .timeout(RPC_TIMEOUT)
            .send()
            .await
            .context("matrix crypto HTTP send failed")?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .context("matrix crypto HTTP body read failed")?;
        if !status.is_success() {
            anyhow::bail!(
                "matrix crypto HTTP returned {} ({} bytes body)",
                status,
                bytes.len()
            );
        }
        Ok(bytes.to_vec())
    }

    /// Inner sendToDevice dispatch (no `mark_request_as_sent` —
    /// caller handles that, since `share_room_key`'s requests are
    /// dispatched outside the normal queue).
    async fn send_to_device_inner(
        &self,
        txn_id: &TransactionId,
        req: &matrix_sdk_crypto::types::requests::ToDeviceRequest,
    ) -> Result<()> {
        let event_type = req.event_type.to_string();
        let path = format!(
            "{}/_matrix/client/v3/sendToDevice/{}/{}",
            self.inner.base_url,
            urlencoding::encode(&event_type),
            urlencoding::encode(txn_id.as_str()),
        );
        let body = serde_json::json!({ "messages": req.messages });
        let resp = self
            .inner
            .http
            .put(&path)
            .bearer_auth((self.inner.token_provider)())
            .json(&body)
            .timeout(RPC_TIMEOUT)
            .send()
            .await
            .context("matrix sendToDevice request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("matrix sendToDevice returned {}", resp.status());
        }
        Ok(())
    }
}

// -- Sync-response parsers --------------------------------------

fn parse_to_device_events(sync: &Value) -> Vec<Raw<AnyToDeviceEvent>> {
    let arr = sync
        .pointer("/to_device/events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    arr.into_iter()
        .filter_map(|v| {
            let s = serde_json::to_string(&v).ok()?;
            let raw_value = serde_json::value::RawValue::from_string(s).ok()?;
            Some(Raw::from_json(raw_value))
        })
        .collect()
}

fn parse_device_lists(sync: &Value) -> DeviceLists {
    let mut lists = DeviceLists::default();
    if let Some(arr) = sync
        .pointer("/device_lists/changed")
        .and_then(|v| v.as_array())
    {
        lists.changed = arr
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|s| OwnedUserId::try_from(s).ok())
            .collect();
    }
    if let Some(arr) = sync
        .pointer("/device_lists/left")
        .and_then(|v| v.as_array())
    {
        lists.left = arr
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|s| OwnedUserId::try_from(s).ok())
            .collect();
    }
    lists
}

fn parse_otk_counts(
    sync: &Value,
) -> std::collections::BTreeMap<OneTimeKeyAlgorithm, ruma::UInt> {
    let mut out = std::collections::BTreeMap::new();
    if let Some(obj) = sync
        .pointer("/device_one_time_keys_count")
        .and_then(|v| v.as_object())
    {
        for (k, v) in obj {
            let algo = OneTimeKeyAlgorithm::from(k.as_str());
            if let Some(n) = v.as_u64() {
                if let Ok(u) = ruma::UInt::try_from(n) {
                    out.insert(algo, u);
                }
            }
        }
    }
    out
}

fn parse_unused_fallback(sync: &Value) -> Option<Vec<OneTimeKeyAlgorithm>> {
    sync.pointer("/device_unused_fallback_key_types")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(OneTimeKeyAlgorithm::from)
                .collect()
        })
}

/// Construct a SupportedVersions advertising the spec versions
/// fennec targets. Used as the path-builder input for ruma
/// OutgoingRequests so they pick the v3 paths.
fn supported_versions() -> SupportedVersions {
    let mut versions = BTreeSet::new();
    versions.insert(MatrixVersion::V1_13);
    SupportedVersions {
        versions,
        features: BTreeSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn dummy_token() -> Arc<dyn Fn() -> String + Send + Sync> {
        Arc::new(|| "test-token".to_string())
    }

    #[tokio::test]
    async fn opens_store_and_loads_machine() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");
        assert_eq!(crypto.user_id(), "@bot:example.org");
        assert_eq!(crypto.device_id(), "DEVICE0");
    }

    #[tokio::test]
    async fn encrypted_room_tracking_round_trip() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");
        assert!(!crypto.is_encrypted("!a:example.org"));
        crypto.mark_encrypted("!a:example.org");
        assert!(crypto.is_encrypted("!a:example.org"));
        assert!(!crypto.is_encrypted("!b:example.org"));
    }

    #[tokio::test]
    async fn store_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let make_cfg = || MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: path.clone(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let c1 = MatrixCrypto::new(make_cfg()).await.expect("init");
        let device1 = c1.device_id();
        drop(c1);
        let c2 = MatrixCrypto::new(make_cfg()).await.expect("reopen");
        assert_eq!(c2.device_id(), device1);
    }

    #[test]
    fn parse_to_device_events_handles_missing() {
        let sync = serde_json::json!({});
        assert!(parse_to_device_events(&sync).is_empty());
    }

    #[test]
    fn parse_to_device_events_extracts_array() {
        let sync = serde_json::json!({
            "to_device": {
                "events": [
                    {"type": "m.dummy", "content": {}}
                ]
            }
        });
        assert_eq!(parse_to_device_events(&sync).len(), 1);
    }

    #[test]
    fn parse_otk_counts_extracts_known_algos() {
        let sync = serde_json::json!({
            "device_one_time_keys_count": {
                "signed_curve25519": 50
            }
        });
        let counts = parse_otk_counts(&sync);
        let algo = OneTimeKeyAlgorithm::from("signed_curve25519");
        assert_eq!(counts.get(&algo).copied(), Some(ruma::UInt::from(50u32)));
    }

    #[test]
    fn parse_device_lists_extracts_changed_and_left() {
        let sync = serde_json::json!({
            "device_lists": {
                "changed": ["@a:example.org"],
                "left": ["@b:example.org"]
            }
        });
        let lists = parse_device_lists(&sync);
        assert_eq!(lists.changed.len(), 1);
        assert_eq!(lists.left.len(), 1);
    }

    #[test]
    fn supported_versions_includes_v1_13() {
        let sv = supported_versions();
        assert!(sv.versions.contains(&MatrixVersion::V1_13));
    }

    /// Attachment encrypt → decrypt round-trip against the same
    /// MatrixCrypto handle. Doesn't need paired machines because
    /// attachment crypto is symmetric / self-contained — the key
    /// and IV travel inline with the ciphertext.
    #[tokio::test]
    async fn attachment_encrypt_decrypt_round_trip() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");

        let plaintext = b"the quick brown fennec jumps over the lazy dog \
                          (long enough to span the AES-CTR block boundary)"
            .to_vec();
        let (ciphertext, info_json) = crypto
            .encrypt_attachment(&plaintext)
            .expect("encrypt");
        // Sanity: encrypted bytes shouldn't equal plaintext, and
        // info JSON should carry the v / key / iv / hashes shape.
        assert_ne!(ciphertext, plaintext);
        assert_eq!(info_json["v"], "v2");
        assert!(info_json.get("key").is_some());
        assert!(info_json.get("iv").is_some());
        assert!(info_json.get("hashes").is_some());

        let recovered = crypto
            .decrypt_attachment(&info_json, ciphertext)
            .expect("decrypt");
        assert_eq!(recovered, plaintext);
    }

    #[tokio::test]
    async fn attachment_decrypt_rejects_tampered_ciphertext() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");
        let plaintext = b"sensitive payload".to_vec();
        let (mut ciphertext, info_json) = crypto
            .encrypt_attachment(&plaintext)
            .expect("encrypt");
        // Flip a bit in the middle of the ciphertext — sha256
        // mismatch should make decrypt error rather than silently
        // returning garbage.
        let idx = ciphertext.len() / 2;
        ciphertext[idx] ^= 0x01;
        let result = crypto.decrypt_attachment(&info_json, ciphertext);
        assert!(
            result.is_err(),
            "expected decrypt to reject tampered ciphertext, got {:?}",
            result.map(|v| v.len())
        );
    }

    #[tokio::test]
    async fn attachment_decrypt_rejects_malformed_info() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");
        // Bogus info — missing the version, key, iv, hashes
        // entirely. Must error rather than panic.
        let bad = serde_json::json!({"not": "valid"});
        let result = crypto.decrypt_attachment(&bad, vec![1, 2, 3]);
        assert!(result.is_err());
    }

    /// Verify that process_sync_changes accepts a realistic-shape
    /// sync response without panicking. This isn't a round-trip
    /// test (we'd need paired machines for that), but it locks in
    /// the wire-format → API mapping our parsers do.
    #[tokio::test]
    async fn process_sync_changes_accepts_realistic_response() {
        let dir = tempdir().unwrap();
        let cfg = MatrixCryptoConfig {
            homeserver: "https://matrix.example.org".into(),
            user_id: "@bot:example.org".into(),
            device_id: Some("DEVICE0".into()),
            crypto_store_dir: dir.path().to_path_buf(),
            crypto_store_passphrase: None,
            http: Client::new(),
            token_provider: dummy_token(),
        };
        let crypto = MatrixCrypto::new(cfg).await.expect("init");
        let sync = serde_json::json!({
            "next_batch": "s12_0",
            "to_device": { "events": [] },
            "device_lists": { "changed": [], "left": [] },
            "device_one_time_keys_count": { "signed_curve25519": 50 },
            "device_unused_fallback_key_types": ["signed_curve25519"]
        });
        crypto.process_sync_changes(&sync).await.expect("ok");
    }
}
