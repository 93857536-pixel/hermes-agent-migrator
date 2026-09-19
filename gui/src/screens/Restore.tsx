import React, { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import {
  checkEnvironment,
  onProgress,
  restoreHermes,
  verifyPackage,
} from "../lib/invoke";
import {
  EnvCheck,
  ProgressEvt,
  RestoreOptions,
  RestoreReport,
  defaultRestoreOptions,
} from "../lib/migrator";

export default function Restore() {
  const { t } = useI18n();
  const [pkg, setPkg] = useState("");
  const [opts, setOpts] = useState<RestoreOptions>(defaultRestoreOptions);
  const [env, setEnv] = useState<EnvCheck | null>(null);
  const [failures, setFailures] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<ProgressEvt | null>(null);
  const [done, setDone] = useState<RestoreReport | null>(null);
  const [rolledBack, setRolledBack] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const unref = useRef<(() => void) | null>(null);

  useEffect(() => () => unref.current?.(), []);

  const inspect = async () => {
    if (!pkg) return;
    setErr(null);
    setDone(null);
    setRolledBack(false);
    try {
      setEnv(await checkEnvironment(pkg));
      const f = await verifyPackage(pkg);
      setFailures(f);
    } catch (e) {
      setErr(String(e));
    }
  };

  const run = async () => {
    setBusy(true);
    setErr(null);
    setDone(null);
    setRolledBack(false);
    setProgress({ stage: "", pct: 0, done: 0, total: 0 });
    try {
      unref.current = await onProgress(setProgress);
      const r = await restoreHermes(pkg, opts);
      setDone(r.report);
    } catch (e) {
      const msg = String(e);
      // A failed verification surfaces as IntegrityFailed + rollback note.
      if (/rolled back/i.test(msg)) setRolledBack(true);
      setErr(msg);
    } finally {
      setBusy(false);
      unref.current?.();
      unref.current = null;
    }
  };

  const canRun = !!pkg && !!env?.ready && (failures === null || failures.length === 0);

  return (
    <div className="screen">
      <h2>{t("restore.title")}</h2>
      <p className="sub">{t("restore.subtitle")}</p>

      <div className="card">
        <div className="out-row">
          <label className="out-label">{t("restore.choose")}</label>
          <input
            className="mono"
            placeholder="~/hermes-migration.hermesmig"
            value={pkg}
            onChange={(e) => {
              setPkg(e.target.value);
              setEnv(null);
              setFailures(null);
            }}
          />
        </div>
        {pkg && (
          <div className="row">
            <button className="ghost" onClick={inspect} disabled={busy}>
              {t("restore.env_check")}
            </button>
            {!env && <span className="muted small">{t("restore.no_pkg")}</span>}
          </div>
        )}

        {failures !== null && (
          <div className="note error">
            Integrity check found {failures.length} mismatching file
            {failures.length === 1 ? "" : "s"}:
            <ul className="small">
              {failures.slice(0, 5).map((f) => (
                <li key={f} className="mono">
                  {f}
                </li>
              ))}
            </ul>
          </div>
        )}

        {env && (
          <div className="env-check">
            <div className={`pill ${env.ready ? "ok" : "warn"}`}>
              {env.ready ? t("common.ready") : env.reason ?? t("common.failed")}
            </div>
            <ul className="checks-list">
              {env.items.map((it) => (
                <li key={it.name} className={it.ok ? "ok" : it.severity === "hard" ? "bad" : "warn"}>
                  <b>{it.name}</b> — {it.detail}
                </li>
              ))}
            </ul>
          </div>
        )}

        <div className="secrets-box">
          <input
            type="password"
            placeholder={t("restore.secret_passphrase")}
            value={opts.passphrase ?? ""}
            onChange={(e) =>
              setOpts((o) => ({ ...o, passphrase: e.target.value || null }))
            }
          />
          <label className="check block">
            <input
              type="checkbox"
              checked={opts.apply_path_repair}
              onChange={(e) =>
                setOpts((o) => ({ ...o, apply_path_repair: e.target.checked }))
              }
            />
            <span>{t("restore.path_repair")}</span>
          </label>
        </div>

        <button className="primary" onClick={run} disabled={!canRun || busy}>
          {busy ? t("restore.running") : t("restore.run")}
        </button>

        {busy && progress && (
          <div className="progress">
            <div className="progress-track">
              <div
                className="progress-fill"
                style={{ width: `${progress.pct}%` }}
              />
            </div>
            <div className="progress-label">
              {progress.stage}
              {progress.total > 0 && ` (${progress.done}/${progress.total})`}
              {` · ${progress.pct}%`}
            </div>
          </div>
        )}

        {err && <div className="note error">{err}</div>}

        {done && (
          <div className={`note ${done.notes.some((n) => /rolled back/.test(n)) ? "error" : "ok"}`}>
            {rolledBack || done.notes.some((n) => /rolled back/.test(n)) ? (
              <b>{t("restore.rolled_back")}</b>
            ) : (
              <b>{t("restore.success")}</b>
            )}
            <ul className="small">
              {done.verified.map(([c, ok]) => (
                <li key={c}>
                  {ok ? "✓" : "✗"} {c}
                </li>
              ))}
            </ul>
            {done.secrets_restored > 0 && (
              <div className="small">
                {done.secrets_restored} secret(s) restored (0600).
              </div>
            )}
            {done.needs_reauth.length > 0 && (
              <>
                <b>{t("restore.needs_reauth")}:</b>
                <ul className="small">
                  {done.needs_reauth.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                </ul>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
