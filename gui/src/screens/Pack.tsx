import React, { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import { packHermes, onProgress } from "../lib/invoke";
import {
  PackOptions,
  PackResult,
  ProgressEvt,
  defaultPackOptions,
} from "../lib/migrator";

const COMP_KEYS = [
  "config",
  "skills",
  "plugins",
  "sessions",
  "state_db",
  "cron",
  "memories",
  "kanban",
  "pastes",
] as const;

export default function Pack() {
  const { t } = useI18n();
  const [opts, setOpts] = useState<PackOptions>(defaultPackOptions);
  const [outPath, setOutPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<ProgressEvt | null>(null);
  const [done, setDone] = useState<PackResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const unref = useRef<(() => void) | null>(null);

  useEffect(() => {
    return () => unref.current?.();
  }, []);

  const toggle = (k: (typeof COMP_KEYS)[number]) =>
    setOpts((o) => ({ ...o, [`include_${k}`]: !o[`include_${k}`] }));

  const run = async () => {
    setBusy(true);
    setErr(null);
    setDone(null);
    setProgress({ stage: "", pct: 0, done: 0, total: 0 });
    try {
      unref.current = await onProgress(setProgress);
      const result = await packHermes(opts, outPath || "hermes-migration.hermesmig");
      setDone(result);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
      unref.current?.();
      unref.current = null;
    }
  };

  return (
    <div className="screen">
      <h2>{t("pack.title")}</h2>
      <p className="sub">{t("pack.subtitle")}</p>

      <div className="card">
        <h3>{t("pack.select")}</h3>
        <div className="checks">
          {COMP_KEYS.map((k) => (
            <label key={k} className="check">
              <input
                type="checkbox"
                checked={opts[`include_${k}`]}
                onChange={() => toggle(k)}
              />
              <span>{t(`comp.${k}`)}</span>
            </label>
          ))}
        </div>

        <label className="check block">
          <input
            type="checkbox"
            checked={opts.include_application}
            onChange={(e) =>
              setOpts((o) => ({ ...o, include_application: e.target.checked }))
            }
          />
          <span>{t("pack.application")}</span>
        </label>

        <div className="secrets-box">
          <label className="check block">
            <input
              type="checkbox"
              checked={opts.secrets_passphrase !== null}
              onChange={(e) =>
                setOpts((o) => ({
                  ...o,
                  secrets_passphrase: e.target.checked
                    ? (o.secrets_passphrase ?? "")
                    : null,
                }))
              }
            />
            <span>{t("pack.secrets")}</span>
          </label>
          <p className="help">{t("pack.secrets_help")}</p>
          {opts.secrets_passphrase !== null && (
            <input
              type="password"
              placeholder={t("pack.passphrase")}
              value={opts.secrets_passphrase}
              onChange={(e) =>
                setOpts((o) => ({
                  ...o,
                  secrets_passphrase: e.target.value,
                }))
              }
            />
          )}
        </div>

        <div className="out-row">
          <label className="out-label">{t("pack.out")}</label>
          <input
            className="mono"
            placeholder="~/hermes-migration.hermesmig"
            value={outPath}
            onChange={(e) => setOutPath(e.target.value)}
          />
        </div>

        <button className="primary" onClick={run} disabled={busy}>
          {busy ? t("pack.creating") : t("pack.create")}
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

        {done && !busy && (
          <div className="note ok">
            <b>{t("pack.done")}</b>
            <div className="mono small">{done.out_path}</div>
            <div className="small muted">{done.manifest_summary}</div>
            <p className="help">{t("pack.done_help")}</p>
          </div>
        )}
      </div>
    </div>
  );
}
