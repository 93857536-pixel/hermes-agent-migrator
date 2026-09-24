//! Pack: build a single `.hermesmig` migration package from a hermes home.
//!
//! The package is a standard ZIP container carrying our own
//! **Hermes Migration Format v1** on top:
//!
//! ```text
//! HermesAgent-2026-09-19.hermesmig     (ZIP)
//!   manifest.json          Hermes Migration Format v1 manifest
//!   environment.json       source environment snapshot
//!   secrets.enc            AES-256-GCM bundle (Argon2 KDF) — optional
//!   payload/config/config.yaml
//!   payload/skills/...
//!   payload/plugins/...
//!   payload/sessions/...
//!   payload/state_db/state.db (+ -wal/-shm)
//!   payload/cron/...
//!   payload/memories/...
//!   payload/kanban/kanban.db
//!   payload/pastes/...
//!   payload/application/...  (opt-in source snapshot, no venv/node_modules)
//! ```
//!
//! Secrets live *inside* the package so the user carries exactly one file.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::checksum;
use crate::error::MigratorError;
use crate::manifest::{Manifest, PackedComponent, FORMAT_VERSION};
use crate::scan::{self, ScanReport};
use crate::secrets;

/// Progress callback: `(stage label, 0..=100 percent, items done, items total)`.
pub type ProgressFn = Box<dyn FnMut(&str, u8, u64, u64)>;

/// Which components the user selected for a package.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackOptions {
    pub include_config: bool,
    pub include_skills: bool,
    pub include_plugins: bool,
    pub include_sessions: bool,
    pub include_state_db: bool,
    pub include_cron: bool,
    pub include_memories: bool,
    pub include_kanban: bool,
    pub include_pastes: bool,
    /// Opt-in: source snapshot of `hermes-agent/` (excludes machine-local
    /// venv + node_modules + pycache; those are rebuilt on the target).
    pub include_application: bool,
    /// Passphrase for the secrets bundle; `None` => secrets not packaged.
    pub secrets_passphrase: Option<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            include_config: true,
            include_skills: true,
            include_plugins: true,
            include_sessions: true,
            include_state_db: true,
            include_cron: true,
            include_memories: true,
            include_kanban: true,
            include_pastes: true,
            include_application: false,
            secrets_passphrase: None,
        }
    }
}

/// A staged file that will enter the package.
struct Entry {
    /// ZIP path, e.g. `payload/skills/foo/SKILL.md`.
    zip_path: String,
    /// Source path on disk.
    src: PathBuf,
    /// Logical component this entry belongs to.
    component: &'static str,
}

/// (staged entries, secret items) produced for a pack plan.
type CollectResult = Result<(Vec<Entry>, BTreeMap<String, Vec<u8>>), MigratorError>;

/// Everything needed to pack a hermes home.
pub struct PackPlan {
    pub hermes_home: PathBuf,
    pub manifest: Manifest,
}

/// Walk `dir` (recursively) and stage every regular file under it.
fn add_dir(entries: &mut Vec<Entry>, name: &'static str, dir: &Path) -> Result<(), MigratorError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d)
            .map_err(|e| MigratorError::Other(format!("read_dir {}: {}", d.display(), e)))?;
        for e in rd {
            let e = e.map_err(|e| MigratorError::Other(format!("dir entry: {}", e)))?;
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                let rel = p
                    .strip_prefix(dir)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                entries.push(Entry {
                    zip_path: format!("payload/{}/{}", name, rel),
                    src: p,
                    component: name,
                });
            }
        }
    }
    Ok(())
}

/// Stage a single file (no-op if it does not exist).
fn add_file(entries: &mut Vec<Entry>, name: &'static str, file: &Path) {
    if file.is_file() {
        entries.push(Entry {
            zip_path: format!(
                "payload/{}/{}",
                name,
                file.file_name().unwrap().to_string_lossy()
            ),
            src: file.to_path_buf(),
            component: name,
        });
    }
}

/// Build a pack plan from a scan report + options (no I/O).
pub fn plan_pack(report: &ScanReport, _opts: &PackOptions) -> PackPlan {
    let home = PathBuf::from(report.hermes_home.clone().unwrap_or_default());
    let version = report
        .install
        .as_ref()
        .map(|i| i.version.clone())
        .unwrap_or_default();

    let mut manifest = Manifest::new_skeleton(
        crate::platform::current().os_name(),
        &crate::platform::current().architecture(),
        &version,
    );
    manifest.layout_version = FORMAT_VERSION;
    manifest.source_environment = crate::manifest::SourceEnvironment {
        os: crate::platform::current().os_name().to_string(),
        arch: crate::platform::current().architecture(),
        hermes_version: version.clone(),
        layout_version: crate::HERMES_LAYOUT_VERSION,
        home_dir: home
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        hermes_home: home.to_string_lossy().to_string(),
        workspace: crate::platform::current()
            .workspace_dir()
            .to_string_lossy()
            .to_string(),
        created_at: manifest.created_at.clone(),
    };

    // Source side of the path mapper is known now; target side is filled at
    // restore time for the destination machine.
    let source_home = home
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let source_ws = crate::platform::current()
        .workspace_dir()
        .to_string_lossy()
        .to_string();
    manifest.path_mapper = crate::pathmapper::PathMapper::new(
        &source_home,
        &home.to_string_lossy(),
        &source_ws,
        "",
        "",
        "",
    );
    manifest.notes = report.notes.clone();

    PackPlan {
        hermes_home: home,
        manifest,
    }
}

/// Collect the staged file list + secret items for a plan.
fn collect_entries(home: &Path, opts: &PackOptions, _report: &ScanReport) -> CollectResult {
    let mut entries: Vec<Entry> = Vec::new();
    // secret key -> plaintext bytes (encrypted before it ever reaches a disk
    // in plaintext form inside the package).
    let mut secret_items: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    if opts.include_config {
        add_file(&mut entries, "config", &home.join("config.yaml"));
    }
    if opts.include_skills {
        add_dir(&mut entries, "skills", &home.join("skills"))?;
    }
    if opts.include_plugins {
        add_dir(&mut entries, "plugins", &home.join("plugins"))?;
    }
    if opts.include_sessions {
        add_dir(&mut entries, "sessions", &home.join("sessions"))?;
    }
    if opts.include_state_db {
        add_file(&mut entries, "state_db", &home.join("state.db"));
        for suffix in ["-wal", "-shm"] {
            add_file(
                &mut entries,
                "state_db",
                &home.join(format!("state.db{}", suffix)),
            );
        }
    }
    if opts.include_cron {
        add_dir(&mut entries, "cron", &home.join("cron"))?;
    }
    if opts.include_memories {
        add_dir(&mut entries, "memories", &home.join("memories"))?;
    }
    if opts.include_kanban {
        add_file(&mut entries, "kanban", &home.join("kanban.db"));
        add_file(&mut entries, "kanban", &home.join("kanban.db-wal"));
    }
    if opts.include_pastes {
        add_dir(&mut entries, "pastes", &home.join("pastes"))?;
    }
    if opts.include_application {
        let app = home.join("hermes-agent");
        if app.is_dir() {
            let skip: &[&str] = &[
                "venv",
                "node_modules",
                "__pycache__",
                ".git",
                "dist",
                "build",
                ".venv",
            ];
            let mut stack = vec![app.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    let Ok(md) = e.metadata() else { continue };
                    let fname = p
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if md.is_dir() {
                        if skip.contains(&fname.as_str()) {
                            continue;
                        }
                        stack.push(p);
                    } else if md.is_file() {
                        let rel = p
                            .strip_prefix(&app)
                            .map(|r| r.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default();
                        entries.push(Entry {
                            zip_path: format!("payload/application/{}", rel),
                            src: p,
                            component: "application",
                        });
                    }
                }
            }
        }
    }

    // Secrets: encrypted into the bundle, never stored in plaintext.
    if opts.secrets_passphrase.is_some() {
        for name in secrets::SECRET_FILES {
            let p = home.join(name);
            if let Ok(data) = std::fs::read(&p) {
                if !data.is_empty() {
                    secret_items.insert(name.to_string(), data);
                }
            }
        }
        let weixin = home.join("weixin");
        if weixin.is_dir() {
            let mut stack = vec![weixin.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_file() && p.extension().is_some_and(|x| x == "json") {
                        if let Ok(data) = std::fs::read(&p) {
                            let rel = p
                                .strip_prefix(&weixin)
                                .map(|r| r.to_string_lossy().to_string())
                                .unwrap_or_default();
                            secret_items.insert(format!("weixin/{}", rel), data);
                        }
                    } else if p.is_dir() {
                        stack.push(p);
                    }
                }
            }
        }
    }

    Ok((entries, secret_items))
}

/// Create the migration package at `out_path`. Emits progress through
/// `progress`.
pub fn pack(
    plan: &PackPlan,
    opts: &PackOptions,
    out_path: &Path,
    progress: &mut ProgressFn,
) -> Result<Manifest, MigratorError> {
    let report = scan::scan_hermes_root(&plan.hermes_home);
    if !report.detected {
        return Err(MigratorError::HermesNotFound(
            plan.hermes_home.display().to_string(),
        ));
    }

    let mut manifest = plan.manifest.clone();
    let (entries, secret_items) = collect_entries(&plan.hermes_home, opts, &report)?;

    // Free-space check: a deflate archive can exceed source size on
    // incompressible data; budget ~1.5x + 8 MiB headroom.
    let total_source: u64 = entries
        .iter()
        .map(|e| e.src.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    let parent = out_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    if let Some(free) = crate::platform::current().free_space_at(&parent) {
        let need = total_source * 3 / 2 + 8 * 1024 * 1024;
        if free < need {
            return Err(MigratorError::InsufficientDiskSpace { need, have: free });
        }
    }

    progress("Preparing Hermes environment", 4, 0, entries.len() as u64);

    // ---- Secrets (encrypted, goes inside the package).
    let mut secret_bundle: Option<secrets::SecretsBundle> = None;
    if let Some(passphrase) = &opts.secrets_passphrase {
        if !secret_items.is_empty() {
            progress("Encrypting secrets", 8, 0, secret_items.len() as u64);
            let items: BTreeMap<String, (String, Vec<u8>)> = secret_items
                .iter()
                .map(|(k, v)| (k.clone(), (k.clone(), v.clone())))
                .collect();
            secret_bundle = Some(secrets::encrypt_secrets(passphrase, items)?);
            manifest.has_secrets = true;
            manifest
                .notes
                .push("secrets.enc included — restore requires the pack passphrase".into());
            progress(
                "Encrypting secrets",
                8,
                secret_items.len() as u64,
                secret_items.len() as u64,
            );
        }
    }

    // ---- Write the ZIP package.
    let staging = out_path.with_extension("partial");
    if staging.exists() {
        let _ = std::fs::remove_file(&staging);
    }

    {
        let file = std::fs::File::create(&staging)?;
        let mut zip = zip::ZipWriter::new(file);
        let zopts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // environment.json
        let env_json = serde_json::to_vec_pretty(&manifest.source_environment).unwrap_or_default();
        zip.start_file("environment.json", zopts)?;
        zip.write_all(&env_json)?;

        // payload files
        let total = entries.len() as u64;
        let mut checksums: BTreeMap<String, String> = BTreeMap::new();
        // `permissions` is only ever *written* on unix (from the mode bits);
        // on Windows it stays an empty read-only map, so `mut` would trip
        // `unused_mut` there under `-D warnings`.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut permissions: BTreeMap<String, u32> = BTreeMap::new();
        let mut per_component: BTreeMap<&'static str, (u64, u64)> = BTreeMap::new();

        for (i, e) in entries.iter().enumerate() {
            let data = std::fs::read(&e.src)?;
            let hex = checksum::bytes_sha256(&data);
            checksums.insert(e.zip_path.clone(), hex);

            if let Ok(md) = e.src.metadata() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode_u32 = md.permissions().mode();
                    if mode_u32 != 0 {
                        permissions.insert(e.zip_path.clone(), mode_u32);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = &md;
                }
            }

            let slot = per_component.entry(e.component).or_insert((0, 0));
            slot.0 += 1;
            slot.1 += data.len() as u64;

            zip.start_file(&e.zip_path, zopts)?;
            zip.write_all(&data)?;

            let pct = 8 + (i.saturating_add(1) as u64 * 84 / total.max(1)) as u8;
            let stage = match e.component {
                "skills" => "Collecting: skills",
                "plugins" => "Collecting: plugins",
                "sessions" => "Collecting: sessions",
                "state_db" => "Collecting: state database",
                "cron" => "Collecting: cron",
                "memories" => "Collecting: memories",
                "kanban" => "Collecting: kanban",
                "pastes" => "Collecting: pastes",
                "config" => "Collecting: configuration",
                "application" => "Collecting: application snapshot",
                _ => "Collecting",
            };
            progress(stage, pct.min(92), i as u64 + 1, total);
        }

        // components
        for (name, (count, bytes)) in per_component.iter() {
            manifest.components.insert(
                name.to_string(),
                PackedComponent {
                    name: name.to_string(),
                    file_count: *count,
                    byte_size: *bytes,
                    checksums: checksums
                        .iter()
                        .filter(|(k, _)| k.starts_with(&format!("payload/{}", name)))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    permissions: permissions
                        .iter()
                        .filter(|(k, _)| k.starts_with(&format!("payload/{}", name)))
                        .map(|(k, v)| (k.clone(), *v))
                        .collect(),
                },
            );
        }

        // secrets.enc
        if let Some(bundle) = &secret_bundle {
            progress("Encrypting secrets", 93, 0, 0);
            let bytes = serde_json::to_vec(bundle)?;
            manifest
                .checksums
                .insert("secrets.enc".into(), checksum::bytes_sha256(&bytes));
            zip.start_file("secrets.enc", zopts)?;
            zip.write_all(&bytes)?;
        }

        // manifest.json (last; now all checksums are final)
        progress("Hashing & writing manifest", 96, 0, 0);
        manifest.checksums.extend(
            checksums
                .iter()
                .filter(|(k, _)| !k.starts_with("secrets.enc"))
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let manifest_json = serde_json::to_vec_pretty(&manifest)?;
        zip.start_file("manifest.json", zopts)?;
        zip.write_all(&manifest_json)?;

        zip.finish()?;
    }

    // Atomic move into final name.
    std::fs::rename(&staging, out_path)?;
    progress("Complete", 100, 0, 0);

    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Read side (used by verify / list / restore).
// ---------------------------------------------------------------------------

/// Open a package and read back its manifest.
pub fn read_manifest(path: &Path) -> Result<Manifest, MigratorError> {
    let manifest_json = read_entry(path, "manifest.json")?;
    serde_json::from_slice(&manifest_json).map_err(|e| MigratorError::InvalidPackage(e.to_string()))
}

/// Read a named top-level entry from the package.
pub fn read_entry(path: &Path, name: &str) -> Result<Vec<u8>, MigratorError> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut f = named_file(&mut archive, name)
        .ok_or_else(|| MigratorError::MissingFile(name.to_string()))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// All `payload/...` paths present in the package (for `list`).
pub fn list_entries(path: &Path) -> Result<Vec<String>, MigratorError> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let f = archive.by_index(i)?;
        let n = f.name().to_string();
        if n.starts_with("payload/") {
            out.push(n);
        }
    }
    out.sort();
    Ok(out)
}

/// Verify every checksum recorded in the manifest against the actual entry
/// bytes. Returns the list of failing paths (empty = integrity passed).
pub fn verify_package(path: &Path) -> Result<(Manifest, Vec<String>), MigratorError> {
    let manifest = read_manifest(path)?;
    manifest.check_compatibility()?;

    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut failures = Vec::new();

    for (rel, want) in manifest.checksums.iter() {
        if let Some(got) = recompute(&mut archive, rel) {
            if got != *want {
                failures.push(rel.clone());
            }
        } else {
            failures.push(rel.clone());
        }
    }

    Ok((manifest, failures))
}

fn recompute(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = named_file(archive, name)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(checksum::bytes_sha256(&buf))
}

fn named_file<'a>(
    archive: &'a mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Option<zip::read::ZipFile<'a>> {
    archive.by_name(name).ok()
}

/// Restore-side access: read the decrypted secrets bundle.
pub fn read_secrets(
    path: &Path,
    passphrase: &str,
) -> Result<BTreeMap<String, Vec<u8>>, MigratorError> {
    let raw = read_entry(path, "secrets.enc")?;
    let bundle: secrets::SecretsBundle =
        serde_json::from_slice(&raw).map_err(|e| MigratorError::Secrets(e.to_string()))?;
    secrets::decrypt_secrets(passphrase, &bundle)
}

/// Restore-side access: read back the source environment snapshot.
pub fn read_environment(path: &Path) -> Result<crate::manifest::SourceEnvironment, MigratorError> {
    let raw = read_entry(path, "environment.json")?;
    Ok(serde_json::from_slice(&raw)?)
}

/// Restore-side access: extract a single payload entry to `dest`.
pub fn extract_entry(path: &Path, zip_name: &str, dest: &Path) -> Result<(), MigratorError> {
    let raw = read_entry(path, zip_name)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, raw)?;
    Ok(())
}
