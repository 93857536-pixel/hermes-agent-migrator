//! `cloud` subcommands for the CLI.
//!
//! Backends are resolved at command time ([`resolve_store`]):
//!   - an explicit `--server <url>` flag → remote;
//!   - otherwise a persisted remote in `~/.hermes-migrator/config.json`
//!     (set via `cloud server set`) → remote;
//!   - otherwise the local reference backend (`~/.hermes-migrator/cloud`).
//!
//! Device id + (optionally) the remote server live under `~/.hermes-migrator/`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use migrator_core::cloud::{decrypt_configuration, encrypt_configuration, DeviceId};
use migrator_core::pack::{self, PackOptions};
use migrator_core::remote::{CloudConfig, CloudStore, LocalCloudStore, RemoteBackend};
use migrator_core::restore::{self, RestoreOptions};
use migrator_core::scan;

pub(crate) fn cloud_root() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not locate home directory")?;
    Ok(home.join(".hermes-migrator"))
}

fn load_device_id() -> Result<DeviceId> {
    let p = cloud_root()?.join("device.json");
    if p.exists() {
        let raw = std::fs::read_to_string(&p).context("reading device.json")?;
        let v: serde_json::Value = serde_json::from_str(&raw).context("malformed device.json")?;
        let s = v
            .get("device_id")
            .and_then(|x| x.as_str())
            .context("device.json missing device_id")?;
        DeviceId::validate(s)?;
        return Ok(DeviceId(s.to_string()));
    }
    let id = DeviceId::generate();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({
        "version": 1,
        "device_id": id.0,
        "created_at": now_ms(),
    });
    std::fs::write(&p, body.to_string())?;
    Ok(id)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn mask(id: &str) -> String {
    if id.len() <= 12 {
        return id.to_string();
    }
    format!("{}…{}", &id[..8], &id[id.len() - 4..])
}

/// Which backend to talk to for a command: an explicit `--server` flag wins,
/// then the persisted remote (if any), then the local reference store.
fn resolve_store(
    server_url: &Option<String>,
    root: &Option<PathBuf>,
) -> Result<Box<dyn CloudStore>> {
    if let Some(u) = server_url.as_deref().filter(|s| !s.is_empty()) {
        return Ok(Box::new(RemoteBackend::new(u)));
    }
    let cfg = CloudConfig::load()?;
    if cfg.has_remote() {
        if let Some(u) = cfg.server_url.clone() {
            return Ok(Box::new(RemoteBackend::new(u)));
        }
    }
    let r = pick_root(root);
    Ok(Box::new(LocalCloudStore::new(r)))
}

fn pick_root(flag: &Option<PathBuf>) -> PathBuf {
    flag.clone().unwrap_or_else(server_root_root)
}

fn server_root_root() -> PathBuf {
    // Best-effort default; errors surface at use time.
    dirs::home_dir()
        .map(|h| h.join(".hermes-migrator").join("cloud"))
        .unwrap_or_else(|| PathBuf::from(".hermes-migrator/cloud"))
}

#[derive(Args)]
pub struct StatusArgs {
    /// Explicit remote server base URL (overrides the saved config).
    #[arg(long = "server")]
    server_url: Option<String>,
    /// Reference-server root for the LOCAL backend (default:
    /// ~/.hermes-migrator/cloud). Ignored when a remote is in use.
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct UploadArgs {
    #[arg(long = "server")]
    server_url: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
    /// Passphrase that encrypts the configuration client-side.
    #[arg(long)]
    passphrase: Option<String>,
    /// Overwrite the existing configuration.
    #[arg(long)]
    overwrite: bool,
}

#[derive(Args)]
pub struct RestoreArgs {
    #[arg(long = "server")]
    server_url: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    passphrase: Option<String>,
    /// Restore into a specific hermes home (default: this machine's).
    #[arg(long)]
    target: Option<PathBuf>,
}

#[derive(Args)]
pub struct DeleteArgs {
    #[arg(long = "server")]
    server_url: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct TestArgs {
    #[arg(long = "server")]
    server_url: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
}

// ---- `cloud server` (manage the persisted backend) ------------------------

#[derive(Args)]
pub struct ServerSetArgs {
    /// Remote server base URL, e.g. `https://linminhao.top/hermes-cloud`.
    url: String,
}

#[derive(clap::Subcommand)]
pub enum ServerCmd {
    /// Point this device at a remote cloud server.
    Set(ServerSetArgs),
    /// Show the currently configured backend.
    Show,
    /// Remove the configured remote server (fall back to local).
    Clear,
}

#[derive(clap::Subcommand)]
pub enum CloudCmd {
    /// Show this device's cloud configuration status.
    Status(StatusArgs),
    /// Upload (or overwrite) the current configuration to the cloud.
    Upload(UploadArgs),
    /// Download THE configuration and restore it onto this machine.
    Restore(RestoreArgs),
    /// Delete THIS device's configuration.
    Delete(DeleteArgs),
    /// Test the connection / device identity.
    Test(TestArgs),
    /// Configure which cloud backend this device uses.
    #[command(subcommand)]
    Server(ServerCmd),
}

pub(crate) fn dispatch(cmd: &CloudCmd) -> Result<()> {
    match cmd {
        CloudCmd::Status(StatusArgs { server_url, root }) => {
            let store = resolve_store(server_url, root)?;
            let id = load_device_id()?;
            let token = store
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let meta = store
                .has_configuration(&id.0, &token)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            println!("backend:  {}", backend_label(server_url, root));
            println!("device:   {}", mask(&id.0));
            match meta {
                Some(m) => println!(
                    "cloud configuration: {} bytes, sha256 {}, uploaded {}",
                    m.size_bytes,
                    m.sha256,
                    fmt_ms(m.uploaded_at)
                ),
                None => println!("cloud configuration: none"),
            }
            Ok(())
        }

        CloudCmd::Upload(UploadArgs {
            server_url,
            root,
            passphrase,
            overwrite,
        }) => {
            let store = resolve_store(server_url, root)?;
            let id = load_device_id()?;
            let token = store
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let ph = passphrase
                .clone()
                .or_else(prompt_passphrase_opt)
                .context("a passphrase is required to encrypt the cloud configuration")?;

            let home = scan::locate_hermes_home()?;
            let report = scan::scan_hermes_root(&home);
            let opts = PackOptions::default();
            let plan = pack::plan_pack(&report, &opts);
            let out = cloud_root()?.join("staging.hermesmig");
            let mut progress: pack::ProgressFn = Box::new(|stage, pct, done, total| {
                eprintln!("{stage:40} {pct:3}%  [{done}/{total}]");
            });
            pack::pack(&plan, &opts, &out, &mut progress)?;
            let raw = std::fs::read(&out)?;
            let blob = encrypt_configuration(&ph, &raw)?;
            let _ = std::fs::remove_file(&out);

            let meta = store
                .upsert_configuration(&id.0, &token, &blob, *overwrite)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            println!("backend:  {}", backend_label(server_url, root));
            println!(
                "uploaded: {} bytes, sha256 {} (overwrite={})",
                meta.size_bytes, meta.sha256, *overwrite
            );
            Ok(())
        }

        CloudCmd::Restore(RestoreArgs {
            server_url,
            root,
            passphrase,
            target,
        }) => {
            let store = resolve_store(server_url, root)?;
            let id = load_device_id()?;
            let token = store
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let blob = store
                .download_configuration(&id.0, &token, None)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?
                .context("no cloud configuration on this device")?;
            let ph = passphrase
                .clone()
                .or_else(prompt_passphrase_opt)
                .context("a passphrase is required to decrypt the cloud configuration")?;
            let raw = decrypt_configuration(&ph, &blob)?;
            let pkg = cloud_root()?.join("restore.hermesmig");
            std::fs::write(&pkg, &raw)?;
            let opts = RestoreOptions {
                passphrase: Some(ph),
                target_hermes_home: target.clone(),
                only_components: None,
                apply_path_repair: true,
            };
            let mut progress: pack::ProgressFn = Box::new(|stage, pct, done, total| {
                eprintln!("{stage:40} {pct:3}%  [{done}/{total}]")
            });
            let (_m, rep) = restore::restore(&pkg, &opts, &mut progress)?;
            let _ = std::fs::remove_file(&pkg);
            for (name, ok) in &rep.verified {
                println!("  {} {name}", if *ok { "✓" } else { "✗" });
            }
            if rep.secrets_restored > 0 {
                println!("  ✓ {} secret(s) restored", rep.secrets_restored);
            }
            Ok(())
        }

        CloudCmd::Delete(DeleteArgs { server_url, root }) => {
            let store = resolve_store(server_url, root)?;
            let id = load_device_id()?;
            let token = store
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let n = store
                .delete_configuration(&id.0, &token)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let _ = store.sweep_orphans(&id.0, &token);
            println!("deleted: {n} object(s); orphans scheduled for 24 h cleanup");
            Ok(())
        }

        CloudCmd::Test(TestArgs { server_url, root }) => {
            let store = resolve_store(server_url, root)?;
            let id = load_device_id()?;
            let t0 = std::time::Instant::now();
            let ver = store
                .api_version()
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let _ = store
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let lat = t0.elapsed().as_millis() as u32;
            println!("reachable: true");
            println!("backend:   {}", backend_label(server_url, root));
            println!("api version: {ver}");
            println!("latency: {lat} ms");
            println!("device:    {}", mask(&id.0));
            Ok(())
        }

        CloudCmd::Server(ServerCmd::Set(a)) => {
            let u = a.url.trim();
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                anyhow::bail!("server URL must start with http:// or https://");
            }
            let mut cfg = CloudConfig::load()?;
            cfg.server_url = Some(u.to_string());
            cfg.save()?;
            println!("cloud server set to: {u}");
            println!(
                "subsequent `cloud` commands now use the remote backend (until `cloud server clear`)."
            );
            Ok(())
        }

        CloudCmd::Server(ServerCmd::Show) => {
            let cfg = CloudConfig::load()?;
            match cfg.server_url.clone() {
                Some(u) => println!("cloud server: {u}"),
                None => println!("cloud server: (none — using the local reference backend)"),
            }
            Ok(())
        }

        CloudCmd::Server(ServerCmd::Clear) => {
            let mut cfg = CloudConfig::load()?;
            cfg.server_url = None;
            cfg.save()?;
            println!("cloud server cleared; using the local reference backend.");
            Ok(())
        }
    }
}

/// Human label of which backend a command is targeting (for the user).
fn backend_label(server_url: &Option<String>, _root: &Option<PathBuf>) -> String {
    if let Some(u) = server_url.as_deref().filter(|s| !s.is_empty()) {
        return format!("remote ({u})");
    }
    if let Ok(cfg) = CloudConfig::load() {
        if let Some(u) = cfg.server_url {
            return format!("remote ({u})");
        }
    }
    "local".to_string()
}

fn prompt_passphrase_opt() -> Option<String> {
    eprint!("cloud passphrase: ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let secs = ms / 1000;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h}h {m:02}m {s:02}s")
}
