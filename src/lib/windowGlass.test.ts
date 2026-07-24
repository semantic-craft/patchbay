import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyWindowGlassAttribute } from "./windowGlass";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

describe("applyWindowGlassAttribute", () => {
  let root: HTMLElement;

  beforeEach(() => {
    invokeMock.mockReset();
    root = document.createElement("html");
  });

  it("marks the root when the backend applied real liquid glass", async () => {
    invokeMock.mockResolvedValue("liquid-glass");
    await expect(applyWindowGlassAttribute(root)).resolves.toBe("liquid-glass");
    expect(invokeMock).toHaveBeenCalledWith("window_glass_status");
    expect(root.dataset.windowGlass).toBe("liquid-glass");
  });

  it("marks the root when the backend fell back to vibrancy", async () => {
    invokeMock.mockResolvedValue("vibrancy");
    await expect(applyWindowGlassAttribute(root)).resolves.toBe("vibrancy");
    expect(root.dataset.windowGlass).toBe("vibrancy");
  });

  it("leaves the wallpaper opaque when the backend reports none", async () => {
    invokeMock.mockResolvedValue("none");
    await expect(applyWindowGlassAttribute(root)).resolves.toBe("none");
    expect(root.dataset.windowGlass).toBeUndefined();
  });

  it("leaves the wallpaper opaque when the invoke itself fails", async () => {
    invokeMock.mockRejectedValue(new Error("not running under tauri"));
    await expect(applyWindowGlassAttribute(root)).resolves.toBe("none");
    expect(root.dataset.windowGlass).toBeUndefined();
  });
});
