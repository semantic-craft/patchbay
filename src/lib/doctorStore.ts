import { useSyncExternalStore } from "react";
import type { ChainDoctorReport } from "./tauri";

/**
 * App-wide holder for the latest chain Doctor report (#30: the sidebar health
 * dots consume the same report the workbench renders as evidence cards).
 *
 * The workbench owns the scan cadence and PUBLISHES here after each load; the
 * sidebar only subscribes. A dedicated store keeps this one-way flow out of
 * AppContext (whose refresh cycle would otherwise trigger a second Doctor scan
 * per filesystem event) and lets views stay renderable without a provider.
 */
let report: ChainDoctorReport | null = null;
const listeners = new Set<() => void>();

export function publishDoctorReport(next: ChainDoctorReport | null): void {
  report = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useDoctorReport(): ChainDoctorReport | null {
  return useSyncExternalStore(subscribe, () => report);
}
