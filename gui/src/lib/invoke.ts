import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ScanReport,
  PackOptions,
  PackResult,
  RestoreOptions,
  RestoreReport,
  EnvCheck,
  ProgressEvt,
  CloudStatus,
  ConnTest,
  CloudBackendInfo,
  CloudRestoreDone,
  SystemInfo,
  InstallOptions,
  InstallDone,
  InstallerLogLine,
} from "./migrator";

const toTauriError = (e: unknown): string => {
  const s = typeof e === "string" ? e : JSON.stringify(e);
  // Tauri returns Err values as the stringified error; strip quotes.
  return s.replace(/^"|"$/g, "");
};

export const scanHermes = () =>
  invoke<ScanReport>("scan").catch((e) => Promise.reject(toTauriError(e)));

export const packHermes = (opts: PackOptions, outPath: string) =>
  invoke<PackResult>("pack", { opts, outPath }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const checkEnvironment = (pkgPath: string) =>
  invoke<EnvCheck>("check_environment", { pkgPath }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const restoreHermes = (pkgPath: string, opts: RestoreOptions) =>
  invoke<{ report: RestoreReport }>("restore", {
    pkgPath,
    opts,
  }).catch((e) => Promise.reject(toTauriError(e)));

export const verifyPackage = (pkgPath: string) =>
  invoke<string[]>("verify_package", { pkgPath }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const backupCurrentHermes = () =>
  invoke<string>("backup_current_hermes").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

// ---- Cloud ----------------------------------------------------------------

export const cloudTestConnection = () =>
  invoke<ConnTest>("cloud_test_connection").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudStatus = () =>
  invoke<CloudStatus>("cloud_status").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudUpload = (passphrase: string, overwrite: boolean) =>
  invoke<CloudStatus>("cloud_upload", { passphrase, overwrite }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudDownloadRestore = (passphrase: string, outPath: string) =>
  invoke<CloudRestoreDone>("cloud_download_restore", {
    passphrase,
    outPath,
  }).catch((e) => Promise.reject(toTauriError(e)));

export const cloudDelete = () =>
  invoke<CloudStatus>("cloud_delete").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudServerInfo = () =>
  invoke<CloudBackendInfo>("cloud_server_info").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudServerSet = (url: string) =>
  invoke<CloudBackendInfo>("cloud_server_set", { url }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const cloudServerClear = () =>
  invoke<CloudBackendInfo>("cloud_server_clear").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

/** Subscribe to `migrator://progress` events; returns an unlisten fn. */
export async function onProgress(
  fn: (evt: ProgressEvt) => void,
): Promise<() => void> {
  const unlisten = await listen<ProgressEvt>("migrator://progress", (e) =>
    fn(e.payload),
  );
  return unlisten;
}

// ---- Installer (Setup screen) --------------------------------------------

export const installerDetectSystem = () =>
  invoke<SystemInfo>("installer_detect_system").catch((e) =>
    Promise.reject(toTauriError(e)),
  );

export const installerInstall = (opts: InstallOptions) =>
  invoke<InstallDone>("installer_install", { opts }).catch((e) =>
    Promise.reject(toTauriError(e)),
  );

/** Subscribe to `installer://log` events (streamed installer output). */
export async function onInstallerLog(
  fn: (line: string) => void,
): Promise<() => void> {
  const unlisten = await listen<InstallerLogLine>("installer://log", (e) =>
    fn(e.payload.line),
  );
  return unlisten;
}
