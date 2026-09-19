//! `cloud` subcommands for the CLI.
//!
//! Uses the same local reference server (`FileServer`) the GUI talks to, so
//! the one-device / one-configuration contract is exercised identically
//! without a live deployment. Device id + server root live under
//! `~/.hermes-migrator/`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use migrator_core::cloud::{decrypt_configuration, encrypt_configuration, DeviceId};
use migrator_core::cloudserver::FileServer;
use migrator_core::pack::{self, PackOptions};
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

#[derive(Args)]
pub struct StatusArgs {
    /// Reference-server root (default: ~/.hermes-migrator/cloud).
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct UploadArgs {
    #[arg(long)]
    root: Option<PathBuf>,
    /// Passphrase that encrypts the configuration client-side.
    #[arg(long)]
    passphrase: Option<String>,
    /// Overwrite the existing configuration (requires passphrase).
    #[arg(long)]
    overwrite: bool,
}

#[derive(Args)]
pub struct RestoreArgs {
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
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct TestArgs {
    #[arg(long)]
    root: Option<PathBuf>,
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

pub(crate) fn dispatch(cmd: &CloudCmd) -> Result<()> {
    match cmd {
        CloudCmd::Status(StatusArgs { root }) => {
            let r = pick_root(root);
            let id = load_device_id()?;
            let server = FileServer::new(&r);
            let token = server
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let meta = server
                .has_configuration(&id.0, &token)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            println!("device: {}", mask(&id.0));
            match meta {
                Some(m) => {
                    println!(
                        "cloud configuration: {} bytes, sha256 {}, uploaded {}",
                        m.size_bytes,
                        m.sha256,
                        fmt_ms(m.uploaded_at)
                    );
                }
                None => println!("cloud configuration: none"),
            }
            Ok(())
        }

        CloudCmd::Upload(UploadArgs {
            root,
            passphrase,
            overwrite,
        }) => {
            let r = pick_root(root);
            let id = load_device_id()?;
            let server = FileServer::new(&r);
            let token = server
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
            let out = r.join("staging.hermesmig");
            let mut progress: pack::ProgressFn = Box::new(|stage, pct, done, total| {
                eprintln!("{stage:40} {pct:3}%  [{done}/{total}]");
            });
            pack::pack(&plan, &opts, &out, &mut progress)?;
            let raw = std::fs::read(&out)?;
            let blob = encrypt_configuration(&ph, &raw)?;
            let _ = std::fs::remove_file(&out);

            let meta = server
                .upsert_configuration(&id.0, &token, &blob, *overwrite)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            println!(
                "uploaded: {} bytes, sha256 {} (overwrite={})",
                meta.size_bytes, meta.sha256, *overwrite
            );
            Ok(())
        }

        CloudCmd::Restore(RestoreArgs {
            root,
            passphrase,
            target,
        }) => {
            let r = pick_root(root);
            let id = load_device_id()?;
            let server = FileServer::new(&r);
            let token = server
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let blob = server
                .download_configuration(&id.0, &token, None)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?
                .context("no cloud configuration on this device")?;
            let ph = passphrase
                .clone()
                .or_else(prompt_passphrase_opt)
                .context("a passphrase is required to decrypt the cloud configuration")?;
            let raw = decrypt_configuration(&ph, &blob)?;
            let pkg = r.join("restore.hermesmig");
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

        CloudCmd::Delete(DeleteArgs { root }) => {
            let r = pick_root(root);
            let id = load_device_id()?;
            let server = FileServer::new(&r);
            let token = server
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let n = server
                .delete_configuration(&id.0, &token)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let _ = server.sweep_orphans();
            println!("deleted: {n} object(s); orphans swept for 24 h cleanup");
            Ok(())
        }

        CloudCmd::Test(TestArgs { root }) => {
            let r = pick_root(root);
            let id = load_device_id()?;
            let t0 = std::time::Instant::now();
            let server = FileServer::new(&r);
            server
                .register(&id)
                .map_err(|e| anyhow::anyhow!("{}", e.code()))?;
            let lat = t0.elapsed().as_millis() as u32;
            println!("reachable: true");
            println!("api version: 1");
            println!("latency: {lat} ms");
            println!("device: {}", mask(&id.0));
            Ok(())
        }
    }
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
