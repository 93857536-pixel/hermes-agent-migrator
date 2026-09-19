//! hermes-migrator — CLI front-end for the migrator-core engine.
//!
//! Commands: scan | pack | restore | verify | list | backup
//! GUI (Tauri) and this CLI share the exact same core: no duplicated logic.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use migrator_core::pack::{self, PackOptions};
use migrator_core::restore::{self, RestoreOptions};
use migrator_core::scan;

#[derive(Parser)]
#[command(
    name = "hermes-migrator",
    version,
    about = "Cross-platform migration & backup tool for Hermes Agent",
    long_about = "Scans a Hermes Agent environment, packages it into a single \
                  .hermesmig file (encrypted secrets included), and restores it \
                  on another machine with automatic path repair and integrity \
                  verification."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Be quiet (no human-friendly log lines, only errors).
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan the local Hermes Agent environment.
    Scan,
    /// Create a migration package.
    Pack(PackArgs),
    /// Restore a migration package onto this machine.
    Restore(RestoreArgs),
    /// Verify package integrity (checksums).
    Verify(PkgPath),
    /// List the contents of a migration package.
    List(PkgPath),
    /// Back up the existing hermes home (timestamped directory).
    Backup,
}

#[derive(Args)]
struct PkgPath {
    /// Path to a .hermesmig package.
    package: PathBuf,
}

#[derive(Args)]
struct PackArgs {
    /// Where to write the package (default: ~/HermesAgent-<date>.hermesmig).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Passphrase that encrypts the secrets bundle (omitted => secrets not
    /// packaged).
    #[arg(long)]
    passphrase: Option<String>,
    /// Also include the application source snapshot (excludes venv/
    /// node_modules; rebuilt on the target).
    #[arg(long)]
    include_application: bool,
    /// Emit the manifest as JSON to stdout instead of a human summary.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct RestoreArgs {
    /// Path to the .hermesmig package to restore.
    package: PathBuf,
    /// Passphrase for the encrypted secrets (prompted if the package has
    /// them and this flag is absent).
    #[arg(long)]
    passphrase: Option<String>,
    /// Restore into a specific hermes home (default: this machine's).
    #[arg(long)]
    target: Option<PathBuf>,
    /// Only restore these comma-separated components.
    #[arg(long, value_delimiter = ',')]
    components: Option<Vec<String>>,
    /// Skip automatic path repair (leave source paths intact).
    #[arg(long)]
    no_path_repair: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let result = match &cli.cmd {
        Cmd::Scan => cmd_scan(),
        Cmd::Pack(a) => cmd_pack(a),
        Cmd::Restore(a) => cmd_restore(a),
        Cmd::Verify(p) => cmd_verify(&p.package),
        Cmd::List(p) => cmd_list(&p.package),
        Cmd::Backup => cmd_backup(),
    };
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("error: {e}");
            Err(e)
        }
    }
}

fn say(quiet: bool, msg: &str) {
    if !quiet {
        println!("{msg}");
    }
}

fn cmd_scan() -> Result<()> {
    let home = scan::locate_hermes_home()?;
    let report = scan::scan_hermes_root(&home);
    println!(
        "Hermes Agent: {}",
        if report.detected {
            "detected"
        } else {
            "not detected"
        }
    );
    println!("Hermes home: {}", home.display());
    if let Some(inst) = &report.install {
        println!(
            "Version: {}  ({} install at {})",
            inst.version, inst.install_kind, inst.root
        );
    }
    for (name, status) in &report.components {
        println!("  {} {name}", status.symbol());
    }
    println!(
        "\nTotals: {} files, {} bytes",
        report.total_files, report.total_bytes
    );
    for note in &report.notes {
        println!("note: {note}");
    }
    Ok(())
}

fn cmd_pack(args: &PackArgs) -> Result<()> {
    let quiet = false; // progress goes through the progress closure below
    let home = scan::locate_hermes_home()?;
    let report = scan::scan_hermes_root(&home);
    if !report.detected {
        anyhow::bail!("no Hermes Agent found under {}", home.display());
    }

    let opts = PackOptions {
        include_application: args.include_application,
        secrets_passphrase: args.passphrase.clone(),
        ..PackOptions::default()
    };

    let out = args.out.clone().unwrap_or_else(|| {
        let dir = std::env::temp_dir();
        dir.join("HermesAgent-migration.hermesmig")
    });

    let plan = pack::plan_pack(&report, &opts);

    say(quiet, "packing…");
    let mut progress: pack::ProgressFn = Box::new(|stage: &str, pct: u8, done: u64, total: u64| {
        if total > 0 {
            println!("{stage:40} {pct:3}%  [{done}/{total}]");
        } else {
            println!("{stage:40} {pct:3}%");
        }
    });
    let manifest = pack::pack(&plan, &opts, &out, &mut progress)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!("\nMigration complete: {}", out.display());
        for (name, c) in &manifest.components {
            println!("  {name:12} {} files, {} bytes", c.file_count, c.byte_size);
        }
        if manifest.has_secrets {
            println!("  secrets:   encrypted (passphrase required to restore)");
        }
    }
    Ok(())
}

fn prompt_passphrase() -> Result<String> {
    // A minimal blocking read of a line; the GUI path never uses this.
    eprint!("secrets passphrase: ");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn cmd_restore(args: &RestoreArgs) -> Result<()> {
    // 1) environment check
    let env = restore::check_environment(&args.package)?;
    println!("environment check:");
    for item in &env.items {
        let mark = if item.ok { "✓" } else { "✗" };
        println!("  {mark} {} — {}", item.name, item.detail);
    }
    if !env.ready {
        anyhow::bail!(
            "migration cannot continue: {}",
            env.reason.unwrap_or_else(|| "environment not ready".into())
        );
    }

    // 2) passphrase (required when the package has secrets)
    let needs = needs_passphrase(&args.package)?;
    let passphrase = if needs {
        match args.passphrase.clone() {
            Some(p) => Some(p),
            None => Some(prompt_passphrase()?),
        }
    } else {
        args.passphrase.clone()
    };

    let opts = RestoreOptions {
        passphrase,
        target_hermes_home: args.target.clone(),
        only_components: args.components.clone(),
        apply_path_repair: !args.no_path_repair,
    };

    // 3) auto-backup of an existing home before touching it (rollback safety)
    let target = opts
        .target_hermes_home
        .clone()
        .unwrap_or_else(locate_hermes_home_default);
    let backup_dir = if target.join("config.yaml").exists() {
        let b = restore::backup_hermes_home(&target)?;
        say(
            false,
            &format!(
                "existing hermes home found — auto-backup written to {}",
                b.display()
            ),
        );
        Some(b)
    } else {
        None
    };
    let _ = backup_dir; // kept on disk; rollback uses in-memory snapshot inside
                        // restore() for file-level undo, this is the extra net.

    let mut progress: pack::ProgressFn = Box::new(|stage: &str, pct: u8, done: u64, total: u64| {
        if total > 0 {
            println!("{stage:40} {pct:3}%  [{done}/{total}]");
        } else {
            println!("{stage:40} {pct:3}%");
        }
    });
    let (manifest, report) = restore::restore(&args.package, &opts, &mut progress)?;

    println!("\nrestore complete:");
    for (name, ok) in &report.verified {
        let mark = if *ok { "✓" } else { "✗" };
        println!("  {mark} {name}");
    }
    if report.secrets_restored > 0 {
        println!("  ✓ {} secret(s) restored", report.secrets_restored);
    }
    if !report.needs_reauth.is_empty() {
        for n in &report.needs_reauth {
            println!("  ⚠ {n}");
        }
    }
    let _ = manifest;
    Ok(())
}

/// Whether the package stores an encrypted secrets bundle.
fn needs_passphrase(pkg: &std::path::Path) -> Result<bool> {
    let m = pack::read_manifest(pkg)?;
    Ok(m.has_secrets)
}

fn cmd_verify(pkg: &std::path::Path) -> Result<()> {
    let (manifest, failures) = pack::verify_package(pkg)?;
    if failures.is_empty() {
        println!(
            "integrity check passed: {} files, format v{}, hermes {}",
            manifest.checksums.len(),
            manifest.format_version,
            manifest.hermes_version
        );
        Ok(())
    } else {
        println!(
            "integrity check FAILED: {} of {} checksums mismatched",
            failures.len(),
            manifest.checksums.len()
        );
        for f in &failures {
            println!("  ✗ {f}");
        }
        anyhow::bail!("integrity check failed")
    }
}

fn cmd_list(pkg: &std::path::Path) -> Result<()> {
    let m = pack::read_manifest(pkg)?;
    println!(
        "package: hermes {} · {} · created {}\n",
        m.hermes_version, m.platform, m.created_at
    );
    for (name, c) in &m.components {
        println!("  {name:12} {} files, {} bytes", c.file_count, c.byte_size);
    }
    if m.has_secrets {
        println!("  secrets      encrypted bundle present");
    }
    let entries = pack::list_entries(pkg)?;
    println!(
        "\n{} payload entries (run with -v to see all)",
        entries.len()
    );
    Ok(())
}

fn cmd_backup() -> Result<()> {
    let home = scan::locate_hermes_home()?;
    let dest = restore::backup_hermes_home(&home)?;
    println!("backup written to {}", dest.display());
    Ok(())
}

/// locate_hermes_home that never panics (for the backup target decision).
fn locate_hermes_home_default() -> PathBuf {
    scan::locate_hermes_home().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(".hermes"))
            .unwrap_or_else(|| PathBuf::from(".hermes"))
    })
}
