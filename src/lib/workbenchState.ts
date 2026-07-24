import type { ChainDoctorReport, ChainFinding, ChainProject, ChainSeverity } from "./tauri";

/**
 * The workbench's exception-driven state (#26). The prototype's repair machine
 * runs `green → attention (evidence cards) → repairing (live apply) → repaired
 * (journalled, undoable)`; this is the entry fork that decides whether there is
 * anything to handle at all.
 *
 * `unknown` is not a health verdict: Doctor could not be reached, so the
 * workbench asserts nothing and falls back to the full link list. Only a report
 * we actually have can claim `green`.
 */
export type WorkbenchState = "unknown" | "green" | "attention";

/**
 * The findings this project needs its user to handle.
 *
 * Every project-scoped finding carries its project among the affected objects,
 * so a path match scopes the global report to one workbench. Repo-level
 * findings (an orphaned checkout) name no project and belong to Skill Sources,
 * not here.
 *
 * Findings the user has already dismissed are not in `report.findings` at all —
 * the service moves them to `report.ignored` — so an ignore decision needs no
 * second filter to keep the workbench green.
 */
export function projectFindings(
  report: ChainDoctorReport | null,
  project: ChainProject | null,
): ChainFinding[] {
  return findingsForPath(report, project?.path ?? null);
}

/** Same scoping as [projectFindings], by canonical path — the identity both
 * the topology and the project registry share, so the sidebar (which only has
 * registry records) can reuse it. */
export function findingsForPath(
  report: ChainDoctorReport | null,
  path: string | null,
): ChainFinding[] {
  if (!report || !path) return [];
  return report.findings.filter((finding) =>
    finding.affected.some((obj) => obj.kind === "project" && obj.path === path),
  );
}

export function workbenchState(
  report: ChainDoctorReport | null,
  project: ChainProject | null,
): WorkbenchState {
  if (!report || !project) return "unknown";
  return projectFindings(report, project).length === 0 ? "green" : "attention";
}

/** High-to-low severity rank for client-side ordering. Doctor's own ordering
 * is explicitly not a wire contract, so the workbench sorts for itself. */
export const SEVERITY_RANK: Record<ChainSeverity, number> = {
  violation: 0,
  warning: 1,
  advice: 2,
  notice: 3,
};

/** One project's health for the sidebar dot: the workbench state plus the
 * worst severity among its findings (null when green or unknown). */
export function projectHealth(
  report: ChainDoctorReport | null,
  path: string,
): { state: WorkbenchState; worst: ChainSeverity | null } {
  if (!report) return { state: "unknown", worst: null };
  const findings = findingsForPath(report, path);
  if (findings.length === 0) return { state: "green", worst: null };
  const worst = findings.reduce((acc, finding) =>
    SEVERITY_RANK[finding.severity] < SEVERITY_RANK[acc.severity] ? finding : acc,
  );
  return { state: "attention", worst: worst.severity };
}
