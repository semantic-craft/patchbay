/**
 * Report-timestamp helpers for the 多机 page.
 *
 * These live outside `Fleet.tsx` because a module that exports React components
 * may not also export plain functions without breaking Fast Refresh — the
 * `react-refresh/only-export-components` rule, which is part of `npm run lint`
 * and therefore a release gate.
 */

/** A report older than this is flagged stale in the column header (design §6). */
export const REPORT_STALE_MS = 7 * 24 * 60 * 60 * 1000;

/** Bucketed relative age for report timestamps; pure so tests can pin `now`. */
export function reportAge(reportedAt: string, now: number): { key: string; count: number } {
  const seconds = Math.max(0, Math.floor((now - Date.parse(reportedAt)) / 1000));
  if (seconds < 10) return { key: "fleet.age.justNow", count: 0 };
  if (seconds < 60) return { key: "fleet.age.secondsAgo", count: seconds };
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return { key: "fleet.age.minutesAgo", count: minutes };
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return { key: "fleet.age.hoursAgo", count: hours };
  return { key: "fleet.age.daysAgo", count: Math.floor(hours / 24) };
}
