//! Hermes Migration Format v1 — the manifest written at the head of every
//! `.hermesmig` package.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::pathmapper::PathMapper;
use crate::scan::HermesInstall;

/// Current format version.
pub const FORMAT_VERSION: u32 = 1;

/// A component that was packed (config, skills, plugins, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedComponent {
    /// Logical name, e.g. "skills".
    pub name: String,
    /// Entries (relative paths under the component) — for large trees this is
    /// the top-level set; the payload archive holds the full file list.
    pub file_count: u64,
    pub byte_size: u64,
    /// SHA-256 of each path (relative) -> hex digest.
    pub checksums: BTreeMap<String, String>,
    /// POSIX mode bits (0 = unknown/not preserved) per relative path, so
    /// restore can re-apply exec bits on Unix.
    #[serde(default)]
    pub permissions: BTreeMap<String, u32>,
}

/// Environment snapshot of the source machine (informational; used to detect
/// arch mismatch at restore time).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceEnvironment {
    pub os: String,
    pub arch: String,
    pub hermes_version: String,
    pub layout_version: u32,
    pub home_dir: String,
    pub hermes_home: String,
    pub workspace: String,
    pub created_at: String,
}

/// The Hermes Migration Format v1 manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub hermes_version: String,
    pub layout_version: u32,
    pub platform: String,
    pub architecture: String,
    pub created_at: String,
    /// Logical component -> packed info.
    pub components: BTreeMap<String, PackedComponent>,
    pub source_environment: SourceEnvironment,
    /// Path repair rules (source prefixes -> target placeholders). The target
    /// side is filled in at restore time by PathMapper.
    pub path_mapper: PathMapper,
    /// Per-file SHA-256 of everything in the payload (path under payload/ ->
    /// hex).
    #[serde(default)]
    pub checksums: BTreeMap<String, String>,
    /// True when a secrets.enc blob is present.
    pub has_secrets: bool,
    /// Human-readable notes carried over from the scan.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Manifest {
    pub fn new_skeleton(platform: &str, architecture: &str, hermes_version: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            hermes_version: hermes_version.to_string(),
            layout_version: crate::HERMES_LAYOUT_VERSION,
            platform: platform.to_string(),
            architecture: architecture.to_string(),
            created_at: crate::platform::current().now_iso8601(),
            components: BTreeMap::new(),
            source_environment: SourceEnvironment::default(),
            path_mapper: PathMapper::new("", "", "", "", "", ""),
            checksums: BTreeMap::new(),
            has_secrets: false,
            notes: vec![],
        }
    }

    /// Validate that this manifest can be consumed by this engine.
    pub fn check_compatibility(&self) -> Result<(), crate::MigratorError> {
        if self.format_version > FORMAT_VERSION {
            return Err(crate::MigratorError::Other(format!(
                "package uses format v{}, this tool understands up to v{}",
                self.format_version, FORMAT_VERSION
            )));
        }
        if self.layout_version > crate::HERMES_LAYOUT_VERSION {
            return Err(crate::MigratorError::UnsupportedLayoutVersion(
                self.layout_version,
            ));
        }
        Ok(())
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            hermes_version: String::new(),
            layout_version: crate::HERMES_LAYOUT_VERSION,
            platform: String::new(),
            architecture: String::new(),
            created_at: String::new(),
            components: BTreeMap::new(),
            source_environment: SourceEnvironment::default(),
            path_mapper: PathMapper::default(),
            checksums: BTreeMap::new(),
            has_secrets: false,
            notes: vec![],
        }
    }
}

/// Which logical components exist in a package (drives restore order).
pub const COMPONENT_NAMES: &[&str] = &[
    "config",
    "skills",
    "plugins",
    "sessions",
    "state_db",
    "cron",
    "memories",
    "kanban",
    "pastes",
    "application",
];

/// Component -> the on-disk name used inside `payload/`.
pub fn payload_dir_for(component: &str) -> &str {
    component
}

/// The install record carried when the user opts into the application
/// snapshot component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationInfo {
    pub install: Option<HermesInstall>,
    /// Snapshot kind: "source" (hermes-agent tree) or "runtime" (venv+
    /// node_modules; machine-local, discouraged).
    pub snapshot_kind: String,
}
