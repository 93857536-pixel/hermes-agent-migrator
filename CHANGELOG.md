# Changelog

All notable changes to **Hermes Agent Migrator** are documented here.
This project follows [Semantic Versioning](https://semver.org/).

## [0.2.0] — 2026-09-19

### Added — Cloud configuration (one device, one configuration)

- **Cloud engine** (`migrator-core::cloud` + `cloudserver`): a
  random-UUID `DeviceId` (never a hardware fingerprint), client-side
  encryption reusing the AES-256-GCM + Argon2id secrets primitives, and
  the one-device / one-configuration contract — CREATE only when absent,
  OVERWRITE only when present, atomic replacement, 50 MiB sealed-blob cap,
  per-device rate limiting, a 24 h orphan cleanup job, and full multi-device
  isolation. Ships with an embeddable, file-backed reference server so the
  whole system is runnable and testable without a live deployment
  (15 cloud tests).
- **GUI**: a new **Cloud** screen (available / no-configuration states,
  upload, overwrite, delete, download-and-restore, first-run cloud-storage
  notice, Test Connection) and a new **Settings** screen (server status,
  re-viewable Privacy / Security / Important Information). The app now
  opens with a **first-launch disclaimer gate**. i18n is now three-way:
  **English / 简体中文 / 繁體中文**.
- **CLI**: `hermes-migrator cloud {status, upload, restore, delete, test}`.
- **Docs**: `docs/cloud.md`, `docs/privacy.md`, `docs/security.md`, and
  `docs/cloud-alibaba.md` (production backend reference: Cloud API + OSS +
  database — one-device API, OSS object layout, rate limits, reconciliation,
  cleanup, storage monitoring). README gains a Cloud section + CLI examples.

[0.2.0]: https://github.com/93857536-pixel/hermes-agent-migrator/releases/tag/v0.2.0

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
