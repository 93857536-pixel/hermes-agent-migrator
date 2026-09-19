//! Platform adaptation layer.
//!
//! Windows and macOS have different home layouts, permission semantics, and
//! install conventions. Everything platform-specific funnels through
//! [`PlatformAdapter`] so the core engine stays portable.

use std::path::PathBuf;

/// A platform adapter.
pub trait PlatformAdapter: Send + Sync {
    fn os_name(&self) -> &'static str;
    fn architecture(&self) -> String;
    fn home_dir(&self) -> PathBuf;
    /// Default Hermes home: `<home>/.hermes` (Hermes's convention on both
    /// platforms; override-able via $HERMES_HOME in tests).
    fn hermes_home(&self) -> Option<PathBuf>;
    /// Hermes has no separate workspace dir in the v1 layout; sessions live
    /// under the home. Return hermes home as the "workspace" root so
    /// PathMapper still has three anchors.
    fn workspace_dir(&self) -> PathBuf;
    /// Is Hermes installed? (wrapper or venv present)
    fn hermes_installed(&self) -> bool;
    /// Best-effort version string.
    fn hermes_version(&self) -> Option<String>;
    /// Free space on the volume holding `path`, in bytes.
    fn free_space_at(&self, path: &std::path::Path) -> Option<u64>;
    fn now_iso8601(&self) -> String;
}

/// Current platform, chosen at runtime (compile-time cfgs would prevent
/// testing the other side).
pub struct CurrentPlatform;

impl PlatformAdapter for CurrentPlatform {
    fn os_name(&self) -> &'static str {
        if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "unix"
        }
    }

    fn architecture(&self) -> String {
        std::env::consts::ARCH.to_string()
    }

    fn home_dir(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_default()
    }

    fn hermes_home(&self) -> Option<PathBuf> {
        if let Ok(h) = std::env::var("HERMES_HOME") {
            let p = PathBuf::from(h);
            if p.join("config.yaml").is_file() {
                return Some(p);
            }
        }
        self.home_dir()
            .join(".hermes")
            .is_dir()
            .then(|| self.home_dir().join(".hermes"))
    }

    fn workspace_dir(&self) -> PathBuf {
        self.hermes_home()
            .unwrap_or_else(|| self.home_dir().join(".hermes"))
    }

    fn hermes_installed(&self) -> bool {
        if let Some(hh) = self.hermes_home() {
            hh.join("hermes-agent").is_dir() || hh.join("bin").is_dir()
        } else {
            false
        }
    }

    fn hermes_version(&self) -> Option<String> {
        crate::scan::read_agent_version(&self.hermes_home()?)
    }

    fn free_space_at(&self, path: &std::path::Path) -> Option<u64> {
        // libstd has no cross-platform statvfs; approximate by trying to
        // write nothing and reading the OS via platform calls.
        free_space_impl(path)
    }

    fn now_iso8601(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Format as RFC3339 UTC without pulling in a date crate.
        crate::platform::time_format_rfc3339(secs)
    }
}

/// Minimal RFC3339 (UTC) formatter — avoids a heavyweight datetime dep in
/// the core. (Y-M-D T H:M:SZ)
pub fn time_format_rfc3339(unix_secs: u64) -> String {
    let days = unix_secs / 86400;
    let rem = unix_secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days (proleptic Gregorian; 1970-01-01 = day 0).
    let z = (days as i64) + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097; // [0, ...)
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let doy = doe - 365 * yoe - (yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let dom = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = mp + 3 - 12 * (mp / 10); // [1, 12]
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, dom, h, m, s
    )
}

fn free_space_impl(path: &std::path::Path) -> Option<u64> {
    // `df -k <path>`: header + one data line; column 3 = 1024-blocks available.
    // (Cross-platform advisory check; a Windows adapter would use FFI instead.)
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let last_line = text.lines().last()?;
    let fields: Vec<&str> = last_line.split_whitespace().collect();
    fields.get(3)?.parse::<u64>().ok().map(|k| k * 1024)
}

/// `current()` — the adapter for this machine.
pub fn current() -> Box<dyn PlatformAdapter> {
    Box::new(CurrentPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_epoch_is_1970() {
        assert_eq!(time_format_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_known_date() {
        // 2026-09-19 00:00:00 UTC = 1789776000 (verified independently)
        let d = time_format_rfc3339(1_789_776_000);
        assert!(d.ends_with("T00:00:00Z"));
        assert!(d.starts_with("2026-09-19"));
    }

    #[test]
    fn current_adapter_reports_sane_os() {
        let p = current();
        assert!(!p.os_name().is_empty());
        assert!(!p.home_dir().as_os_str().is_empty());
    }
}
