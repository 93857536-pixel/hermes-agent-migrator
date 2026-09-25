//! Automatic Hermes Agent installation: system detection + official installer.
//!
//! The migrator never vendors Hermes binaries. [`detect`] inspects the host
//! (OS / arch / existing Hermes install / available tooling) and [`install`]
//! shells out to the upstream installer, streaming its output line by line
//! into a log callback:
//!
//! * macOS / Linux: `https://hermes-agent.nousresearch.com/install.sh`
//!   (`bash`, `--non-interactive`, optional `--skip-browser`)
//! * Windows: `https://hermes-agent.nousresearch.com/install.ps1`
//!   (`powershell -File`, `-NonInteractive`, optional `-SkipBrowser`)
//!
//! After the installer exits the engine re-runs [`detect`] and reports
//! whether Hermes is visible on the machine.

use std::collections::VecDeque;
use std::io::BufRead;
use std::io::BufReader;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::error::MigratorError;
use crate::platform::{CurrentPlatform, PlatformAdapter};

/// Upstream installer URLs. The official source of truth — bump here when
/// Hermes ships a new install path.
pub const INSTALL_SH_URL: &str = "https://hermes-agent.nousresearch.com/install.sh";
pub const INSTALL_PS1_URL: &str = "https://hermes-agent.nousresearch.com/install.ps1";

/// Hard cap for an installer run so the GUI never hangs forever.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(45 * 60);

/// How many installer log lines are retained for the post-install report.
pub const LOG_TAIL_LINES: usize = 300;

/// Read-only snapshot of the host system, for the Setup screen.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    /// A Hermes install (wrapper or venv) is visible at the hermes home.
    pub hermes_installed: bool,
    pub hermes_home: Option<String>,
    /// Best-effort version, when installed.
    pub hermes_version: Option<String>,
    /// `git --version` available. Required by the POSIX installer; on
    /// Windows the installer can bootstrap Git for Windows when missing.
    pub git_available: bool,
    /// Python visible on PATH. PM (the installer's package manager) ships
    /// its own pinned Python, so this is informational only.
    pub python_available: bool,
    /// `curl --version`. Required by the POSIX installer (and by this
    /// module's downloader).
    pub curl_available: bool,
    /// Free space on the volume holding the user's home dir, when detectable.
    pub free_space_bytes: Option<u64>,
}

/// Options for [`install`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallOptions {
    /// Skip the default browser-tools step (faster, smaller download).
    #[serde(default)]
    pub skip_browser: bool,
}

/// Outcome of an install attempt.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallReport {
    /// The installer process exited with status 0.
    pub success: bool,
    /// Which installer ran, e.g. `install.sh (bash --non-interactive)`.
    pub installer: String,
    /// Re-detection after the install finished.
    pub hermes_installed_now: bool,
    pub hermes_home: Option<String>,
    pub hermes_version: Option<String>,
    /// The last [`LOG_TAIL_LINES`] installer output lines.
    pub log_tail: Vec<String>,
}

/// Detect the host system. Read-only; never spawns the installer.
pub fn detect() -> SystemInfo {
    let p = CurrentPlatform;
    let hermes_home = p.hermes_home();
    let installed = p.hermes_installed();
    let python_cmd = if cfg!(windows) { "python" } else { "python3" };
    SystemInfo {
        os: p.os_name().to_string(),
        arch: p.architecture(),
        hermes_installed: installed,
        hermes_home: hermes_home
            .as_ref()
            .map(|h| h.to_string_lossy().into_owned()),
        hermes_version: installed.then(|| p.hermes_version()).flatten(),
        git_available: tool_available("git", &["--version"]),
        python_available: tool_available(python_cmd, &["--version"]),
        curl_available: tool_available("curl", &["--version"]),
        free_space_bytes: p.free_space_at(&p.home_dir()),
    }
}

/// Run a `--version`-style probe, tolerating a missing binary.
fn tool_available(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| child.wait().map(|s| s.success()).unwrap_or(false))
        .unwrap_or(false)
}

/// Run the official installer, streaming its output to `on_log`.
///
/// `on_log` is invoked once per installer output line (stdout and stderr
/// merged, arrival order). Blocks until the installer exits or
/// [`INSTALL_TIMEOUT`] elapses (then the child is killed and an error
/// returned).
pub fn install(
    opts: &InstallOptions,
    on_log: &mut impl FnMut(&str),
) -> Result<InstallReport, MigratorError> {
    #[cfg(windows)]
    {
        install_windows(opts, on_log)
    }
    #[cfg(not(windows))]
    {
        install_posix(opts, on_log)
    }
}

/// macOS / Linux: download `install.sh` and run it with bash.
#[cfg(not(windows))]
fn install_posix(
    opts: &InstallOptions,
    on_log: &mut impl FnMut(&str),
) -> Result<InstallReport, MigratorError> {
    if !tool_available("curl", &["--version"]) {
        return Err(MigratorError::Other(
            "curl is required to download the Hermes installer. Install curl and retry.".into(),
        ));
    }
    let (script, mut args) = download_install_script(on_log)?;
    args.push("--non-interactive".into());
    if opts.skip_browser {
        args.push("--skip-browser".into());
    }
    let mut cmd = Command::new("bash");
    cmd.arg(&script).args(&args);
    run_streamed(&mut cmd, "install.sh (bash --non-interactive)", on_log)
}

/// Windows: download `install.ps1` via PowerShell and run it non-interactively.
#[cfg(windows)]
fn install_windows(
    opts: &InstallOptions,
    on_log: &mut impl FnMut(&str),
) -> Result<InstallReport, MigratorError> {
    let ps1 = download_install_script(on_log)?;
    let mut args: Vec<String> = vec![
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-File".into(),
        ps1.display().to_string(),
        "-NonInteractive".into(),
    ];
    if opts.skip_browser {
        args.push("-SkipBrowser".into());
    }
    let mut cmd = Command::new("powershell");
    cmd.args(&args);
    run_streamed(&mut cmd, "install.ps1 (powershell -NonInteractive)", on_log)
}

/// Download the installer into a temp dir; returns (script path, empty args
/// vector). POSIX uses curl, Windows uses `Invoke-WebRequest`.
#[cfg(not(windows))]
fn download_install_script(
    _on_log: &mut impl FnMut(&str),
) -> Result<(std::path::PathBuf, Vec<String>), MigratorError> {
    let tmp = std::env::temp_dir().join(format!("hermes-migrator-install-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(MigratorError::Io)?;
    let script = tmp.join("install.sh");
    let dl = Command::new("curl")
        .args(["-fsSL", INSTALL_SH_URL, "-o"])
        .arg(&script)
        .output()
        .map_err(MigratorError::Io)?;
    if !dl.status.success() {
        let msg = String::from_utf8_lossy(&dl.stderr).trim().to_string();
        return Err(MigratorError::Other(format!(
            "failed to download {INSTALL_SH_URL}: {msg}"
        )));
    }
    Ok((script, Vec::new()))
}

/// Download `install.ps1` into a temp dir (Windows has no curl requirement).
#[cfg(windows)]
fn download_install_script(
    _on_log: &mut impl FnMut(&str),
) -> Result<std::path::PathBuf, MigratorError> {
    let tmp = std::env::temp_dir().join(format!("hermes-migrator-install-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(MigratorError::Io)?;
    let ps1 = tmp.join("install.ps1");
    let dl = Command::new("powershell")
        .args(["-NoProfile", "-Command"])
        .arg(format!(
            "Invoke-WebRequest -UseBasicParsing -Uri '{INSTALL_PS1_URL}' -OutFile '{path}'",
            path = ps1.display().to_string().replace('\'', "''")
        ))
        .output()
        .map_err(MigratorError::Io)?;
    if !dl.status.success() {
        let msg = String::from_utf8_lossy(&dl.stderr).trim().to_string();
        return Err(MigratorError::Other(format!(
            "failed to download {INSTALL_PS1_URL}: {msg}"
        )));
    }
    Ok(ps1)
}

/// Spawn `cmd`, stream both output pipes into `on_log`, enforce the timeout,
/// then re-detect and build the report.
fn run_streamed(
    cmd: &mut Command,
    label: &str,
    on_log: &mut impl FnMut(&str),
) -> Result<InstallReport, MigratorError> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(MigratorError::Io)?;
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stderr = BufReader::new(child.stderr.take().unwrap());

    let (tx, rx) = mpsc::channel::<String>();
    let tx_err = tx.clone();
    let t_out = std::thread::spawn(move || {
        let mut out = stdout.lines();
        while let Some(Ok(line)) = out.next() {
            let _ = tx.send(line);
        }
    });
    let t_err = std::thread::spawn(move || {
        let mut err = stderr.lines();
        while let Some(Ok(line)) = err.next() {
            let _ = tx_err.send(line);
        }
    });

    let mut tail = VecDeque::new();
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => {
                on_log(&line);
                push_tail(&mut tail, line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if t_out.is_finished() && t_err.is_finished() {
                    break;
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(MigratorError::Other(format!(
                        "installer timed out after {} minutes",
                        INSTALL_TIMEOUT.as_secs() / 60
                    )));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(MigratorError::Io)?;
    let _ = t_out.join();
    let _ = t_err.join();

    let after = detect();
    Ok(InstallReport {
        success: status.success(),
        installer: label.to_string(),
        hermes_installed_now: after.hermes_installed,
        hermes_home: after.hermes_home,
        hermes_version: after.hermes_version,
        log_tail: tail.into_iter().collect(),
    })
}

fn push_tail(q: &mut VecDeque<String>, line: String) {
    q.push_back(line);
    if q.len() > LOG_TAIL_LINES {
        q.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detect_reports_current_platform() {
        let i = detect();
        assert!(!i.os.is_empty());
        assert!(!i.arch.is_empty());
        // Self-consistency: if a hermes home exists, the installed flag
        // matches the file layout rules from the platform adapter.
        if let Some(home) = &i.hermes_home {
            let expected = Path::new(home).join("hermes-agent").is_dir()
                || Path::new(home).join("bin").is_dir();
            assert_eq!(i.hermes_installed, expected);
        }
        assert_eq!(i.hermes_version.is_some(), i.hermes_installed);
    }

    #[test]
    fn install_options_default() {
        let o: InstallOptions = serde_json::from_str("{}").unwrap();
        assert!(!o.skip_browser);
        let o: InstallOptions = serde_json::from_str(r#"{"skip_browser": true}"#).unwrap();
        assert!(o.skip_browser);
    }

    #[test]
    fn tail_is_capped() {
        let mut q = VecDeque::new();
        for n in 0..(LOG_TAIL_LINES * 2) {
            push_tail(&mut q, format!("line-{n}"));
        }
        assert_eq!(q.len(), LOG_TAIL_LINES);
        assert_eq!(
            q.front().unwrap().as_str(),
            format!("line-{}", LOG_TAIL_LINES)
        );
        assert_eq!(
            q.back().unwrap().as_str(),
            format!("line-{}", LOG_TAIL_LINES * 2 - 1)
        );
    }
}
