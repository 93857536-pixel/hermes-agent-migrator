// Types that mirror the Rust structs returned by the Tauri commands.
// Keep in sync with crates/migrator-core (serde lowercase enums, BTreeMap -> Record).

export interface HermesInstall {
  root: string;
  version: string;
  install_kind: string;
}

export interface ScanReport {
  detected: boolean;
  hermes_home: string | null; // Option<String>
  install: HermesInstall | null;
  config_files: string[];
  /** component name -> status */
  components: Record<string, "present" | "missing" | "attention">;
  notes: string[];
  total_files: number;
  total_bytes: number;
}

export interface PackOptions {
  include_config: boolean;
  include_skills: boolean;
  include_plugins: boolean;
  include_sessions: boolean;
  include_state_db: boolean;
  include_cron: boolean;
  include_memories: boolean;
  include_kanban: boolean;
  include_pastes: boolean;
  include_application: boolean;
  secrets_passphrase: string | null;
}

export function defaultPackOptions(): PackOptions {
  return {
    include_config: true,
    include_skills: true,
    include_plugins: true,
    include_sessions: true,
    include_state_db: true,
    include_cron: true,
    include_memories: true,
    include_kanban: true,
    include_pastes: true,
    include_application: false,
    secrets_passphrase: null,
  };
}

export interface PackResult {
  out_path: string;
  manifest_summary: string;
}

export interface CheckItem {
  name: string;
  ok: boolean;
  severity: "hard" | "soft";
  detail: string;
}

export interface EnvCheck {
  ready: boolean;
  items: CheckItem[];
  reason: string | null;
}

export interface RestoreOptions {
  passphrase: string | null;
  target_hermes_home: string | null;
  only_components: string[] | null;
  apply_path_repair: boolean;
}

export function defaultRestoreOptions(): RestoreOptions {
  return {
    passphrase: null,
    target_hermes_home: null,
    only_components: null,
    apply_path_repair: true,
  };
}

export interface RestoreReport {
  /** [component, all-verified] tuples (JS arrays) */
  verified: [string, boolean][];
  secrets_restored: number;
  needs_reauth: string[];
  notes: string[];
}

export interface ProgressEvt {
  stage: string;
  pct: number;
  done: number;
  total: number;
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
