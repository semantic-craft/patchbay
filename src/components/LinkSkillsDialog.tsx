import { useState } from "react";
import { X, Link2, ArrowLeft, ShieldCheck, ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import { chainPlanLink, chainApplyLink } from "../lib/tauri";
import type { ChainApplyOutcome, ChainLinkPlan, ChainPreset, ChainRepo } from "../lib/tauri";
import { TONE_BADGE } from "../lib/chainUi";
import { CHAIN_AGENTS, SkillPicker } from "./SkillPicker";

interface Props {
  open: boolean;
  projectName: string;
  projectPath: string;
  repos: ChainRepo[];
  /** Preset 起步 pills（#36：与接入向导共用同一挑选流程）。 */
  presets?: ChainPreset[];
  onClose: () => void;
  onLinked: () => void;
}

const ACTION_TONE: Record<string, keyof typeof TONE_BADGE> = {
  created: "ok",
  exists: "dim",
  removed: "ok",
  absent: "dim",
  skipped: "warn",
  conflict: "warn",
  error: "err",
};

/** A single previewed target or applied result — both carry name/path/action/message. */
interface ItemLike {
  name: string;
  path: string;
  action: string;
  message: string | null;
}

function ItemRow({ item }: { item: ItemLike }) {
  return (
    <div className="flex items-baseline gap-2 font-mono text-[11.5px]">
      <span
        className={cn(
          "shrink-0 rounded-full border px-1.5 py-px font-sans text-[10.5px] font-medium",
          TONE_BADGE[ACTION_TONE[item.action] ?? "dim"]
        )}
      >
        {item.action}
      </span>
      <span className="shrink-0 text-secondary">{item.name}</span>
      <span className="break-all text-faint">{item.path}</span>
      {item.message && <span className="break-all text-muted">· {item.message}</span>}
    </div>
  );
}

export function LinkSkillsDialog({ open, projectName, projectPath, repos, presets, onClose, onLinked }: Props) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [agents, setAgents] = useState<Set<string>>(new Set(["claude", "codex"]));
  const [loading, setLoading] = useState(false);
  const [plan, setPlan] = useState<ChainLinkPlan | null>(null);
  const [outcome, setOutcome] = useState<ChainApplyOutcome | null>(null);

  if (!open) return null;

  const reset = () => {
    setSelected(new Set());
    setPlan(null);
    setOutcome(null);
  };

  const handleClose = () => {
    reset();
    onClose();
  };

  const toggleAgent = (agent: string) => {
    const next = new Set(agents);
    if (next.has(agent)) next.delete(agent);
    else next.add(agent);
    setAgents(next);
  };

  // Step 1 -> 2: build a read-only preview of every target, action, and conflict.
  const preview = async () => {
    if (selected.size === 0 || agents.size === 0) return;
    setLoading(true);
    try {
      setPlan(await chainPlanLink(projectPath, [...selected], [...agents]));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  // Step 2 -> 3: apply the exact plan; the backend refuses anything that changed
  // and only reports success after a rescan observes the chain.
  const apply = async () => {
    if (!plan) return;
    setLoading(true);
    try {
      const result = await chainApplyLink(plan);
      setOutcome(result);
      if (result.verified) {
        toast.success(t("chain.applyVerified", { count: result.observed.length }));
      } else {
        toast.warning(t("chain.applyUnverified"));
      }
      onLinked();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={handleClose} />
      <div className="relative flex max-h-[80vh] w-full max-w-2xl flex-col rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-primary">
            <Link2 className="h-4 w-4 text-accent" />
            {t("chain.linkDialogTitle", { project: projectName })}
          </h2>
          <button
            onClick={handleClose}
            className="rounded p-1 text-muted transition-colors outline-none hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Step 1: select skills + agent entries */}
        {!plan && !outcome && (
          <>
            <div className="mb-3 flex flex-wrap items-center gap-2">
              <span className="text-[12px] text-muted">{t("chain.agentsLabel")}</span>
              {CHAIN_AGENTS.map((agent) => (
                <button
                  key={agent}
                  onClick={() => toggleAgent(agent)}
                  className={cn(
                    "rounded-full border px-2.5 py-1 text-[12px] font-medium transition-colors outline-none",
                    agents.has(agent)
                      ? "border-accent-border bg-surface-active text-secondary"
                      : "border-border-subtle text-muted hover:text-tertiary"
                  )}
                >
                  {agent}
                </button>
              ))}
              <span className="ml-auto text-[12px] text-muted">
                {t("chain.selectedCount", { count: selected.size })}
              </span>
            </div>

            <SkillPicker
              repos={repos}
              selected={selected}
              onChange={setSelected}
              presets={presets}
            />

            <div className="mt-4 flex justify-end gap-2">
              <button onClick={handleClose} className="app-button-secondary">
                {t("common.cancel")}
              </button>
              <button
                onClick={() => void preview()}
                disabled={loading || selected.size === 0 || agents.size === 0}
                className="app-button-primary"
              >
                {loading ? t("chain.planning") : t("chain.previewPlan")}
              </button>
            </div>
          </>
        )}

        {/* Step 2: preview the plan before writing anything */}
        {plan && !outcome && (
          <>
            <div className="mb-2">
              <div className="app-section-title">{t("chain.planTitle")}</div>
              <p className="mt-0.5 text-[12px] text-muted">{t("chain.planHint")}</p>
            </div>
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto rounded-lg border border-border-subtle p-3">
              <div>
                <div className="app-section-title mb-1.5">{t("chain.resultSkills")}</div>
                <div className="space-y-1">
                  {plan.skills.map((item) => (
                    <ItemRow key={item.path} item={item} />
                  ))}
                  {plan.skills.length === 0 && (
                    <div className="text-[12px] text-muted">—</div>
                  )}
                </div>
              </div>
              <div>
                <div className="app-section-title mb-1.5">{t("chain.resultEntries")}</div>
                <div className="space-y-1">
                  {plan.entries.map((item) => (
                    <ItemRow key={item.path} item={item} />
                  ))}
                  {plan.entries.length === 0 && (
                    <div className="text-[12px] text-muted">—</div>
                  )}
                </div>
              </div>
            </div>
            <div className="mt-4 flex justify-between gap-2">
              <button onClick={() => setPlan(null)} className="app-button-secondary">
                <ArrowLeft className="h-4 w-4" />
                {t("chain.back")}
              </button>
              <button
                onClick={() => void apply()}
                disabled={loading}
                className="app-button-primary"
              >
                {loading ? t("chain.applying") : t("chain.apply")}
              </button>
            </div>
          </>
        )}

        {/* Step 3: applied result, with the rescan verdict */}
        {outcome && (
          <>
            <div
              className={cn(
                "mb-3 flex items-center gap-2 rounded-lg border px-3 py-2 text-[12px]",
                outcome.verified
                  ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-400"
                  : "border-amber-500/25 bg-amber-500/10 text-amber-400"
              )}
            >
              {outcome.verified ? (
                <ShieldCheck className="h-4 w-4" />
              ) : (
                <ShieldAlert className="h-4 w-4" />
              )}
              <span>
                {outcome.verified
                  ? t("chain.applyVerified", { count: outcome.observed.length })
                  : t("chain.applyUnverified")}
              </span>
              {outcome.missing.length > 0 && (
                <span className="ml-auto font-mono text-[11px]">
                  {t("chain.missing", { names: outcome.missing.join(", ") })}
                </span>
              )}
            </div>
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto">
              <div>
                <div className="app-section-title mb-1.5">{t("chain.resultSkills")}</div>
                <div className="space-y-1">
                  {outcome.report.skills.map((item) => (
                    <ItemRow key={item.path} item={item} />
                  ))}
                </div>
              </div>
              <div>
                <div className="app-section-title mb-1.5">{t("chain.resultEntries")}</div>
                <div className="space-y-1">
                  {outcome.report.entries.map((item) => (
                    <ItemRow key={item.path} item={item} />
                  ))}
                </div>
              </div>
            </div>
            <div className="mt-4 flex justify-end">
              <button onClick={handleClose} className="app-button-secondary">
                {t("chain.close")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
