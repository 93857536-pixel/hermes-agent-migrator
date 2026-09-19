import React, { useEffect, useState } from "react";
import { I18nProvider, useI18n } from "./i18n";
import Scan from "./screens/Scan";
import Pack from "./screens/Pack";
import Restore from "./screens/Restore";

type Screen = "scan" | "pack" | "restore";

const theme = () => {
  const m = window.matchMedia?.("(prefers-color-scheme: dark)");
  return m?.matches ? "dark" : "light";
};

function Shell() {
  const { t, lang, setLang } = useI18n();
  const [screen, setScreen] = useState<Screen>("scan");
  const [dark, setDark] = useState<boolean>(() => theme() === "dark");

  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  }, [dark]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const on = (e: MediaQueryListEvent) => setDark(e.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  const nav: Screen[] = ["scan", "pack", "restore"];

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
            onClick={() => setLang(lang === "en" ? "zh" : "en")}
            title="Switch language"
          >
            {lang === "en" ? "中文" : "EN"}
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
      </main>

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
