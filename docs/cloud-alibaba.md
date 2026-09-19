# Alibaba Cloud Backend Reference / 阿里云生产对接参考

This document describes the **production backend** for the cloud
configuration feature: Cloud API + Alibaba Cloud OSS + database. The
shipped, runnable reference implementation (`migrator_core::cloudserver`)
honours the same public surface, so the client, CLI, GUI and tests all run
without a live deployment; swapping in this backend is a drop-in.

本文档描述云端配置功能的**生产后端**：云 API + 阿里云 OSS + 数据库。
随附的参考实现（`migrator_core::cloudserver`）遵守同一套公共接口，客户端、
CLI、GUI 与测试均可在无实际部署的情况下运行；接入本后端即"换而不用改"。

---

## 1. Architecture / 架构

```
Client (Windows / macOS)
   Hermes Agent Migrator
            │  HTTPS (per-device token)
            ▼
        Cloud API  ────────  Load Balancer
            │
            ▼
   Alibaba Cloud
   ┌────────┴────────┐
   ▼                 ▼
Database            OSS
(accounts, tokens,  (sealed blobs +
 one active cfg      orphan objects)
 per device, audit,
 rate-limit counters)
```

### Client boundary / 客户端边界

- The app talks **only** to the **Cloud API**. 应用只访问**云 API**。
- The app **never** holds or transmits Alibaba Cloud **AccessKey /
  Secret**. Credentials live server-side. 应用**绝不**持有或传输阿里云
  AccessKey / Secret；密钥只在服务端。
- The app never talks to OSS or the database directly. 应用从不直接访问
  OSS 或数据库。
- The client only ever sees an **opaque encrypted blob** plus public
  metadata. It never learns server-internal details. 客户端只看到**不透明
  加密数据块**与公开元数据，从不知道服务器内部细节。

---

## 2. One Device → One Configuration / 单设备单配置语义

The invariant the whole system enforces:

```
   One Device
     → One active cloud configuration
```

| Operation | Precondition | Result |
|-----------|--------------|--------|
| **REGISTER** | always | returns a per-device bearer token |
| **CREATE** | device has **no** active config | active config created; a second CREATE → `CONFIGURATION_ALREADY_EXISTS` |
| **OVERWRITE** | device **has** an active config | active config atomically replaced; the old object becomes an orphan |
| **DOWNLOAD** | active config present | returns the sealed blob + expected SHA-256 |
| **DELETE** | active config present | active config removed; the blob moves to the 24 h orphan queue |
| **rate limit** | — | ≤ N operations / window, else `RATE_LIMITED` |

Concurrent operations on one device are serialised (single guard
acquisition per operation) so that a 100-way concurrent upload race yields
exactly one winner and 99 `RATE_LIMITED` / serialised outcomes — never two
active configurations. 单设备上的并发操作串行化（每次操作取一次锁）：100 路
并发上传竞争只会产生一个赢家，其余被限流/串行，绝不会得到两份有效配置。

---

## 3. OSS layout / OSS 对象结构

```
bucket/
  configs/
    {device_id}/
      {sha256}.enc            ← active sealed blob (deterministic key)
      {sha256}.enc.tmp        ← in-flight temp object (atomic replace)
  orphans/
    {device_id}-{sha256}.enc  ← replaced / deleted blobs, swept after 24 h
    {device_id}-{sha256}.ttl  ← expiry timestamp (ms)
```

- **Deterministic key** `{device_id}/{sha256}`: re-uploading an identical
  configuration is a no-op (same object). 确定性 key：重复上传相同配置为
  幂等（同一对象）。
- **Atomic replacement**: write `{sha}.enc.tmp`, verify SHA-256, then
  atomically rename into `{sha}.enc`. The previous object is renamed to
  `orphans/` with a 24 h TTL. 原子替换：写 `.tmp` → 校验 → 原子 rename；旧
  对象移入 `orphans/` 并带 24 h 有效期。
- **No unbounded growth**: overwrites never accumulate active objects;
  orphans are swept by the cleanup job. 覆盖不会产生无限堆积；孤儿由清理
  任务回收。

---

## 4. Rate limiting & size caps / 限流与大小上限

Per-device counters (windowed, e.g. 60 s):

| Limit | Default | Exceeding error |
|-------|---------|-----------------|
| Request rate | 60 req/min | `RATE_LIMITED` |
| Upload size | 50 MiB sealed blob | `CONFIGURATION_SIZE_LIMIT_EXCEEDED` |
| Download rate | 120 req/min | `RATE_LIMITED` |
| Diagnostic upload | 2 MiB log | `DIAGNOSTIC_SIZE_LIMIT_EXCEEDED` |

Defaults are tuned to prevent a single malicious or misbehaving device from
saturating the service. 默认值用于防止单台恶意/异常设备拖垮服务。

---

## 5. Storage monitoring & audit / 存储监控与对账

Admin dashboard data (the reference backend exposes the same via
`audit_active_configs()` / `object_count()` / `sweep_orphans()`):

- Total devices
- Active configurations
- Total encrypted storage
- Temporary objects in flight
- Orphaned objects (and their TTL)
- Diagnostic storage

Reconciliation: active-config count in the database must equal the count of
live `configs/{id}/{sha}.enc` objects. A cleanup pass may delete orphans
only — it **must never** delete an active configuration. 对账：数据库里的
有效配置数必须等于 `configs/{id}/{sha}.enc` 存活对象数；清理任务只删孤儿，
**绝不**删有效配置。

---

## 6. Cleanup job / 清理任务

- Orphan objects expire after **24 h** (configurable). 孤儿对象 **24 h** 后
  过期（可配）。
- The job deletes expired orphans + any stale `.tmp` temp object older than
  N hours. 任务删除过期孤儿与超过 N 小时的残留 `.tmp`。
- A crash mid-overwrite leaves at most one orphan; it is swept on the next
  run. 覆盖中途崩溃最多遗留一个孤儿，下次运行回收。
- Soft-delete (if chosen) **must** have a retention period ending in a final
  permanent purge — the goal is to stop the cloud accumulating old
  configurations. 若采用软删除，必须设保留期并最终永久清理。

---

## 7. API error codes / API 错误码

Stable wire codes (see `migrator_core::cloud::CloudError::code`):

| Code | HTTP-ish | Meaning |
|------|----------|---------|
| `CONFIGURATION_ALREADY_EXISTS` | 409 | CREATE while a config exists |
| `CONFIGURATION_NOT_FOUND` | 404 | operation on a missing config |
| `CONFIGURATION_OVERWRITE_NOT_AUTHORIZED` | 403 | OVERWRITE when no config exists |
| `CONFIGURATION_SIZE_LIMIT_EXCEEDED` | 413 | blob > 50 MiB |
| `CONFIGURATION_UPLOAD_FAILED` | 500 | server write failed |
| `CONFIGURATION_DELETE_FAILED` | 500 | server delete failed |
| `CONFIGURATION_INTEGRITY_CHECK_FAILED` | 500 | SHA-256 mismatch |
| `DEVICE_NOT_REGISTERED` | 404 | unknown device id |
| `DEVICE_NOT_AUTHORIZED` | 401 | bad token |
| `AUTHENTICATION_REQUIRED` | 401 | no token supplied |
| `RATE_LIMITED` | 429 | window exceeded |
| `DECRYPTION_FAILED` | 400 | client-side only (wrong passphrase) |

---

## 8. Go-live checklist / 上线清单

- [ ] Cloud API deployed behind a Load Balancer, TLS mandatory.
- [ ] OSS bucket with the layout in §3 + lifecycle rules for `orphans/`.
- [ ] Database tables: devices, tokens, active-config rows, rate counters.
- [ ] AccessKey / Secret **server-side only**; least-privilege RAM policy.
- [ ] Orphan cleanup job on a timer (24 h TTL + stale `.tmp` sweep).
- [ ] Admin dashboard: the metrics in §5, cleanup action guarded against
      active-config deletion.
- [ ] Rate limits & size caps enforced at the edge (§4).
- [ ] Privacy / security docs (`docs/privacy.md`, `docs/security.md`)
      published and linked from the app's Settings.
