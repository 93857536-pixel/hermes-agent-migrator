import React, { useState } from "react";
import { useI18n } from "../i18n";
import { scanHermes } from "../lib/invoke";
import { ScanReport, fmtBytes } from "../lib/migrator";

const STATUS_ICON: Record<string, string> = {
  present: "✓",
  attention: "⚠",
  missing: "—",
};

export default function Scan({ onGoPack }: { onGoPack: () => void }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<ScanReport | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setErr(null);
    try {
      setReport(await scanHermes());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const compRows = report
    ? Object.entries(report.components).map(([name, status]) => ({
        name,
        status,
      }))
    : [];

  return (
    <div className="screen">
      <h2>{t("scan.title")}</h2>
      <p className="sub">{t("scan.subtitle")}</p>

      <div className="row">
        <button className="primary" onClick={run} disabled={busy}>
          {busy ? t("scan.running") : t("scan.run")}
        </button>
        {report?.detected && (
          <button className="ghost" onClick={onGoPack}>
            {t("scan.to_pack")}
          </button>
        )}
      </div>

      {err && <div className="note error">{err}</div>}

      {report && (
        <div className="card">
          <div className={`pill ${report.detected ? "ok" : "warn"}`}>
            {report.detected ? t("scan.detected") : t("scan.notdetected")}
          </div>

          <dl className="kv">
            <div>
              <dt>{t("scan.home")}</dt>
              <dd className="mono">{report.hermes_home ?? "—"}</dd>
            </div>
            {report.install && (
              <div>
                <dt>{t("scan.version")}</dt>
                <dd className="mono">
                  {report.install.version || "?"} · {report.install.install_kind}
                </dd>
              </div>
            )}
            <div>
              <dt>{t("scan.total")}</dt>
              <dd>
                {report.total_files} · {fmtBytes(report.total_bytes)}
              </dd>
            </div>
          </dl>

          <h3>{t("scan.components")}</h3>
          <ul className="comps">
            {compRows.map((c) => (
              <li key={c.name}>
                <span className="comp-name">
                  {t(`comp.${c.name}`) ?? c.name}
                </span>
                <span className={`dot ${c.status}`} title={c.status}>
                  {STATUS_ICON[c.status] ?? c.status}
                </span>
              </li>
            ))}
          </ul>

          {report.notes.length > 0 && (
            <>
              <h3>{t("scan.notes")}</h3>
              <ul className="notes">
                {report.notes.map((n, i) => (
                  <li key={i}>{n}</li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
    </div>
  );
}
