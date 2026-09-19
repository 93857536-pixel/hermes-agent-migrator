# Cloud Configuration / 云端配置

Hermes Agent Migrator can securely store **one encrypted cloud
configuration per device**. Each device keeps at most one active
configuration; it can be overwritten or deleted and re-uploaded.

Hermes Agent Migrator 可为**每台设备**安全存储**一份加密云端配置**。
每台设备最多保留一份有效配置，可覆盖或删除后重新上传。

---

## How cloud backup works / 云端备份工作原理

```
Hermes Agent Configuration
        ↓
Local Encryption (AES-256-GCM + Argon2id, client-side)
        ↓
Encrypted Blob  ──(SHA-256)──►
        ↓
HTTPS
        ↓
Cloud API
        ↓
Alibaba Cloud OSS
```

- **Client-side encryption.** The plaintext configuration never leaves the
  machine in any readable form. The server stores only the sealed blob plus
  metadata. 客户端加密：明文配置永不以可读形式离开本机；服务器只存封密数据
  与元数据。
- **The server can never read your data.** It does not store your
  passphrase. If you lose it, the cloud service cannot decrypt your
  configuration for you. 服务器读不到你的数据，也不存你的口令；口令丢失则
  云端无法替你解密。
- **One device → one configuration.** A device may hold at most one active
  cloud configuration. CREATE is only allowed when absent; OVERWRITE is
  only allowed when present. 一台设备最多一份有效配置：无则可创建，有则只能
  覆盖。
- **Atomic replacement.** An overwrite writes a temporary object, verifies
  it, then atomically replaces the active one; the previous object is moved
  to a 24 h orphan queue and swept by the cleanup job. 覆盖是原子的：写临时
  对象→校验→原子替换，旧对象进入 24 小时孤儿队列后由清理任务回收。

## Device identity / 设备标识

- The **Device ID is a random UUID v4**, generated on first cloud use and
  stored in `~/.hermes-migrator/device.json`. It is **not** a hardware
  fingerprint: it carries no user name, host name, serial, or MAC, and
  cannot be used for advertising or tracking. 设备标识为随机 UUIDv4，
  首次使用云功能时生成，**不是**硬件指纹，不含用户名/主机名/序列号/MAC，
  不可用于广告或追踪。
- The same machine keeps one id across Wi-Fi / VPN / ISP changes. 同一台
  机器在切换 Wi-Fi/VPN/运营商后保持同一标识。

## What the cloud service may process / 云服务可能处理的信息

| Item | Why |
|------|-----|
| Account identifier | cloud storage, configuration management |
| Device identifier | device identification, one-device-one-config |
| Public IP | security, diagnostics, abuse prevention |
| Configuration metadata | size / sha / uploaded-at bookkeeping |
| Encrypted configuration blob | the stored (readable-no) config |
| Diagnostic information | debugging, with user review before upload |

**Never used for:** advertising profiling, selling user information, or
unrelated tracking. 绝不用于：广告画像、出售用户信息、无关追踪。

## Commands / 命令

```sh
# device status
hermes-migrator cloud status

# test reachability + device identity
hermes-migrator cloud test

# upload (or overwrite) the current configuration
hermes-migrator cloud upload --passphrase "s3cret"
hermes-migrator cloud upload --passphrase "s3cret" --overwrite

# download THE configuration and restore it onto this machine
hermes-migrator cloud restore --passphrase "s3cret"

# delete this device's configuration
hermes-migrator cloud delete
```

The GUI's **Cloud** tab and **Settings → Cloud** offer the same operations,
with a first-run "Cloud Storage Notice" and a Test-Connection panel. GUI 的
**云端**标签与 **设置 → 云端** 提供相同操作，含首启「云端存储须知」与
「连接测试」面板。

## Error codes / 错误码

Stable wire codes surfaced by the API and the CLI: 由 API 与 CLI 暴露的稳定
线格式错误码。

| Code | Meaning |
|------|---------|
| `CONFIGURATION_NOT_FOUND` | device has no active configuration |
| `CONFIGURATION_ALREADY_EXISTS` | CREATE attempted while one exists |
| `CONFIGURATION_OVERWRITE_NOT_AUTHORIZED` | OVERWRITE attempted when absent |
| `CONFIGURATION_SIZE_LIMIT_EXCEEDED` | sealed blob exceeds the 50 MiB cap |
| `CONFIGURATION_UPLOAD_FAILED` | the upload / write could not complete |
| `CONFIGURATION_DELETE_FAILED` | the delete could not complete |
| `CONFIGURATION_INTEGRITY_CHECK_FAILED` | SHA-256 of blob ≠ stored sha |
| `DECRYPTION_FAILED` | wrong passphrase / corrupted ciphertext |
| `RATE_LIMITED` | too many operations in the window |
| `DEVICE_NOT_REGISTERED` / `DEVICE_NOT_AUTHORIZED` | bad / unknown device token |

## See also / 相关

- `docs/cloud-alibaba.md` — the production backend (Cloud API + OSS + DB)
  reference design. 生产后端（云 API + OSS + 数据库）参考设计。
- `docs/privacy.md`, `docs/security.md` — data handling & cryptography.
  数据处理与加密说明。
