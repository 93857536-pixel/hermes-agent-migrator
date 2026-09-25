import React, { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import {
  installerDetectSystem,
  installerInstall,
  onInstallerLog,
} from "../lib/invoke";
import {
  InstallDone,
  InstallOptions,
  SystemInfo,
  defaultInstallOptions,
  fmtBytes,
} from "../lib/migrator";

const MAX_LOG_LINES = 400;

/** Setup screen: system detection + one-click automatic Hermes Agent
 *  installation via the official installer. The installer's own output is
 *  streamed into a live log box; on completion the detection snapshot is
 *  re-run so the user sees the version without leaving the screen. */
export default function Setup() {
  const { t } = useI18n();
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [opts, setOpts] = useState<InstallOptions>(defaultInstallOptions);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const [done, setDone] = useState<InstallDone | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const logRef = useRef<HTMLPreElement | null>(null);
  const unref = useRef<(() => void) | null>(null);

  const refresh = async () => {
    try {
      setSys(await installerDetectSystem());
    } catch (e) {
      setErr(String(e));
    }
  };

  useEffect(() => {
    refresh();
    return () => unref.current?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Keep the log box pinned to the bottom while streaming.
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [log]);

  const run = async () => {
    setBusy(true);
    setErr(null);
    setDone(null);
    setLog([t("setup.log_start")]);
    unref.current = await onInstallerLog((line) => {
      setLog((prev) => {
        const next = [...prev, line];
        return next.length > MAX_LOG_LINES
          ? next.slice(next.length - MAX_LOG_LINES)
          : next;
      });
    });
    try {
      setDone(await installerInstall(opts));
      // Re-detect after the install so the system card reflects reality.
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      unref.current?.();
      unref.current = null;
      setBusy(false);
    }
  };

  const toolRow = (
    label: string,
    ok: boolean | null,
    help: string,
  ) => (
    <div className="row" style={{ justifyContent: "space-between" }}>
      <span className="k" title={help}>
        {label}
      </span>
      <span className={`dot ${ok ? "present" : ok === null ? "missing" : "attention"}`}>
        {ok ? "✓" : ok === null ? "—" : "⚠"}
      </span>
    </div>
  );

  return (
    <div className="screen">
      <h2>{t("setup.title")}</h2>
      <p className="sub">{t("setup.subtitle")}</p>

      {/* System detection */}
      <div className="card">
        <h3>{t("setup.system")}</h3>
        {sys ? (
          <>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="k">{t("setup.platform")}</span>
              <span className="mono">
                {sys.os} / {sys.arch}
              </span>
            </div>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="k">{t("setup.hermes")}</span>
              <div className={`pill ${sys.hermes_installed ? "ok" : "warn"}`}>
                {sys.hermes_installed
                  ? t("setup.hermes_yes")
                  : t("setup.hermes_no")}
                {sys.hermes_version && (
                  <span className="mono"> {sys.hermes_version}</span>
                )}
              </div>
            </div>
            {sys.hermes_home && (
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span className="k">{t("setup.home")}</span>
                <span className="mono">{sys.hermes_home}</span>
              </div>
            )}
            <h3>{t("setup.tools")}</h3>
            {toolRow(t("setup.git"), sys.git_available, t("setup.git_help"))}
            {toolRow(t("setup.python"), sys.python_available, t("setup.python_help"))}
            {toolRow(t("setup.curl"), sys.curl_available, t("setup.curl_help"))}
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="k">{t("setup.free")}</span>
              <span>{sys.free_space_bytes ? fmtBytes(sys.free_space_bytes) : "—"}</span>
            </div>
          </>
        ) : (
          <div className="muted small">{t("setup.detecting")}</div>
        )}
        <div className="row">
          <button className="ghost" onClick={refresh} disabled={busy}>
            {t("setup.redetect")}
          </button>
        </div>
      </div>

      {/* Install */}
      <div className="card">
        <h3>{t("setup.install")}</h3>
        <p className="muted small">{t("setup.install_help")}</p>
        <label className="check block">
          <input
            type="checkbox"
            checked={opts.skip_browser}
            onChange={(e) => setOpts((o) => ({ ...o, skip_browser: e.target.checked }))}
            disabled={busy}
          />
          <span>{t("setup.skip_browser")}</span>
        </label>
        <div className="row">
          <button className="primary" onClick={run} disabled={busy}>
            {busy ? t("setup.installing") : t("setup.install_btn")}
          </button>
        </div>
        {err && <div className="note error">{err}</div>}
        {done && (
          <div
            className={`pill ${done.success ? "ok" : "warn"}`}
            style={{ marginBottom: 8 }}
          >
            {done.success
              ? t("setup.done_ok")
              : t("setup.done_fail")}
            {done.hermes_version && (
              <span className="mono"> {t("setup.hermes_now")} {done.hermes_version}</span>
            )}
          </div>
        )}
      </div>

      {/* Live installer log */}
      {log.length > 0 && (
        <div className="card">
          <h3>{t("setup.log")}</h3>
          <pre ref={logRef} className="logbox">
            {log.join("\n")}
          </pre>
        </div>
      )}
    </div>
  );
}
