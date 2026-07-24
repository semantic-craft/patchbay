import { describe, expect, it, vi } from "vitest";
import {
  APP_UPDATE_ENABLED_SETTING,
  APP_UPDATE_LAST_ATTEMPT_SETTING,
  APP_UPDATE_THROTTLE_MS,
  installAvailableAppUpdate,
  isAppAutoUpdateEnabled,
  runAutomaticAppUpdateCheck,
  type AppUpdateCandidate,
  type AppUpdaterRuntime,
} from "./appUpdater";

const NOW = new Date("2026-07-16T08:00:00.000Z");

function candidate(version = "1.29.4"): AppUpdateCandidate {
  return {
    version,
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
  };
}

function runtime(overrides: Partial<AppUpdaterRuntime> = {}): AppUpdaterRuntime {
  return {
    getSetting: vi.fn().mockResolvedValue(null),
    setSetting: vi.fn().mockResolvedValue(undefined),
    checkForUpdate: vi.fn().mockResolvedValue(null),
    relaunch: vi.fn().mockResolvedValue(undefined),
    now: () => NOW,
    logError: vi.fn(),
    ...overrides,
  };
}

describe("runAutomaticAppUpdateCheck", () => {
  it("is enabled by default and persists the attempt before checking", async () => {
    const update = candidate();
    const events: string[] = [];
    const deps = runtime({
      setSetting: vi.fn(async () => { events.push("persist"); }),
      checkForUpdate: vi.fn(async () => {
        events.push("check");
        return update;
      }),
    });

    await expect(runAutomaticAppUpdateCheck(deps)).resolves.toEqual({
      status: "update-available",
      version: "1.29.4",
    });
    expect(events).toEqual(["persist", "check"]);
    expect(deps.setSetting).toHaveBeenCalledWith(
      APP_UPDATE_LAST_ATTEMPT_SETTING,
      NOW.toISOString(),
    );
    expect(update.close).toHaveBeenCalledOnce();
  });

  it("does not check when explicitly disabled", async () => {
    const deps = runtime({
      getSetting: vi.fn(async (key) =>
        key === APP_UPDATE_ENABLED_SETTING ? "off" : null,
      ),
    });

    await expect(runAutomaticAppUpdateCheck(deps)).resolves.toEqual({ status: "disabled" });
    expect(deps.checkForUpdate).not.toHaveBeenCalled();
    expect(deps.setSetting).not.toHaveBeenCalled();
  });

  it("does not check again inside the 24 hour throttle window", async () => {
    const recentAttempt = new Date(NOW.getTime() - APP_UPDATE_THROTTLE_MS + 1).toISOString();
    const deps = runtime({
      getSetting: vi.fn(async (key) =>
        key === APP_UPDATE_LAST_ATTEMPT_SETTING ? recentAttempt : null,
      ),
    });

    await expect(runAutomaticAppUpdateCheck(deps)).resolves.toEqual({ status: "throttled" });
    expect(deps.checkForUpdate).not.toHaveBeenCalled();
    expect(deps.setSetting).not.toHaveBeenCalled();
  });

  it("records a due check with no update", async () => {
    const staleAttempt = new Date(NOW.getTime() - APP_UPDATE_THROTTLE_MS).toISOString();
    const deps = runtime({
      getSetting: vi.fn(async (key) =>
        key === APP_UPDATE_LAST_ATTEMPT_SETTING ? staleAttempt : null,
      ),
    });

    await expect(runAutomaticAppUpdateCheck(deps)).resolves.toEqual({ status: "no-update" });
    expect(deps.checkForUpdate).toHaveBeenCalledOnce();
  });

  it("logs a failed due check without rejecting startup", async () => {
    const failure = new Error("offline");
    const deps = runtime({
      checkForUpdate: vi.fn().mockRejectedValue(failure),
    });

    await expect(runAutomaticAppUpdateCheck(deps)).resolves.toEqual({ status: "failed" });
    expect(deps.logError).toHaveBeenCalledWith("Automatic app update check failed", failure);
    expect(deps.setSetting).toHaveBeenCalledWith(
      APP_UPDATE_LAST_ATTEMPT_SETTING,
      NOW.toISOString(),
    );
  });
});

describe("isAppAutoUpdateEnabled", () => {
  it("defaults to enabled and recognizes persisted disabled values", () => {
    expect(isAppAutoUpdateEnabled(null)).toBe(true);
    expect(isAppAutoUpdateEnabled("on")).toBe(true);
    expect(isAppAutoUpdateEnabled(" OFF ")).toBe(false);
    expect(isAppAutoUpdateEnabled("false")).toBe(false);
  });
});

describe("installAvailableAppUpdate", () => {
  it("downloads the signed update and explicitly relaunches", async () => {
    const update = candidate();
    const deps = runtime({ checkForUpdate: vi.fn().mockResolvedValue(update) });

    await expect(installAvailableAppUpdate(deps, "1.29.4")).resolves.toEqual({
      status: "installed",
      version: "1.29.4",
    });
    expect(update.downloadAndInstall).toHaveBeenCalledOnce();
    expect(update.close).toHaveBeenCalledOnce();
    expect(deps.relaunch).toHaveBeenCalledOnce();
  });

  it("requires fresh approval when the available version changed", async () => {
    const update = candidate("1.29.5");
    const deps = runtime({ checkForUpdate: vi.fn().mockResolvedValue(update) });

    await expect(installAvailableAppUpdate(deps, "1.29.4")).resolves.toEqual({
      status: "version-changed",
      version: "1.29.5",
    });
    expect(update.close).toHaveBeenCalledOnce();
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(deps.relaunch).not.toHaveBeenCalled();
  });

  it("does not relaunch when the update is no longer available", async () => {
    const deps = runtime();

    await expect(installAvailableAppUpdate(deps, "1.29.4")).resolves.toEqual({ status: "no-update" });
    expect(deps.relaunch).not.toHaveBeenCalled();
  });
});
