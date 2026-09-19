import React, { useEffect, useState } from "react";
import { I18nProvider, useI18n, nextLang, curLangLabel } from "./i18n";
import Scan from "./screens/Scan";
import Pack from "./screens/Pack";
import Restore from "./screens/Restore";
import Cloud from "./screens/Cloud";
import Settings from "./screens/Settings";

type Screen = "scan" | "pack" | "restore" | "cloud" | "settings";

const theme = () => {
  const m = window.matchMedia?.("(prefers-color-scheme: dark)");
  return m?.matches ? "dark" : "light";
};

/** First-launch disclaimer gate (requirement §40): the user must read the
 *  Important Information and tick the acknowledgement before continuing.
 *  Re-viewable later via Settings → "Important Information". */
function DisclaimerGate({ onAccept }: { onAccept: () => void }) {
  const { t } = useI18n();
  const [ack, setAck] = useState(false);
  return (
    <div className="screen">
      <h2>{t("app.welcome")}</h2>
      <div className="card">
        <div className="pill warn">
          <span className="dot" /> {t("disclaimer.title")}
        </div>
        <pre className="muted small notice-body">{t("disclaimer.body")}</pre>
        <label className="check block">
          <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
          <span>{t("disclaimer.ack")}</span>
        </label>
        <button className="primary" onClick={onAccept} disabled={!ack}>
          {t("disclaimer.continue")}
        </button>
      </div>
    </div>
  );
}

function Shell() {
  const { t, lang, setLang } = useI18n();
  const [screen, setScreen] = useState<Screen>("scan");
  const [dark, setDark] = useState<boolean>(() => theme() === "dark");
  const [accepted, setAccepted] = useState(
    () => localStorage.getItem("hm-disclaimer-accepted") === "1",
  );

  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  }, [dark]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const on = (e: MediaQueryListEvent) => setDark(e.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  const nav: Screen[] = ["scan", "pack", "restore", "cloud", "settings"];

  const accept = () => {
    localStorage.setItem("hm-disclaimer-accepted", "1");
    setAccepted(true);
  };

  return (
    <div className="layout">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">⇄</span>
          <div>
            <h1 className="brand-title">{t("app.title")}</h1>
            <p className="brand-tagline">{t("app.tagline")}</p>
          </div>
        </div>
        <div className="topbar-actions">
          <button
            className="chip"
            onClick={() => setLang(nextLang(lang))}
            title="Switch language"
          >
            {curLangLabel(lang)}
          </button>
          <button
            className="chip"
            onClick={() => setDark((d) => !d)}
            title="Toggle theme"
          >
            {dark ? "☀" : "☾"}
          </button>
        </div>
      </header>

      {accepted ? (
        <>
          <nav className="tabs">
            {nav.map((s) => (
              <button
                key={s}
                className={`tab ${screen === s ? "active" : ""}`}
                onClick={() => setScreen(s)}
              >
                {t(`nav.${s}`)}
              </button>
            ))}
          </nav>

          <main className="content">
            {screen === "scan" && <Scan onGoPack={() => setScreen("pack")} />}
            {screen === "pack" && <Pack />}
            {screen === "restore" && <Restore />}
            {screen === "cloud" && <Cloud />}
            {screen === "settings" && <Settings />}
          </main>
        </>
      ) : (
        <main className="content">
          <DisclaimerGate onAccept={accept} />
        </main>
      )}

      <footer className="foot">
        <span>{t("app.title")}</span>
      </footer>
    </div>
  );
}

export default function App() {
  return (
    <I18nProvider>
      <Shell />
    </I18nProvider>
  );
}
