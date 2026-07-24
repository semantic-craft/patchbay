import type { ChainEntryStatus, ChainRepoState } from "./tauri";

export type ChainTone = "ok" | "warn" | "err" | "dim";

/** Convention verdict per entry status: ok = follows the three-step link
 * model, warn = works but deviates (direct / unmanaged copy), err = broken. */
export const STATUS_TONE: Record<ChainEntryStatus, ChainTone> = {
  link_repo: "ok",
  via_agents: "ok",
  private: "dim",
  internal: "dim",
  external: "dim",
  direct: "warn",
  copy: "warn",
  broken: "err",
};

/** Health verdict per repository tracking state: ok = up to date, warn =
 * out of sync in a recoverable way (ahead / behind / diverged / dirty),
 * err = could not be inspected, dim = nothing to compare against. */
export const HEALTH_TONE: Record<ChainRepoState, ChainTone> = {
  up_to_date: "ok",
  ahead: "warn",
  behind: "warn",
  diverged: "warn",
  no_upstream: "dim",
  detached: "warn",
  scan_error: "err",
};

export const TONE_BADGE: Record<ChainTone, string> = {
  ok: "border-emerald-500/25 bg-emerald-500/10 text-emerald-400",
  warn: "border-amber-500/25 bg-amber-500/10 text-amber-400",
  err: "border-red-500/25 bg-red-500/10 text-red-400",
  dim: "border-border-subtle bg-surface-hover text-muted",
};

export const TONE_DOT: Record<ChainTone, string> = {
  ok: "bg-emerald-400",
  warn: "bg-amber-400",
  err: "bg-red-400",
  dim: "bg-gray-400",
};

export const TONE_STROKE: Record<ChainTone, string> = {
  ok: "#34d399",
  warn: "#fbbf24",
  err: "#f87171",
  dim: "#9ca3af",
};

/** A scan older than this is flagged "stale" — its data may no longer match
 * disk. Purely advisory; the user decides when to rescan. */
export const SCAN_STALE_MS = 5 * 60 * 1000;

/** Map a scan completion time (epoch ms) to a localized "scanned … ago" i18n
 * key plus its interpolation count. Buckets: <10s just now, then seconds,
 * minutes, hours, days. Kept pure so the caller owns rendering + localization. */
export function relativeScanTime(
  scannedAt: number,
  now: number,
): { key: string; count: number } {
  const seconds = Math.max(0, Math.floor((now - scannedAt) / 1000));
  if (seconds < 10) return { key: "chain.freshness.scannedJustNow", count: 0 };
  if (seconds < 60) return { key: "chain.freshness.scannedSecondsAgo", count: seconds };
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return { key: "chain.freshness.scannedMinutesAgo", count: minutes };
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return { key: "chain.freshness.scannedHoursAgo", count: hours };
  return { key: "chain.freshness.scannedDaysAgo", count: Math.floor(hours / 24) };
}

/** Shorten an absolute path for display: warehouse and home become prefixes
 * the user actually thinks in. Matches against every configured warehouse
 * root (longest first, so the most specific root wins). */
export function shortenPath(p: string, warehouseRoots: string[], projectsRoot: string): string {
  const roots = warehouseRoots
    .filter(Boolean)
    .slice()
    .sort((a, b) => b.length - a.length);
  for (const root of roots) {
    if (p.startsWith(root)) {
      const base = root.split("/").pop() || root;
      return base + p.slice(root.length);
    }
  }
  if (projectsRoot && p.startsWith(projectsRoot)) {
    return "~/Projects" + p.slice(projectsRoot.length);
  }
  const home = projectsRoot.replace(/\/Projects$/, "");
  if (home && p.startsWith(home)) {
    return "~" + p.slice(home.length);
  }
  return p;
}
