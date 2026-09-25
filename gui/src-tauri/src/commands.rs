//! Tauri command layer for Hermes Agent Migrator.
//!
//! Thin, typed wrappers over `migrator-core`. Long-running pack/restore work
//! runs synchronously in the Tauri command thread pool (commands here are
//! sync functions, so Tauri already moves them off the main thread). Progress
//! flows back to the UI as `migrator://progress` events.

use std::path::PathBuf;
use std::sync::mpsc;

use migrator_core::pack::{self, PackOptions};
use migrator_core::restore::{self, RestoreOptions};
use migrator_core::scan;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub mod cloud;
pub mod installer;

/// Progress event payload. Mirrors `ProgressEvt` on the frontend.
#[derive(Debug, Clone, Serialize)]
struct ProgressEvt {
    stage: String,
    pct: u8,
    done: u64,
    total: u64,
}

/// Result returned after a successful pack.
#[derive(Debug, Clone, Serialize)]
pub struct PackDone {
    pub out_path: String,
    pub manifest_summary: String,
}

/// Result returned after a successful restore.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreDone {
    pub report: restore::RestoreReport,
}

/// Bridge an `mpsc` channel to Tauri progress events. The core engine expects
/// a `&mut Box<dyn FnMut(&str, u8, u64, u64)>`; we hand it a sender that
/// forwards into a channel consumed by a spawned emitter thread.
pub(crate) fn progress_fn(app: &AppHandle) -> pack::ProgressFn {
    let (tx, rx) = mpsc::channel::<ProgressEvt>();
    let app2 = app.clone();
    std::thread::spawn(move || {
        for evt in rx {
            let _ = app2.emit("migrator://progress", &evt);
        }
    });
    Box::new(move |stage: &str, pct: u8, done: u64, total: u64| {
        let _ = tx.send(ProgressEvt {
            stage: stage.to_string(),
            pct,
            done,
            total,
        });
    })
}

/// Locate and scan this machine's Hermes home.
#[tauri::command]
pub fn scan() -> Result<scan::ScanReport, String> {
    let home = scan::locate_hermes_home().map_err(|e| e.to_string())?;
    Ok(scan::scan_hermes_root(&home))
}

/// Build a migration package at `out_path`.
#[tauri::command]
pub fn pack(app: AppHandle, opts: PackOptions, out_path: String) -> Result<PackDone, String> {
    let out = PathBuf::from(out_path);
    let home = scan::locate_hermes_home().map_err(|e| e.to_string())?;
    let report = scan::scan_hermes_root(&home);
    let plan = pack::plan_pack(&report, &opts);
    let mut progress = progress_fn(&app);
    let manifest = pack::pack(&plan, &opts, &out, &mut progress).map_err(|e| e.to_string())?;

    let summary = format!(
        "{} component(s), {} entry, created {} for {} / {}",
        manifest.components.len(),
        manifest.checksums.len(),
        manifest.hermes_version,
        manifest.platform,
        manifest.architecture
    );
    Ok(PackDone {
        out_path: out.display().to_string(),
        manifest_summary: summary,
    })
}

/// Run the pre-restore environment check against a package.
#[tauri::command]
pub fn check_environment(pkg_path: String) -> Result<restore::EnvCheck, String> {
    let pkg = PathBuf::from(pkg_path);
    restore::check_environment(&pkg).map_err(|e| e.to_string())
}

/// Restore a package onto this machine.
#[tauri::command]
pub fn restore(
    app: AppHandle,
    pkg_path: String,
    opts: RestoreOptions,
) -> Result<RestoreDone, String> {
    let pkg = PathBuf::from(pkg_path);
    let mut progress = progress_fn(&app);
    let (_manifest, report) =
        restore::restore(&pkg, &opts, &mut progress).map_err(|e| e.to_string())?;
    Ok(RestoreDone { report })
}

/// Integrity-verify a package's checksums; returns the list of mismatches.
#[tauri::command]
pub fn verify_package(pkg_path: String) -> Result<Vec<String>, String> {
    let pkg = PathBuf::from(pkg_path);
    let (_manifest, failures) = pack::verify_package(&pkg).map_err(|e| e.to_string())?;
    Ok(failures)
}

/// Back up the current Hermes home to a timestamped directory.
#[tauri::command]
pub fn backup_current_hermes() -> Result<String, String> {
    let home = scan::locate_hermes_home().map_err(|e| e.to_string())?;
    let dest = restore::backup_hermes_home(&home).map_err(|e| e.to_string())?;
    Ok(dest.display().to_string())
}
