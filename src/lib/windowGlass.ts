import { getWindowGlassStatus, type WindowGlassTier } from "./tauri";

/**
 * Window-level glass (#37): mark `<html>` with the tier the backend actually
 * applied so the CSS wallpaper can yield to the native material behind the
 * transparent window. When the backend reports "none" (apply failed,
 * non-macOS) — or the invoke itself fails — the attribute stays off and the
 * opaque wallpaper keeps covering the window, so nothing shows through bare.
 */
export async function applyWindowGlassAttribute(
  root: HTMLElement = document.documentElement
): Promise<WindowGlassTier> {
  let tier: WindowGlassTier = "none";
  try {
    tier = await getWindowGlassStatus();
  } catch {
    // Not running under Tauri (tests, plain browser) — keep the wallpaper.
  }
  if (tier === "liquid-glass" || tier === "vibrancy") {
    root.dataset.windowGlass = tier;
  }
  return tier;
}
