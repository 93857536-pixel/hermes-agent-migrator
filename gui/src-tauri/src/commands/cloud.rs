//! Cloud configuration Tauri commands.
//!
//! The backend is resolved per command: a remote server configured in
//! `~/.hermes-migrator/config.json` (via `cloud_server_set` / Settings →
//! Cloud) → HTTP; otherwise the local reference backend rooted at
//! `~/.hermes-migrator/cloud` (the deployment-ready server API it
//! implements is documented in `docs/cloud-alibaba.md`). The device
//! identity is a random UUIDv4 generated on first cloud use and stored in
//! `~/.hermes-migrator/device.json`.

use std::path::PathBuf;

use migrator_core::cloud::{decrypt_configuration, encrypt_configuration, DeviceId};
use migrator_core::pack::{self, PackOptions};
use migrator_core::remote::{CloudConfig, CloudStore, LocalCloudStore, RemoteBackend};
use migrator_core::scan;
use serde::Serialize;

use super::progress_fn;

pub fn cloud_root() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("could not locate home directory")?;
    Ok(home.join(".hermes-migrator"))
}

fn device_path() -> Result<PathBuf, String> {
    Ok(cloud_root()?.join("device.json"))
}

fn server_root() -> Result<PathBuf, String> {
    let root = cloud_root()?.join("cloud");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(root)
}

/// Load or create the persisted device id (random UUIDv4, not a hardware
/// fingerprint; stable for the life of this app install).
pub fn load_device_id() -> Result<DeviceId, String> {
    let p = device_path()?;
    if p.exists() {
        let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if let Some(s) = v.get("device_id") {
            let s: &str = s.as_str().ok_or("malformed device.json")?;
            DeviceId::validate(s).map_err(|e| e.to_string())?;
            return Ok(DeviceId(s.to_string()));
        }
    }
    let id = DeviceId::generate();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::json!({
        "version": 1,
        "device_id": id.0,
        "created_at": chrono_lite_now_ms(),
    });
    std::fs::write(&p, body.to_string()).map_err(|e| e.to_string())?;
    Ok(id)
}

fn chrono_lite_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The backend this GUI is talking to: remote (configured) or local.
fn resolve_store() -> Result<Box<dyn CloudStore>, String> {
    let cfg = CloudConfig::load().map_err(|e| e.to_string())?;
    if cfg.has_remote() {
        if let Some(url) = cfg.server_url.clone() {
            return Ok(Box::new(RemoteBackend::new(url)));
        }
    }
    Ok(Box::new(LocalCloudStore::new(server_root()?)))
}

fn register(store: &dyn CloudStore, id: &DeviceId) -> Result<String, String> {
    store.register(id).map_err(|e| e.code().to_string())
}

/// Connection/identity test for the Settings → Cloud screen: returns
/// reachability, API version, latency and the (masked) device id. Never
/// returns full server credentials or internal IPs in UI-facing fields.
#[derive(Debug, Clone, Serialize)]
pub struct ConnTest {
    pub reachable: bool,
    pub api_version: u32,
    pub latency_ms: u32,
    pub device_id_masked: String,
    /// Which backend the test hit: `local` or the remote base URL.
    pub backend: String,
}

fn backend_label() -> String {
    if let Ok(cfg) = CloudConfig::load() {
        if let Some(u) = cfg.server_url {
            return format!("remote ({u})");
        }
    }
    "local".to_string()
}

#[tauri::command]
pub fn cloud_test_connection() -> Result<ConnTest, String> {
    let t0 = std::time::Instant::now();
    let id = load_device_id()?;
    let store = resolve_store()?;
    let ver = store.api_version().map_err(|e| e.code().to_string())?;
    let _ = register(store.as_ref(), &id)?;
    Ok(ConnTest {
        reachable: true,
        api_version: ver,
        latency_ms: t0.elapsed().as_millis() as u32,
        device_id_masked: mask_device_id(&id.0),
        backend: backend_label(),
    })
}

pub fn mask_device_id(id: &str) -> String {
    if id.len() <= 12 {
        return id.to_string();
    }
    format!("{}…{}", &id[..8], &id[id.len() - 4..])
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudStatus {
    pub device_id_masked: String,
    pub has_configuration: bool,
    pub sha256: Option<String>,
    pub size_bytes: u64,
    pub uploaded_at_ms: u64,
}

#[tauri::command]
pub fn cloud_status() -> Result<CloudStatus, String> {
    let id = load_device_id()?;
    let store = resolve_store()?;
    let token = register(store.as_ref(), &id)?;
    let meta = store
        .has_configuration(&id.0, &token)
        .map_err(|e| e.code().to_string())?;
    Ok(CloudStatus {
        device_id_masked: mask_device_id(&id.0),
        has_configuration: meta.is_some(),
        sha256: meta.as_ref().map(|m| m.sha256.clone()),
        size_bytes: meta.as_ref().map_or(0, |m| m.size_bytes),
        uploaded_at_ms: meta.as_ref().map_or(0, |m| m.uploaded_at),
    })
}

/// Upload the current Hermes configuration to the cloud (CREATE when absent,
/// OVERWRITE when present and `overwrite` is set). The package is built
/// exactly like a local pack, encrypted client-side with the user's
/// passphrase, and only then handed to the backend.
#[tauri::command]
pub fn cloud_upload(
    app: tauri::AppHandle,
    passphrase: String,
    overwrite: bool,
) -> Result<CloudStatus, String> {
    let id = load_device_id()?;
    let store = resolve_store()?;
    let token = register(store.as_ref(), &id)?;

    // Build the same configuration payload a local pack would produce.
    let home = scan::locate_hermes_home().map_err(|e| e.to_string())?;
    let report = scan::scan_hermes_root(&home);
    let opts = PackOptions::default();
    let plan = pack::plan_pack(&report, &opts);
    let out_dir = cloud_root()?.join("staging");
    let _ = std::fs::create_dir_all(&out_dir);
    let pkg = out_dir.join("cloud-staging.hermesmig");
    {
        let mut progress = progress_fn(&app);
        let _manifest = pack::pack(&plan, &opts, &pkg, &mut progress).map_err(|e| e.to_string())?;
    }
    let raw = std::fs::read(&pkg).map_err(|e| e.to_string())?;
    let blob = encrypt_configuration(&passphrase, &raw).map_err(|e| e.to_string())?;

    let meta = store
        .upsert_configuration(&id.0, &token, &blob, overwrite)
        .map_err(|e| e.code().to_string())?;

    let _ = std::fs::remove_file(&pkg);
    Ok(CloudStatus {
        device_id_masked: mask_device_id(&id.0),
        has_configuration: true,
        sha256: Some(meta.sha256.clone()),
        size_bytes: meta.size_bytes,
        uploaded_at_ms: meta.uploaded_at,
    })
}

/// Download & restore: pull THE blob, decrypt with the passphrase, write it
/// to `out_path`, then run the same restore pipeline as a local file.
#[derive(Debug, Clone, Serialize)]
pub struct CloudRestoreDone {
    pub package_path: String,
    pub report: migrator_core::restore::RestoreReport,
}

#[tauri::command]
pub fn cloud_download_restore(
    app: tauri::AppHandle,
    passphrase: String,
    out_path: String,
) -> Result<CloudRestoreDone, String> {
    let id = load_device_id()?;
    let store = resolve_store()?;
    let token = register(store.as_ref(), &id)?;
    let blob = store
        .download_configuration(&id.0, &token, None)
        .map_err(|e| e.code().to_string())?
        .ok_or("no cloud configuration on this device")?;
    let raw = decrypt_configuration(&passphrase, &blob).map_err(|e| e.to_string())?;
    let out = PathBuf::from(out_path);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, raw).map_err(|e| e.to_string())?;
    let mut progress = progress_fn(&app);
    let report = migrator_core::restore::RestoreOptions {
        passphrase: Some(passphrase),
        ..Default::default()
    };
    let (_m, r) =
        migrator_core::restore::restore(&out, &report, &mut progress).map_err(|e| e.to_string())?;
    Ok(CloudRestoreDone {
        package_path: out.to_string_lossy().into_owned(),
        report: r,
    })
}

/// Delete THE configuration (server schedules the blob for the 24 h orphan
/// cleanup; the device may upload again afterwards).
#[tauri::command]
pub fn cloud_delete() -> Result<CloudStatus, String> {
    let id = load_device_id()?;
    let store = resolve_store()?;
    let token = register(store.as_ref(), &id)?;
    let _n = store
        .delete_configuration(&id.0, &token)
        .map_err(|e| e.code().to_string())?;
    let _ = store.sweep_orphans(&id.0, &token);
    let meta = store
        .has_configuration(&id.0, &token)
        .map_err(|e| e.code().to_string())?;
    Ok(CloudStatus {
        device_id_masked: mask_device_id(&id.0),
        has_configuration: meta.is_some(),
        sha256: meta.as_ref().map(|m| m.sha256.clone()),
        size_bytes: meta.as_ref().map_or(0, |m| m.size_bytes),
        uploaded_at_ms: meta.as_ref().map_or(0, |m| m.uploaded_at),
    })
}

// ---- backend management (Settings → Cloud) ---------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CloudBackendInfo {
    pub server_url: Option<String>,
    pub using_remote: bool,
}

#[tauri::command]
pub fn cloud_server_info() -> Result<CloudBackendInfo, String> {
    let cfg = CloudConfig::load().map_err(|e| e.to_string())?;
    Ok(CloudBackendInfo {
        server_url: cfg.server_url.clone(),
        using_remote: cfg.has_remote(),
    })
}

/// Point this device at a remote cloud server (persisted to
/// `~/.hermes-migrator/config.json`; subsequent cloud commands go remote).
#[tauri::command]
pub fn cloud_server_set(url: String) -> Result<CloudBackendInfo, String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("server URL must start with http:// or https://".into());
    }
    let mut cfg = CloudConfig::load().map_err(|e| e.to_string())?;
    cfg.server_url = Some(u.to_string());
    cfg.save().map_err(|e| e.to_string())?;
    Ok(CloudBackendInfo {
        server_url: cfg.server_url.clone(),
        using_remote: true,
    })
}

#[tauri::command]
pub fn cloud_server_clear() -> Result<CloudBackendInfo, String> {
    let mut cfg = CloudConfig::load().map_err(|e| e.to_string())?;
    cfg.server_url = None;
    cfg.save().map_err(|e| e.to_string())?;
    Ok(CloudBackendInfo {
        server_url: None,
        using_remote: false,
    })
}

/// Run the 24 h orphan-cleanup job (local backend; remote backends expose
/// the same via `sweep_orphans`). Exposed so operators/tests can trigger
/// it; a real deployment runs it on a server timer.
#[tauri::command]
pub fn cloud_sweep_orphans() -> Result<u32, String> {
    let store = resolve_store()?;
    let id = load_device_id()?;
    let token = register(store.as_ref(), &id)?;
    store
        .sweep_orphans(&id.0, &token)
        .map_err(|e| e.code().to_string())
}
