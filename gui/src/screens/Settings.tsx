import React, { useState } from "react";
import { nextLang, curLangLabel, useI18n } from "../i18n";
import { cloudStatus, cloudTestConnection } from "../lib/invoke";
import { CloudStatus, ConnTest } from "../lib/migrator";

/** Settings → Cloud: server status + Test Connection, re-viewable privacy,
 *  security and the first-launch disclaimer. All copy from i18n. */
export default function Settings() {
  const { t, lang, setLang } = useI18n();
  const [conn, setConn] = useState<ConnTest | null>(null);
  const [st, setSt] = useState<CloudStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [showPrivacy, setShowPrivacy] = useState(false);
  const [showSecurity, setShowSecurity] = useState(false);
  const [showDisclaimer, setShowDisclaimer] = useState(false);

  const test = async () => {
    setBusy(true);
    setErr(null);
    try {
      const [c, s] = await Promise.all([cloudTestConnection(), cloudStatus()]);
      setConn(c);
      setSt(s);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="screen">
      <h2>{t("settings.title")}</h2>
      <p className="sub">{t("cloud.subtitle")}</p>

      {/* Language */}
      <div className="card">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="k">{t("settings.language")}</span>
          <button className="chip" onClick={() => setLang(nextLang(lang))}>
            {curLangLabel(lang)} → {nextLang(lang) === "en" ? "EN" : nextLang(lang) === "zh" ? "简中" : "繁中"}
          </button>
        </div>
      </div>

      {/* Cloud server */}
      <div className="card">
        <h3>{t("settings.cloud_server")}</h3>
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="k">{t("settings.server_status")}</span>
          <div className="pill">{conn?.reachable ? t("cloud.reachable") : "—"}</div>
        </div>
        {conn && (
          <div className="kv-list">
            <div className="kv">
              <span className="k">{t("cloud.api_version")}</span>
              <span>{conn.api_version}</span>
            </div>
            <div className="kv">
              <span className="k">{t("cloud.latency")}</span>
              <span>{conn.latency_ms} ms</span>
            </div>
            <div className="kv">
              <span className="k">{t("settings.device")}</span>
              <span className="mono">{st?.device_id_masked ?? conn.device_id_masked}</span>
            </div>
          </div>
        )}
        {err && <div className="note error">{t("cloud.test_failed")}: {err}</div>}
        <div className="row">
          <button className="primary" onClick={test} disabled={busy}>
            {busy ? t("cloud.busy") : t("settings.test_connection")}
          </button>
        </div>
      </div>

      {/* Privacy + Security (re-viewable) */}
      <div className="card">
        <div className="row">
          <button className="ghost" onClick={() => setShowPrivacy((v) => !v)}>
            {t("settings.privacy_policy")}
          </button>
          <button className="ghost" onClick={() => setShowSecurity((v) => !v)}>
            {t("settings.security")}
          </button>
          <button className="ghost" onClick={() => setShowDisclaimer((v) => !v)}>
            {t("settings.view_disclaimer")}
          </button>
        </div>
        {showPrivacy && (
          <div className="note ok">
            <b>{t("privacy.title")}</b>
            <pre className="small muted">{t("privacy.data")}</pre>
            <pre className="small muted">{t("privacy.use")}</pre>
            <pre className="small muted">{t("privacy.notused")}</pre>
          </div>
        )}
        {showSecurity && (
          <div className="note ok">
            <b>{t("security.title")}</b>
            <pre className="small muted">{t("security.body")}</pre>
          </div>
        )}
        {showDisclaimer && (
          <div className="note ok">
            <b>{t("disclaimer.title")}</b>
            <pre className="small muted">{t("disclaimer.body")}</pre>
          </div>
        )}
      </div>
    </div>
  );
}
