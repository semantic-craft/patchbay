import { useState } from "react";
import { X, ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import { chainPlanRemediate, chainApplyRemediate } from "../lib/tauri";
import type { ChainGuardViolation, ChainProject, ChainRemediationPlan } from "../lib/tauri";

const AGENTS = ["claude", "codex", "copilot", "opencode"] as const;

interface Props {
  open: boolean;
  /** The global-surface violation being remediated. */
  violation: ChainGuardViolation | null;
  /** Display name of the global Agent surface the violation sits on. */
  agent: string;
  /** Registered projects the Skill can be linked into. */
  projects: ChainProject[];
  onClose: () => void;
  onDone: () => void;
}

/**
 * Remediate one Global Guard violation: pick a registered project and target
 * Agents, preview the plan (the project link plus whether the global entry can
 * be retired, or manual guidance for a physical entry), then apply. The backend
 * only retires the global symlink after the project chain verifies.
 */
export function RemediateDialog({ open, violation, agent, projects, onClose, onDone }: Props) {
  const { t } = useTranslation();
  const [projectPath, setProjectPath] = useState<string>("");
  const [agents, setAgents] = useState<Set<string>>(new Set(["claude", "codex"]));
  const [plan, setPlan] = useState<ChainRemediationPlan | null>(null);
  const [loading, setLoading] = useState(false);

  if (!open || !violation) return null;

  const reset = () => {
    setPlan(null);
    setProjectPath("");
    setAgents(new Set(["claude", "codex"]));
  };
  const close = () => {
    reset();
    onClose();
  };

  const toggleAgent = (agent: string) => {
    const next = new Set(agents);
    if (next.has(agent)) next.delete(agent);
    else next.add(agent);
    setAgents(next);
  };

  const preview = async () => {
    if (!projectPath || agents.size === 0) return;
    setLoading(true);
    try {
      setPlan(await chainPlanRemediate(violation.path, projectPath, [...agents]));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  const apply = async () => {
    if (!plan) return;
    setLoading(true);
    try {
      const outcome = await chainApplyRemediate(plan);
      if (outcome.verified) {
        toast.success(`${violation.skill} ✓`);
      } else if (outcome.guidance) {
        toast.warning(outcome.guidance);
      } else {
        toast.warning(t("chain.remediate.unverified", { name: violation.skill }));
      }
      onDone();
      close();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={close} />
      <div className="relative w-full max-w-md rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-primary">
            <ShieldAlert className="h-4 w-4 text-red-400" />
            {t("chain.remediate.title", { name: violation.skill })}
          </h2>
          <button
            onClick={close}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mb-4 font-mono text-[11.5px] text-muted">
          {agent} · {violation.is_link ? t("chain.remediate.symlink") : t("chain.remediate.physical")}
        </p>

        {!plan ? (
          <>
            <label className="mb-1 block text-[12px] text-muted">{t("chain.remediate.project")}</label>
            <select
              value={projectPath}
              onChange={(e) => setProjectPath(e.target.value)}
              className="mb-4 w-full rounded-[4px] border border-border-subtle bg-bg-secondary px-2.5 py-1.5 text-[13px] text-secondary outline-none"
            >
              <option value="">{t("chain.remediate.projectPlaceholder")}</option>
              {projects.map((p) => (
                <option key={p.path} value={p.path}>
                  {p.name}
                </option>
              ))}
            </select>

            <div className="mb-5 flex items-center gap-2">
              <span className="text-[12px] text-muted">{t("chain.agentsLabel")}</span>
              {AGENTS.map((agent) => (
                <button
                  key={agent}
                  onClick={() => toggleAgent(agent)}
                  className={cn(
                    "rounded-full border px-2.5 py-0.5 text-[11.5px] font-medium transition-colors outline-none",
                    agents.has(agent)
                      ? "border-blue-500/40 bg-blue-500/10 text-blue-300"
                      : "border-border-subtle text-muted"
                  )}
                >
                  {agent}
                </button>
              ))}
            </div>

            <div className="flex justify-end gap-2">
              <button
                onClick={close}
                className="rounded-[4px] px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none hover:bg-surface-hover hover:text-secondary"
              >
                {t("common.cancel")}
              </button>
              <button
                onClick={preview}
                disabled={loading || !projectPath || agents.size === 0}
                className="rounded-[4px] bg-blue-600 px-3 py-1.5 text-[13px] font-medium text-white outline-none disabled:opacity-40"
              >
                {t("chain.remediate.preview")}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="mb-4 rounded-lg border border-border-subtle bg-bg-secondary px-3 py-2.5 text-[12.5px] text-secondary">
              {plan.guidance ? (
                <p className="text-amber-400">{plan.guidance}</p>
              ) : (
                <>
                  <p>{t("chain.remediate.willLink", { project: shortName(plan.project), agents: plan.agents.join(", ") })}</p>
                  <p className="mt-1 text-tertiary">
                    {plan.remove_global
                      ? t("chain.remediate.willRemove")
                      : t("chain.remediate.willKeep")}
                  </p>
                </>
              )}
            </div>
            <div className="flex justify-end gap-2">
              <button
                onClick={() => setPlan(null)}
                className="rounded-[4px] px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none hover:bg-surface-hover hover:text-secondary"
              >
                {t("chain.remediate.back")}
              </button>
              <button
                onClick={apply}
                disabled={loading || (!plan.remove_global && !plan.link_plan)}
                className="rounded-[4px] bg-blue-600 px-3 py-1.5 text-[13px] font-medium text-white outline-none disabled:opacity-40"
              >
                {t("chain.remediate.apply")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function shortName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
