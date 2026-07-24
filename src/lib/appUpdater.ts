import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import { getSettings, setSettings } from "./tauri";

export const APP_UPDATE_ENABLED_SETTING = "app_auto_update_enabled";
export const APP_UPDATE_LAST_ATTEMPT_SETTING = "app_auto_update_last_attempt_at";
export const APP_UPDATE_THROTTLE_MS = 24 * 60 * 60 * 1000;

export interface AppUpdateCandidate {
  version: string;
  downloadAndInstall: () => Promise<void>;
  close: () => Promise<void>;
}

export interface AppUpdaterRuntime {
  getSetting: (key: string) => Promise<string | null>;
  setSetting: (key: string, value: string) => Promise<void>;
  checkForUpdate: () => Promise<AppUpdateCandidate | null>;
  relaunch: () => Promise<void>;
  now: () => Date;
  logError: (message: string, error: unknown) => void;
}

export type AutomaticAppUpdateCheckResult =
  | { status: "disabled" | "throttled" | "no-update" | "failed" }
  | { status: "update-available"; version: string };

export type AppUpdateInstallResult =
  | { status: "no-update" }
  | { status: "version-changed"; version: string }
  | { status: "installed"; version: string };

export function isAppAutoUpdateEnabled(value: string | null): boolean {
  return !["false", "0", "no", "off"].includes(value?.trim().toLowerCase() ?? "");
}

function isThrottled(lastAttempt: string | null, now: Date): boolean {
  if (!lastAttempt) return false;
  const timestamp = Date.parse(lastAttempt);
  return Number.isFinite(timestamp) && now.getTime() - timestamp < APP_UPDATE_THROTTLE_MS;
}

export async function runAutomaticAppUpdateCheck(
  runtime: AppUpdaterRuntime,
): Promise<AutomaticAppUpdateCheckResult> {
  try {
    if (!isAppAutoUpdateEnabled(await runtime.getSetting(APP_UPDATE_ENABLED_SETTING))) {
      return { status: "disabled" };
    }

    const now = runtime.now();
    if (isThrottled(await runtime.getSetting(APP_UPDATE_LAST_ATTEMPT_SETTING), now)) {
      return { status: "throttled" };
    }

    await runtime.setSetting(APP_UPDATE_LAST_ATTEMPT_SETTING, now.toISOString());
    const update = await runtime.checkForUpdate();
    if (!update) return { status: "no-update" };

    const version = update.version;
    await update.close();
    return { status: "update-available", version };
  } catch (error) {
    runtime.logError("Automatic app update check failed", error);
    return { status: "failed" };
  }
}

export async function installAvailableAppUpdate(
  runtime: AppUpdaterRuntime,
  approvedVersion: string,
): Promise<AppUpdateInstallResult> {
  const update = await runtime.checkForUpdate();
  if (!update) return { status: "no-update" };
  if (update.version !== approvedVersion) {
    const version = update.version;
    await update.close();
    return { status: "version-changed", version };
  }

  try {
    await update.downloadAndInstall();
  } finally {
    await update.close();
  }
  await runtime.relaunch();
  return { status: "installed", version: update.version };
}

export const tauriAppUpdaterRuntime: AppUpdaterRuntime = {
  getSetting: getSettings,
  setSetting: setSettings,
  checkForUpdate: check,
  relaunch,
  now: () => new Date(),
  logError: (message, error) => console.error(message, error),
};
