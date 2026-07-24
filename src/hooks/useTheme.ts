import { useState, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "../lib/tauri";

export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "theme";

function getSystemTheme(): ResolvedTheme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function applyThemeClass(resolved: ResolvedTheme) {
  const root = document.documentElement;
  if (resolved === "dark") {
    root.classList.add("dark");
  } else {
    root.classList.remove("dark");
  }
}

/**
 * Keep the native window appearance in step with the app theme so the
 * window-level glass material (#37) and title bar never mismatch the UI.
 * `null` hands the window back to the OS when the user picks "system".
 */
function syncWindowTheme(theme: Theme, resolved: ResolvedTheme) {
  try {
    getCurrentWindow()
      .setTheme(theme === "system" ? null : resolved)
      .catch(() => {});
  } catch {
    // Not running under Tauri (tests, plain browser) — CSS-only theming.
  }
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system")
      return stored;
    return "dark";
  });

  const resolvedTheme: ResolvedTheme =
    theme === "system" ? getSystemTheme() : theme;

  // Apply class + native window appearance on mount and theme change
  useEffect(() => {
    applyThemeClass(resolvedTheme);
    syncWindowTheme(theme, resolvedTheme);
  }, [theme, resolvedTheme]);

  // Listen for system preference changes when in "system" mode
  useEffect(() => {
    if (theme !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => applyThemeClass(getSystemTheme());
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [theme]);

  // Load from Tauri settings on mount
  useEffect(() => {
    api.getSettings("theme").then((v) => {
      if (v === "light" || v === "dark" || v === "system") {
        setThemeState(v);
        localStorage.setItem(STORAGE_KEY, v);
      }
    });
  }, []);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    localStorage.setItem(STORAGE_KEY, next);
    api.setSettings("theme", next);
  }, []);

  return { theme, setTheme, resolvedTheme };
}
