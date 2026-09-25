//! hermes-cloud-server — the runnable, self-hostable backend for the Hermes
//! Agent Migrator cloud-configuration feature.
//!
//! This is the single-binary reference deployment described in
//! `docs/cloud-alibaba.md`: the Cloud API surface in front of the store,
//! honouring the one-device / one-configuration contract, the stable wire
//! error codes and the 24 h orphan-cleanup job. In a large deployment the
//! store would be "database + OSS"; here the store is the file-backed
//! [`migrator_core::cloudserver::FileServer`] under a configurable root
//! (the "oss" layout: `configs/`, `orphans/`, `devices.json`).
//!
//! Wire format — shared with `migrator_core::remote`:
//!
//! | method | path                    | auth  | body                              |
//!|--------|-------------------------|-------|-----------------------------------|
//!| POST   | `/v1/register`          | —     | `{"device_id"}` → `{"token"}`    |
//!| POST   | `/v1/has-config`        | hdrs  | — → `{"meta": meta \| null}`      |
//!| POST   | `/v1/upsert`            | hdrs  | `{"blob","overwrite"}` → meta     |
//!| POST   | `/v1/download`          | hdrs  | `{"expected_sha"?}` → `{"blob"?}` |
//!| POST   | `/v1/delete`            | hdrs  | — → `{"n": n}`                    |
//!| POST   | `/v1/sweep-orphans`     | hdrs  | — → `{"n": n}`                    |
//!| GET    | `/v1/health`            | —     | — → `{"api_version": 1}`         |
//!
//! Identity headers: `X-HM-Device` + `X-HM-Token` (register returns the
//! token). All non-2xx answers carry the stable `{"code","message"}` body
//! with the matching HTTP status (`CloudError::http_status`).
//!
//! Run:
//! ```sh
//! hermes-cloud-server --listen 127.0.0.1:8084 --root /var/lib/hermes-cloud
//! # reverse-proxy path prefix (CF tunnel / nginx):
//! hermes-cloud-server --listen 0.0.0.0:8084 --root /var/lib/hermes-cloud --prefix /hermes-cloud
//! ```

use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use migrator_core::cloud::{CloudError, DeviceId};
use migrator_core::cloudserver::FileServer;
use migrator_core::remote::{
    BlobResp, CountResp, ErrBody, HealthResp, MetaResp, RegisterReq, RegisterResp, UpsertReq,
};
use serde::Serialize;

/// Outer guard against a runaway request body. The design doc's 50 MiB
/// "sealed blob" cap is the production *default*; a self-hosted personal
/// server (the common case for this repo) must accept real Hermes homes,
/// which easily reach hundreds of MiB packed. 2 GiB is a sane envelope
/// (base64 wire cost is ~4/3 of the pack size).
const MAX_BODY: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "hermes-cloud-server",
    version,
    about = "Self-hosted cloud configuration backend for Hermes Agent Migrator"
)]
struct Args {
    /// Address to bind, e.g. `127.0.0.1:8084`.
    #[arg(long, default_value = "127.0.0.1:8084")]
    listen: String,
    /// Store root (devices.json, configs/, orphans/).
    #[arg(long, default_value = ".hermes-cloud")]
    root: PathBuf,
    /// URL path prefix the server is mounted under (reverse proxy). The
    /// server accepts `/v1/...` and, when set, `/{prefix}/v1/...`.
    #[arg(long, default_value = "")]
    prefix: String,
    /// Run one orphan-sweep pass at startup (also sweeps every hour).
    #[arg(long, default_value_t = true)]
    sweep_on_start: bool,
    /// Max sealed-blob size in MiB the store will accept (default 2048).
    /// The design doc's production default is 50 MiB; a personal
    /// self-hosted server should allow real Hermes homes (hundreds of MiB
    /// packed).
    #[arg(long, default_value_t = 2048)]
    max_blob_mib: u32,
}

/// (http status, content-type, body bytes).
type ApiResponse = (u16, &'static str, Vec<u8>);

fn json_ok<T: Serialize>(v: &T) -> ApiResponse {
    let body = serde_json::to_vec(v).expect("cloud wire types are always serializable");
    (200, "application/json", body)
}

fn json_err(e: &CloudError, extra: Option<String>) -> ApiResponse {
    let body = serde_json::to_vec(&ErrBody {
        code: e.code().to_string(),
        message: extra.unwrap_or_else(|| e.code().to_string()),
    })
    .expect("ErrBody is always serializable");
    (e.http_status(), "application/json", body)
}

fn bad_request(msg: &str) -> ApiResponse {
    let body = serde_json::to_vec(&ErrBody {
        code: "BAD_REQUEST".into(),
        message: msg.into(),
    })
    .expect("ErrBody is always serializable");
    (400, "application/json", body)
}

struct Server {
    store: Arc<FileServer>,
    prefix: String,
}

impl Server {
    fn route(&self, url: &str) -> Option<&'static str> {
        let p = self.prefix.trim_end_matches('/');
        let mut cands: Vec<String> = Vec::new();
        if p.is_empty() {
            cands.push(url.trim_end_matches('/').to_string());
        } else {
            let target = format!("{p}/");
            if let Some(s) = url.strip_prefix(&target) {
                let m = s.trim_end_matches('/');
                if m.is_empty() {
                    cands.push("/".to_string());
                } else {
                    cands.push(format!("/{m}"));
                }
            }
            cands.push(url.trim_end_matches('/').to_string());
        }
        for c in &cands {
            match c.as_str() {
                "/v1/register" => return Some("register"),
                "/v1/has-config" => return Some("has-config"),
                "/v1/upsert" => return Some("upsert"),
                "/v1/download" => return Some("download"),
                "/v1/delete" => return Some("delete"),
                "/v1/sweep-orphans" => return Some("sweep-orphans"),
                "/v1/health" => return Some("health"),
                _ => {}
            }
        }
        None
    }

    fn handle(&self, req: &mut tiny_http::Request) -> ApiResponse {
        let method = req.method().as_str();
        let url = req.url();
        if method == "GET" && self.route(url) == Some("health") {
            return json_ok(&HealthResp { api_version: 1 });
        }
        if method != "POST" {
            return bad_request("method not allowed (use POST /v1/* or GET /v1/health)");
        }
        let route = match self.route(url) {
            Some(r) => r,
            None => return bad_request(&format!("unknown route: {url}")),
        };

        let mut body = String::new();
        req.as_reader()
            .take(MAX_BODY)
            .read_to_string(&mut body)
            .ok();
        let parsed: Option<serde_json::Value> = if body.is_empty() {
            None
        } else {
            match serde_json::from_str(&body) {
                Ok(v) => Some(v),
                Err(e) => return bad_request(&format!("malformed JSON body: {e}")),
            }
        };

        let device = req
            .headers()
            .iter()
            .find(|h| h.field.as_str() == "X-HM-Device")
            .map(|h| h.value.to_string());
        let token = req
            .headers()
            .iter()
            .find(|h| h.field.as_str() == "X-HM-Token")
            .map(|h| h.value.to_string());

        match route {
            "register" => self.handle_register(parsed.as_ref()),
            "has-config" => self.authed_op(
                &self.store,
                device.as_deref(),
                token.as_deref(),
                |store, d, t| {
                    let meta = store.has_configuration(d, t)?;
                    Ok(json_ok(&MetaResp { meta }))
                },
            ),
            "upsert" => self.authed_op(
                &self.store,
                device.as_deref(),
                token.as_deref(),
                |store, d, t| {
                    let u: UpsertReq = parsed
                        .as_ref()
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .ok_or(CloudError::ConfigurationUploadFailed)?;
                    let meta = store.upsert_configuration(d, t, &u.blob, u.overwrite)?;
                    Ok(json_ok(&MetaResp { meta: Some(meta) }))
                },
            ),
            "download" => self.authed_op(
                &self.store,
                device.as_deref(),
                token.as_deref(),
                |store, d, t| {
                    let expected = parsed
                        .as_ref()
                        .and_then(|v| v.get("expected_sha"))
                        .and_then(|s| s.as_str());
                    let blob = store.download_configuration(d, t, expected)?;
                    Ok(json_ok(&BlobResp { blob }))
                },
            ),
            "delete" => self.authed_op(
                &self.store,
                device.as_deref(),
                token.as_deref(),
                |store, d, t| {
                    let n = store.delete_configuration(d, t)?;
                    Ok(json_ok(&CountResp { n }))
                },
            ),
            "sweep-orphans" => self.authed_op(
                &self.store,
                device.as_deref(),
                token.as_deref(),
                |store, _d, _t| {
                    Ok(json_ok(&CountResp {
                        n: store.sweep_orphans(),
                    }))
                },
            ),
            _ => unreachable!("route() validated {route}"),
        }
    }

    fn handle_register(&self, body: Option<&serde_json::Value>) -> ApiResponse {
        let req: RegisterReq = match body.and_then(|v| serde_json::from_value(v.clone()).ok()) {
            Some(r) => r,
            None => return bad_request("register requires a JSON body {\"device_id\": ...}"),
        };
        let id = DeviceId(req.device_id.clone());
        let dev = match DeviceId::validate(&id.0) {
            Ok(()) => id,
            Err(_) => return bad_request("invalid device_id (expected a UUID v4)"),
        };
        match self.store.register(&dev) {
            Ok(token) => json_ok(&RegisterResp { token }),
            Err(e) => json_err(&e, None),
        }
    }

    fn authed_op<F>(
        &self,
        store: &FileServer,
        device: Option<&str>,
        token: Option<&str>,
        f: F,
    ) -> ApiResponse
    where
        F: FnOnce(&FileServer, &str, &str) -> Result<ApiResponse, CloudError>,
    {
        let (d, t) = match (device, token) {
            (Some(d), Some(t)) => (d, t),
            _ => {
                let e = CloudError::AuthenticationRequired;
                return json_err(&e, None);
            }
        };
        match f(store, d, t) {
            Ok(r) => r,
            Err(e) => json_err(&e, None),
        }
    }
}

/// The store's own 24 h orphan-cleanup job, on an hourly timer.
fn sweep_loop(store: Arc<FileServer>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        let n = store.sweep_orphans();
        if n > 0 {
            eprintln!("[hermes-cloud-server] swept {n} expired orphan object(s)");
        }
    });
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = args
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --listen address: {e}"))?;
    let store = Arc::new(FileServer::new(&args.root).with_limits(
        migrator_core::cloud::BackendLimits {
            max_blob_bytes: (args.max_blob_mib as u64) * 1024 * 1024,
            ..Default::default()
        },
    ));
    if args.sweep_on_start {
        let n = store.sweep_orphans();
        eprintln!("[hermes-cloud-server] startup sweep removed {n} orphan object(s)");
    }
    sweep_loop(store.clone());

    let srv = tiny_http::Server::http(addr).map_err(|e| anyhow::anyhow!("bind failed: {e}"))?;
    eprintln!("[hermes-cloud-server] listening on http://{}", addr);
    eprintln!(
        "[hermes-cloud-server] store root: {:?}",
        args.root.as_path()
    );
    if !args.prefix.is_empty() {
        eprintln!(
            "[hermes-cloud-server] accepting /{} prefix",
            args.prefix.trim_end_matches('/')
        );
    }

    let s = Server {
        store,
        prefix: args.prefix.clone(),
    };
    for req in srv.incoming_requests() {
        handle_one(&s, req);
    }
    Ok(())
}

/// Serve one request; errors are logged, never fatal.
fn handle_one(server: &Server, mut req: tiny_http::Request) {
    let (code, ctype, body) = server.handle(&mut req);
    eprintln!(
        "[hermes-cloud-server] {} {} -> {}",
        req.method().as_str(),
        req.url(),
        code
    );
    if code >= 400 {
        eprintln!(
            "[hermes-cloud-server]   body: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let data_len = body.len();
    let cursor = std::io::Cursor::new(body);
    let mut resp = tiny_http::Response::new(
        tiny_http::StatusCode(code),
        Vec::new(),
        cursor,
        Some(data_len),
        None,
    );
    if let Ok(h) = tiny_http::Header::from_bytes("Content-Type", ctype) {
        resp.add_header(h);
    }
    if let Ok(h) = tiny_http::Header::from_bytes("Access-Control-Allow-Origin", "*") {
        resp.add_header(h);
    }
    if let Err(e) = req.respond(resp) {
        eprintln!("[hermes-cloud-server] write failure: {e}");
    }
}
