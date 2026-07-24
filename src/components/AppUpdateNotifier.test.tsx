import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("sonner", () => ({
  toast: {
    info: vi.fn(),
    loading: vi.fn(),
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { toast } from "sonner";
import { AppUpdateNotifier } from "./AppUpdateNotifier";
import type { AppUpdateCandidate, AppUpdaterRuntime } from "../lib/appUpdater";

function candidate(version = "1.29.4"): AppUpdateCandidate {
  return {
    version,
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
  };
}

function runtime(updates: Array<AppUpdateCandidate | null>): AppUpdaterRuntime {
  return {
    getSetting: vi.fn().mockResolvedValue(null),
    setSetting: vi.fn().mockResolvedValue(undefined),
    checkForUpdate: vi.fn().mockImplementation(async () => updates.shift() ?? null),
    relaunch: vi.fn().mockResolvedValue(undefined),
    now: () => new Date("2026-07-16T08:00:00.000Z"),
    logError: vi.fn(),
  };
}

describe("AppUpdateNotifier", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(toast.info).mockReset();
    vi.mocked(toast.loading).mockReset();
    vi.mocked(toast.success).mockReset();
    vi.mocked(toast.error).mockReset();
  });

  afterEach(() => vi.useRealTimers());

  it("delays startup checking and emits one persistent actionable notification", async () => {
    const checkCandidate = candidate();
    const installCandidate = candidate();
    const deps = runtime([checkCandidate, installCandidate]);

    const view = render(<AppUpdateNotifier runtime={deps} delayMs={3000} />);
    view.rerender(<AppUpdateNotifier runtime={deps} delayMs={3000} />);

    expect(deps.checkForUpdate).not.toHaveBeenCalled();
    await act(async () => { vi.advanceTimersByTime(2999); });
    expect(deps.checkForUpdate).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });

    expect(toast.info).toHaveBeenCalledOnce();
    expect(toast.info).toHaveBeenCalledWith(
      "Patchbay 1.29.4 is ready to install.",
      expect.objectContaining({
        id: "app-update-available",
        duration: Infinity,
        action: expect.objectContaining({ label: "Install and restart" }),
      }),
    );
    expect(installCandidate.downloadAndInstall).not.toHaveBeenCalled();
    expect(deps.relaunch).not.toHaveBeenCalled();

    const options = vi.mocked(toast.info).mock.calls[0][1];
    await act(async () => {
      await options?.action?.onClick?.();
    });

    expect(installCandidate.downloadAndInstall).toHaveBeenCalledOnce();
    expect(deps.relaunch).toHaveBeenCalledOnce();
    expect(deps.checkForUpdate).toHaveBeenCalledTimes(2);
  });

  it("cancels the delayed check when startup integration unmounts", async () => {
    const deps = runtime([candidate()]);
    const view = render(<AppUpdateNotifier runtime={deps} delayMs={3000} />);

    view.unmount();
    await act(async () => { vi.advanceTimersByTime(3000); });

    expect(deps.checkForUpdate).not.toHaveBeenCalled();
    expect(toast.info).not.toHaveBeenCalled();
  });

  it("asks again instead of installing when the release version changed", async () => {
    const firstCheck = candidate("1.29.4");
    const changedCheck = candidate("1.29.5");
    const deps = runtime([firstCheck, changedCheck]);
    render(<AppUpdateNotifier runtime={deps} delayMs={3000} />);

    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    const firstOptions = vi.mocked(toast.info).mock.calls[0][1];
    await act(async () => {
      await firstOptions?.action?.onClick?.();
    });

    expect(changedCheck.downloadAndInstall).not.toHaveBeenCalled();
    expect(deps.relaunch).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledTimes(2);
    expect(toast.info).toHaveBeenLastCalledWith(
      "Patchbay 1.29.5 is ready to install.",
      expect.objectContaining({ duration: Infinity }),
    );
  });
});
