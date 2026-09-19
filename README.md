# Hermes Agent Migrator

> Move your Hermes Agent environment from one machine to another — safely.
> 把你的 Hermes Agent 环境安全地迁移到另一台电脑。

Hermes Agent Migrator scans a Hermes Agent installation, packages your
configuration and user data into a single portable `.hermesmig` file, and
restores it on the target machine — with integrity verification, automatic
backup, rollback, and encrypted secrets.

它扫描一台机器上的 Hermes Agent 安装，把你的配置与用户数据打包成一个可携带
的 `.hermesmig` 文件，并在目标机器上恢复 —— 全程带完整性校验、自动备份、
失败回滚和加密密钥通道。

```
 ┌─────────────────┐   scan    ┌──────────────────┐   pack    ┌──────────────────┐
 │  Source machine │ ───────▶ │  migrator engine │ ───────▶ │  .hermesmig    │
 │  ~/.hermes/...  │          │  (Rust, shared)  │          │  (portable ZIP)│
 └─────────────────┘          └──────────────────┘          └───────┬──────────┘
                                                                    │
 ┌─────────────────┐   restore   ┌──────────────────┐  verify +     ▼
 │  Target machine │ ◀────────── │  migrator engine │  backup + rollback
 │  ~/.hermes/...  │             │  (GUI / CLI)     │
 └─────────────────┘             └──────────────────┘
```

## Features / 特性

- **One shared engine** — GUI and CLI are thin shells over the same Rust
  core (`migrator-core`), so both behave identically.
  单一核心引擎 —— GUI 与 CLI 都只是同一个 Rust 核心（`migrator-core`）的外壳。
- **Scan** — auto-detects the Hermes home, agent version, and per-component
  status (config / skills / plugins / sessions / state DB / cron / memories /
  kanban / pastes / secrets / application).
  扫描 —— 自动检测 Hermes 目录、Agent 版本与各组件状态。
- **Portable packaging** — machine-local artifacts (venv, node_modules,
  caches, locks) are intentionally *not* shipped; they are rebuilt on the
  target. Only your real data moves.
  可移植打包 —— 机器本地产物（venv、node_modules、缓存、锁）不随包迁移，
  在目标机重建；只搬运真正属于你的数据。
- **Encrypted secrets** — API keys and tokens are AES-256-GCM encrypted
  (Argon2id key derivation) behind a passphrase you choose. OS-native
  keychains are never touched; anything that still needs re-authentication
  is reported explicitly.
  加密密钥 —— API Key 与令牌用口令加密（AES-256-GCM + Argon2id），
  不碰系统钥匙串；仍需重新鉴权的内容会在报告中明确列出。
- **Safe restore** — pre-flight environment check (platform / arch / disk /
  write access / runtime), automatic timestamped backup of the existing
  home, per-file SHA-256 verification, and full rollback if anything
  mismatches. User data is never deleted.
  安全恢复 —— 前置环境检查（平台/架构/磁盘/写权限/运行时）、自动备份现有
  目录、逐文件 SHA-256 校验、任何失配自动整体回滚，绝不删除用户数据。
- **Path repair** — absolute paths inside migrated text files are rewritten
  to the target machine's layout (can be disabled per-restore).
  路径修复 —— 迁移文本文件内的绝对路径自动改写为目标机路径（可关闭）。

## Requirements / 系统要求

| Component | Minimum |
|-----------|---------|
| Hermes Agent | v0.20.x layout (HERMES_LAYOUT_VERSION 1) |
| OS | macOS 13+ / Windows 10+ / Ubuntu 22.04+ |
| For a fresh target | Python 3 (the agent runtime) and ~2 GB free disk |

## Installation / 安装

**GUI (recommended) / GUI（推荐）：** download the installer for your
platform from the [Releases page](https://github.com/93857536-pixel/hermes-agent-migrator/releases).

**CLI:**

```sh
cargo install --path crates/hermes-migrator-cli
# or build from source:
cargo build --release && cp target/release/hermes-migrator ~/bin/
```

## Usage / 使用方法

### GUI

1. **Scan** — click "Run scan"; the app detects your Hermes home and shows
   component status. 扫描 —— 检测你的 Hermes 目录并展示各组件状态。
2. **Package** — choose what to include, optionally add a passphrase for
   encrypted secrets, pick an output path, create the `.hermesmig` file.
   打包 —— 选择要包含的内容，可选口令加密密钥，选择输出路径，生成文件。
3. **Restore** (on the target machine) — point the app at the file, run the
   environment check, enter the passphrase (if any), and restore. Watch the
   live progress; failures roll back automatically. 恢复 —— 在目标机上指定
   文件，运行环境检查，输入口令（如有），开始恢复；失败自动回滚。

### CLI

```sh
# 1) see what would be migrated
hermes-migrator scan

# 2) create a package
hermes-migrator pack --passphrase "s3cret" --out ~/migration.hermesmig

# 3) inspect / integrity-check a package
hermes-migrator list ~/migration.hermesmig
hermes-migrator verify ~/migration.hermesmig

# 4) on the target machine
hermes-migrator restore ~/migration.hermesmig --passphrase "s3cret"
# add --no-path-repair to keep original absolute paths

# 5) standalone backup of the current home
hermes-migrator backup
```

All subcommands accept `--help` for full options. 所有子命令均可 `--help`。

## Package format / 包格式 (`.hermesmig`)

A ZIP container (`Hermes Migration Format v1`):

```
manifest.json     # format version, hermes version, platform/arch, path rules
environment.json  # source-machine snapshot (OS, arch, python, hermes home)
secrets.enc       # AES-256-GCM + Argon2id bundle (only if passphrase given)
payload/<kind>/   # config, skills, sessions, state_db, cron, memories, ...
```

Every payload file is covered by a SHA-256 checksum in the manifest.
每个载荷文件都带 manifest 内的 SHA-256 校验和。

## Development / 开发

```sh
cargo test --workspace       # 16 unit + 4 integration tests
cargo clippy --workspace -- -D warnings
cd gui && npm run build      # frontend
cd gui && cargo tauri dev    # run the GUI in dev
```

Layout:

```
crates/migrator-core      # pure Rust engine (scan/pack/restore/verify/secrets)
crates/hermes-migrator-cli# clap CLI
gui/                      # Tauri v2 + React/TS frontend (three screens)
tests/                    # e2e helper fixtures
```

## Security notes / 安全说明

- The migration tool **never** reads or writes OS keychains or credential
  stores. 迁移工具从不读写系统钥匙串或凭据库。
- Secrets stay encrypted at rest inside the package; the passphrase is never
  stored by the app. 密钥在包内始终以密文存放；口令不会被应用保存。
- Restores take a full backup first; a failed verification rolls back to
  that backup. 恢复前自动备份；校验失败自动回滚到备份。
- Report items marked "needs re-authentication" are credentials (e.g.
  device-bound tokens) that cannot be migrated by design. 标记为"需要重新
  鉴权"的项是设计如此、无法迁移的凭据（如设备绑定的 token）。

## License / 许可证

[MIT](LICENSE) — see `LICENSE` for details. 详见 `LICENSE`。

## Contributing / 贡献

Forks, PRs, and issues are welcome. When reporting a migration failure,
attach the output of `hermes-migrator verify <pkg>` and the restore report.
欢迎提交 PR 与 Issue；报告迁移失败时请附上 `verify` 输出与恢复报告。
