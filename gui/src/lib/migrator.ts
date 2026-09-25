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

// ---- Cloud (one device, one configuration) --------------------------------

export interface CloudStatus {
  device_id_masked: string;
  has_configuration: boolean;
  sha256: string | null;
  size_bytes: number;
  uploaded_at_ms: number;
}

export interface ConnTest {
  reachable: boolean;
  api_version: number;
  latency_ms: number;
  device_id_masked: string;
}

export interface CloudRestoreDone {
  package_path: string;
  report: RestoreReport;
}

// ---- Installer (Setup screen) --------------------------------------------

export interface SystemInfo {
  os: string;
  arch: string;
  hermes_installed: boolean;
  hermes_home: string | null;
  hermes_version: string | null;
  git_available: boolean;
  python_available: boolean;
  curl_available: boolean;
  free_space_bytes: number | null;
}

export interface InstallOptions {
  skip_browser: boolean;
}

export function defaultInstallOptions(): InstallOptions {
  return { skip_browser: false };
}

export interface InstallDone {
  success: boolean;
  installer: string;
  hermes_installed_now: boolean;
  hermes_home: string | null;
  hermes_version: string | null;
  log_tail: string[];
}

export interface InstallerLogLine {
  line: string;
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
