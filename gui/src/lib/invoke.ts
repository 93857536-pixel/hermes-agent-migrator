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

/** Subscribe to `migrator://progress` events; returns an unlisten fn. */
export async function onProgress(
  fn: (evt: ProgressEvt) => void,
): Promise<() => void> {
  const unlisten = await listen<ProgressEvt>("migrator://progress", (e) =>
    fn(e.payload),
  );
  return unlisten;
}
