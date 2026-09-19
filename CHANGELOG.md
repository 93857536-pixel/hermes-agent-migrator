# Changelog

All notable changes to **Hermes Agent Migrator** are documented here.
This project follows [Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-09-19

### Added — first public release

- **Migration engine** (`migrator-core`, shared by GUI and CLI):
  - Environment scanner: auto-detects the Hermes Agent home, agent version,
    and per-component status (config, skills, plugins, sessions, state DB,
    cron, memories, kanban, pastes, secrets, application).
  - Package format: `Hermes Migration Format v1` — a `.hermesmig` ZIP
    container with `manifest.json`, per-file SHA-256 `checksums`, an
    `environment.json` source snapshot, and an encrypted `secrets.enc`.
  - **Secrets encryption**: API keys / tokens are encrypted with
    AES-256-GCM, key derived via Argon2id from a user passphrase.
    OS-native keychains are never touched; items that need re-auth are
    explicitly reported.
  - **Portable by design**: machine-local artifacts (venv, node_modules,
    caches, locks, `bin/`) are excluded and re-installed on the target;
    user data (config, skills, sessions, DBs, memories, weixin layout…)
    is migrated with absolute-path repair back to the target machine.
  - **Restore safety**: pre-restore environment check (platform / arch /
    write access / disk space / runtime), automatic backup of the existing
    home, per-file integrity verification, and full rollback on any
    checksum mismatch. User data is never deleted.
- **CLI** (`hermes-migrator`): `scan`, `pack`, `verify`, `restore`,
  `list`, `manifest`, `backup` subcommands with rich output.
- **GUI** (Tauri v2 + React/TS, three screens): Scan → Package → Restore,
  bilingual (English / 简体中文), light & dark themes, live progress
  events, environment-check display, rollback notifications.
- **CI/CD**: cross-platform GitHub Actions — core tests on
  Ubuntu/Windows/macOS, GUI builds + installers (Windows NSIS, macOS DMG)
  on tag push, auto GitHub Releases.

[0.1.0]: https://github.com/93857536-pixel/hermes-agent-migrator/releases/tag/v0.1.0
