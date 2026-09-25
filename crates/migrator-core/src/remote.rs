//! Remote cloud backend: talk to a self-hosted `hermes-cloud-server` over
//! HTTP.
//!
//! The server is a single binary (tiny_http + [`crate::cloudserver::FileServer`])
//! that honours the same one-device / one-configuration contract documented
//! in `docs/cloud-alibaba.md`. Clients pick a backend at runtime:
//!
//! - a configured `server_url` (in `~/.hermes-migrator/config.json`) →
//!   [`RemoteBackend`] (HTTP);
//! - otherwise → the local reference backend ([`crate::cloudserver::FileServer`]).
//!
//! Wire format (JSON, stable — see [`crate::cloud`] for the error codes):
//!
//! | endpoint            | auth  | body                                        |
//! |---------------------|-------|---------------------------------------------|
//! | POST `{base}/v1/register`     | —         | `{"device_id"}` → `{"token"}`            |
//! | POST `{base}/v1/has-config`   | device    | — → `{"meta": StoredConfigMeta \| null}`  |
//! | POST `{base}/v1/upsert`       | device    | `{"blob","overwrite"}` → `{"meta"}`      |
//! | POST `{base}/v1/download`     | device    | `{"expected_sha":…?}` → `{"blob":…?}`   |
//! | POST `{base}/v1/delete`       | device    | — → `{"deleted": n}`                     |
//! | POST `{base}/v1/sweep-orphans`| device    | — → `{"swept": n}`                       |
//! | GET  `{base}/v1/health`       | —         | — → `{"api_version": 1}`                 |
//!
//! Errors answer with the matching HTTP status (see
//! [`crate::cloud::CloudError::http_status`]) and body
//! `{"code","message"}`; anything unparseable is surfaced as
//! [`crate::cloud::CloudError::Network`].

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::cloud::{CloudBlob, CloudError, DeviceId, StoredConfigMeta};
use crate::cloudserver::FileServer;

// ---------------------------------------------------------------------------
// Common backend surface
// ---------------------------------------------------------------------------

/// The one-device / one-configuration operations, backed either by the local
/// reference store or by a remote HTTP server. CLI/GUI hold a
/// `Box<dyn CloudStore>` resolved once per command.
pub trait CloudStore: Send + Sync {
    /// Register the device (idempotent; same id → same token).
    fn register(&self, device_id: &DeviceId) -> Result<String, CloudError>;

    /// Does this device hold its one active configuration?
    fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError>;

    /// Atomic upsert: CREATE when absent, OVERWRITE when present + overwrite.
    fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError>;

    /// Download THE blob (integrity-verified server-side).
    fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError>;

    /// Delete THE configuration (blob scheduled for the 24 h sweep).
    fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError>;

    /// Run the orphan-cleanup job; returns objects removed.
    fn sweep_orphans(&self, device_id: &str, token: &str) -> Result<u32, CloudError>;

    /// API version of the backing service (1 for all shipped builds).
    fn api_version(&self) -> Result<u32, CloudError>;
}

/// The local reference backend, exposed through the same surface a remote
/// server honours — so swapping is a drop-in.
pub struct LocalCloudStore {
    server: FileServer,
}

impl LocalCloudStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            server: FileServer::new(root.into()),
        }
    }

    /// The underlying store (tests / inspection).
    pub fn file_server(&self) -> &FileServer {
        &self.server
    }
}

impl Default for LocalCloudStore {
    fn default() -> Self {
        Self::new(default_server_root())
    }
}

impl CloudStore for LocalCloudStore {
    fn register(&self, device_id: &DeviceId) -> Result<String, CloudError> {
        self.server.register(device_id)
    }

    fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError> {
        self.server.has_configuration(device_id, token)
    }

    fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        self.server
            .upsert_configuration(device_id, token, blob, overwrite)
    }

    fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError> {
        self.server
            .download_configuration(device_id, token, expected_sha)
    }

    fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        self.server.delete_configuration(device_id, token)
    }

    fn sweep_orphans(&self, _device_id: &str, _token: &str) -> Result<u32, CloudError> {
        Ok(self.server.sweep_orphans())
    }

    fn api_version(&self) -> Result<u32, CloudError> {
        Ok(1)
    }
}

// ---------------------------------------------------------------------------
// Persisted client configuration
// ---------------------------------------------------------------------------

/// `~/.hermes-migrator/config.json` — which cloud backend this client uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudConfig {
    pub version: u32,
    /// Remote server base URL, e.g. `https://linminhao.top/hermes-cloud`.
    /// `None` → the local reference backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
}

/// The directory cloud state lives in (`~/.hermes-migrator`).
pub fn cloud_home() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".hermes-migrator"))
}

/// Default local reference-server root (`~/.hermes-migrator/cloud`).
pub fn default_server_root() -> PathBuf {
    cloud_home()
        .map(|h| h.join("cloud"))
        .unwrap_or_else(|| PathBuf::from(".hermes-migrator/cloud"))
}

impl CloudConfig {
    pub fn config_path() -> Option<PathBuf> {
        cloud_home().map(|h| h.join("config.json"))
    }

    /// Load the persisted config; missing file → the default (local).
    pub fn load() -> Result<CloudConfig, crate::MigratorError> {
        match Self::config_path() {
            None => Ok(CloudConfig::default()),
            Some(p) if !p.exists() => Ok(CloudConfig::default()),
            Some(p) => {
                let raw = std::fs::read_to_string(&p)
                    .map_err(|e| crate::MigratorError::Other(format!("reading {p:?}: {e}")))?;
                let c: CloudConfig = serde_json::from_str(&raw).map_err(|e| {
                    crate::MigratorError::Other(format!("malformed config.json: {e}"))
                })?;
                Ok(c)
            }
        }
    }

    pub fn save(&self) -> Result<(), crate::MigratorError> {
        let p = Self::config_path()
            .ok_or_else(|| crate::MigratorError::Other("cannot locate cloud config path".into()))?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::MigratorError::Other(format!("creating config dir: {e}")))?;
        }
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| crate::MigratorError::Other(format!("serializing config: {e}")))?;
        std::fs::write(&p, raw)
            .map_err(|e| crate::MigratorError::Other(format!("writing config.json: {e}")))?;
        Ok(())
    }

    /// True when a remote backend is configured (and looks sane).
    pub fn has_remote(&self) -> bool {
        self.server_url
            .as_deref()
            .is_some_and(|u| u.starts_with("http://") || u.starts_with("https://"))
    }
}

/// Resolve which backend this client should talk to right now:
/// configured remote first, otherwise the local reference store.
pub fn resolve_cloud_store() -> Result<Box<dyn CloudStore>, crate::MigratorError> {
    let cfg = CloudConfig::load()?;
    if cfg.has_remote() {
        if let Some(url) = cfg.server_url.clone() {
            return Ok(Box::new(RemoteBackend::new(url)));
        }
    }
    Ok(Box::new(LocalCloudStore::default()))
}

// ---------------------------------------------------------------------------
// Wire types (shared with hermes-cloud-server)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterReq {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResp {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResp {
    pub meta: Option<StoredConfigMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertReq {
    pub blob: CloudBlob,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobResp {
    pub blob: Option<CloudBlob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountResp {
    pub n: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResp {
    pub api_version: u32,
}

/// Stable error body on non-2xx responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrBody {
    pub code: String,
    #[serde(default)]
    pub message: String,
}

// ---------------------------------------------------------------------------
// Remote backend (HTTP client)
// ---------------------------------------------------------------------------

/// HTTP client for a running `hermes-cloud-server` (any base URL; a reverse
/// proxy path prefix like `/hermes-cloud` is part of `base_url`).
#[derive(Debug, Clone)]
pub struct RemoteBackend {
    /// Base URL including any reverse-proxy prefix, e.g.
    /// `https://linminhao.top/hermes-cloud`. Trailing slashes are trimmed.
    base_url: String,
}

impl RemoteBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Transfer-timeout for a payload of `payload_bytes`: base 60 s plus a
    /// budget assuming at least ~200 KiB/s (cross-border tunnel realistic
    /// worst case), capped at 30 min. Small ops stay at the 60 s base.
    fn transfer_timeout(&self, payload_bytes: u64) -> std::time::Duration {
        let extra = payload_bytes.min(u64::from(u32::MAX)).div_ceil(200 * 1024);
        std::time::Duration::from_secs(60 + extra.min(1740))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Parse a non-2xx response into the stable wire error.
    fn error_from_status(status: u16, body: Option<String>) -> CloudError {
        if let Some(b) = body {
            if let Ok(e) = serde_json::from_str::<ErrBody>(&b) {
                // Trust the parsed code; keep the mapping consistent.
                return CloudError::from_code(&e.code);
            }
        }
        // No parseable body: fall back to the status mapping.
        match status {
            404 => CloudError::ConfigurationNotFound,
            409 => CloudError::ConfigurationAlreadyExists,
            403 => CloudError::ConfigurationOverwriteNotAuthorized,
            413 => CloudError::ConfigurationSizeLimitExceeded,
            401 => CloudError::DeviceNotAuthorized,
            429 => CloudError::RateLimited,
            500 => CloudError::ConfigurationUploadFailed,
            _ => CloudError::Network,
        }
    }

    fn do_req<R>(
        &self,
        method: &str,
        path: &str,
        device: Option<&str>,
        token: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> Result<R, CloudError>
    where
        R: for<'de> Deserialize<'de>,
    {
        self.do_req_timeout(method, path, device, token, body, self.transfer_timeout(0))
    }

    /// Same as [`do_req`] but with an explicit per-request transfer timeout —
    /// used for large blob transfers (upsert / download) where the default
    /// 60 s global would abort a slow cross-border upload.
    fn do_req_timeout<R>(
        &self,
        method: &str,
        path: &str,
        device: Option<&str>,
        token: Option<&str>,
        body: Option<&impl Serialize>,
        timeout: std::time::Duration,
    ) -> Result<R, CloudError>
    where
        R: for<'de> Deserialize<'de>,
    {
        let agent = ureq::builder().timeout(timeout).build();
        let url = self.url(path);
        let mut req = match method {
            "GET" => agent.get(&url),
            _ => agent.post(&url),
        };
        if let Some(d) = device {
            req = req.set("X-HM-Device", d);
        }
        if let Some(t) = token {
            req = req.set("X-HM-Token", t);
        }

        let res = if let Some(b) = body {
            req.send_json(b).map_err(Self::ureq_err)
        } else {
            req.call().map_err(Self::ureq_err)
        }?;
        let out: R = res.into_json().map_err(|_| CloudError::Network)?;
        Ok(out)
    }

    /// Map a `ureq::Error` (status or transport) to the stable wire error.
    fn ureq_err(e: ureq::Error) -> CloudError {
        match e {
            ureq::Error::Status(code, resp) => {
                let body = Self::read_err_body(resp);
                Self::error_from_status(code, body)
            }
            ureq::Error::Transport(_) => CloudError::Network,
        }
    }

    /// Read a non-2xx response body (bounded to 4 KiB) for error mapping.
    fn read_err_body(resp: ureq::Response) -> Option<String> {
        use std::io::Read;
        let mut s = String::new();
        let _ = resp.into_reader().read_to_string(&mut s);
        if s.len() <= 4096 {
            Some(s)
        } else {
            None
        }
    }

    /// Register the device, returning the per-device bearer token.
    pub fn register(&self, device_id: &DeviceId) -> Result<String, CloudError> {
        let r: RegisterResp = self.do_req(
            "POST",
            "/v1/register",
            Some(&device_id.0),
            None,
            Some(&RegisterReq {
                device_id: device_id.0.clone(),
            }),
        )?;
        Ok(r.token)
    }

    pub fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError> {
        let r: MetaResp = self.do_req(
            "POST",
            "/v1/has-config",
            Some(device_id),
            Some(token),
            None::<&()>,
        )?;
        Ok(r.meta)
    }

    pub fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        // The base64 payload dominates the wire size; give the transfer a
        // budget scaled to it (cross-border CF tunnels can be slow).
        let timeout = self.transfer_timeout(blob.payload.len() as u64);
        let r: MetaResp = self.do_req_timeout(
            "POST",
            "/v1/upsert",
            Some(device_id),
            Some(token),
            Some(&UpsertReq {
                blob: blob.clone(),
                overwrite,
            }),
            timeout,
        )?;
        r.meta.ok_or(CloudError::ConfigurationUploadFailed)
    }

    pub fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError> {
        // Response size unknown up front; use the capped (30 min) budget.
        let r: BlobResp = self.do_req_timeout(
            "POST",
            "/v1/download",
            Some(device_id),
            Some(token),
            Some(&DownloadReq {
                expected_sha: expected_sha.map(str::to_string),
            }),
            self.transfer_timeout(u64::MAX),
        )?;
        Ok(r.blob)
    }

    pub fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        let r: CountResp = self.do_req(
            "POST",
            "/v1/delete",
            Some(device_id),
            Some(token),
            None::<&()>,
        )?;
        Ok(r.n)
    }

    /// Trigger the server's 24 h orphan-cleanup job.
    pub fn sweep_orphans(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        let r: CountResp = self.do_req(
            "POST",
            "/v1/sweep-orphans",
            Some(device_id),
            Some(token),
            None::<&()>,
        )?;
        Ok(r.n)
    }

    /// Connection/identity test for the Settings screen.
    pub fn api_version(&self) -> Result<u32, CloudError> {
        let h: HealthResp = self.do_req("GET", "/v1/health", None, None, None::<&()>)?;
        Ok(h.api_version)
    }
}

impl CloudStore for RemoteBackend {
    fn register(&self, device_id: &DeviceId) -> Result<String, CloudError> {
        RemoteBackend::register(self, device_id)
    }

    fn has_configuration(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<Option<StoredConfigMeta>, CloudError> {
        RemoteBackend::has_configuration(self, device_id, token)
    }

    fn upsert_configuration(
        &self,
        device_id: &str,
        token: &str,
        blob: &CloudBlob,
        overwrite: bool,
    ) -> Result<StoredConfigMeta, CloudError> {
        RemoteBackend::upsert_configuration(self, device_id, token, blob, overwrite)
    }

    fn download_configuration(
        &self,
        device_id: &str,
        token: &str,
        expected_sha: Option<&str>,
    ) -> Result<Option<CloudBlob>, CloudError> {
        RemoteBackend::download_configuration(self, device_id, token, expected_sha)
    }

    fn delete_configuration(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        RemoteBackend::delete_configuration(self, device_id, token)
    }

    fn sweep_orphans(&self, device_id: &str, token: &str) -> Result<u32, CloudError> {
        RemoteBackend::sweep_orphans(self, device_id, token)
    }

    fn api_version(&self) -> Result<u32, CloudError> {
        RemoteBackend::api_version(self)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_local_default() {
        let c = CloudConfig::default();
        assert!(!c.has_remote());
        assert_eq!(c.server_url, None);
    }

    #[test]
    fn config_rejects_non_http_urls() {
        let mut c = CloudConfig::default();
        c.server_url = Some("ftp://nope".into());
        assert!(!c.has_remote());
        c.server_url = Some("https://linminhao.top/hermes-cloud".into());
        assert!(c.has_remote());
    }

    #[test]
    fn error_from_status_prefers_body_code() {
        let body =
            Some(r#"{"code":"CONFIGURATION_ALREADY_EXISTS","message":"a config exists"}"#.into());
        assert_eq!(
            RemoteBackend::error_from_status(409, body),
            CloudError::ConfigurationAlreadyExists
        );
    }

    #[test]
    fn error_from_status_falls_back_to_status_code() {
        assert_eq!(
            RemoteBackend::error_from_status(429, None),
            CloudError::RateLimited
        );
        assert_eq!(
            RemoteBackend::error_from_status(500, Some("plain text".into())),
            CloudError::ConfigurationUploadFailed
        );
    }

    #[test]
    fn url_building_trims_trailing_slash() {
        let b = RemoteBackend::new("https://x.example/hermes-cloud/");
        assert_eq!(
            b.url("/v1/health"),
            "https://x.example/hermes-cloud/v1/health"
        );
    }

    #[test]
    fn local_store_matches_file_server_behaviour() {
        use crate::cloud::encrypt_configuration;
        let dir = std::env::temp_dir().join(format!("hm-remote-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = LocalCloudStore::new(&dir);
        let d = DeviceId::generate();
        let tok = store.register(&d).unwrap();
        assert!(store.has_configuration(&d.0, &tok).unwrap().is_none());
        let blob = encrypt_configuration("s", b"payload").unwrap();
        let meta = store
            .upsert_configuration(&d.0, &tok, &blob, false)
            .unwrap();
        assert!(store.has_configuration(&d.0, &tok).unwrap().is_some());
        let got = store
            .download_configuration(&d.0, &tok, Some(&meta.sha256))
            .unwrap();
        assert_eq!(got.as_ref().unwrap(), &blob);
        assert_eq!(store.delete_configuration(&d.0, &tok).unwrap(), 1);
        let _ = store.sweep_orphans(&d.0, &tok);
        assert_eq!(store.api_version().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
