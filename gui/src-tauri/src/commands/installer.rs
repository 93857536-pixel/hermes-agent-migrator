//! Tauri commands for automatic Hermes Agent installation.
//!
//! * [`installer_detect_system`] — read-only host snapshot for the Setup
//!   screen (OS / arch / existing install / tooling / free space).
//! * [`installer_install`] — runs the official installer in non-interactive
//!   mode, streaming each output line to the frontend as an
//!   `installer://log` event, then reports post-install detection.

use std::sync::mpsc;

use migrator_core::installer::{self, InstallOptions};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// System snapshot returned to the Setup screen.
#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub hermes_installed: bool,
    pub hermes_home: Option<String>,
    pub hermes_version: Option<String>,
    pub git_available: bool,
    pub python_available: bool,
    pub curl_available: bool,
    pub free_space_bytes: Option<u64>,
}

/// Payload of one streamed installer output line.
#[derive(Debug, Clone, Serialize)]
pub struct InstallerLogLine {
    pub line: String,
}

/// Result returned after the installer exits.
#[derive(Debug, Clone, Serialize)]
pub struct InstallDone {
    pub success: bool,
    pub installer: String,
    pub hermes_installed_now: bool,
    pub hermes_home: Option<String>,
    pub hermes_version: Option<String>,
    pub log_tail: Vec<String>,
}

/// Read-only snapshot of the host system.
#[tauri::command]
pub fn installer_detect_system() -> SystemInfo {
    let i = installer::detect();
    SystemInfo {
        os: i.os,
        arch: i.arch,
        hermes_installed: i.hermes_installed,
        hermes_home: i.hermes_home,
        hermes_version: i.hermes_version,
        git_available: i.git_available,
        python_available: i.python_available,
        curl_available: i.curl_available,
        free_space_bytes: i.free_space_bytes,
    }
}

/// Run the official Hermes installer (non-interactive), streaming its
/// output to `installer://log` events. Synchronous: Tauri moves sync
/// commands onto the command thread pool, so the UI stays responsive.
#[tauri::command]
pub fn installer_install(app: AppHandle, opts: InstallOptions) -> Result<InstallDone, String> {
    let (tx, rx) = mpsc::channel::<String>();
    let app2 = app.clone();
    std::thread::spawn(move || {
        for line in rx {
            let _ = app2.emit("installer://log", &InstallerLogLine { line });
        }
    });

    let mut on_log = |line: &str| {
        let _ = tx.send(line.to_string());
    };
    let report = installer::install(&opts, &mut on_log).map_err(|e| e.to_string())?;

    Ok(InstallDone {
        success: report.success,
        installer: report.installer,
        hermes_installed_now: report.hermes_installed_now,
        hermes_home: report.hermes_home,
        hermes_version: report.hermes_version,
        log_tail: report.log_tail,
    })
}
