/**
 * Which window chrome the OS draws for us.
 *
 * macOS hides its own title bar (`titleBarStyle: "Overlay"`), so the app draws
 * a drag strip and offsets content below it. Windows keeps a native caption
 * bar, which sits *above* that strip — two title bars and ~28px of dead space.
 * Tagging the root with `data-platform="windows"` collapses `--titlebar-h` to
 * zero, and every offset derives from that one variable.
 */
export type Platform = "windows" | "other";

export function detectPlatform(userAgent: string): Platform {
  return userAgent.includes("Windows") ? "windows" : "other";
}

export function applyPlatformAttribute(
  root: HTMLElement = document.documentElement,
  userAgent: string = navigator.userAgent
): Platform {
  const platform = detectPlatform(userAgent);
  root.dataset.platform = platform;
  return platform;
}
