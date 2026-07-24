import type { ChainTone } from "./chainUi";
import type { InstructionsEntryState, InstructionsGlobalFile } from "./tauri";

/**
 * Health verdict per entry shape (design §1, §3):
 * - ok: the compliant forms — pure/append wrapper, and native agents that read
 *   the canonical body directly.
 * - dim: symlink entry — a compliant variant, informational only (carries a
 *   Windows caveat, so it is not celebrated as the norm).
 * - warn: deviations or functional gaps — content stranded in an entry body,
 *   or an agent with no entry into the project at all.
 */
export const INSTRUCTIONS_STATE_TONE: Record<InstructionsEntryState, ChainTone> = {
  wrapper: "ok",
  wrapper_plus: "ok",
  native: "ok",
  symlink: "dim",
  body: "warn",
  missing: "warn",
};

/** Human-readable byte size in KiB units (design shows `12.3 KiB`). Bytes are
 * the ground-truth measure (§2); tokens are only an estimate on top. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kib = bytes / 1024;
  if (kib < 1024) return `${kib.toFixed(1)} KiB`;
  return `${(kib / 1024).toFixed(1)} MiB`;
}

/** Token estimate, ALWAYS prefixed with `~` — the count is an estimate, never
 * exact (design §2; AC: token values always carry `~`). */
export function formatTokens(est: number): string {
  if (est < 1000) return `~${est}`;
  return `~${(est / 1000).toFixed(1)}k`;
}

/** Per-agent global-resident token estimate: the sum over every global surface
 * that agent reads. Derived from `scan.globals` + their reader sets — the same
 * arithmetic the service uses, so the overview bar never re-computes cost. */
export function agentGlobalTokens(globals: InstructionsGlobalFile[], agent: string): number {
  return globals.reduce((sum, g) => (g.readers.includes(agent) ? sum + g.est_tokens : sum), 0);
}
