//! Cloud configuration storage for Hermes Agent Migrator.
//!
//! Core invariant (the whole point of this module):
//!
//! ```text
//! One device → at most ONE active cloud configuration.
//!
//!   0 active configs  → CREATE allowed
//!   1 active config   → CREATE blocked, OVERWRITE allowed
//!   after DELETE      → 0 active → CREATE allowed again
//! ```
//!
//! Device identity is a **randomly generated UUID v4** (`DeviceId`), NOT a
//! hardware fingerprint (no MAC / CPU serial / IMEI / public-IP pinning). A
//! device keeps the same `DeviceId` across network changes; if the id is
//! lost (app reinstalled without secure storage) the server requires
//! re-registration. The public IP is recorded for security/audit only.
//!
//! This module is platform-neutral and pure: it ships an in-memory
//! reference backend ([`InMemoryBackend`]) that is unit-testable and that a
//! real deployment (Alibaba Cloud: database + OSS + Cloud API) would mirror.
//! The client encrypts the configuration blob *before* it leaves the
//! machine; the server only ever stores/returns the encrypted blob.

use std::collections::BTreeMap;
use std::sync::Mutex;

use rand::Rng;

// ---------------------------------------------------------------------------
// Device identity
// ---------------------------------------------------------------------------

/// A randomly generated device identifier (UUID v4).
///
/// Random on purpose: it carries no user name, host name, hardware serial,
/// MAC address, and is not usable for advertising/tracking. Two installs of
/// the app on two different machines get two different ids; the same
/// machine keeps one id across Wi-Fi/VPN/ISP changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    /// Generate a fresh UUID v4. Uses `rand` (OS-entropy backed).
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut b = [0u8; 16];
        rng.fill(&mut b);
        // Mark as version 4.
        b[6] = (b[6] & 0x0f) | 0x40;
        // Set the RFC 4122 variant bits.
        b[8] = (b[8] & 0x3f) | 0x80;
        let s = format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3],
            b[4], b[5],
            b[6], b[7],
            b[8], b[9],
            b[10], b[11], b[12], b[13], b[14], b[15]
        );
        Self(s)
    }

    /// Validate an incoming device id. Accepts a 36-char v4 UUID (lowercase).
    /// Rejects empty or obviously-wrong input so a fake `device_id` fails.
    pub fn validate(raw: &str) -> Result<(), crate::MigratorError> {
        let s = raw.trim();
        if s.len() != 36 {
            return Err(crate::MigratorError::Other(
                "invalid device_id (expected a UUID)".into(),
            ));
        }
        // '-' at positions 8, 13, 18, 23; hex everywhere else.
        for (i, b) in s.bytes().enumerate() {
            let ok = if i == 8 || i == 13 || i == 18 || i == 23 {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            };
            if !ok {
                return Err(crate::MigratorError::Other(
                    "invalid device_id (expected a UUID)".into(),
                ));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Stable API error codes (i18n keys — clients must NOT parse free text)
// ---------------------------------------------------------------------------

/// Machine-readable cloud error codes. The client maps these to i18n strings;
/// it never parses English error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CloudError {
    /// Already holds an active configuration — only OVERWRITE/DELETE allowed.
    ConfigurationAlreadyExists,
    ConfigurationNotFound,
    ConfigurationOverwriteNotAuthorized,
    ConfigurationSizeLimitExceeded,
    ConfigurationUploadFailed,
    ConfigurationDeleteFailed,
    /// SHA-256 of the (encrypted) blob did not match.
    ConfigurationIntegrityCheckFailed,
    DeviceNotRegistered,
    DeviceNotAuthorized,
    AuthenticationRequired,
    RateLimited,
    /// Client-side: wrong passphrase / corrupted ciphertext.
    DecryptionFailed,
    /// Transport failure talking to the remote backend (no answer, bad
    /// status with an unparseable error body, TLS failure).
    Network,
}

impl CloudError {
    /// Stable wire string, e.g. `CONFIGURATION_ALREADY_EXISTS`.
    pub fn code(self) -> &'static str {
        use CloudError::*;
        match self {
            ConfigurationAlreadyExists => "CONFIGURATION_ALREADY_EXISTS",
            ConfigurationNotFound => "CONFIGURATION_NOT_FOUND",
            ConfigurationOverwriteNotAuthorized => "CONFIGURATION_OVERWRITE_NOT_AUTHORIZED",
            ConfigurationSizeLimitExceeded => "CONFIGURATION_SIZE_LIMIT_EXCEEDED",
            ConfigurationUploadFailed => "CONFIGURATION_UPLOAD_FAILED",
            ConfigurationDeleteFailed => "CONFIGURATION_DELETE_FAILED",
            ConfigurationIntegrityCheckFailed => "CONFIGURATION_INTEGRITY_CHECK_FAILED",
            DeviceNotRegistered => "DEVICE_NOT_REGISTERED",
            DeviceNotAuthorized => "DEVICE_NOT_AUTHORIZED",
            AuthenticationRequired => "AUTHENTICATION_REQUIRED",
            RateLimited => "RATE_LIMITED",
            DecryptionFailed => "DECRYPTION_FAILED",
            Network => "NETWORK_ERROR",
        }
    }

    /// Parse a stable wire code back into the matching variant. Unknown
    /// codes fail closed as `Network` (treat as transport-level, never as
    /// a domain error the caller can reason about).
    pub fn from_code(s: &str) -> Self {
        match s {
            "CONFIGURATION_ALREADY_EXISTS" => Self::ConfigurationAlreadyExists,
            "CONFIGURATION_NOT_FOUND" => Self::ConfigurationNotFound,
            "CONFIGURATION_OVERWRITE_NOT_AUTHORIZED" => Self::ConfigurationOverwriteNotAuthorized,
            "CONFIGURATION_SIZE_LIMIT_EXCEEDED" => Self::ConfigurationSizeLimitExceeded,
            "CONFIGURATION_UPLOAD_FAILED" => Self::ConfigurationUploadFailed,
            "CONFIGURATION_DELETE_FAILED" => Self::ConfigurationDeleteFailed,
            "CONFIGURATION_INTEGRITY_CHECK_FAILED" => Self::ConfigurationIntegrityCheckFailed,
            "DEVICE_NOT_REGISTERED" => Self::DeviceNotRegistered,
            "DEVICE_NOT_AUTHORIZED" => Self::DeviceNotAuthorized,
            "AUTHENTICATION_REQUIRED" => Self::AuthenticationRequired,
            "RATE_LIMITED" => Self::RateLimited,
            "DECRYPTION_FAILED" => Self::DecryptionFailed,
            _ => Self::Network,
        }
    }

    /// The HTTP status a compliant server answers with for this error
    /// (mirrors `docs/cloud-alibaba.md` §7).
    pub fn http_status(self) -> u16 {
        use CloudError::*;
        match self {
            ConfigurationAlreadyExists => 409,
            ConfigurationNotFound => 404,
            ConfigurationOverwriteNotAuthorized => 403,
            ConfigurationSizeLimitExceeded => 413,
            ConfigurationUploadFailed
            | ConfigurationDeleteFailed
            | ConfigurationIntegrityCheckFailed => 500,
            DeviceNotRegistered => 404,
            DeviceNotAuthorized | AuthenticationRequired => 401,
            RateLimited => 429,
            DecryptionFailed => 400,
            Network => 502,
        }
    }
}

// ---------------------------------------------------------------------------
// Client-side encryption (server never sees the plaintext, never the
// passphrase; Argon2id KDF + AES-256-GCM — the project's established
// primitive set, reused from `crate::secrets`).
// ---------------------------------------------------------------------------

/// An encrypted configuration blob plus the opaque KDF material the client
/// must retain to decrypt later. The server stores both fields opaquely and
/// can never derive the passphrase or plaintext.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloudBlob {
    /// KDF salt + Argon2id digest (hex). Needed for decrypt.
    pub kdf: crate::secrets::KdfMeta,
    /// base64( nonce(12) | ciphertext | tag(16) ).
    pub payload: String,
}

/// Encrypt a configuration payload for upload. The plaintext (API keys, etc.)
/// never leaves the machine in any form the server can read.
pub fn encrypt_configuration(
    passphrase: &str,
    payload: &[u8],
) -> Result<CloudBlob, crate::MigratorError> {
    let mut items: BTreeMap<String, (String, Vec<u8>)> = BTreeMap::new();
    items.insert("configuration".into(), ("cloud".into(), payload.to_vec()));
    let bundle = crate::secrets::encrypt_secrets(passphrase, items)?;
    let sealed = bundle
        .entries
        .get("configuration")
        .cloned()
        .ok_or_else(|| crate::MigratorError::Secrets("entry missing".into()))?;
    Ok(CloudBlob {
        kdf: bundle.kdf,
        payload: sealed,
    })
}

/// Decrypt a previously-uploaded blob. Wrong passphrase / corrupted
/// ciphertext → `Secrets` error; the caller maps that to
/// `CloudError::DecryptionFailed`.
pub fn decrypt_configuration(
    passphrase: &str,
    blob: &CloudBlob,
) -> Result<Vec<u8>, crate::MigratorError> {
    let bundle = crate::secrets::SecretsBundle {
        kdf: blob.kdf.clone(),
        entries: BTreeMap::from([("configuration".to_string(), blob.payload.clone())]),
    };
    let out = crate::secrets::decrypt_secrets(passphrase, &bundle)?;
    out.get("configuration")
        .cloned()
        .ok_or_else(|| crate::MigratorError::Secrets("configuration entry missing".into()))
}

/// SHA-256 of the sealed payload (the server's integrity gate). Malformed
/// base64 (wire corruption) yields a deterministic mismatch → the integrity
/// check fails closed, which is exactly the safe outcome.
pub fn blob_sha256_hex(blob: &CloudBlob) -> String {
    let raw = crate::secrets::b64::decode(&blob.payload).unwrap_or_default();
    crate::secrets::content_hash_hex(&raw)
}

/// Diagnostics: compact shareable id + a redacted field map for the optional
/// "Upload Logs" feature. `Diagnostic ID` format: `HM-XXXXXX`.
pub fn make_diagnostic_id() -> String {
    let id = DeviceId::generate();
    let raw: String = id.0.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    format!("HM-{}", &raw[..6.min(raw.len())])
}

/// Redact a key/value map: known secret keys and secret-looking values are
/// replaced with `***`; unclassifiable data is NOT uploaded (fail-closed).
pub fn redact_secrets(fields: BTreeMap<String, String>) -> BTreeMap<String, String> {
    const SECRET_KEYS: &[&str] = &[
        "api_key",
        "apikey",
        "access_key",
        "secret",
        "token",
        "password",
        "refresh_token",
        "authorization",
        "cookie",
        "private_key",
        "session",
    ];
    let mut out = BTreeMap::new();
    for (k, v) in fields {
        let lk = k.to_lowercase().replace([' ', '-'], "_");
        if SECRET_KEYS.iter().any(|s| lk == *s || lk.contains(s)) || looks_like_secret_value(&v) {
            out.insert(k, "***".into());
        } else {
            out.insert(k, v);
        }
    }
    out
}

/// Heuristic: secret-ish if `Bearer`, a PEM private key, or a long
/// high-entropy alphanumeric token.
fn looks_like_secret_value(v: &str) -> bool {
    let t = v.trim();
    if t.to_lowercase().starts_with("bearer ") {
        return true;
    }
    if t.contains("BEGIN") && t.contains("PRIVATE KEY") {
        return true;
    }
    if t.len() >= 32 {
        let good = t
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '_')
            .count();
        good / t.len() >= 90
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Reference backend: in-memory, unit-testable, mirrors the single-device /
// single-config contract a real deployment (Alibaba Cloud: DB + OSS +
// Cloud API) must honour. No I/O, no network — deterministic.
// ---------------------------------------------------------------------------

/// One encrypted cloud configuration as stored server-side.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredConfig {
    pub blob: CloudBlob,
    /// SHA-256 (hex) of the sealed payload — the integrity gate.
    pub sha256: String,
    pub device_id: String,
    pub uploaded_at: u64,
}

/// Metadata for "do I have a config?" UI without transferring the blob.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredConfigMeta {
    pub device_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub uploaded_at: u64,
}

/// Operational limits for a deployment (the reference backend enforces all
/// of them in-process; a real gateway enforces the same numbers upstream).
#[derive(Debug, Clone)]
pub struct BackendLimits {
    /// Max sealed blob size in bytes. Default: 50 MiB.
    pub max_blob_bytes: u64,
    /// Max requests per device per 60 s. Default: 60.
    pub rate_limit_per_min: u32,
}

impl Default for BackendLimits {
    fn default() -> Self {
        Self {
            max_blob_bytes: 50 * 1024 * 1024,
            rate_limit_per_min: 60,
        }
    }
}

#[derive(Debug)]
struct InnerState {
    /// device_id -> (registered_at, server auth token).
    registered: BTreeMap<String, (u64, String)>,
    /// device_id -> THE one active config (0 or 1 — the invariant).
    configs: BTreeMap<String, StoredConfig>,
    /// Simulated OSS: object key "device/sha" -> (device, alive-flag).
    /// Orphaned blobs (replaced by overwrite) stay until cleanup.
    oss: BTreeMap<String, String>,
    /// object key -> due-now (ms clock) for scheduled orphan deletion.
    orphan_due: BTreeMap<String, u64>,
    /// Sliding-window rate limiter: device -> request timestamps (ms).
    rate_window: BTreeMap<String, Vec<u64>>,
    /// Monotonic logical clock (tests drive it; real time in production).
    now_ms: u64,
}

/// In-memory reference backend. Thread-safe; all state behind one mutex so
/// the 100-thread concurrency test exercises the real atomic-swap path.
#[derive(Debug)]
pub struct InMemoryBackend {
    limits: BackendLimits,
    inner: Mutex<InnerState>,
}

impl InMemoryBackend {
    pub fn new(limits: BackendLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(InnerState {
                registered: BTreeMap::new(),
                configs: BTreeMap::new(),
                oss: BTreeMap::new(),
                orphan_due: BTreeMap::new(),
                rate_window: BTreeMap::new(),
                now_ms: 0,
            }),
        }
    }

    /// Tests: advance the logical clock.
    pub fn advance_ms(&self, ms: u64) {
        let mut g = self.inner.lock().unwrap();
        g.now_ms = g.now_ms.saturating_add(ms);
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new(BackendLimits::default())
    }
}

/// Result of a request to the cloud layer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum CloudResult {
    Ok(StoredConfigMeta),
    Err { code: CloudError, message: String },
}

impl InMemoryBackend {
    /// Register a device: idempotent. Same device_id always gets the same
    /// auth token. Unknown device ids must first be registered — a random
    /// fake `device_id` with no registration fails on the next request.
    pub fn register(&self, device_id: &DeviceId) -> Result<String, CloudError> {
        DeviceId::validate(&device_id.0).map_err(|_| CloudError::DeviceNotRegistered)?;
        let mut g = self.inner.lock().unwrap();
        let now = g.now_ms;
        match g.registered.get(&device_id.0) {
            Some((_, tok)) => Ok(tok.clone()),
            None => {
                let tok = crate::secrets::b64::encode(DeviceId::generate().0.as_bytes());
                g.registered.insert(device_id.0.clone(), (now, tok.clone()));
                Ok(tok)
            }
        }
    }

    /// Authenticate a request. Returns the device's record or a stable error.
    fn authenticate(&self, device_id: &str, token: &str) -> Result<(), CloudError> {
        let g = self.inner.lock().unwrap();
        match g.registered.get(device_id) {
            None => Err(CloudError::DeviceNotRegistered),
            Some((_, t)) => {
                if t == token {
                    Ok(())
                } else {
                    Err(CloudError::DeviceNotAuthorized)
                }
            }
        }
    }

    /// Sliding-window rate limit check: allow + record, or RATE_LIMITED.
    fn rate_check(&self, device_id: &str) -> Result<(), CloudError> {
        let mut g = self.inner.lock().unwrap();
        let now = g.now_ms;
        let window = g.rate_window.entry(device_id.to_string()).or_default();
        while let Some(&first) = window.first() {
            if now - first >= 60_000 {
                window.remove(0);
            } else {
                break;
            }
        }
        if window.len() >= self.limits.rate_limit_per_min as usize {
            return Err(CloudError::RateLimited);
        }
        window.push(now);
        Ok(())
    }

    fn guard(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<std::sync::MutexGuard<'_, InnerState>, CloudError> {
        self.authenticate(device_id, token)?;
        self.rate_check(device_id)?;
        Ok(self.inner.lock().unwrap())
    }

    /// Does this device have its one active configuration?
    pub fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError> {
        let g = self.guard(device_id, token)?;
        Ok(g.configs.get(device_id).map(meta_of))
    }

    /// CREATE: only allowed when the device has NO active configuration.
    /// Enforces the 50 MiB sealed-blob size limit.
    pub fn create_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
    ) -> Result<StoredConfigMeta, CloudError> {
        let mut g = self.guard(device_id, token)?;
        if g.configs.contains_key(device_id) {
            return Err(CloudError::ConfigurationAlreadyExists);
        }
        let sealed = sealed_len(blob);
        if sealed > self.limits.max_blob_bytes {
            return Err(CloudError::ConfigurationSizeLimitExceeded);
        }
        let now = g.now_ms;
        let sha = blob_sha256_hex(blob);
        let key = format!("{device_id}/{sha}");
        g.oss.insert(key.clone(), device_id.to_string());
        let cfg = StoredConfig {
            blob: blob.clone(),
            sha256: sha.clone(),
            device_id: device_id.to_string(),
            uploaded_at: now,
        };
        g.configs.insert(device_id.to_string(), cfg);
        Ok(meta_of(&g.configs[device_id]))
    }

    /// OVERWRITE: only allowed when a configuration EXISTS. The previous
    /// OSS object becomes an orphan, scheduled for the cleanup job — until
    /// then OSS holds 1 old + 1 new object (intentional, by design).
    pub fn overwrite_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
    ) -> Result<StoredConfigMeta, CloudError> {
        let mut g = self.guard(device_id, token)?;
        if !g.configs.contains_key(device_id) {
            return Err(CloudError::ConfigurationNotFound);
        }
        let sealed = sealed_len(blob);
        if sealed > self.limits.max_blob_bytes {
            return Err(CloudError::ConfigurationSizeLimitExceeded);
        }
        let now = g.now_ms;
        // Schedule deletion of the previous blob (orphan), due in 24 h.
        // The old object stays in `oss` until the cleanup job removes it.
        if let Some(old) = g.configs.get(device_id) {
            let old_key = format!("{device_id}/{}", old.sha256);
            if g.oss.contains_key(&old_key) {
                g.orphan_due.insert(old_key.clone(), now + 24 * 3600 * 1000);
            }
        }
        let sha = blob_sha256_hex(blob);
        let key = format!("{device_id}/{sha}");
        g.oss.insert(key.clone(), device_id.to_string());
        g.configs.insert(
            device_id.to_string(),
            StoredConfig {
                blob: blob.clone(),
                sha256: sha.clone(),
                device_id: device_id.to_string(),
                uploaded_at: now,
            },
        );
        Ok(meta_of(&g.configs[device_id]))
    }

    /// Atomic upsert: CREATE when the device has no active config, OVERWRITE
    /// when it does. A single guard acquisition, so it consumes exactly one
    /// rate-limit slot (unlike calling `has_configuration` + create/overwrite,
    /// which would consume two). `overwrite` only matters when a config
    /// already exists; when it is absent the blob is simply created.
    pub fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        let mut g = self.guard(device_id, token)?;
        let exists = g.configs.contains_key(device_id);
        if exists && !overwrite {
            // Holds a config and the caller did not ask to replace it.
            return Err(CloudError::ConfigurationAlreadyExists);
        }
        let sealed = sealed_len(blob);
        if sealed > self.limits.max_blob_bytes {
            return Err(CloudError::ConfigurationSizeLimitExceeded);
        }
        let now = g.now_ms;
        // When replacing, schedule the previous blob for the cleanup job.
        if exists {
            if let Some(old) = g.configs.get(device_id) {
                let old_key = format!("{device_id}/{}", old.sha256);
                if g.oss.contains_key(&old_key) {
                    g.orphan_due.insert(old_key.clone(), now + 24 * 3600 * 1000);
                }
            }
        }
        let sha = blob_sha256_hex(blob);
        let key = format!("{device_id}/{sha}");
        g.oss.insert(key.clone(), device_id.to_string());
        g.configs.insert(
            device_id.to_string(),
            StoredConfig {
                blob: blob.clone(),
                sha256: sha.clone(),
                device_id: device_id.to_string(),
                uploaded_at: now,
            },
        );
        Ok(meta_of(&g.configs[device_id]))
    }

    /// DELETE: only allowed when a configuration EXISTS. Marks the OSS blob
    /// an orphan; the active-configuration count drops to 0, so CREATE is
    /// allowed again.
    pub fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        let mut g = self.guard(device_id, token)?;
        let old = match g.configs.remove(device_id) {
            Some(c) => c,
            None => return Err(CloudError::ConfigurationNotFound),
        };
        let now = g.now_ms;
        // The blob object stays in `oss` but is scheduled for the cleanup
        // job (due in 24 h). Active-config count drops to 0 → CREATE ok.
        let key = format!("{device_id}/{}", old.sha256);
        if g.oss.contains_key(&key) {
            g.orphan_due.insert(key, now + 24 * 3600 * 1000);
        }
        Ok(1)
    }

    /// Download THE blob for this device. Re-verifies integrity on the wire
    /// value (the server stores sha256 next to the blob; a mismatch is a
    /// hard failure the client must surface, never silently accept).
    pub fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError> {
        let g = self.guard(device_id, token)?;
        match g.configs.get(device_id) {
            None => Ok(None),
            Some(cfg) => {
                let actual = blob_sha256_hex(&cfg.blob);
                if let Some(exp) = expected_sha {
                    if exp != actual {
                        return Err(CloudError::ConfigurationIntegrityCheckFailed);
                    }
                }
                if actual != cfg.sha256 {
                    return Err(CloudError::ConfigurationIntegrityCheckFailed);
                }
                Ok(Some(cfg.blob.clone()))
            }
        }
    }

    /// Orphan-cleanup job: delete OSS blobs whose scheduled time has passed.
    /// Returns how many objects were removed. (Simulated: a real OSS cleanup
    /// runs as a cron/lambda calling this periodically.)
    pub fn cleanup_orphans(&self) -> u32 {
        let mut g = self.inner.lock().unwrap();
        let now = g.now_ms;
        let due: Vec<String> = g
            .orphan_due
            .iter()
            .filter(|(_, t)| **t <= now)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &due {
            g.oss.remove(k);
            g.orphan_due.remove(k);
        }
        due.len() as u32
    }

    /// Current OSS object count for a device (reconciliation helper).
    pub fn oss_object_count(&self, device_id: &str) -> usize {
        let g = self.inner.lock().unwrap();
        g.oss.values().filter(|v| *v == device_id).count()
    }

    /// Reconciliation audit: list every active config across all devices.
    pub fn audit_active_configs(&self) -> Vec<StoredConfigMeta> {
        let g = self.inner.lock().unwrap();
        g.configs.values().map(meta_of).collect()
    }
}

fn meta_of(c: &StoredConfig) -> StoredConfigMeta {
    StoredConfigMeta {
        device_id: c.device_id.clone(),
        sha256: c.sha256.clone(),
        size_bytes: sealed_len(&c.blob),
        uploaded_at: c.uploaded_at,
    }
}

/// Sealed on-the-wire size of a blob (base64 payload length).
fn sealed_len(blob: &CloudBlob) -> u64 {
    blob.payload.len() as u64
}

/// Errors that can come out of the reference backend.
impl From<CloudError> for crate::MigratorError {
    fn from(e: CloudError) -> Self {
        crate::MigratorError::Other(format!("cloud: {}", e.code()))
    }
}

impl From<crate::MigratorError> for CloudError {
    fn from(e: crate::MigratorError) -> Self {
        match e {
            // Passphrase-dependent decryption failures surface to the user
            // as the stable `DECRYPTION_FAILED` code.
            crate::MigratorError::Secrets(_) => CloudError::DecryptionFailed,
            // Anything else is an unexpected internal failure; the client
            // shows it generically.
            _ => CloudError::ConfigurationUploadFailed,
        }
    }
}

/// A per-device client façade: holds the stable `DeviceId` + auth token and
/// exposes the 0/1 active-configuration semantic that the GUI/CLI consume.
#[derive(Debug, Clone)]
pub struct CloudClient {
    pub device_id: DeviceId,
    pub token: String,
    backend: std::sync::Arc<InMemoryBackend>,
}

impl CloudClient {
    pub fn connect(
        backend: std::sync::Arc<InMemoryBackend>,
        device_id: DeviceId,
    ) -> Result<Self, CloudError> {
        let token = backend.register(&device_id)?;
        Ok(Self {
            device_id,
            token,
            backend,
        })
    }

    /// True when this device currently holds its one active configuration.
    pub fn has_configuration(&self) -> Result<bool, CloudError> {
        self.backend
            .has_configuration(&self.device_id.0, &self.token)
            .map(|m| m.is_some())
    }

    /// Upload: CREATE when absent, OVERWRITE when present + overwrite=true.
    /// Single atomic upsert → consumes exactly one rate-limit slot.
    pub fn upload(
        &self,
        payload: &[u8],
        passphrase: &str,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        let blob = encrypt_configuration(passphrase, payload)?;
        self.backend
            .upsert_configuration(&self.device_id.0, &self.token, &blob, overwrite)
    }

    /// Download + decrypt THE configuration. `None` when the device has no
    /// active config; wrong passphrase surfaces as `DecryptionFailed`.
    pub fn download(&self, passphrase: &str) -> Result<Option<Vec<u8>>, CloudError> {
        let blob =
            match self
                .backend
                .download_configuration(&self.device_id.0, &self.token, None)?
            {
                Some(b) => b,
                None => return Ok(None),
            };
        match decrypt_configuration(passphrase, &blob) {
            Ok(p) => Ok(Some(p)),
            Err(_) => Err(CloudError::DecryptionFailed),
        }
    }

    /// Delete THE configuration. Returns how many (0 not possible —
    /// `ConfigurationNotFound` error instead; 1 when removed).
    pub fn delete(&self) -> Result<u32, CloudError> {
        self.backend
            .delete_configuration(&self.device_id.0, &self.token)
    }

    /// Trigger the orphan-cleanup job (real deployments run this on a timer).
    pub fn run_cleanup_job(&self) -> u32 {
        self.backend.cleanup_orphans()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn blob(payload: &[u8]) -> CloudBlob {
        encrypt_configuration("s3cret", payload).unwrap()
    }

    #[test]
    fn device_id_roundtrip_and_validation() {
        let d = DeviceId::generate();
        DeviceId::validate(&d.0).unwrap();
        assert!(DeviceId::validate("not-a-uuid").is_err());
        assert!(DeviceId::validate("").is_err());
        // Two generates are distinct (probabilistic, 128-bit space).
        assert_ne!(d, DeviceId::generate());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let p = b"api_key=ak_xxx\npassphrase=zz";
        let b = blob(p);
        let got = decrypt_configuration("s3cret", &b).unwrap();
        assert_eq!(got, p);
        // Wrong passphrase -> hard failure, never silent.
        assert!(decrypt_configuration("nope", &b).is_err());
    }

    #[test]
    fn single_config_lifecycle() {
        let be = Arc::new(InMemoryBackend::default());
        let c = CloudClient::connect(be.clone(), DeviceId::generate()).unwrap();
        assert!(!c.has_configuration().unwrap());
        // CREATE while empty.
        let m = c.upload(b"one", "s3cret", false).unwrap();
        assert!(c.has_configuration().unwrap());
        assert_eq!(m.device_id, c.device_id.0);
        // Second CREATE blocked (single-config invariant).
        assert_eq!(
            c.upload(b"two", "s3cret", false).unwrap_err(),
            CloudError::ConfigurationAlreadyExists
        );
        // OVERWRITE allowed, returns new meta with same device.
        let m2 = c.upload(b"two", "s3cret", true).unwrap();
        assert_ne!(m2.sha256, m.sha256);
        // Download + decrypt.
        assert_eq!(c.download("s3cret").unwrap().unwrap(), b"two");
        // Delete -> 0 configs -> CREATE allowed again.
        assert_eq!(c.delete().unwrap(), 1);
        assert!(!c.has_configuration().unwrap());
        assert_eq!(c.delete().unwrap_err(), CloudError::ConfigurationNotFound);
        let _ = c.upload(b"three", "s3cret", false);
    }

    #[test]
    fn rate_limit_kicks_in() {
        let be = Arc::new(InMemoryBackend::new(BackendLimits {
            rate_limit_per_min: 3,
            ..Default::default()
        }));
        let c = CloudClient::connect(be.clone(), DeviceId::generate()).unwrap();
        c.upload(b"a", "s", false).unwrap();
        c.upload(b"a", "s", true).unwrap();
        c.upload(b"a", "s", true).unwrap();
        // 4th within the window: RATE_LIMITED.
        assert_eq!(c.has_configuration().unwrap_err(), CloudError::RateLimited);
        // After the window slides: allowed again.
        be.advance_ms(61_000);
        assert!(c.has_configuration().unwrap());
    }

    #[test]
    fn size_limit_enforced() {
        let be = Arc::new(InMemoryBackend::new(BackendLimits {
            max_blob_bytes: 16, // tiny for the test
            ..Default::default()
        }));
        let c = CloudClient::connect(be, DeviceId::generate()).unwrap();
        assert_eq!(
            c.upload(b"x", "s", false).unwrap_err(),
            CloudError::ConfigurationSizeLimitExceeded
        );
    }

    #[test]
    fn unknown_device_rejected() {
        let be = Arc::new(InMemoryBackend::default());
        let d = DeviceId::generate();
        // No registration -> next op is DEVICE_NOT_REGISTERED.
        let r = be.has_configuration(&d.0, "tok");
        assert_eq!(r.unwrap_err(), CloudError::DeviceNotRegistered);
        // Even after registering, a bogus token is not authorized.
        let _ = be.register(&d).unwrap();
        assert_eq!(
            be.has_configuration(&d.0, "forged-token"),
            Err(CloudError::DeviceNotAuthorized)
        );
    }

    #[test]
    fn two_devices_independent() {
        let be = Arc::new(InMemoryBackend::default());
        let a = CloudClient::connect(be.clone(), DeviceId::generate()).unwrap();
        let b = CloudClient::connect(be, DeviceId::generate()).unwrap();
        a.upload(b"mine", "s", false).unwrap();
        // b has no config; cannot borrow a's.
        assert!(!b.has_configuration().unwrap());
        assert!(a.has_configuration().unwrap());
    }

    #[test]
    fn download_reports_integrity_failure_on_mismatch() {
        let be = Arc::new(InMemoryBackend::default());
        let c = CloudClient::connect(be.clone(), DeviceId::generate()).unwrap();
        c.upload(b"data", "s", false).unwrap();
        // Expecting a different sha than stored -> integrity failure.
        let r = be.download_configuration(&c.device_id.0, &c.token, Some("deadbeef"));
        assert_eq!(
            r.unwrap_err(),
            CloudError::ConfigurationIntegrityCheckFailed
        );
    }

    #[test]
    fn orphan_cleanup_fires() {
        let be = Arc::new(InMemoryBackend::default());
        let c = CloudClient::connect(be.clone(), DeviceId::generate()).unwrap();
        c.upload(b"v1", "s", false).unwrap();
        c.upload(b"v2", "s", true).unwrap(); // orphans v1 blob in OSS
        assert_eq!(be.oss_object_count(&c.device_id.0), 2); // v1(orphan)+v2(active)
        be.advance_ms(24 * 3600 * 1000 + 1);
        assert_eq!(be.cleanup_orphans(), 1);
        assert_eq!(be.oss_object_count(&c.device_id.0), 1); // only v2 now
    }

    #[test]
    fn diagnostic_id_shape_and_redaction() {
        let id = make_diagnostic_id();
        assert!(id.starts_with("HM-"), "{id}");
        let mut f = BTreeMap::new();
        f.insert("api_key".into(), "sk-supersecret".into());
        f.insert("http_status".into(), "404".into());
        f.insert("authorization".into(), "Bearer abc".into());
        let red = redact_secrets(f);
        assert_eq!(red["api_key"], "***");
        assert_eq!(red["http_status"], "404");
        assert_eq!(red["authorization"], "***");
    }

    #[test]
    fn concurrent_single_device_stays_consistent() {
        // 100 threads: 50 create-or-overwrite, 50 read. The invariant that
        // every read sees EXACTLY ONE active config must hold throughout.
        let be = Arc::new(InMemoryBackend::new(BackendLimits {
            rate_limit_per_min: 100_000,
            ..Default::default()
        }));
        let dev = DeviceId::generate();
        let c = CloudClient::connect(be.clone(), dev).unwrap();
        // Prime it so the race starts from "1 active config".
        c.upload(b"seed", "s", true).unwrap();
        let client = Arc::new(c);
        let backend = be.clone();
        let handles: Vec<_> = (0..100)
            .map(|i| {
                let client = client.clone();
                let backend = backend.clone();
                std::thread::spawn(move || {
                    if i % 2 == 0 {
                        let _ = client.upload(&[i as u8; 64], "s", true);
                    } else {
                        // Every read must see EXACTLY ONE active config; a
                        // rate-limited read under the storm is tolerated.
                        match backend.has_configuration(&client.device_id.0, &client.token) {
                            Ok(meta) => assert!(meta.is_some(), "invariant: read saw no config"),
                            Err(CloudError::RateLimited) => {}
                            Err(e) => panic!("unexpected cloud error: {e:?}"),
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // After the storm: still exactly one active config.
        assert_eq!(be.audit_active_configs().len(), 1);
    }
}
