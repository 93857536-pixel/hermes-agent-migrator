# Security / 安全说明

## Cryptography / 加密

- **Secrets** are encrypted **client-side** with AES-256-GCM; the key is
  derived from your passphrase with **Argon2id**. The app never stores the
  passphrase. 密钥在**客户端**用 AES-256-GCM 加密；密钥由口令经 **Argon2id**
  派生。应用从不保存口令。
- **Cloud configurations** reuse the same primitives: the sealed blob is
  `kdf + payload`, and the server stores both fields opaquely. It never sees
  the plaintext or the passphrase. 云端配置复用同一套原语：封密数据块为
  `kdf + payload`，服务端将两字段原样存放，永远看不到明文或口令。
- **Integrity**: every blob is covered by a SHA-256 digest; the backend
  re-checks it on download and fails closed on mismatch (a `dead` sha ⇒
  `CONFIGURATION_INTEGRITY_CHECK_FAILED`). 完整性：每个数据块带 SHA-256
  摘要，后端下载时复检，失败即封闭（sha 不符 ⇒ 完整性校验失败）。

## Machine-specific artifacts / 机器相关的产物

`venv`, `node_modules`, and OS-native keychains are **not** migrated —
they are re-installed / re-authenticated on the target machine. 机器相关产物
（venv、node_modules、系统钥匙串）不随迁移迁移，在目标机重新安装 / 重新鉴权。

## Restore safety / 恢复安全

- A full backup is taken before any restore; failed verification **rolls
  back** to that backup. 恢复前先全量备份；校验失败自动回滚到备份。
- Restored secrets are written back with restrictive `0600` permissions.
  恢复的密钥以严格的 0600 权限写回。

## Cloud reference backend / 云端参考后端

The shipped backend (`migrator_core::cloudserver`) is a local, file-backed
reference implementation that enforces the full one-device / one-configuration
contract: registration, create-only-when-absent, overwrite-only-when-present,
atomic replacement, 24 h orphan sweep, per-device rate limiting, and the 50 MiB
size cap. A production deployment replaces this module with the Cloud API +
OSS + database described in `docs/cloud-alibaba.md`; the public surface is the
same. 随附后端为本地文件参考实现，完整执行单设备单配置契约；生产部署以云 API +
OSS + 数据库替换该模块，公共接口不变。

## Re-viewing in the app / 应用内查看

**Settings → Security** shows this summary. 应用内 **设置 → 安全** 展示摘要。
