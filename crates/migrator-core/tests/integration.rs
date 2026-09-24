//! Integration test: the full pack → verify → restore → secrets → rollback
//! loop on throwaway fake hermes homes. Nothing here touches the real
//! `~/.hermes`.

use std::io::Write;
use std::path::Path;

use migrator_core::pack::{self, PackOptions};
use migrator_core::restore::{self, RestoreOptions};
use migrator_core::scan;
use migrator_core::MigratorError;

fn noop_progress() -> pack::ProgressFn {
    Box::new(|_stage, _pct, _done, _total| {})
}

/// Build a fake source home tree under `base/.hermes` and return its path.
fn make_fake_home(base: &Path) -> std::path::PathBuf {
    let hermes = base.join(".hermes");
    // Forward-slash-normalized: the path-repair engine matches its rules in
    // `/` form (see `pathmapper`), so the fixture writes source refs in that
    // form too. On unix this is identical to `display()`; on Windows it
    // avoids backslashes the `/`-based rules would never match.
    let hs = hermes.to_string_lossy().replace('\\', "/");
    std::fs::create_dir_all(hermes.join("skills/demo")).unwrap();
    std::fs::create_dir_all(hermes.join("memories")).unwrap();
    std::fs::create_dir_all(hermes.join("weixin/accounts")).unwrap();
    std::fs::write(
        hermes.join("config.yaml"),
        format!("workdir: {hs}\nstate: {hs}/state.db\n"),
    )
    .unwrap();
    std::fs::write(hermes.join("skills/demo/SKILL.md"), "# demo skill").unwrap();
    std::fs::write(hermes.join("memories/user.md"), "prefers coffee").unwrap();
    std::fs::write(hermes.join("state.db"), b"sqlite-fake-bytes").unwrap();
    // Secrets: .env + weixin tokens (collected into secrets.enc at pack time).
    std::fs::write(hermes.join(".env"), "OPENAI_API_KEY=sk-test-123\n").unwrap();
    std::fs::write(
        hermes.join("weixin/accounts/token.json"),
        r#"{ "access_token": "wx-test-token" }"#,
    )
    .unwrap();
    hermes
}

fn empty_map() -> std::collections::BTreeMap<String, migrator_core::manifest::PackedComponent> {
    std::collections::BTreeMap::new()
}

#[allow(dead_code)]
fn _unused_empty_map() {
    let _ = empty_map();
}

#[test]
fn pack_verify_restore_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let src_home = make_fake_home(tmp.path());

    // ---- pack.
    let mut opts = PackOptions::default();
    opts.secrets_passphrase = Some("hunter2".into());
    let plan = pack::plan_pack(&scan::scan_hermes_root(&src_home), &opts);
    let pkg = tmp.path().join("mig.hermesmig");
    let manifest = pack::pack(&plan, &opts, &pkg, &mut noop_progress()).unwrap();
    assert!(
        manifest.has_secrets,
        "secrets bundle must be in the package"
    );
    assert!(pkg.exists(), "package file written");

    // ---- verify package integrity (clean => no failures).
    let (m, failures) = pack::verify_package(&pkg).unwrap();
    let _ = m;
    assert!(
        failures.is_empty(),
        "clean package must verify: {failures:?}"
    );

    // ---- restore into a *different* fake target (never the real home).
    let target = tmp.path().join("target-home");
    let mut ropts = RestoreOptions::default();
    ropts.target_hermes_home = Some(target.clone());
    ropts.passphrase = Some("hunter2".into());
    let (restored, report) = restore::restore(&pkg, &ropts, &mut noop_progress()).unwrap();
    assert_eq!(restored.format_version, manifest.format_version);

    // Regular payload files landed.
    let cfg = std::fs::read_to_string(target.join("config.yaml")).unwrap();
    let skill = std::fs::read_to_string(target.join("skills/demo/SKILL.md")).unwrap();
    assert_eq!(skill, "# demo skill");
    std::fs::read(target.join("state.db")).unwrap();

    // Path repair: source-home references in config.yaml now point at target.
    // The repair engine emits forward-slash paths on every platform, so
    // assert against the normalized form (identical to `display()` on unix).
    let target_str = target.to_string_lossy().replace('\\', "/");
    assert!(
        cfg.contains(&format!("workdir: {target_str}"))
            || cfg.contains(&format!("workdir: {target_str}/")),
        "path repair must rewrite source home -> target home, got: {cfg}"
    );
    let src_home_str = src_home.to_string_lossy().replace('\\', "/");
    assert!(!cfg.contains(&src_home_str), "source path leaked: {cfg}");

    // Secrets restored, locked to 0600 on Unix.
    assert_eq!(
        report.secrets_restored, 2,
        ".env + weixin token must restore"
    );
    let env = std::fs::read_to_string(target.join(".env")).unwrap();
    assert!(
        env.contains("sk-test-123"),
        "secret content must round-trip"
    );
    let tok = std::fs::read_to_string(target.join("weixin/accounts/token.json")).unwrap();
    assert!(tok.contains("wx-test-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target.join(".env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "restored secrets must be 0600");
    }

    // Every requested component verified clean.
    assert!(
        report.verified.iter().all(|(_, ok)| *ok),
        "component verification failed: {:?}",
        report.verified
    );
}

#[test]
fn wrong_passphrase_surfaces_reauth_not_fake_success() {
    let tmp = tempfile::tempdir().unwrap();
    let src_home = make_fake_home(tmp.path());
    let mut opts = PackOptions::default();
    opts.secrets_passphrase = Some("right-pass".into());
    let plan = pack::plan_pack(&scan::scan_hermes_root(&src_home), &opts);
    let pkg = tmp.path().join("mig.hermesmig");
    pack::pack(&plan, &opts, &pkg, &mut noop_progress()).unwrap();

    let target = tmp.path().join("target-home");
    let mut ropts = RestoreOptions::default();
    ropts.target_hermes_home = Some(target.clone());
    ropts.passphrase = Some("WRONG-pass".into());
    // restore itself succeeds (secrets are a soft step) but MUST report re-auth.
    let (_m, report) = restore::restore(&pkg, &ropts, &mut noop_progress()).unwrap();
    assert_eq!(report.secrets_restored, 0);
    assert!(
        !report.needs_reauth.is_empty(),
        "wrong passphrase must be surfaced, not silently skipped"
    );
    assert!(
        !target.join(".env").exists(),
        "no plaintext secret may land"
    );
}

/// Rebuild the package zip with one payload entry tampered, keeping
/// manifest.json (and its checksums) intact — the exact scenario the
/// rollback path must undo.
fn tamper_zip_entry(src: &Path, out: &Path, entry: &str, extra: &[u8]) {
    let mut in_archive = zip::ZipArchive::new(std::fs::File::open(src).unwrap()).unwrap();
    let file = std::fs::File::create(out).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let zopts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..in_archive.len() {
        let mut f = in_archive.by_index(i).unwrap();
        let name = f.name().to_string();
        writer.start_file(&name, zopts.clone()).unwrap();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut data).unwrap();
        if name == entry {
            data.extend_from_slice(extra);
        }
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn checksum_tamper_rolls_back_previous_state() {
    let tmp = tempfile::tempdir().unwrap();
    let src_home = make_fake_home(tmp.path());
    let opts = PackOptions::default();
    let plan = pack::plan_pack(&scan::scan_hermes_root(&src_home), &opts);
    let pkg = tmp.path().join("mig.hermesmig");
    pack::pack(&plan, &opts, &pkg, &mut noop_progress()).unwrap();

    // Pre-existing target state that the rollback must survive.
    let target = tmp.path().join("target-home");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("config.yaml"), "old-user-config\n").unwrap();

    let tampered = tmp.path().join("tampered.hermesmig");
    tamper_zip_entry(
        &pkg,
        &tampered,
        "payload/config/config.yaml",
        b"tampered-by-attacker",
    );

    let mut ropts = RestoreOptions::default();
    ropts.target_hermes_home = Some(target.clone());
    ropts.apply_path_repair = false; // keep assertions about bytes exact
    let err = restore::restore(&tampered, &ropts, &mut noop_progress())
        .err()
        .expect("tampered package must FAIL, never fake success");
    assert!(
        matches!(err, MigratorError::IntegrityFailed(_)),
        "expected IntegrityFailed, got: {err:?}"
    );

    // Rollback restored the pre-existing bytes verbatim.
    let cfg = std::fs::read_to_string(target.join("config.yaml")).unwrap();
    assert_eq!(cfg, "old-user-config\n", "rollback must restore old state");
}

#[test]
fn backup_hermes_home_skips_machine_local_trees() {
    let tmp = tempfile::tempdir().unwrap();
    let home = make_fake_home(tmp.path());
    // Machine-local junk that must NOT be backed up:
    std::fs::create_dir_all(home.join("hermes-agent/venv")).unwrap();
    std::fs::write(home.join("hermes-agent/venv/pyvenv.cfg"), "x").unwrap();

    let dest = restore::backup_hermes_home(&home).unwrap();
    assert!(dest.is_dir());
    assert!(
        !dest.join("hermes-agent").exists(),
        "venv tree must be skipped"
    );
    assert!(dest.join("config.yaml").exists(), "user data must be kept");
    assert!(dest.join(".env").exists());
}

// silence the unused-helper warning if a future refactor drops it
#[allow(dead_code)]
fn _keep(_x: u8) {
    let _ = empty_map();
}
