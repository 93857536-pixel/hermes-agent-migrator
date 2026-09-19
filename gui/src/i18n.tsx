import { createContext, useContext, useState } from "react";

export type Lang = "en" | "zh";

const dict: Record<Lang, Record<string, string>> = {
  en: {
    "app.title": "Hermes Agent Migrator",
    "app.tagline": "Move your Hermes environment anywhere, effortlessly.",
    "nav.scan": "Scan",
    "nav.pack": "Package",
    "nav.restore": "Restore",
    "scan.title": "Scan your environment",
    "scan.subtitle": "Detect your Hermes Agent installation and user data.",
    "scan.run": "Run scan",
    "scan.running": "Scanning…",
    "scan.detected": "Hermes Agent detected",
    "scan.notdetected": "Hermes Agent not found",
    "scan.home": "Hermes home",
    "scan.version": "Version",
    "scan.components": "Components",
    "scan.notes": "Notes",
    "scan.total": "Files / size",
    "scan.to_pack": "Continue to packaging →",
    "comp.config": "Configuration",
    "comp.skills": "Skills",
    "comp.plugins": "Plugins",
    "comp.sessions": "Sessions",
    "comp.state_db": "State database",
    "comp.cron": "Scheduled tasks",
    "comp.memories": "Memories",
    "comp.kanban": "Kanban",
    "comp.pastes": "Pastes",
    "comp.secrets": "Secrets",
    "comp.application": "Application",
    "pack.title": "Create a migration package",
    "pack.subtitle": "Bundle your configuration and user data into one portable file.",
    "pack.select": "What to include",
    "pack.application": "Include application source (excludes venv / node_modules)",
    "pack.secrets": "Encrypt secrets with a passphrase",
    "pack.secrets_help": "API keys and tokens are encrypted with AES-256-GCM (Argon2id KDF). You'll need this passphrase on the target machine.",
    "pack.passphrase": "Passphrase",
    "pack.out": "Output file",
    "pack.create": "Create package",
    "pack.creating": "Packaging…",
    "pack.done": "Migration complete",
    "pack.done_help": "Transfer this file to the target machine and run Restore.",
    "restore.title": "Restore a migration package",
    "restore.subtitle": "Open a .hermesmig file and restore it onto this machine.",
    "restore.choose": "Choose package",
    "restore.no_pkg": "No package selected",
    "restore.target": "Restore to",
    "restore.target_default": "This machine's Hermes home",
    "restore.env_check": "Environment check",
    "restore.needs_reauth": "Needs re-authentication",
    "restore.run": "Run restore",
    "restore.running": "Restoring…",
    "restore.result": "Restore result",
    "restore.success": "All components verified",
    "restore.rolled_back": "Verification failed — previous state restored",
    "restore.secret_passphrase": "Passphrase",
    "restore.path_repair": "Rewrite absolute paths to this machine",
    "common.ready": "Ready",
    "common.failed": "Failed",
    "common.cancel": "Cancel",
    "common.close": "Close",
    "common.bytes": "B",
  },
  zh: {
    "app.title": "Hermes Agent 迁移工具",
    "app.tagline": "把你的 Hermes 环境，轻松迁移到任何一台电脑。",
    "nav.scan": "扫描",
    "nav.pack": "打包",
    "nav.restore": "恢复",
    "scan.title": "扫描你的环境",
    "scan.subtitle": "检测你的 Hermes Agent 安装与用户数据。",
    "scan.run": "开始扫描",
    "scan.running": "正在扫描…",
    "scan.detected": "检测到 Hermes Agent",
    "scan.notdetected": "未找到 Hermes Agent",
    "scan.home": "Hermes 目录",
    "scan.version": "版本",
    "scan.components": "组件",
    "scan.notes": "说明",
    "scan.total": "文件 / 大小",
    "scan.to_pack": "进入打包 →",
    "comp.config": "配置",
    "comp.skills": "技能",
    "comp.plugins": "插件",
    "comp.sessions": "会话",
    "comp.state_db": "状态数据库",
    "comp.cron": "定时任务",
    "comp.memories": "记忆",
    "comp.kanban": "看板",
    "comp.pastes": "粘贴记录",
    "comp.secrets": "密钥",
    "comp.application": "应用本体",
    "pack.title": "创建迁移包",
    "pack.subtitle": "把配置与用户数据打包成一个可携带文件。",
    "pack.select": "选择要包含的内容",
    "pack.application": "包含应用源码（排除 venv / node_modules）",
    "pack.secrets": "用口令加密密钥",
    "pack.secrets_help": "API Key 与令牌使用 AES-256-GCM（Argon2id 派生）加密。目标机器恢复时需要该口令。",
    "pack.passphrase": "口令",
    "pack.out": "输出文件",
    "pack.create": "创建迁移包",
    "pack.creating": "正在打包…",
    "pack.done": "迁移完成",
    "pack.done_help": "把这个文件传到目标机器，然后运行「恢复」。",
    "restore.title": "恢复迁移包",
    "restore.subtitle": "打开一个 .hermesmig 文件并恢复到本机。",
    "restore.choose": "选择迁移包",
    "restore.no_pkg": "尚未选择迁移包",
    "restore.target": "恢复到",
    "restore.target_default": "本机的 Hermes 目录",
    "restore.env_check": "环境检查",
    "restore.needs_reauth": "需要重新鉴权",
    "restore.run": "开始恢复",
    "restore.running": "正在恢复…",
    "restore.result": "恢复结果",
    "restore.success": "所有组件校验通过",
    "restore.rolled_back": "校验失败 — 已回滚到之前状态",
    "restore.secret_passphrase": "口令",
    "restore.path_repair": "把绝对路径改写为本机路径",
    "common.ready": "就绪",
    "common.failed": "失败",
    "common.cancel": "取消",
    "common.close": "关闭",
    "common.bytes": "字节",
  },
};

export function detectLang(): Lang {
  const l = (typeof navigator !== "undefined" ? navigator.language : "en").toLowerCase();
  return l.startsWith("zh") ? "zh" : "en";
}

interface I18nCtx {
  lang: Lang;
  setLang: (l: Lang) => void;
  t: (k: string) => string;
}

const Ctx = createContext<I18nCtx>({
  lang: "en",
  setLang: () => {},
  t: (k) => k,
});

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [lang, setLang] = useState<Lang>(detectLang());
  const t = (k: string) => dict[lang][k] ?? dict.en[k] ?? k;
  return <Ctx.Provider value={{ lang, setLang, t }}>{children}</Ctx.Provider>;
}

export function useI18n() {
  return useContext(Ctx);
}
