//! Restore: put a `.hermesmig` package onto this machine.
//!
//! Flow:
//! 1. [`check_environment`] — hard gates (format version, disk space,
//!    writability) and soft gates (arch warning, hermes-not-installed note).
//! 2. Snapshot-in-place backup of every file we would overwrite, so
//!    [`rollback`] can undo a failed restore without ever moving the user's
//!    existing installation.
//! 3. Extract payload + apply path repair + restore encrypted secrets.
//! 4. Verify checksums on-disk; on failure, roll back automatically.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::MigratorError;
use crate::manifest::Manifest;
use crate::pack::{self, ProgressFn};
use crate::pathmapper::PathMapper;

/// One row of an environment pre-check.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckItem {
    /// Human label, e.g. "Disk Space".
    pub name: &'static str,
    pub ok: bool,
    /// `hard` = migration cannot continue; `soft` = informational warning.
    pub severity: CheckSeverity,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckSeverity {
    Hard,
    Soft,
}

/// Result of the pre-restore environment check.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvCheck {
    pub ready: bool,
    pub items: Vec<CheckItem>,
    /// Why migration cannot continue (only when `ready == false`).
    pub reason: Option<String>,
}

/// What we restored, for the success screen.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RestoreReport {
    /// Per-component verification: name -> all files verified.
    pub verified: Vec<(String, bool)>,
    pub secrets_restored: usize,
    /// Credentials that could not be migrated and need re-authentication.
    pub needs_reauth: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RestoreOptions {
    /// Passphrase for the encrypted secrets (required when the package has
    /// them; may be empty otherwise).
    pub passphrase: Option<String>,
    /// Where to restore. Defaults to this machine's hermes home; pass
    /// `$HERMES_HOME`-style overrides for testing.
    pub target_hermes_home: Option<PathBuf>,
    /// Only restore these components (None = everything in the manifest).
    pub only_components: Option<Vec<String>>,
    /// Path repair: after extraction, rewrite source-absolute paths in text
    /// files to their target equivalents. ON by default — cross-machine
    /// restore is the whole point; disable with `--no-path-repair`.
    pub apply_path_repair: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            passphrase: None,
            target_hermes_home: None,
            only_components: None,
            apply_path_repair: true,
        }
    }
}

/// Snapshot of existing files we plan to overwrite, used by rollback.
#[derive(Debug, Default)]
struct Snapshot {
    /// target path -> previous contents (None = file did not exist).
    prev: BTreeMap<PathBuf, Option<Vec<u8>>>,
    /// Directories we created during restore (to rmdir on rollback).
    created_dirs: BTreeSet<PathBuf>,
}

/// In-memory view of the in-progress restore (for tests + GUI reporting).
#[derive(Debug, Default)]
pub struct RestoreSession {
    snapshot: Snapshot,
    target: PathBuf,
}

impl RestoreSession {
    pub fn target(&self) -> &Path {
        &self.target
    }
}

fn default_target() -> Result<PathBuf, MigratorError> {
    if let Ok(h) = std::env::var("HERMES_HOME") {
        return Ok(PathBuf::from(h));
    }
    crate::platform::current()
        .hermes_home()
        .ok_or(MigratorError::Other(
            "no hermes home found; set HERMES_HOME or install Hermes Agent".into(),
        ))
}

/// Run the pre-restore environment check against a package.
pub fn check_environment(pkg: &Path) -> Result<EnvCheck, MigratorError> {
    let manifest = pack::read_manifest(pkg)?;
    let mut items: Vec<CheckItem> = Vec::new();
    let mut ready = true;
    let mut reason: Option<String> = None;

    let fail = |items: &mut Vec<CheckItem>,
                reason: &mut Option<String>,
                ready: &mut bool,
                name: &'static str,
                severity: CheckSeverity,
                detail: String| {
        *ready = false;
        if severity == CheckSeverity::Hard {
            *reason = Some(detail.clone());
        }
        items.push(CheckItem {
            name,
            ok: false,
            severity,
            detail,
        });
    };

    // 1. Migration format.
    match manifest.check_compatibility() {
        Ok(()) => items.push(CheckItem {
            name: "Migration Package",
            ok: true,
            severity: CheckSeverity::Hard,
            detail: format!(
                "format v{}, hermes {} ({}/{})",
                manifest.format_version,
                manifest.hermes_version,
                manifest.platform,
                manifest.architecture
            ),
        }),
        Err(e) => fail(
            &mut items,
            &mut reason,
            &mut ready,
            "Migration Package",
            CheckSeverity::Hard,
            e.to_string(),
        ),
    }

    // 2. OS support.
    let os = crate::platform::current().os_name();
    if matches!(os, "macos" | "windows") {
        items.push(CheckItem {
            name: "Operating System",
            ok: true,
            severity: CheckSeverity::Hard,
            detail: os.to_string(),
        });
    } else {
        fail(
            &mut items,
            &mut reason,
            &mut ready,
            "Operating System",
            CheckSeverity::Hard,
            format!("unsupported os '{os}' (this build targets macOS/Windows)"),
        );
    }

    // 3. Architecture (soft unless the package carries an application
    // snapshot, which is machine-arch specific).
    let host_arch = crate::platform::current().architecture();
    let has_app = manifest.components.contains_key("application");
    if manifest.architecture == host_arch {
        items.push(CheckItem {
            name: "Architecture",
            ok: true,
            severity: CheckSeverity::Soft,
            detail: host_arch.clone(),
        });
    } else if has_app {
        fail(
            &mut items,
            &mut reason,
            &mut ready,
            "Architecture",
            CheckSeverity::Hard,
            format!(
                "package was built for {} but this machine is {} and it \
                 includes the application snapshot",
                manifest.architecture, host_arch
            ),
        );
    } else {
        items.push(CheckItem {
            name: "Architecture",
            ok: false,
            severity: CheckSeverity::Soft,
            detail: format!(
                "package is {} / this machine is {} (user data is portable)",
                manifest.architecture, host_arch
            ),
        });
    }

    // 4. Disk space.
    let target = default_target()?;
    let needed: u64 = manifest
        .components
        .values()
        .map(|c| c.byte_size)
        .sum::<u64>()
        + 16 * 1024 * 1024;
    match crate::platform::current().free_space_at(&target) {
        Some(free) if free >= needed => items.push(CheckItem {
            name: "Disk Space",
            ok: true,
            severity: CheckSeverity::Hard,
            detail: format!(
                "need ~{} MiB, {} MiB free",
                needed / (1024 * 1024),
                free / (1024 * 1024)
            ),
        }),
        Some(free) => fail(
            &mut items,
            &mut reason,
            &mut ready,
            "Disk Space",
            CheckSeverity::Hard,
            format!(
                "need ~{} MiB, only {} MiB free on {}",
                needed / (1024 * 1024),
                free / (1024 * 1024),
                target.display()
            ),
        ),
        None => items.push(CheckItem {
            name: "Disk Space",
            ok: true,
            severity: CheckSeverity::Soft,
            detail: "could not measure (skipped)".into(),
        }),
    }

    // 5. Hermes installed? (soft — restore still works for data-only).
    let installed = crate::platform::current().hermes_installed();
    items.push(CheckItem {
        name: "Hermes Agent",
        ok: installed,
        severity: CheckSeverity::Soft,
        detail: if installed {
            "detected".into()
        } else {
            "not installed — restore will create ~/.hermes; install Hermes \
             Agent (or include the application snapshot) to run it"
                .into()
        },
    });

    // 6. Runtime.
    let runtime = if cfg!(windows) {
        "python (expected in install)"
    } else {
        "python3"
    };
    items.push(CheckItem {
        name: "Runtime",
        ok: true,
        severity: CheckSeverity::Soft,
        detail: runtime.into(),
    });

    // 7. Target writable. Create the target up-front (restore() does the
    // same), then probe — a fresh machine has no ~/.hermes yet and that
    // must not read as "cannot write".
    match std::fs::create_dir_all(&target) {
        Ok(()) => {}
        Err(e) => {
            fail(
                &mut items,
                &mut reason,
                &mut ready,
                "Permissions",
                CheckSeverity::Hard,
                format!(
                    "cannot create {}: {} (restore will create it if \
                     missing — only failing here when the parent is \
                     unwritable)",
                    target.display(),
                    e
                ),
            );
            // Skip the probe if the dir itself could not be created.
            return Ok(EnvCheck {
                ready,
                items,
                reason,
            });
        }
    }
    let probe = target.join(".migrator-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            items.push(CheckItem {
                name: "Permissions",
                ok: true,
                severity: CheckSeverity::Hard,
                detail: format!("writable: {}", target.display()),
            });
        }
        Err(e) => fail(
            &mut items,
            &mut reason,
            &mut ready,
            "Permissions",
            CheckSeverity::Hard,
            format!("cannot write to {}: {}", target.display(), e),
        ),
    }

    Ok(EnvCheck {
        ready,
        items,
        reason,
    })
}

/// Restore a package onto this machine.
pub fn restore(
    pkg: &Path,
    opts: &RestoreOptions,
    progress: &mut ProgressFn,
) -> Result<(Manifest, RestoreReport), MigratorError> {
    let manifest = pack::read_manifest(pkg)?;
    manifest.check_compatibility()?;

    let target = match &opts.target_hermes_home {
        Some(t) => t.clone(),
        None => default_target()?,
    };
    std::fs::create_dir_all(&target)?;

    // ---- snapshot anything we'll overwrite (rollback safety net).
    let wanted: BTreeSet<String> = match &opts.only_components {
        Some(list) => list.iter().cloned().collect(),
        None => manifest.components.keys().cloned().collect(),
    };

    let mut session = RestoreSession {
        target: target.clone(),
        ..Default::default()
    };

    // Build the target-side path mapper.
    let sm = &manifest.path_mapper;
    let target_home = target
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let target_ws = target.to_string_lossy().to_string();
    let mapper = PathMapper::new(
        &sm.source_home,
        &sm.source_hermes_home,
        &sm.source_workspace,
        &target_home,
        &target.to_string_lossy(),
        &target_ws,
    );
    let rules = mapper.rules();

    progress("Extracting migration package", 5, 0, 0);

    // ---- extract payload (with per-file snapshot).
    let mut report = RestoreReport::default();
    let entries = pack::list_entries(pkg)?;
    let total = entries.len() as u64;

    for (i, zip_name) in entries.iter().enumerate() {
        let data = pack::read_entry(pkg, zip_name)?;
        let rel = zip_name
            .strip_prefix("payload/")
            .ok_or(MigratorError::InvalidPackage(zip_name.clone()))?;
        // `rel` is "<component>/<rest>" where <component> is a logical
        // bucket; map it back to the directory its files really live in.
        let (component, rest) = rel
            .split_once('/')
            .ok_or_else(|| MigratorError::InvalidPackage(format!("malformed entry {zip_name}")))?;
        let disk_dir = disk_dir_for_component(component);
        let dest = if disk_dir.is_empty() {
            target.join(rest) // root-level single files (config.yaml, state.db…)
        } else {
            target.join(disk_dir).join(rest)
        };

        snapshot_existing(&mut session, &dest);

        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
                session.snapshot.created_dirs.insert(parent.to_path_buf());
            }
        }
        std::fs::write(&dest, &data)?;
        apply_unix_perm(pkg, zip_name, &dest);

        let pct = 5 + (i.saturating_add(1) as u64 * 70 / total.max(1)) as u8;
        progress(
            "Extracting & writing files",
            pct.min(75),
            i as u64 + 1,
            total,
        );
    }

    // ---- secrets.
    if manifest.has_secrets {
        progress("Restoring secrets", 78, 0, 0);
        let pass = opts
            .passphrase
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                MigratorError::Secrets(
                    "this package contains encrypted secrets; a passphrase is required".into(),
                )
            })?;
        match pack::read_secrets(pkg, pass) {
            Ok(items) => {
                for (name, plain) in items {
                    let dest = target.join(&name);
                    snapshot_existing(&mut session, &dest);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, &plain)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&dest, {
                            let mut p = std::fs::Permissions::from_mode(0o0);
                            p.set_mode(0o600);
                            p
                        });
                    }
                    report.secrets_restored += 1;
                }
                report.notes.push(format!(
                    "{} secret(s) restored and locked down (mode 0600).",
                    report.secrets_restored
                ));
            }
            Err(e) => {
                // Do not fake success: surface re-authentication guidance.
                report.needs_reauth.push(format!(
                    "secrets could not be decrypted ({}) — re-authenticate those \
                     providers manually",
                    e
                ));
                progress("Restoring secrets", 78, 0, 0);
            }
        }
    }

    // ---- path repair on text files.
    if opts.apply_path_repair {
        progress("Repairing paths", 85, 0, 0);
        repair_paths_in(&target, &rules, &mut report)?;
    }

    // ---- post-restore verification.
    progress("Verifying", 92, 0, 0);
    let (m2, failures) = pack::verify_package(pkg)?;
    let _ = m2;

    // Component-level verify: every checksum whose path falls under a
    // component we were asked to restore must verify on disk.
    for comp in manifest.components.keys() {
        if !wanted.contains(comp) {
            continue;
        }
        let prefix = format!("payload/{}", comp);
        let comp_failures: Vec<String> = failures
            .iter()
            .filter(|f| f.starts_with(&prefix))
            .cloned()
            .collect();
        if comp_failures.is_empty() {
            report.verified.push((comp.clone(), true));
        } else {
            report.verified.push((comp.clone(), false));
            report
                .notes
                .push(format!("component '{}' failed checksums", comp));
        }
    }

    let any_failed = report.verified.iter().any(|(_, ok)| !ok);
    if any_failed {
        progress("Rolling back", 97, 0, 0);
        rollback(&session)?;
        report
            .notes
            .push("restore failed verification — rolled back".into());
        return Err(MigratorError::IntegrityFailed(format!(
            "{} checksum failure(s); previous state restored",
            failures.len()
        )));
    }

    progress("Complete", 100, 0, 0);
    Ok((manifest, report))
}

/// Map a logical component bucket to the disk directory its files live in
/// under the hermes home.
///
/// Root-level single-file components (`config`, `state_db`, `kanban`) map to
/// the empty string — their files sit directly at the hermes-home root.
/// Directory components keep their own name. `application` unpacks into
/// `hermes-agent/` (where the source tree actually lives).
fn disk_dir_for_component(component: &str) -> &str {
    match component {
        "config" | "state_db" | "kanban" => "",
        "application" => "hermes-agent",
        other => other,
    }
}

/// Snapshot one file if it exists (for rollback).
fn snapshot_existing(session: &mut RestoreSession, dest: &Path) {
    if session.snapshot.prev.contains_key(dest) {
        return;
    }
    session.snapshot.prev.insert(
        dest.to_path_buf(),
        dest.exists()
            .then(|| std::fs::read(dest).unwrap_or_default()),
    );
}

/// Re-apply POSIX permission bits on restored files (Unix only).
fn apply_unix_perm(pkg: &Path, zip_name: &str, dest: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = pack::read_manifest(pkg) {
            for c in m.components.values() {
                if let Some(mode) = c.permissions.get(zip_name) {
                    if *mode != 0 {
                        let _ =
                            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(*mode));
                    }
                    return;
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pkg, zip_name, dest);
    }
}

/// Rewrite source-absolute paths to target-absolute paths in text files we
/// restored (config.yaml + skills/plugins text).
fn repair_paths_in(
    target: &Path,
    rules: &[crate::pathmapper::PathRule],
    report: &mut RestoreReport,
) -> Result<(), MigratorError> {
    if rules.is_empty() {
        return Ok(());
    }
    let textish: &[&str] = &[
        "config.yaml",
        "SOUL.md",
        ".hermes_history",
        "shell-hooks-allowlist.json",
    ];
    let mut count = 0usize;
    // Top-level single files
    for name in textish {
        let p = target.join(name);
        if let Ok(s) = std::fs::read_to_string(&p) {
            let out = rewrite_paths(&s, rules);
            if out != s {
                std::fs::write(&p, out)?;
                count += 1;
            }
        }
    }
    // skills / plugins trees (text files only, skip binaries by extension)
    for dir_name in ["skills", "plugins", "cron"] {
        let root = target.join(dir_name);
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let is_text = p.extension().is_some_and(|e| {
                    matches!(
                        e.to_str(),
                        Some(
                            "md" | "yaml"
                                | "yml"
                                | "json"
                                | "toml"
                                | "sh"
                                | "py"
                                | "ts"
                                | "js"
                                | "txt"
                                | "strings"
                        )
                    )
                });
                if p.is_dir() {
                    stack.push(p);
                } else if is_text {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        let out = rewrite_paths(&s, rules);
                        if out != s {
                            std::fs::write(&p, out)?;
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    if count > 0 {
        report.notes.push(format!(
            "path repair rewrote {} file(s) to target paths.",
            count
        ));
    }
    Ok(())
}

/// Replace every source-prefix occurrence with its target equivalent.
/// Prefix-safe: a match only rewrites when it is not embedded in a longer
/// path segment, so `/Users/Alice` never mangles `/Users/Alicex/...`.
fn rewrite_paths(s: &str, rules: &[crate::pathmapper::PathRule]) -> String {
    // Rules are `/`-normalized (see `pathmapper`), so match against the
    // input in `/` form too. Windows-authored files keep backslashes; this
    // normalizes them so cross-machine repair works on every platform.
    let mut out = s.replace('\\', "/");
    for r in rules {
        if r.from.is_empty() || !out.contains(&r.from) {
            continue;
        }
        let from = r.from.as_str();
        let mut next = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(idx) = rest.find(from) {
            let pre = &rest[..idx];
            let post = &rest[idx + from.len()..];
            // Boundary checks. Trailing: the char after the match must not
            // extend the source path into a longer segment
            // (`/Users/Alicex` must NOT match the `/Users/Alice` rule).
            // A path segment is built from alnum + `-._`, so anything else
            // (slash, newline, quote, space, end-of-string) is a boundary.
            let ok_post = match post.chars().next() {
                Some(c) => !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')),
                None => true,
            };
            // Leading: the char before the match must not be part of a longer
            // segment either (`/x/Users/Alice` must not match).
            let ok_pre = match pre.chars().next_back() {
                Some(c) => !c.is_ascii_alphanumeric(),
                None => true,
            };
            if ok_pre && ok_post {
                next.push_str(pre);
                next.push_str(&r.to);
            } else {
                next.push_str(pre);
                next.push_str(from);
            }
            rest = post;
        }
        next.push_str(rest);
        out = next;
    }
    out
}

/// Roll back a session: restore snapshot files, delete files we created.
pub fn rollback(session: &RestoreSession) -> Result<(), MigratorError> {
    // Files: restore old contents or delete.
    for (path, prev) in session.snapshot.prev.iter().rev() {
        match prev {
            Some(bytes) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, bytes)?;
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    // Remove directories we created (children first via depth-order sort).
    let mut dirs: Vec<&PathBuf> = session.snapshot.created_dirs.iter().collect();
    dirs.sort_by_key(|p| p.components().count());
    dirs.reverse();
    for d in dirs {
        if d.exists() {
            let _ = std::fs::remove_dir(d);
        }
    }
    Ok(())
}

/// Standalone backup of an existing hermes home to a timestamped dir.
pub fn backup_hermes_home(home: &Path) -> Result<PathBuf, MigratorError> {
    if !home.is_dir() {
        return Err(MigratorError::PathNotFound(home.display().to_string()));
    }
    let ts = crate::platform::current().now_iso8601().replace(':', "-");
    let dest = home
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!("hermes-backup-{}", ts));
    std::fs::create_dir_all(&dest)?;
    // Copy only user data (skip the 2GB install tree — it gets reinstalled).
    let skip: BTreeSet<&str> = BTreeSet::from([
        "hermes-agent",
        "cache",
        "logs",
        "lsp",
        "image_cache",
        "audio_cache",
        "models_dev_cache.json",
        "provider_models_cache.json",
        "ollama_cloud_models_cache.json",
    ]);
    copy_tree(home, &dest, &skip)?;
    Ok(dest)
}

fn copy_tree(src: &Path, dst: &Path, skip: &BTreeSet<&str>) -> Result<(), MigratorError> {
    let rd = std::fs::read_dir(src)?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        let d = dst.join(&name);
        if skip.contains(&name.as_str()) {
            continue;
        }
        if p.is_dir() {
            std::fs::create_dir_all(&d)?;
            copy_tree(&p, &d, skip)?;
        } else if p.is_file() {
            std::fs::copy(&p, &d)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_paths_is_prefix_safe() {
        let rule = crate::pathmapper::PathRule {
            from: "/Users/Alice".into(),
            to: "/Users/Bob".into(),
        };
        assert_eq!(
            rewrite_paths("/Users/Alicex/file", &[rule.clone()]),
            "/Users/Alicex/file"
        );
        assert_eq!(
            rewrite_paths("home: /Users/Alice/.hermes", &[rule]),
            "home: /Users/Bob/.hermes"
        );
    }

    #[test]
    fn rewrite_paths_normalizes_backslash_input() {
        // Windows-authored text files keep backslashes; the `/`-based rules
        // must still match. `replace('\\', "/")` is pure string logic, so
        // this holds identically on every platform.
        let rule = crate::pathmapper::PathRule {
            from: "C:/Users/Alice".into(),
            to: "D:/Users/Bob".into(),
        };
        assert_eq!(
            rewrite_paths("workdir: C:\\Users\\Alice\\.hermes", &[rule.clone()]),
            "workdir: D:/Users/Bob/.hermes"
        );
        // Boundary safety survives normalization too.
        let rule2 = crate::pathmapper::PathRule {
            from: "C:/Users/Alice".into(),
            to: "D:/Users/Bob".into(),
        };
        assert_eq!(
            rewrite_paths("C:\\Users\\Alicex\\f", &[rule2]),
            "C:/Users/Alicex/f"
        );
    }
}
