import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ShieldCheck, Stethoscope, Trash2, Wrench } from "lucide-react";
import { cn } from "../../utils";
import { CHAIN_RELINK_THRESHOLD, chainApplyRepair, chainPlanRepair } from "../../lib/tauri";
import type {
  ChainFinding,
  ChainRepairCandidate,
  ChainRepairItem,
  ChainRepairOutcome,
  ChainRepairPlan,
  ChainSeverity,
} from "../../lib/tauri";
import { LivePanel, useLiveRepair } from "./liveRepair";

/** Deviations the deterministic repair engine can act on (issue #10 + #30). */
const REPAIRABLE = new Set(["broken", "direct", "legacy"]);

const STRIPE: Record<ChainSeverity, string> = {
  violation: "bg-red-400",
  warning: "bg-amber-400",
  advice: "bg-blue-400",
  notice: "bg-gray-400",
};

const BADGE: Record<ChainSeverity, string> = {
  violation: "border-red-500/25 bg-red-500/10 text-red-400",
  warning: "border-amber-500/25 bg-amber-500/10 text-amber-400",
  advice: "border-blue-500/25 bg-blue-500/10 text-blue-400",
  notice: "border-border-subtle bg-surface-hover text-muted",
};

interface RepairState {
  plan: ChainRepairPlan | null;
  loading: boolean;
  applying: boolean;
  error: string | null;
  outcome: ChainRepairOutcome | null;
}


interface EvidenceCardProps {
  finding: ChainFinding;
  /** Located candidates for a broken finding, best first; empty otherwise. */
  candidates: ChainRepairCandidate[];
  onViewDiagnosis: () => void;
  /** Manual path (existing unlink flow); broken findings only. */
  onManual: (() => void) | null;
  /** Called after a VERIFIED apply so the workbench rescans back to green. */
  onRepaired: () => void;
}

/**
 * 故障证据卡（#30）：断链等 finding 浮现在工作台状态槽，证据（失效目标、
 * 候选新路径 + 匹配度、git 线索）直接印在卡上。主按钮走既有的确定性
 * plan → 预览 → apply → verified 修复闭环；产品文案里的「Agent」= 自动修复器。
 */
export function EvidenceCard({
  finding,
  candidates,
  onViewDiagnosis,
  onManual,
  onRepaired,
}: EvidenceCardProps) {
  const { t } = useTranslation();
  const [repair, setRepair] = useState<RepairState | null>(null);

  const skillName =
    finding.affected.find((obj) => obj.kind === "skill")?.name ??
    finding.affected[0]?.name ??
    "";
  const broken = finding.deviation === "broken";
  const best = candidates[0] ?? null;
  // Below the threshold the planner falls back to REMOVING the dangling link,
  // so the primary button must say so instead of promising a rebuild.
  const relinkable = broken && best !== null && best.score >= CHAIN_RELINK_THRESHOLD;
  const repairable = REPAIRABLE.has(finding.deviation);

  const startRepair = async () => {
    setRepair({ plan: null, loading: true, applying: false, error: null, outcome: null });
    try {
      const plan = await chainPlanRepair([finding.fingerprint]);
      setRepair((cur) => (cur ? { ...cur, plan, loading: false } : cur));
    } catch (e) {
      setRepair((cur) => (cur ? { ...cur, loading: false, error: String(e) } : cur));
    }
  };

  const confirmRepair = async (plan: ChainRepairPlan) => {
    setRepair((cur) => (cur ? { ...cur, applying: true, error: null } : cur));
    try {
      const outcome = await chainApplyRepair(plan);
      setRepair((cur) => (cur ? { ...cur, applying: false, outcome } : cur));
      if (outcome.verified) {
        toast.success(t("chain.doctor.repairVerified"));
        onRepaired();
      }
    } catch (e) {
      setRepair((cur) => (cur ? { ...cur, applying: false, error: String(e) } : cur));
    }
  };

  // 直播修复（#32/#33 共享 hook）：主按钮直接开跑，#31 撤销兜底。
  const liveRepair = useLiveRepair([finding.fingerprint], onRepaired);
  const live = liveRepair.live;

  return (
    <div
      data-testid="evidence-card"
      data-deviation={finding.deviation}
      data-severity={finding.severity}
      className="app-glass-card flex gap-3 px-4 py-3.5"
    >
      <span className={cn("w-1 shrink-0 self-stretch rounded-full", STRIPE[finding.severity])} />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13.5px] font-semibold text-primary">
            {broken
              ? t("chain.workbench.brokenTitle", { name: skillName })
              : `${t(`chain.doctor.deviation.${finding.deviation}`)}: ${skillName}`}
          </span>
          <span
            className={cn(
              "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
              BADGE[finding.severity],
            )}
          >
            {broken
              ? t("chain.workbench.targetMissing")
              : t(`chain.doctor.severity.${finding.severity}`)}
          </span>
        </div>

        {broken ? (
          <p className="text-[12.5px] leading-relaxed text-tertiary">
            {t("chain.workbench.brokenDesc", {
              entry: finding.evidence.entry_path,
              target: finding.evidence.final_target,
            })}
          </p>
        ) : (
          <div className="font-mono text-[11.5px] text-muted">
            <span className="break-all text-secondary">{finding.evidence.entry_path}</span>
            {finding.evidence.final_target !== finding.evidence.entry_path && (
              <span className="break-all"> → {finding.evidence.final_target}</span>
            )}
          </div>
        )}

        {broken && best && (
          <div
            data-testid="card-candidate"
            className="flex flex-wrap items-center gap-2 rounded-[10px] border border-accent-border bg-accent/10 px-3 py-1.5 text-[12px] font-medium text-primary"
          >
            {t("chain.workbench.candidateFound", { name: best.name })}
            <span className="ml-auto text-[10.5px] font-semibold text-accent">
              {t("chain.workbench.candidateMatch", { score: best.score })}
              {best.reason === "git_rename" &&
                best.renamed_at !== null &&
                ` · ${t("chain.workbench.candidateRenamed", {
                  date: new Date(best.renamed_at * 1000).toLocaleDateString(),
                })}`}
            </span>
          </div>
        )}

        <div className="mt-1 flex flex-wrap gap-2">
          {repairable &&
            (broken && !relinkable ? (
              <button
                data-testid="card-remove"
                onClick={() => void startRepair()}
                disabled={repair !== null}
                title={t("chain.workbench.removeBrokenHint")}
                className="flex items-center gap-1.5 rounded-full border border-red-500/25 bg-red-500/10 px-3 py-1 text-[12px] font-medium text-red-400 transition-colors hover:bg-red-500/15 disabled:opacity-50"
              >
                <Trash2 className="h-3 w-3" />
                {t("chain.workbench.removeBroken")}
              </button>
            ) : (
              <button
                data-testid="card-repair"
                onClick={() => void liveRepair.run()}
                disabled={repair !== null || live !== null}
                className="app-button-primary h-7 px-3 text-[12px]"
              >
                <Wrench className="h-3 w-3" />
                {t("chain.workbench.repairAgent")}
              </button>
            ))}
          <button
            data-testid="card-diagnose"
            onClick={onViewDiagnosis}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <Stethoscope className="h-3 w-3" />
            {t("chain.workbench.cardDiagnose")}
          </button>
          {onManual && (
            <button
              data-testid="card-manual"
              onClick={onManual}
              disabled={repair !== null || live !== null}
              className="app-button-secondary h-7 px-3 text-[12px]"
            >
              {t("chain.workbench.cardManual")}
            </button>
          )}
        </div>

        {live && (
          <LivePanel
            live={live}
            onTogglePause={() => void liveRepair.togglePause()}
            onTakeover={() => void liveRepair.takeover()}
            onRetry={() => void liveRepair.run()}
            onClose={liveRepair.close}
          />
        )}

        {repair && (
          <RepairPanel
            repair={repair}
            onConfirm={(plan) => void confirmRepair(plan)}
            onCancel={() => setRepair(null)}
          />
        )}
      </div>
    </div>
  );
}

/** Inline plan preview → confirm → outcome, mirroring the Doctor page's flow:
 * what the user confirms is the plan's actual edits (paths and actions), so
 * the card's promise and the write can never diverge. */
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
  const writable = (item: ChainRepairItem) =>
    item.action === "create" || item.action === "repoint" || item.action === "remove";
  const hasWork = plan?.items.some(writable) ?? false;

  return (
    <div
      data-testid="card-repair-panel"
      className="mt-1 space-y-2 rounded-[10px] border border-accent-border bg-accent/[0.04] p-2.5 text-[11.5px]"
    >
      <div className="app-section-title">{t("chain.doctor.repairPreviewTitle")}</div>

      {loading && <div className="text-muted">{t("chain.doctor.repairPlanning")}</div>}
      {error && (
        <div className="text-red-400">
          {t("chain.doctor.repairFailed")}: {error}
        </div>
      )}

      {plan && (
        <>
          {plan.unsupported.length > 0 && (
            <div className="text-amber-400">{t("chain.doctor.repairUnsupported")}</div>
          )}
          {plan.items.length === 0 && plan.unsupported.length === 0 && (
            <div className="text-muted">{t("chain.doctor.repairNoItems")}</div>
          )}
          {plan.items.length > 0 && (
            <ul data-testid="card-repair-items" className="space-y-1 font-mono">
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
                  <span className="min-w-0 flex-1 truncate text-muted">
                    {item.new_target ? `${item.path} → ${item.new_target}` : item.path}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {outcome && (
        <div
          data-testid="card-repair-outcome"
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
            data-testid="card-repair-confirm"
            onClick={() => plan && onConfirm(plan)}
            disabled={applying || !hasWork}
            className="rounded-full border border-accent-border bg-accent/10 px-2.5 py-0.5 font-medium text-accent transition-colors hover:bg-accent/15 disabled:opacity-50"
          >
            {applying ? t("chain.doctor.repairApplying") : t("chain.doctor.repairConfirm")}
          </button>
        )}
        <button
          data-testid="card-repair-cancel"
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
