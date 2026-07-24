import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { useTheme } from "./useTheme";
import { getCurrentWindow } from "@tauri-apps/api/window";

vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("../lib/tauri", () => ({
  getSettings: vi.fn().mockResolvedValue(null),
  setSettings: vi.fn().mockResolvedValue(undefined),
}));

// This environment's global localStorage is Node's non-functional stub (see
// App.test.tsx); the hook reads it at mount, so substitute a working one.
const store = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
});

const setThemeMock = vi.fn().mockResolvedValue(undefined);
vi.mocked(getCurrentWindow).mockReturnValue({
  setTheme: setThemeMock,
} as unknown as ReturnType<typeof getCurrentWindow>);

function stubMatchMedia(dark: boolean) {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: dark,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }) as unknown as typeof window.matchMedia;
}

describe("useTheme native window sync", () => {
  beforeEach(() => {
    setThemeMock.mockClear();
    localStorage.clear();
    document.documentElement.classList.remove("dark");
  });

  it("forces the native window dark when the stored theme is dark", async () => {
    localStorage.setItem("theme", "dark");
    renderHook(() => useTheme());
    await waitFor(() => expect(setThemeMock).toHaveBeenCalledWith("dark"));
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("forces the native window light when the stored theme is light", async () => {
    localStorage.setItem("theme", "light");
    renderHook(() => useTheme());
    await waitFor(() => expect(setThemeMock).toHaveBeenCalledWith("light"));
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("hands the native window back to the OS when the theme is system", async () => {
    stubMatchMedia(false);
    localStorage.setItem("theme", "system");
    renderHook(() => useTheme());
    await waitFor(() => expect(setThemeMock).toHaveBeenCalledWith(null));
  });

  it("re-syncs the native window when setTheme changes the theme", async () => {
    localStorage.setItem("theme", "light");
    const { result } = renderHook(() => useTheme());
    await waitFor(() => expect(setThemeMock).toHaveBeenCalledWith("light"));
    result.current.setTheme("dark");
    await waitFor(() => expect(setThemeMock).toHaveBeenCalledWith("dark"));
  });

  it("survives a window backend that rejects (non-Tauri env)", async () => {
    setThemeMock.mockRejectedValueOnce(new Error("no ipc"));
    localStorage.setItem("theme", "dark");
    renderHook(() => useTheme());
    await waitFor(() => expect(setThemeMock).toHaveBeenCalled());
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});
