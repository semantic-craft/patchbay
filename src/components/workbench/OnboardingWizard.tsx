import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Check } from "lucide-react";
import { cn } from "../../utils";
import { chainApplyLink, chainPlanLink } from "../../lib/tauri";
import type { ChainPreset, ChainRepo } from "../../lib/tauri";
import { CHAIN_AGENTS, SkillPicker } from "../SkillPicker";

interface OnboardingWizardProps {
  projectPath: string;
  repos: ChainRepo[];
  presets: ChainPreset[];
  /** 一次性 apply 成功后调用——工作台重载后向导随 0 链接态消失。 */
  onDone: () => void;
}

const STEP_KEYS = [
  "chain.workbench.wizardStepSource",
  "chain.workbench.wizardStepSkills",
  "chain.workbench.wizardStepEntry",
] as const;

/**
 * 新项目三步接入向导（#36，原型 S6）：选来源 → 挑技能（Preset 起步）→
 * 建入口（勾选 agent 面）。终点汇总「将创建 N 条软链 + 入口」，确认后一次
 * `chain_plan_link` + `chain_apply_link` 批量落地——入口目录链由既有引擎在
 * agent 面缺失时自动规划，向导不带私有写路径。
 */
export function OnboardingWizard({ projectPath, repos, presets, onDone }: OnboardingWizardProps) {
  const { t } = useTranslation();
  const [step, setStep] = useState(0);
  // 来源默认全选：常见形态是仓库不多、全都要看。
  const [sources, setSources] = useState<Set<string>>(
    () => new Set(repos.map((repo) => repo.path)),
  );
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [agents, setAgents] = useState<Set<string>>(new Set(["claude", "codex"]));
  const [busy, setBusy] = useState(false);

  const scopedRepos = repos.filter((repo) => sources.has(repo.path));

  const toggleSource = (path: string) => {
    const next = new Set(sources);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setSources(next);
    // 收窄来源后，已勾选但不再可见的技能同步剔除——汇总数不撒谎。
    const visible = new Set(
      repos
        .filter((repo) => next.has(repo.path))
        .flatMap((repo) => repo.skills.map((skill) => skill.path)),
    );
    setSelected((cur) => new Set([...cur].filter((path) => visible.has(path))));
  };

  const toggleAgent = (agent: string) => {
    const next = new Set(agents);
    if (next.has(agent)) next.delete(agent);
    else next.add(agent);
    setAgents(next);
  };

  // 汇总确认 = 一次 batch plan + 一次 apply（US18）；成功与否都以引擎的
  // rescan 验证为准，结束后落回正常工作台。
  const apply = async () => {
    setBusy(true);
    try {
      const plan = await chainPlanLink(projectPath, [...selected], [...agents]);
      const outcome = await chainApplyLink(plan);
      if (outcome.verified) {
        toast.success(t("chain.applyVerified", { count: outcome.observed.length }));
      } else {
        toast.warning(t("chain.applyUnverified"));
      }
      onDone();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const nextDisabled =
    (step === 0 && sources.size === 0) || (step === 1 && selected.size === 0);

  return (
    <div data-testid="onboarding-wizard" className="app-glass-card overflow-hidden">
      {/* 三步进度条 */}
      <div className="flex items-center border-b border-glass-hairline px-4 py-3">
        {STEP_KEYS.map((key, index) => (
          <div key={key} className="flex items-center">
            {index > 0 && <div className="mx-3 h-px w-8 bg-glass-hairline" />}
            <span
              className={cn(
                "flex items-center gap-2 text-[12px] font-medium",
                index === step ? "text-primary" : "text-muted",
              )}
            >
              <span
                className={cn(
                  "flex h-[22px] w-[22px] items-center justify-center rounded-full border text-[11px] font-bold",
                  index <= step
                    ? "border-transparent bg-accent text-white"
                    : "border-glass-hairline bg-glass-soft text-muted",
                )}
              >
                {index < step ? <Check className="h-3 w-3" /> : index + 1}
              </span>
              {t(key)}
            </span>
          </div>
        ))}
      </div>

      {/* 步骤体 */}
      <div className="flex max-h-[46vh] flex-col p-4">
        {step === 0 && (
          <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-border-subtle">
            {repos.map((repo) => (
              <label
                key={repo.path}
                data-testid="wizard-source"
                className="flex cursor-pointer items-center gap-2.5 border-b border-border-subtle px-3 py-2 last:border-b-0 hover:bg-surface-hover"
              >
                <input
                  type="checkbox"
                  checked={sources.has(repo.path)}
                  onChange={() => toggleSource(repo.path)}
                  className="accent-current"
                />
                <span className="font-mono text-[12px] text-secondary">{repo.name}</span>
                <span className="text-[11px] text-faint">
                  {t("chain.workbench.wizardSourceSkills", { count: repo.skills.length })}
                </span>
                <span className="ml-auto break-all font-mono text-[10.5px] text-faint">
                  {repo.root}
                </span>
              </label>
            ))}
            {repos.length === 0 && (
              <div className="px-3 py-6 text-center text-[12.5px] text-muted">
                {t("chain.workbench.wizardNoSources")}
              </div>
            )}
          </div>
        )}

        {step === 1 && (
          <SkillPicker
            repos={scopedRepos}
            selected={selected}
            onChange={setSelected}
            presets={presets}
          />
        )}

        {step === 2 && (
          <div className="space-y-4">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-[12px] text-muted">{t("chain.agentsLabel")}</span>
              {CHAIN_AGENTS.map((agent) => (
                <button
                  key={agent}
                  onClick={() => toggleAgent(agent)}
                  className={cn(
                    "rounded-full border px-2.5 py-1 text-[12px] font-medium transition-colors outline-none",
                    agents.has(agent)
                      ? "border-accent-border bg-surface-active text-secondary"
                      : "border-border-subtle text-muted hover:text-tertiary",
                  )}
                >
                  {agent}
                </button>
              ))}
            </div>
            <div className="rounded-lg border border-border-subtle px-3 py-2.5">
              <div className="font-mono text-[12px] text-secondary">
                .claude/skills → ../.agents/skills
              </div>
              <p className="mt-1 text-[11.5px] text-muted">
                {t("chain.workbench.wizardEntryHint")}
              </p>
            </div>
          </div>
        )}
      </div>

      {/* 页脚：汇总 + 前后导航 */}
      <div className="flex items-center gap-3 border-t border-glass-hairline px-4 py-3">
        <span data-testid="wizard-summary" className="text-[12px] text-muted">
          {step === 0 && t("chain.workbench.wizardSourceSummary", { count: sources.size })}
          {step === 1 && t("chain.workbench.wizardSkillsSummary", { count: selected.size })}
          {step === 2 &&
            t("chain.workbench.wizardEntrySummary", {
              count: selected.size,
              agents: agents.size,
            })}
        </span>
        <div className="ml-auto flex gap-2">
          {step > 0 && (
            <button
              data-testid="wizard-back"
              onClick={() => setStep(step - 1)}
              className="app-button-secondary"
            >
              {t("chain.workbench.wizardBack")}
            </button>
          )}
          {step < 2 ? (
            <button
              data-testid="wizard-next"
              onClick={() => setStep(step + 1)}
              disabled={nextDisabled}
              className="app-button-primary"
            >
              {t("chain.workbench.wizardNext")}
            </button>
          ) : (
            <button
              data-testid="wizard-apply"
              onClick={() => void apply()}
              disabled={busy || agents.size === 0 || selected.size === 0}
              className="app-button-primary"
            >
              {busy
                ? t("chain.workbench.wizardApplying")
                : t("chain.workbench.wizardApply")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
