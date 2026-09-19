//! Hermes Agent discovery and scanning.
//!
//! Everything here is a pure function over a hermes root path, so it is
//! unit-testable against temp dirs and usable from either platform adapter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::MigratorError;

/// A discovered Hermes Agent installation (the application itself).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HermesInstall {
    /// Root of the hermes-agent source/v install, e.g. `~/.hermes/hermes-agent`.
    pub root: String,
    /// Version string, e.g. `0.20.0`. Empty when undetermined.
    pub version: String,
    /// How it was installed: "venv" | "source" | "unknown".
    pub install_kind: String,
}

/// What the scanner found on this machine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScanReport {
    pub detected: bool,
    pub hermes_home: Option<String>,
    pub install: Option<HermesInstall>,
    pub config_files: Vec<String>,
    /// Component name -> status. Keys: config, skills, plugins, sessions,
    /// state_db, cron, memories, kanban, pastes, secrets, application.
    pub components: BTreeMap<String, ComponentStatus>,
    /// Human-readable notes (secrets warnings, machine-local exclusions...).
    pub notes: Vec<String>,
    pub total_files: u64,
    pub total_bytes: u64,
}

/// Status of one scanned component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentStatus {
    Present,
    Missing,
    /// Present but needs user attention (e.g. contains secrets).
    Attention,
}

impl ComponentStatus {
    pub fn symbol(self) -> &'static str {
        match self {
            ComponentStatus::Present => "✓",
            ComponentStatus::Missing => "—",
            ComponentStatus::Attention => "⚠",
        }
    }
}

/// Read the agent version Hermes itself records in `.update_check`
/// (`{"ver": "0.20.0", ...}`) — the most reliable source available.
pub fn read_agent_version(hermes_home: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(hermes_home.join(".update_check")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("ver")?.as_str()?.to_string().into()
}

/// Version from the install tree's `pyproject.toml`, when present.
fn pyproject_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("pyproject.toml")).ok()?;
    text.lines().find_map(|l| {
        l.strip_prefix("version").map(|rest| {
            rest.trim_start_matches('=')
                .trim()
                .trim_matches('"')
                .to_string()
        })
    })
}

/// Detect the application install under `<hermes_home>/hermes-agent`.
pub fn detect_install(hermes_home: &Path) -> Option<HermesInstall> {
    let root = hermes_home.join("hermes-agent");
    if !root.exists() {
        return None;
    }
    let is_venv = root.join("venv").exists();
    Some(HermesInstall {
        version: read_agent_version(hermes_home)
            .or_else(|| pyproject_version(&root))
            .unwrap_or_default(),
        root: root.display().to_string(),
        install_kind: if is_venv {
            "venv".into()
        } else {
            "source".into()
        },
    })
}

fn dir_size(dir: &Path, files: &mut u64, bytes: &mut u64) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                *files += 1;
                *bytes += md.len();
            }
        }
    }
}

fn is_empty_or_missing(p: &Path) -> bool {
    if !p.is_dir() {
        return true;
    }
    match p.read_dir() {
        Ok(mut d) => d.next().is_none(),
        Err(_) => true,
    }
}

fn mark_dir(
    components: &mut BTreeMap<String, ComponentStatus>,
    name: &str,
    p: &Path,
    files: &mut u64,
    bytes: &mut u64,
) {
    if is_empty_or_missing(p) {
        components.insert(name.into(), ComponentStatus::Missing);
    } else {
        dir_size(p, files, bytes);
        components.insert(name.into(), ComponentStatus::Present);
    }
}

fn mark_file(
    components: &mut BTreeMap<String, ComponentStatus>,
    name: &str,
    p: &Path,
    files: &mut u64,
    bytes: &mut u64,
) {
    if p.is_file() {
        *files += 1;
        if let Ok(md) = p.metadata() {
            *bytes += md.len();
        }
        components.insert(name.into(), ComponentStatus::Present);
    } else {
        components.insert(name.into(), ComponentStatus::Missing);
    }
}

/// Scan a directory for a known Hermes environment.
///
/// Pure function over a root so tests can point it at temp dirs and so the
/// Windows/macOS adapters just call it with the platform-correct root.
pub fn scan_hermes_root(root: &Path) -> ScanReport {
    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    let mut notes: Vec<String> = Vec::new();
    let mut components: BTreeMap<String, ComponentStatus> = BTreeMap::new();

    let detected = root.join("config.yaml").is_file();
    if !detected {
        return ScanReport {
            detected,
            hermes_home: root.to_str().map(|s| s.to_string()),
            install: None,
            config_files: vec![],
            components,
            notes: vec![format!("no config.yaml under {}", root.display())],
            total_files: 0,
            total_bytes: 0,
        };
    }

    mark_file(
        &mut components,
        "config",
        &root.join("config.yaml"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "skills",
        &root.join("skills"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "plugins",
        &root.join("plugins"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "sessions",
        &root.join("sessions"),
        &mut files,
        &mut bytes,
    );
    mark_file(
        &mut components,
        "state_db",
        &root.join("state.db"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "cron",
        &root.join("cron"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "memories",
        &root.join("memories"),
        &mut files,
        &mut bytes,
    );
    mark_file(
        &mut components,
        "kanban",
        &root.join("kanban.db"),
        &mut files,
        &mut bytes,
    );
    mark_dir(
        &mut components,
        "pastes",
        &root.join("pastes"),
        &mut files,
        &mut bytes,
    );

    // Secrets: present => Attention, never silently copied.
    let secret_hits = [".env", "auth.json", "channel_directory.json"]
        .into_iter()
        .filter(|n| root.join(n).is_file())
        .count();
    let weixin_dir = root.join("weixin");
    if secret_hits > 0 || weixin_dir.is_dir() {
        components.insert("secrets".into(), ComponentStatus::Attention);
        notes.push(
            "Secrets detected (.env / auth.json / weixin tokens): encrypted into \
             secrets.enc at pack time; never written in plaintext to manifest or logs."
                .into(),
        );
    }

    let app = root.join("hermes-agent");
    if app.is_dir() {
        components.insert("application".into(), ComponentStatus::Present);
        notes.push(format!(
            "Application tree '{}' (machine-local venv + node_modules) is excluded \
             from packages by default.",
            app.display()
        ));
    } else {
        components.insert("application".into(), ComponentStatus::Missing);
    }

    let config_files = root
        .read_dir()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n == "config.yaml" || n.starts_with("config.yaml.bak")
        })
        .map(|e| e.path().display().to_string())
        .collect();

    ScanReport {
        detected,
        hermes_home: root.to_str().map(|s| s.to_string()),
        install: detect_install(root),
        config_files,
        components,
        notes,
        total_files: files,
        total_bytes: bytes,
    }
}

/// Locate the Hermes home directory on this machine:
/// 1. `$HERMES_HOME` env var (Hermes's own override),
/// 2. `~/.hermes` when it contains `config.yaml`.
pub fn locate_hermes_home() -> Result<PathBuf, MigratorError> {
    if let Ok(h) = std::env::var("HERMES_HOME") {
        let p = PathBuf::from(h);
        if p.join("config.yaml").is_file() {
            return Ok(p);
        }
        return Err(MigratorError::HermesNotFound(p.display().to_string()));
    }
    match dirs::home_dir() {
        Some(home) => {
            let p = home.join(".hermes");
            if p.join("config.yaml").is_file() {
                Ok(p)
            } else {
                Err(MigratorError::HermesNotFound(p.display().to_string()))
            }
        }
        None => Err(MigratorError::Other("cannot determine home dir".into())),
    }
}
