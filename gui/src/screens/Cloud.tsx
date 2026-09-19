import React, { useCallback, useEffect, useState } from "react";
import { useI18n } from "../i18n";
import {
  cloudDelete,
  cloudDownloadRestore,
  cloudStatus,
  cloudTestConnection,
  cloudUpload,
} from "../lib/invoke";
import { CloudStatus, ConnTest, fmtBytes } from "../lib/migrator";

type Notice = "uploaded" | "updated" | "deleted" | "restored" | null;
type Gate = "overwrite" | "delete" | null;

function fmtMs(ms: number): string {
  if (!ms) return "—";
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return String(ms);
  }
}

export default function Cloud() {
  const { t } = useI18n();
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [conn, setConn] = useState<ConnTest | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [outPath, setOutPath] = useState("~/.hermes-migrator/cloud-restore.hermesmig");
  const [busy, setBusy] = useState(false);
  const [gate, setGate] = useState<Gate>(null);
  const [note, setNote] = useState<Notice>(null);
  const [err, setErr] = useState<string | null>(null);
  // First-run cloud-storage notice gate (persisted in localStorage).
  const [seenNotice, setSeenNotice] = useState(false);

  useEffect(() => {
    setSeenNotice(localStorage.getItem("hm-cloud-notice-accepted") === "1");
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [s, c] = await Promise.all([cloudStatus(), cloudTestConnection()]);
      setStatus(s);
      setConn(c);
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const acceptNotice = () => {
    localStorage.setItem("hm-cloud-notice-accepted", "1");
    setSeenNotice(true);
  };

  const hasConfig = status?.has_configuration ?? false;

  const runUpload = async (overwrite: boolean) => {
    setBusy(true);
    setErr(null);
    setNote(null);
    setGate(null);
    try {
      const s = await cloudUpload(passphrase, overwrite);
      setStatus(s);
      setNote(overwrite ? "updated" : "uploaded");
    } catch (e) {
      // An "already exists" error when creating → bounce to overwrite gate.
      if (/CONFIGURATION_ALREADY_EXISTS/.test(String(e))) {
        setGate("overwrite");
        setPassphrase("");
      } else {
        setErr(String(e));
      }
    } finally {
      setBusy(false);
    }
  };

  const runOverwrite = async () => {
    setBusy(true);
    setErr(null);
    setNote(null);
    setGate(null);
    try {
      const s = await cloudUpload(passphrase, true);
      setStatus(s);
      setNote("updated");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const runDelete = async () => {
    setBusy(true);
    setErr(null);
    setNote(null);
    setGate(null);
    try {
      const s = await cloudDelete();
      setStatus(s);
      setNote("deleted");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const runRestore = async () => {
    setBusy(true);
    setErr(null);
    setNote(null);
    try {
      const r = await cloudDownloadRestore(passphrase, outPath);
      setNote("restored");
      void r.report; // verified on the backend; surface success
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  // ---- first-run notice gate -------------------------------------------
  if (!seenNotice) {
    return (
      <div className="screen">
        <h2>{t("cloud.title")}</h2>
        <p className="sub">{t("cloud.subtitle")}</p>
        <div className="card">
          <div className="pill warn">
            <span className="dot" /> {t("cloud.notice_title")}
          </div>
          <pre className="muted small notice-body">{t("cloud.notice_body")}</pre>
          <button className="primary" onClick={acceptNotice}>
            {t("disclaimer.ack")}
          </button>
        </div>
      </div>
    );
  }

  // ---- gate modals (overwrite / delete) ---------------------------------
  if (gate === "overwrite") {
    return (
      <div className="screen">
        <h2>{t("cloud.overwrite_confirm_title")}</h2>
        <div className="card">
          <pre className="muted small notice-body">{t("cloud.overwrite_confirm_body")}</pre>
          <div className="secrets-box">
            <input
              type="password"
              placeholder={t("cloud.passphrase")}
              value={passphrase}
              onChange={(e) => setPassphrase(e.target.value)}
            />
            <p className="help small">{t("cloud.encrypt_help")}</p>
          </div>
          <div className="row">
            <button className="ghost" onClick={() => setGate(null)} disabled={busy}>
              {t("common.cancel")}
            </button>
            <button className="primary" onClick={runOverwrite} disabled={busy || !passphrase}>
              {busy ? t("cloud.busy") : t("cloud.confirm_overwrite")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (gate === "delete") {
    return (
      <div className="screen">
        <h2>{t("cloud.delete_confirm_title")}</h2>
        <div className="card">
          <pre className="muted small notice-body">{t("cloud.delete_confirm_body")}</pre>
          <div className="row">
            <button className="ghost" onClick={() => setGate(null)} disabled={busy}>
              {t("common.cancel")}
            </button>
            <button className="primary" onClick={runDelete} disabled={busy}>
              {busy ? t("cloud.busy") : t("cloud.delete_confirm_btn")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ---- main two-state view ---------------------------------------------
  return (
    <div className="screen">
      <h2>{t("cloud.title")}</h2>
      <p className="sub">{t("cloud.subtitle")}</p>

      {/* Connection / server status */}
      <div className="card">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <div className="pill">{conn?.reachable ? t("cloud.reachable") : t("cloud.unreachable")}</div>
          <button className="ghost" onClick={refresh} disabled={busy}>
            {t("cloud.connection")}
          </button>
        </div>
        <div className="kv-list">
          <div className="kv">
            <span className="k">{t("cloud.device")}</span>
            <span className="mono">{status?.device_id_masked ?? "—"}</span>
          </div>
          {conn && (
            <>
              <div className="kv">
                <span className="k">{t("cloud.api_version")}</span>
                <span>{conn.api_version}</span>
              </div>
              <div className="kv">
                <span className="k">{t("cloud.latency")}</span>
                <span>{conn.latency_ms} ms</span>
              </div>
            </>
          )}
        </div>
      </div>

      {err && <div className="note error">{t("cloud.error")}: {err}</div>}

      {/* STATE A: a configuration exists */}
      {hasConfig && (
        <div className="card">
          <div className="pill ok">
            <span className="dot" /> {t("cloud.has_title")}
          </div>
          <div className="kv-list">
            <div className="kv">
              <span className="k">{t("cloud.last_updated")}</span>
              <span>{fmtMs(status!.uploaded_at_ms)}</span>
            </div>
            <div className="kv">
              <span className="k">{t("cloud.storage")}</span>
              <span>{fmtBytes(status!.size_bytes)}</span>
            </div>
            <div className="kv">
              <span className="k">SHA-256</span>
              <span className="mono small">{status!.sha256 ?? "—"}</span>
            </div>
          </div>

          <div className="secrets-box">
            <input
              type="password"
              placeholder={t("cloud.passphrase")}
              value={passphrase}
              onChange={(e) => setPassphrase(e.target.value)}
            />
            <p className="help small">{t("cloud.encrypt_help")}</p>
            <div className="out-row">
              <label className="out-label">{t("restore.target")}</label>
              <input
                className="mono"
                value={outPath}
                onChange={(e) => setOutPath(e.target.value)}
              />
            </div>
          </div>

          <div className="row wrap">
            <button
              className="primary"
              onClick={() => setGate("overwrite")}
              disabled={busy}
            >
              {t("cloud.overwrite")}
            </button>
            <button
              className="primary"
              onClick={runRestore}
              disabled={busy || !passphrase}
            >
              {busy ? t("cloud.restoring") : t("cloud.download_restore")}
            </button>
            <button className="ghost" onClick={() => setGate("delete")} disabled={busy}>
              {t("cloud.delete")}
            </button>
          </div>

          {note && (
            <div className="note ok">
              <b>
                {note === "uploaded" && t("cloud.uploaded_title")}
                {note === "updated" && t("cloud.updated_title")}
                {note === "deleted" && t("cloud.deleted_title")}
                {note === "restored" && t("restore.success")}
              </b>
              <p className="small">
                {note === "uploaded" && t("cloud.uploaded_body")}
                {note === "updated" && t("cloud.updated_body")}
                {note === "deleted" && t("cloud.deleted_body")}
              </p>
            </div>
          )}
        </div>
      )}

      {/* STATE B: no configuration */}
      {!hasConfig && (
        <div className="card">
          <div className="pill warn">
            <span className="dot" /> {t("cloud.no_title")}
          </div>
          <pre className="muted small notice-body">{t("cloud.no_body")}</pre>

          <div className="secrets-box">
            <input
              type="password"
              placeholder={t("cloud.passphrase")}
              value={passphrase}
              onChange={(e) => setPassphrase(e.target.value)}
            />
            <p className="help small">{t("cloud.encrypt_help")}</p>
          </div>

          <button
            className="primary"
            onClick={() => void runUpload(false)}
            disabled={busy || !passphrase}
          >
            {busy ? t("cloud.uploading") : t("cloud.upload")}
          </button>
        </div>
      )}

      {busy && note === null && (
        <div className="progress" style={{ marginTop: 12 }}>
          <div className="bar" style={{ width: "40%" }} />
        </div>
      )}
    </div>
  );
}
