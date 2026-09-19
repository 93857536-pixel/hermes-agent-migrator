//! File-backed reference server for the cloud configuration system.
//!
//! This is the *server side* of the one-device / one-configuration
//! contract, implemented as a plain local directory so the whole system
//! (GUI, CLI, tests) is fully runnable without a live Alibaba Cloud
//! deployment. A real deployment replaces this module with the Cloud
//! API + OSS + database described in the cloud-alibaba doc; the public
//! surface here is what that remote service must honour.
//!
//! Layout on disk (one directory = one "server"):
//!
//! ```text
//! root/
//!   devices.json            // device_id -> auth token (simulated DB table)
//!
//!   configs/
//!     {device_id}.meta.json // StoredConfigMeta + sha256 (simulated DB row)
//!     {device_id}.enc       // sealed blob (simulated OSS object)
//!   orphans/
//!     {device_id}-{sha}.enc // replaced blobs awaiting the 24 h cleanup
//! ```
//!
//! Atomic replacement (the spec's "upload temporary object -> verify ->
//! atomically replace -> delete old object"): a new blob is written to
//! `configs/{id}.enc.tmp` and moved into place with `fs::rename` (atomic on
//! all supported filesystems). The previous blob is moved to `orphans/`
//! with an expiry timestamp; `sweep_orphans` removes any older than 24 h.
//! The metadata file is updated after the blob is safely in place, so a
//! crash can never leave the database claiming a config exists while the
//! object is missing (fail-safe side).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::cloud::{blob_sha256_hex, CloudBlob, CloudError, DeviceId, StoredConfigMeta};

/// A persisted device record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceRecord {
    pub device_id: String,
    pub auth_token: String,
    /// Logical clock (ms) when the device first registered.
    pub registered_at_ms: u64,
    /// Last observed public IP — recorded for security/audit only, never
    /// used as the device identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ip: Option<String>,
}

/// The one active configuration metadata row for a device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigRow {
    pub device_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub uploaded_at_ms: u64,
}

/// A reference "cloud server" rooted at a local directory. Thread-safe.
#[derive(Debug)]
pub struct FileServer {
    root: PathBuf,
    /// logical clock: real servers use the wall clock; tests drive this.
    clock_ms: Mutex<u64>,
    /// sliding-window rate limiter: device_id -> last request times (ms).
    rate_window: Mutex<std::collections::BTreeMap<String, Vec<u64>>>,
    limits: crate::cloud::BackendLimits,
}

impl FileServer {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        for sub in ["configs", "orphans"] {
            fs::create_dir_all(root.join(sub)).ok();
        }
        Self {
            root,
            clock_ms: Mutex::new(now_real_ms()),
            rate_window: Mutex::new(std::collections::BTreeMap::new()),
            limits: crate::cloud::BackendLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: crate::cloud::BackendLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Tests: advance the logical clock.
    pub fn advance_ms(&self, ms: u64) {
        *self.clock_ms.lock().unwrap() += ms;
    }

    fn now(&self) -> u64 {
        *self.clock_ms.lock().unwrap()
    }

    /// Register a device (idempotent; same id -> same token forever).
    pub fn register(&self, device_id: &DeviceId) -> Result<String, CloudError> {
        let rec = self.load_devices()?;
        let tok = match rec.get(&device_id.0) {
            Some(r) => r.auth_token.clone(),
            None => {
                let tok = crate::secrets::b64::encode(DeviceId::generate().0.as_bytes());
                let mut d = rec.clone();
                d.insert(
                    device_id.0.clone(),
                    DeviceRecord {
                        device_id: device_id.0.clone(),
                        auth_token: tok.clone(),
                        registered_at_ms: self.now(),
                        last_ip: None,
                    },
                );
                self.save_devices(&d)?;
                tok
            }
        };
        Ok(tok)
    }

    fn load_devices(&self) -> Result<std::collections::BTreeMap<String, DeviceRecord>, CloudError> {
        let p = self.root.join("devices.json");
        if !p.exists() {
            return Ok(std::collections::BTreeMap::new());
        }
        let raw = fs::read_to_string(&p).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        serde_json::from_str(&raw).map_err(|_| CloudError::ConfigurationUploadFailed)
    }

    fn save_devices(
        &self,
        d: &std::collections::BTreeMap<String, DeviceRecord>,
    ) -> Result<(), CloudError> {
        let raw =
            serde_json::to_string_pretty(d).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        let p = self.root.join("devices.json");
        fs::write(&p, raw).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        Ok(())
    }

    fn authenticate(&self, device_id: &str, token: &str) -> Result<(), CloudError> {
        let rec = self.load_devices()?;
        match rec.get(device_id) {
            None => Err(CloudError::DeviceNotRegistered),
            Some(r) => {
                if r.auth_token == token {
                    Ok(())
                } else {
                    Err(CloudError::DeviceNotAuthorized)
                }
            }
        }
    }

    fn rate_check(&self, device_id: &str) -> Result<(), CloudError> {
        let now = self.now();
        let mut map = self.rate_window.lock().unwrap();
        let window = map.entry(device_id.to_string()).or_default();
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

    fn guard(&self, device_id: &str, token: &str) -> Result<(), CloudError> {
        self.authenticate(device_id, token)?;
        self.rate_check(device_id)?;
        Ok(())
    }

    // ---- per-device file helpers ----------------------------------------

    fn meta_path(&self, device_id: &str) -> PathBuf {
        self.root
            .join("configs")
            .join(format!("{device_id}.meta.json"))
    }
    fn blob_path(&self, device_id: &str) -> PathBuf {
        self.root.join("configs").join(format!("{device_id}.enc"))
    }

    fn load_row(&self, device_id: &str) -> Result<Option<ConfigRow>, CloudError> {
        let p = self.meta_path(device_id);
        if !p.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&p).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        let row: ConfigRow =
            serde_json::from_str(&raw).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        // Reconcile: if the row points at a blob that is missing, the
        // database over-claims; fail-safe is to treat it as absent.
        if !self.blob_path(device_id).exists() {
            return Ok(None);
        }
        Ok(Some(row))
    }

    /// Does this device hold its one active configuration?
    pub fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError> {
        self.guard(device_id, token)?;
        let row = self.load_row(device_id)?;
        Ok(row.map(|r| StoredConfigMeta {
            device_id: r.device_id,
            sha256: r.sha256,
            size_bytes: r.size_bytes,
            uploaded_at: r.uploaded_at_ms,
        }))
    }

    /// CREATE: only when the device has NO active config. Enforces the size
    /// limit. Persists the sealed blob, then metadata (so a crash can't
    /// over-claim).
    pub fn create_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
    ) -> Result<StoredConfigMeta, CloudError> {
        self.guard(device_id, token)?;
        if self.load_row(device_id)?.is_some() {
            return Err(CloudError::ConfigurationAlreadyExists);
        }
        let sealed_len = blob.payload.len() as u64;
        if sealed_len > self.limits.max_blob_bytes {
            return Err(CloudError::ConfigurationSizeLimitExceeded);
        }
        let sha = blob_sha256_hex(blob);
        let body =
            serde_json::to_string(blob).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        // Write to .tmp, then atomic rename into place.
        let tmp = self.blob_path(device_id).with_extension("enc.tmp");
        fs::write(&tmp, &body).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        fs::rename(&tmp, self.blob_path(device_id))
            .map_err(|_| CloudError::ConfigurationUploadFailed)?;
        let row = ConfigRow {
            device_id: device_id.to_string(),
            sha256: sha,
            size_bytes: sealed_len,
            uploaded_at_ms: self.now(),
        };
        self.save_row(&row)?;
        Ok(StoredConfigMeta {
            device_id: row.device_id,
            sha256: row.sha256,
            size_bytes: row.size_bytes,
            uploaded_at: row.uploaded_at_ms,
        })
    }

    /// OVERWRITE: only when a config EXISTS. Atomic replace via temp file +
    /// rename; the previous blob is shunted to `orphans/` for the 24 h
    /// cleanup job (so the store holds at most 1 old + 1 new object).
    pub fn overwrite_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
    ) -> Result<StoredConfigMeta, CloudError> {
        self.guard(device_id, token)?;
        let old = self
            .load_row(device_id)?
            .ok_or(CloudError::ConfigurationNotFound)?;
        let sealed_len = blob.payload.len() as u64;
        if sealed_len > self.limits.max_blob_bytes {
            return Err(CloudError::ConfigurationSizeLimitExceeded);
        }
        let sha = blob_sha256_hex(blob);
        let body =
            serde_json::to_string(blob).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        // 1) move the previous object to orphans/ (scheduled deletion).
        let old_blob = self.blob_path(device_id);
        if old_blob.exists() {
            let orphan = self
                .root
                .join("orphans")
                .join(format!("{device_id}-{}.enc", old.sha256));
            fs::rename(&old_blob, &orphan).ok();
            fs::write(
                self.root
                    .join("orphans")
                    .join(format!("{device_id}-{}.ttl", old.sha256)),
                format!("{}\n", self.now() + 24 * 3600 * 1000),
            )
            .ok();
        }
        // 2) atomic replace of the current object.
        let tmp = old_blob.with_extension("enc.tmp");
        fs::write(&tmp, &body).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        fs::rename(&tmp, &old_blob).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        // 3) update the metadata row.
        let row = ConfigRow {
            device_id: device_id.to_string(),
            sha256: sha,
            size_bytes: sealed_len,
            uploaded_at_ms: self.now(),
        };
        self.save_row(&row)?;
        Ok(StoredConfigMeta {
            device_id: row.device_id,
            sha256: row.sha256,
            size_bytes: row.size_bytes,
            uploaded_at: row.uploaded_at_ms,
        })
    }

    /// Atomic upsert used by the client facade: CREATE when absent,
    /// OVERWRITE when present.
    pub fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        let exists = self.load_row(device_id)?.is_some();
        if exists {
            if !overwrite {
                return Err(CloudError::ConfigurationAlreadyExists);
            }
            self.overwrite_configuration(device_id, token, blob)
        } else {
            self.create_configuration(device_id, token, blob)
        }
    }

    fn save_row(&self, row: &ConfigRow) -> Result<(), CloudError> {
        let raw =
            serde_json::to_string_pretty(row).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        let p = self.meta_path(&row.device_id);
        fs::write(&p, raw).map_err(|_| CloudError::ConfigurationUploadFailed)?;
        Ok(())
    }

    /// DELETE: only when a config EXISTS. Blob goes to orphans/ (24 h), the
    /// row is removed, so CREATE is allowed again afterwards.
    pub fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        self.guard(device_id, token)?;
        let row = self
            .load_row(device_id)?
            .ok_or(CloudError::ConfigurationNotFound)?;
        let blob = self.blob_path(device_id);
        if blob.exists() {
            let orphan = self
                .root
                .join("orphans")
                .join(format!("{device_id}-{}.enc", row.sha256));
            fs::rename(&blob, &orphan).ok();
            fs::write(
                self.root
                    .join("orphans")
                    .join(format!("{device_id}-{}.ttl", row.sha256)),
                format!("{}\n", self.now() + 24 * 3600 * 1000),
            )
            .ok();
        }
        let _ = fs::remove_file(self.meta_path(device_id));
        Ok(1)
    }

    /// Download THE blob, verifying integrity on the wire value. The stored
    /// object is the exact JSON-serialized `CloudBlob` (kdf + sealed
    /// payload), so it round-trips losslessly through the store.
    pub fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError> {
        self.guard(device_id, token)?;
        let row = match self.load_row(device_id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let body = fs::read_to_string(self.blob_path(device_id))
            .map_err(|_| CloudError::ConfigurationNotFound)?;
        let blob: CloudBlob = serde_json::from_str(&body)
            .map_err(|_| CloudError::ConfigurationIntegrityCheckFailed)?;
        let actual = blob_sha256_hex(&blob);
        if let Some(exp) = expected_sha {
            if exp != actual {
                return Err(CloudError::ConfigurationIntegrityCheckFailed);
            }
        }
        if actual != row.sha256 {
            return Err(CloudError::ConfigurationIntegrityCheckFailed);
        }
        if blob.payload.len() as u64 != row.size_bytes {
            return Err(CloudError::ConfigurationIntegrityCheckFailed);
        }
        Ok(Some(blob))
    }

    /// Reconciliation audit: list every active config row.
    pub fn audit_active_configs(&self) -> Vec<StoredConfigMeta> {
        let dir = self.root.join("configs");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(raw) = fs::read_to_string(&p) {
                    if let Ok(row) = serde_json::from_str::<ConfigRow>(&raw) {
                        if self.blob_path(&row.device_id).exists() {
                            out.push(StoredConfigMeta {
                                device_id: row.device_id,
                                sha256: row.sha256,
                                size_bytes: row.size_bytes,
                                uploaded_at: row.uploaded_at_ms,
                            });
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        out
    }

    /// Orphan-cleanup job: delete orphan blobs whose 24 h TTL has passed.
    /// Returns how many objects were removed.
    pub fn sweep_orphans(&self) -> u32 {
        let orphans = self.root.join("orphans");
        let Ok(entries) = fs::read_dir(&orphans) else {
            return 0;
        };
        let now = self.now();
        let mut removed = 0u32;
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().map(|s| s.to_string_lossy().into_owned());
            if let Some(n) = &name {
                if n.ends_with(".ttl") {
                    let sha_part = n.trim_end_matches(".ttl");
                    let body = fs::read_to_string(&p).unwrap_or_default();
                    let due: u64 = body.trim().parse().unwrap_or(0);
                    if now >= due {
                        let enc = orphans.join(format!("{sha_part}.enc"));
                        if enc.exists() {
                            let _ = fs::remove_file(&enc);
                        }
                        let _ = fs::remove_file(&p);
                        removed += 1;
                    }
                }
            }
        }
        removed
    }

    /// How many objects this device currently occupies in the store
    /// (1 active blob + any not-yet-swept orphans).
    pub fn object_count(&self, device_id: &str) -> usize {
        let mut n = 0usize;
        if self.blob_path(device_id).exists() {
            n += 1;
        }
        let orphans = self.root.join("orphans");
        if let Ok(entries) = fs::read_dir(orphans) {
            for e in entries.flatten() {
                if e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{device_id}-"))
                    && e.path().extension().is_some_and(|x| x == "enc")
                {
                    n += 1;
                }
            }
        }
        n
    }
}

fn now_real_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::encrypt_configuration;

    fn blob(payload: &[u8]) -> CloudBlob {
        encrypt_configuration("pass", payload).unwrap()
    }

    #[test]
    fn persist_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hm-cloud-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let srv = FileServer::new(&dir);
        let d = DeviceId::generate();
        let tok = srv.register(&d).unwrap();
        // No config yet.
        assert!(srv.has_configuration(&d.0, &tok).unwrap().is_none());
        let m1 = srv.create_configuration(&d.0, &tok, &blob(b"v1")).unwrap();
        assert!(srv.has_configuration(&d.0, &tok).unwrap().is_some());
        // Second create blocked (invariant).
        assert_eq!(
            srv.create_configuration(&d.0, &tok, &blob(b"x"))
                .unwrap_err(),
            CloudError::ConfigurationAlreadyExists
        );
        // Overwrite -> atomic replace; OSS now holds 1 old orphan + 1 active.
        let m2 = srv
            .overwrite_configuration(&d.0, &tok, &blob(b"v2"))
            .unwrap();
        assert_ne!(m1.sha256, m2.sha256);
        assert_eq!(srv.object_count(&d.0), 2);
        // Download returns a decryptable blob matching the row.
        let got = srv
            .download_configuration(&d.0, &tok, Some(&m2.sha256))
            .unwrap()
            .unwrap();
        let plain = crate::cloud::decrypt_configuration("pass", &got).unwrap();
        assert_eq!(plain, b"v2");
        // Delete -> 0 configs -> create allowed again.
        assert_eq!(srv.delete_configuration(&d.0, &tok).unwrap(), 1);
        assert!(srv.has_configuration(&d.0, &tok).unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_sweep_after_ttl() {
        let dir = std::env::temp_dir().join(format!("hm-cloud-sweep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let srv = FileServer::new(&dir);
        let d = DeviceId::generate();
        let tok = srv.register(&d).unwrap();
        srv.create_configuration(&d.0, &tok, &blob(b"v1")).unwrap();
        srv.overwrite_configuration(&d.0, &tok, &blob(b"v2"))
            .unwrap();
        assert_eq!(srv.object_count(&d.0), 2);
        srv.advance_ms(24 * 3600 * 1000 + 1);
        assert_eq!(srv.sweep_orphans(), 1);
        assert_eq!(srv.object_count(&d.0), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_isolation() {
        let dir = std::env::temp_dir().join(format!("hm-cloud-iso-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let srv = FileServer::new(&dir);
        let a = DeviceId::generate();
        let b = DeviceId::generate();
        let ta = srv.register(&a).unwrap();
        let tb = srv.register(&b).unwrap();
        srv.create_configuration(&a.0, &ta, &blob(b"a")).unwrap();
        assert!(srv.has_configuration(&b.0, &tb).unwrap().is_none());
        // b cannot touch a's config with its own token.
        match srv.download_configuration(&a.0, &tb, None) {
            Err(CloudError::DeviceNotAuthorized) => {}
            other => panic!("expected DeviceNotAuthorized, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rate_limit_persists_across_operations() {
        let dir = std::env::temp_dir().join(format!("hm-cloud-rate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let srv = FileServer::new(&dir).with_limits(crate::cloud::BackendLimits {
            rate_limit_per_min: 2,
            ..Default::default()
        });
        let d = DeviceId::generate();
        let tok = srv.register(&d).unwrap();
        srv.create_configuration(&d.0, &tok, &blob(b"1")).unwrap();
        srv.has_configuration(&d.0, &tok).unwrap();
        // 3rd request inside the window is limited.
        assert_eq!(
            srv.has_configuration(&d.0, &tok).unwrap_err(),
            CloudError::RateLimited
        );
        srv.advance_ms(61_000);
        assert!(srv.has_configuration(&d.0, &tok).unwrap().is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
