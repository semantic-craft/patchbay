import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  RefreshCw,
  ShieldCheck,
  ChevronDown,
  ChevronRight,
  RotateCcw,
  Wrench,
} from "lucide-react";
import { cn } from "../utils";
import { ChainScanStatus } from "../components/ChainScanStatus";
import {
  chainApplyRepair,
  chainDoctorReport,
  chainIgnoreFinding,
  chainPlanRepair,
  chainRestoreFinding,
  instructionsDoctorReport,
  instructionsIgnoreFinding,
  instructionsRestoreFinding,
} from "../lib/tauri";
import type {
  ChainDoctorReport,
  ChainFinding,
  ChainRepairItem,
  ChainRepairOutcome,
  ChainRepairPlan,
  ChainSeverity,
  ChainDeviation,
  InstructionsDoctorReport,
  InstructionsFinding,
  InstructionsRule,
} from "../lib/tauri";

// Fixed high-to-low order for the severity filter row.
const SEVERITIES: ChainSeverity[] = ["violation", "warning", "advice", "notice"];
// Fixed order for the deviation-type filter row (issue #9's six states).
const DEVIATIONS: ChainDeviation[] = [
  "broken",
  "direct",
  "copy",
  "project_private",
  "legacy",
  "orphan",
];
// Stable service presentation order for the instructions rule chips.
const INSTRUCTIONS_RULES: InstructionsRule[] = [
  "instructions.uninitialized",
  "instructions.missing_canonical",
  "instructions.dual_body",
  "instructions.duplicate_content",
  "instructions.missing_entry",
  "instructions.symlink_entry",
  "instructions.broken_import",
  "instructions.import_in_canonical",
  "instructions.oversized_body",
  "instructions.hard_cap_risk",
  "instructions.skill_missing",
  "instructions.skill_unmentioned",
  "instructions.entry_gitignored",
  "instructions.global_cost",
];

// Deviations Doctor can repair/normalize (issue #10). Others are read-only.
const REPAIRABLE: ReadonlySet<ChainDeviation> = new Set<ChainDeviation>([
  "broken",
  "direct",
  "legacy",
]);

/** Local state for an in-progress repair of a single finding. */
interface RepairState {
  fingerprint: string;
  plan: ChainRepairPlan | null;
  loading: boolean;
  applying: boolean;
  error: string | null;
  outcome: ChainRepairOutcome | null;
}

/** Severity → dot + chip/badge styling, mirroring the topology tone tokens. */
const SEVERITY_STYLE: Record<ChainSeverity, { dot: string; badge: string }> = {
  violation: { dot: "bg-red-400", badge: "border-red-500/25 bg-red-500/10 text-red-400" },
  warning: { dot: "bg-amber-400", badge: "border-amber-500/25 bg-amber-500/10 text-amber-400" },
  advice: { dot: "bg-blue-400", badge: "border-blue-500/25 bg-blue-500/10 text-blue-400" },
  notice: { dot: "bg-gray-400", badge: "border-border-subtle bg-surface-hover text-muted" },
};

type DoctorModule = "chain" | "instructions";
type ModuleFinding =
  | { module: "chain"; finding: ChainFinding }
  | { module: "instructions"; finding: InstructionsFinding };

interface DoctorReports {
  chain: ChainDoctorReport;
  instructions: InstructionsDoctorReport;
}

function findingKey(item: ModuleFinding): string {
  return `${item.module}:${item.finding.fingerprint}`;
}

function ruleShort(rule: string): string {
  return rule.slice("instructions.".length);
}

function useCounts<F, K extends string>(findings: F[], key: (f: F) => K) {
  return useMemo(() => {
    const counts = {} as Record<K, number>;
    for (const f of findings) {
      const k = key(f);
      counts[k] = (counts[k] ?? 0) + 1;
    }
    return counts;
  }, [findings, key]);
}

export function ChainDoctor() {
  const { t } = useTranslation();
  const [reports, setReports] = useState<DoctorReports | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Empty set on either axis means "no constraint"; the axes combine with AND.
  const [sevFilter, setSevFilter] = useState<Set<ChainSeverity>>(new Set());
  const [devFilter, setDevFilter] = useState<Set<ChainDeviation>>(new Set());
  const [ruleFilter, setRuleFilter] = useState<Set<InstructionsRule>>(new Set());
  const [expanded, setExpanded] = useState<string | null>(null);
  // At most one finding is being repaired at a time (preview → confirm → apply).
  const [repair, setRepair] = useState<RepairState | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // Fetch every finding once; filtering below is instant and client-side.
      // The service also filters (for CLI parity), but the GUI keeps the full
      // set so facet counts stay stable as the user toggles chips.
      const [chain, instructions] = await Promise.all([
        chainDoctorReport(),
        instructionsDoctorReport(),
      ]);
      setReports({ chain, instructions });
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Persist a decision through the owning module, then reload so the finding
  // moves into the shared Ignored panel. Both modules use the same settings
  // table, while disjoint rule prefixes keep their records separate.
  const ignore = useCallback(
    async (
      module: DoctorModule,
      rule: string,
      fingerprint: string,
      kind: "ignored" | "project_private",
    ) => {
      if (module === "chain") {
        await chainIgnoreFinding(rule, fingerprint, kind, null);
      } else {
        await instructionsIgnoreFinding(rule, fingerprint, null);
      }
      await load();
    },
    [load],
  );
  const restore = useCallback(
    async (module: DoctorModule, rule: string, fingerprint: string) => {
      if (module === "chain") {
        await chainRestoreFinding(rule, fingerprint);
      } else {
        await instructionsRestoreFinding(rule, fingerprint);
      }
      await load();
    },
    [load],
  );

  // Repair flow: preview the bounded edits for a single finding, then apply on
  // confirm. Planning re-scans server-side so the preview reflects current
  // evidence; applying rescans and verifies before reporting success.
  const startRepair = useCallback(async (fingerprint: string) => {
    setRepair({ fingerprint, plan: null, loading: true, applying: false, error: null, outcome: null });
    try {
      const plan = await chainPlanRepair([fingerprint]);
      setRepair((cur) =>
        cur && cur.fingerprint === fingerprint ? { ...cur, plan, loading: false } : cur,
      );
    } catch (e) {
      setRepair((cur) =>
        cur && cur.fingerprint === fingerprint ? { ...cur, loading: false, error: String(e) } : cur,
      );
    }
  }, []);

  const confirmRepair = useCallback(
    async (plan: ChainRepairPlan) => {
      setRepair((cur) => (cur ? { ...cur, applying: true, error: null } : cur));
      try {
        const outcome = await chainApplyRepair(plan);
        setRepair((cur) => (cur ? { ...cur, applying: false, outcome } : cur));
        // Refresh findings so a resolved deviation drops out of the list.
        await load();
      } catch (e) {
        setRepair((cur) => (cur ? { ...cur, applying: false, error: String(e) } : cur));
      }
    },
    [load],
  );

  const cancelRepair = useCallback(() => setRepair(null), []);

  const chainFindings = useMemo(() => reports?.chain.findings ?? [], [reports]);
  const instructionsFindings = useMemo(
    () => reports?.instructions.findings ?? [],
    [reports],
  );
  const findings = useMemo<ModuleFinding[]>(
    () => [
      ...chainFindings.map((finding) => ({ module: "chain" as const, finding })),
      ...instructionsFindings.map((finding) => ({ module: "instructions" as const, finding })),
    ],
    [chainFindings, instructionsFindings],
  );
  const ignored = useMemo<ModuleFinding[]>(
    () => [
      ...(reports?.chain.ignored ?? []).map((finding) => ({ module: "chain" as const, finding })),
      ...(reports?.instructions.ignored ?? []).map((finding) => ({
        module: "instructions" as const,
        finding,
      })),
    ],
    [reports],
  );
  const sevCounts = useCounts(findings, (item) => item.finding.severity);
  const devCounts = useCounts(chainFindings, (finding) => finding.deviation);
  const ruleCounts = useCounts(instructionsFindings, (finding) => finding.rule);

  const filtered = useMemo(() => {
    const typeFilterActive = devFilter.size > 0 || ruleFilter.size > 0;
    return findings.filter((item) => {
      if (sevFilter.size > 0 && !sevFilter.has(item.finding.severity)) return false;
      if (!typeFilterActive) return true;
      return item.module === "chain"
        ? devFilter.has(item.finding.deviation)
        : ruleFilter.has(item.finding.rule);
    });
  }, [findings, sevFilter, devFilter, ruleFilter]);

  const toggle = <T,>(set: Set<T>, value: T): Set<T> => {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    return next;
  };

  // "Clean" is a first-class outcome only when nothing is visible AND nothing
  // is merely hidden — an all-ignored project shows the Ignored panel instead.
  const clean = reports !== null && findings.length === 0 && ignored.length === 0;
  const scannedAt = reports
    ? Math.max(reports.chain.scanned_at, reports.instructions.scanned_at)
    : undefined;

  return (
    <div className="app-page">
      <div className="app-page-header app-toolbar">
        <div>
          <h1 className="app-page-title">{t("chain.doctor.title")}</h1>
          <p className="app-page-subtitle">{t("instructions.doctor.subtitle")}</p>
          <ChainScanStatus scannedAt={scannedAt} loading={loading} />
        </div>
        <button className="app-button-secondary" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
          {t("chain.rescan")}
        </button>
      </div>

      {error && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.scanFailed")}: {error}
        </div>
      )}
      {loading && !reports && (
        <div className="p-4 text-[13px] text-muted">{t("chain.scanning")}</div>
      )}

      {reports && findings.length > 0 && (
        <div className="space-y-2.5">
          {/* Severity filter row */}
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="mr-1 text-[12px] font-medium text-muted">
              {t("chain.doctor.filterSeverity")}
            </span>
            {SEVERITIES.map((sev) => {
              const count = sevCounts[sev] ?? 0;
              const active = sevFilter.has(sev);
              return (
                <button
                  key={sev}
                  data-testid={`sev-${sev}`}
                  aria-pressed={active}
                  disabled={count === 0 && !active}
                  onClick={() => setSevFilter((s) => toggle(s, sev))}
                  className={cn(
                    "flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                    active ? SEVERITY_STYLE[sev].badge : "border-border-subtle text-tertiary hover:border-border",
                    count === 0 && "opacity-40",
                  )}
                >
                  <span className={cn("h-1.5 w-1.5 rounded-full", SEVERITY_STYLE[sev].dot)} />
                  {t(`chain.doctor.severity.${sev}`)}
                  <span className="tabular-nums text-muted">{count}</span>
                </button>
              );
            })}
          </div>

          {/* Module-grouped type filter row: chain deviations + instructions rules. */}
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="mr-1 text-[12px] font-medium text-muted">
              {t("chain.doctor.filterType")}
            </span>
            <span className="text-[10.5px] font-medium uppercase tracking-wide text-faint">
              {t("instructions.doctor.chainGroup")}
            </span>
            {DEVIATIONS.map((dev) => {
              const count = devCounts[dev] ?? 0;
              const active = devFilter.has(dev);
              return (
                <button
                  key={dev}
                  data-testid={`dev-${dev}`}
                  aria-pressed={active}
                  disabled={count === 0 && !active}
                  onClick={() => setDevFilter((s) => toggle(s, dev))}
                  className={cn(
                    "flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                    active
                      ? "border-accent-border bg-accent/10 text-accent"
                      : "border-border-subtle text-tertiary hover:border-border",
                    count === 0 && "opacity-40",
                  )}
                >
                  {t(`chain.doctor.deviation.${dev}`)}
                  <span className="tabular-nums text-muted">{count}</span>
                </button>
              );
            })}
            <span className="ml-1 text-[10.5px] font-medium uppercase tracking-wide text-faint">
              {t("instructions.doctor.instructionsGroup")}
            </span>
            {INSTRUCTIONS_RULES.map((rule) => {
              const count = ruleCounts[rule] ?? 0;
              const active = ruleFilter.has(rule);
              const short = ruleShort(rule);
              return (
                <button
                  key={rule}
                  data-testid={`rule-${short}`}
                  aria-pressed={active}
                  disabled={count === 0 && !active}
                  onClick={() => setRuleFilter((current) => toggle(current, rule))}
                  className={cn(
                    "flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                    active
                      ? "border-accent-border bg-accent/10 text-accent"
                      : "border-border-subtle text-tertiary hover:border-border",
                    count === 0 && "opacity-40",
                  )}
                >
                  {t(`instructions.doctor.rule.${short}`)}
                  <span className="tabular-nums text-muted">{count}</span>
                </button>
              );
            })}
          </div>

          <p className="text-[12px] text-faint">
            {t("chain.doctor.showing", { shown: filtered.length, total: findings.length })}
          </p>
        </div>
      )}

      {/* Clean report — a first-class outcome, not an empty error state. */}
      {clean && (
        <div
          data-testid="doctor-clean"
          className="flex items-center gap-2.5 rounded-xl border border-emerald-500/25 bg-emerald-500/[0.06] px-4 py-3 text-[13px] text-emerald-400"
        >
          <ShieldCheck className="h-4.5 w-4.5" />
          {t("instructions.doctor.clean")}
        </div>
      )}

      {/* Filters exclude everything, but findings exist. */}
      {reports && findings.length > 0 && filtered.length === 0 && (
        <div data-testid="doctor-nomatch" className="app-panel-muted p-4 text-[13px] text-muted">
          {t("chain.doctor.noMatch")}
        </div>
      )}

      <div className="space-y-1.5">
        {filtered.map((item) => {
          const key = findingKey(item);
          return (
            <FindingRow
              key={key}
              module={item.module}
              finding={item.finding}
              open={expanded === key}
              onToggle={() => setExpanded((cur) => (cur === key ? null : key))}
              onIgnore={(rule, fingerprint) =>
                ignore(item.module, rule, fingerprint, "ignored")
              }
              onMarkPrivate={(rule, fingerprint) =>
                ignore(item.module, rule, fingerprint, "project_private")
              }
              repair={
                item.module === "chain" && repair?.fingerprint === item.finding.fingerprint
                  ? repair
                  : null
              }
              onRepair={() => startRepair(item.finding.fingerprint)}
              onConfirmRepair={confirmRepair}
              onCancelRepair={cancelRepair}
            />
          );
        })}
      </div>

      {/* Ignored panel: findings hidden by a persisted decision, each restorable.
          Shown whenever any decision is active, independent of the filters. */}
      {ignored.length > 0 && (
        <div data-testid="ignored-section" className="space-y-1.5 pt-3">
          <div className="app-section-title">
            {t("chain.doctor.ignoredHeading", { count: ignored.length })}
          </div>
          {ignored.map((item) => (
            <IgnoredRow
              key={findingKey(item)}
              module={item.module}
              finding={item.finding}
              onRestore={restore}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function FindingRow({
  module,
  finding,
  open,
  onToggle,
  onIgnore,
  onMarkPrivate,
  repair,
  onRepair,
  onConfirmRepair,
  onCancelRepair,
}: {
  module: DoctorModule;
  finding: ChainFinding | InstructionsFinding;
  open: boolean;
  onToggle: () => void;
  onIgnore: (rule: string, fingerprint: string) => void;
  onMarkPrivate: (rule: string, fingerprint: string) => void;
  repair: RepairState | null;
  onRepair: () => void;
  onConfirmRepair: (plan: ChainRepairPlan) => void;
  onCancelRepair: () => void;
}) {
  const { t } = useTranslation();
  const style = SEVERITY_STYLE[finding.severity];
  // The primary object the finding is about (skill entry, or the repo/surface).
  const primary = finding.affected[0];
  const Chevron = open ? ChevronDown : ChevronRight;
  const chainFinding = "deviation" in finding ? finding : null;
  const instructionsEvidence =
    "primary_path" in finding.evidence ? finding.evidence : null;
  // Marking a physical Skill project-private only makes sense where the entry is
  // itself a real directory the classification legitimizes.
  const canMarkPrivate =
    chainFinding?.deviation === "project_private" || chainFinding?.deviation === "copy";
  // Only broken/direct/legacy deviations can be normalized.
  const canRepair = chainFinding ? REPAIRABLE.has(chainFinding.deviation) : false;
  const findingLabel = chainFinding
    ? t(`chain.doctor.deviation.${chainFinding.deviation}`)
    : t(`instructions.doctor.rule.${ruleShort(finding.rule)}`);

  return (
    <div
      data-testid="finding"
      data-module={module}
      data-deviation={chainFinding?.deviation}
      data-rule={finding.rule}
      data-severity={finding.severity}
      className="rounded-lg border border-border-subtle bg-surface"
    >
      <button
        onClick={onToggle}
        className="flex w-full items-center gap-2.5 px-3 py-2 text-left outline-none"
      >
        <Chevron className="h-3.5 w-3.5 shrink-0 text-faint" />
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", style.dot)} />
        <span className="shrink-0 text-[13px] font-medium text-secondary">
          {findingLabel}
        </span>
        {primary && (
          <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-muted">
            {primary.name}
          </span>
        )}
        <span
          className={cn(
            "ml-auto shrink-0 rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            style.badge,
          )}
        >
          {t(`chain.doctor.severity.${finding.severity}`)}
        </span>
      </button>

      {open && (
        <div data-testid="evidence" className="space-y-3 border-t border-border-subtle px-3 py-2.5">
          {chainFinding ? (
            /* Chain evidence: the same hop-by-hop resolution Link Topology shows. */
            <div>
              <div className="app-section-title mb-1">{t("chain.doctor.evidence")}</div>
              <div className="space-y-0.5 font-mono text-[11.5px]">
                <div className="break-all text-secondary">{chainFinding.evidence.entry_path}</div>
                {chainFinding.evidence.hops.map((hop, i) => (
                  <div key={i} className="break-all text-muted">
                    <span className="text-faint">→ </span>
                    {hop}
                  </div>
                ))}
                {chainFinding.evidence.hops.length === 0 &&
                  chainFinding.evidence.final_target !== chainFinding.evidence.entry_path && (
                    <div className="break-all text-muted">
                      <span className="text-faint">→ </span>
                      {chainFinding.evidence.final_target}
                    </div>
                  )}
              </div>
            </div>
          ) : instructionsEvidence ? (
            /* Instructions evidence is already derived by the service; this
               branch only renders its paths, metric bag, and source locations. */
            <div data-testid="instructions-evidence" className="space-y-2">
              <div>
                <div className="app-section-title mb-1">
                  {t("instructions.doctor.evidence")}
                </div>
                <div className="space-y-0.5 font-mono text-[11.5px]">
                  <div className="break-all text-secondary">{instructionsEvidence.primary_path}</div>
                  {instructionsEvidence.counterpart_path && (
                    <div className="break-all text-muted">
                      <span className="text-faint">→ </span>
                      {instructionsEvidence.counterpart_path}
                    </div>
                  )}
                </div>
              </div>
              {Object.keys(instructionsEvidence.metrics).length > 0 && (
                <div>
                  <div className="app-section-title mb-1">
                    {t("instructions.doctor.metrics")}
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {Object.entries(instructionsEvidence.metrics).map(([name, value]) => (
                      <span
                        key={name}
                        className="rounded-full border border-border-subtle bg-surface-hover px-1.5 py-px font-mono text-[10.5px] text-muted"
                      >
                        {name}={typeof value === "string" ? value : JSON.stringify(value)}
                      </span>
                    ))}
                  </div>
                </div>
              )}
              {instructionsEvidence.locations.length > 0 && (
                <div>
                  <div className="app-section-title mb-1">
                    {t("instructions.doctor.locations")}
                  </div>
                  <ul className="space-y-0.5 font-mono text-[11.5px] text-muted">
                    {instructionsEvidence.locations.map((location) => (
                      <li key={`${location.path}:${location.line}`} className="break-all">
                        {location.path}:{location.line}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          ) : null}

          <div className="flex flex-wrap gap-4 text-[11.5px]">
            <div>
              <div className="app-section-title mb-1">{t("chain.doctor.affected")}</div>
              <ul className="space-y-0.5">
                {finding.affected.map((obj) => (
                  <li key={`${obj.kind}:${obj.path}`} className="text-muted">
                    <span className="text-tertiary">{obj.kind}</span>{" "}
                    <span className="font-mono">{obj.name}</span>
                  </li>
                ))}
              </ul>
            </div>
            <div>
              <div className="app-section-title mb-1">{t("chain.doctor.actions")}</div>
              <div className="flex flex-wrap gap-1">
                {finding.actions.map((action) => (
                  <span
                    key={action}
                    className="rounded-full border border-border-subtle bg-surface-hover px-1.5 py-px text-muted"
                  >
                    {module === "chain"
                      ? t(`chain.doctor.action.${action}`, action)
                      : t(`instructions.doctor.action.${action}`, action)}
                  </span>
                ))}
              </div>
            </div>
          </div>

          <div className="font-mono text-[10.5px] text-faint">
            {finding.rule} · {finding.fingerprint.slice(0, 12)}
          </div>

          {/* Decisions: repair/normalize the chain, hide this finding, or
              classify a physical Skill as project-private. Repair is the only
              mutating action; the others persist a decision and never rewrite
              Skill contents. */}
          <div className="flex flex-wrap gap-1.5 border-t border-border-subtle pt-2.5">
            {canRepair && (
              <button
                data-testid="repair"
                onClick={onRepair}
                disabled={repair !== null}
                className="flex items-center gap-1 rounded-full border border-accent-border bg-accent/10 px-2.5 py-0.5 text-[11.5px] font-medium text-accent transition-colors hover:bg-accent/15 disabled:opacity-50"
              >
                <Wrench className="h-3 w-3" />
                {t("chain.doctor.repair")}
              </button>
            )}
            <button
              data-testid="ignore"
              onClick={() => onIgnore(finding.rule, finding.fingerprint)}
              className="rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 text-[11.5px] font-medium text-muted transition-colors hover:border-border hover:text-secondary"
            >
              {t("chain.doctor.ignore")}
            </button>
            {canMarkPrivate && (
              <button
                data-testid="mark-private"
                onClick={() => onMarkPrivate(finding.rule, finding.fingerprint)}
                className="rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 text-[11.5px] font-medium text-muted transition-colors hover:border-border hover:text-secondary"
              >
                {t("chain.doctor.markPrivate")}
              </button>
            )}
          </div>

          {repair && (
            <RepairPanel
              repair={repair}
              onConfirm={onConfirmRepair}
              onCancel={onCancelRepair}
            />
          )}
        </div>
      )}
    </div>
  );
}

/** Inline repair preview for one finding: shows the previewed edits (and any
 * conflict/unsupported), then applies on confirm and reports the outcome. */
function RepairPanel({
  repair,
  onConfirm,
  onCancel,
}: {
  repair: RepairState;
  onConfirm: (plan: ChainRepairPlan) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const { plan, loading, applying, error, outcome } = repair;
  // A previewed item is applicable only when it actually writes; conflicts and
  // skips are shown but do not enable Apply on their own.
  const writable = (item: ChainRepairItem) =>
    item.action === "create" || item.action === "repoint" || item.action === "remove";
  const hasWork = plan?.items.some(writable) ?? false;

  return (
    <div
      data-testid="repair-panel"
      className="space-y-2 rounded-lg border border-accent-border bg-accent/[0.04] p-2.5 text-[11.5px]"
    >
      <div className="app-section-title">{t("chain.doctor.repairPreviewTitle")}</div>

      {loading && <div className="text-muted">{t("chain.doctor.repairPlanning")}</div>}
      {error && <div className="text-red-400">{t("chain.doctor.repairFailed")}: {error}</div>}

      {plan && (
        <>
          {plan.unsupported.length > 0 && (
            <div data-testid="repair-unsupported" className="text-amber-400">
              {t("chain.doctor.repairUnsupported")}
            </div>
          )}
          {plan.items.length === 0 && plan.unsupported.length === 0 && (
            <div className="text-muted">{t("chain.doctor.repairNoItems")}</div>
          )}
          {plan.items.length > 0 && (
            <ul data-testid="repair-items" className="space-y-1 font-mono">
              {plan.items.map((item, i) => (
                <li key={i} className="flex flex-wrap items-center gap-1.5">
                  <span className="text-tertiary">
                    {t(`chain.doctor.repairKind.${item.kind}`, item.kind)}
                  </span>
                  <span
                    className={cn(
                      "rounded-full border px-1.5 py-px text-[10.5px]",
                      item.action === "conflict" || item.action === "error"
                        ? "border-red-500/25 bg-red-500/10 text-red-400"
                        : item.action === "skip" || item.action === "exists"
                          ? "border-border-subtle bg-surface-hover text-muted"
                          : "border-accent-border bg-accent/10 text-accent",
                    )}
                  >
                    {t(`chain.doctor.repairAction.${item.action}`, item.action)}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-muted">{item.path}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {/* Outcome after apply: the verified badge is the only success signal. */}
      {outcome && (
        <div
          data-testid="repair-outcome"
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 py-1",
            outcome.verified
              ? "bg-emerald-500/[0.08] text-emerald-400"
              : "bg-amber-500/[0.08] text-amber-400",
          )}
        >
          <ShieldCheck className="h-3.5 w-3.5" />
          {outcome.verified
            ? t("chain.doctor.repairVerified")
            : t("chain.doctor.repairUnverified")}
        </div>
      )}

      <div className="flex gap-1.5 pt-0.5">
        {plan && !outcome && (
          <button
            data-testid="repair-confirm"
            onClick={() => onConfirm(plan)}
            disabled={applying || !hasWork}
            className="rounded-full border border-accent-border bg-accent/10 px-2.5 py-0.5 font-medium text-accent transition-colors hover:bg-accent/15 disabled:opacity-50"
          >
            {applying ? t("chain.doctor.repairApplying") : t("chain.doctor.repairConfirm")}
          </button>
        )}
        <button
          data-testid="repair-cancel"
          onClick={onCancel}
          disabled={applying}
          className="rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 font-medium text-muted transition-colors hover:border-border hover:text-secondary disabled:opacity-50"
        >
          {outcome ? t("chain.doctor.repairClose") : t("chain.doctor.repairCancel")}
        </button>
      </div>
    </div>
  );
}

/** A hidden finding in the Ignored panel: enough to recognize it, plus a
 * Restore control that removes its persisted decision. */
function IgnoredRow({
  module,
  finding,
  onRestore,
}: {
  module: DoctorModule;
  finding: ChainFinding | InstructionsFinding;
  onRestore: (module: DoctorModule, rule: string, fingerprint: string) => void;
}) {
  const { t } = useTranslation();
  // The primary object the finding is about (skill entry, or the repo/surface).
  const primary = finding.affected[0];
  const chainFinding = "deviation" in finding ? finding : null;
  const findingLabel = chainFinding
    ? t(`chain.doctor.deviation.${chainFinding.deviation}`)
    : t(`instructions.doctor.rule.${ruleShort(finding.rule)}`);

  return (
    <div
      data-testid="ignored-finding"
      data-module={module}
      data-deviation={chainFinding?.deviation}
      data-rule={finding.rule}
      className="flex items-center gap-2.5 rounded-lg border border-border-subtle bg-surface px-3 py-2 opacity-80"
    >
      <span className="shrink-0 text-[13px] font-medium text-secondary">
        {findingLabel}
      </span>
      {primary && (
        <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-muted">
          {primary.name}
        </span>
      )}
      <button
        data-testid="restore"
        onClick={() => onRestore(module, finding.rule, finding.fingerprint)}
        className="ml-auto flex shrink-0 items-center gap-1 rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 text-[11.5px] font-medium text-muted transition-colors hover:border-border hover:text-secondary"
      >
        <RotateCcw className="h-3 w-3" />
        {t("chain.doctor.restore")}
      </button>
    </div>
  );
}
