import { useMemo, useState } from "react";
import { Search } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import type { ChainPreset, ChainRepo } from "../lib/tauri";

/** The Agent surfaces a chain link can enter through — the shared list the
 * link dialog and the onboarding wizard both offer. */
export const CHAIN_AGENTS = ["claude", "codex", "copilot", "opencode", "qoderwork"] as const;

interface SkillPickerProps {
  repos: ChainRepo[];
  /** Selected Original paths — the exact values `chain_plan_link` consumes. */
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  /** Preset 起步 pills（#35 套装）；缺省或空列表时不渲染该行。 */
  presets?: ChainPreset[];
}

/**
 * 套装挑选组件（#26 决策：向导与「＋ 链接技能」共用）：按 Preset 起步 pills +
 * 搜索 + 按仓库分组的技能勾选列表。只管「选了哪些原件路径」——agent 面与
 * plan/apply 归调用方。
 */
export function SkillPicker({ repos, selected, onChange, presets = [] }: SkillPickerProps) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");

  const groups = useMemo(() => {
    const q = search.trim().toLowerCase();
    return repos
      .map((repo) => ({
        repo: repo.name,
        root: repo.root,
        skills: repo.skills.filter((s) => !q || s.name.toLowerCase().includes(q)),
      }))
      .filter((g) => g.skills.length > 0);
  }, [repos, search]);

  // Only worth labeling a Skill's source root when more than one root feeds the picker.
  const multiRoot = useMemo(() => new Set(repos.map((r) => r.root)).size > 1, [repos]);

  // 当前来源里真实存在的原件路径——Preset 引用按它裁剪。
  const available = useMemo(
    () => new Set(repos.flatMap((repo) => repo.skills.map((skill) => skill.path))),
    [repos],
  );

  const toggleSkill = (path: string) => {
    const next = new Set(selected);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    onChange(next);
  };

  // Preset 起步 = 用套装的可用引用整体替换当前勾选；不在当前来源里的引用
  // 明确报数，而不是静默丢弃。
  const applyPreset = (preset: ChainPreset) => {
    const found = preset.skills.filter((skill) => available.has(skill.path));
    onChange(new Set(found.map((skill) => skill.path)));
    const missing = preset.skills.length - found.length;
    if (missing > 0) {
      toast.warning(t("chain.workbench.wizardPresetMissing", { count: missing }));
    }
  };

  // A pill lights up when the selection is exactly the preset's available refs.
  const presetActive = (preset: ChainPreset) => {
    const found = preset.skills.filter((skill) => available.has(skill.path));
    return (
      found.length > 0 &&
      selected.size === found.length &&
      found.every((skill) => selected.has(skill.path))
    );
  };

  return (
    <div data-testid="skill-picker" className="flex min-h-0 flex-1 flex-col">
      {presets.length > 0 && (
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <span className="text-[11.5px] font-semibold text-muted">
            {t("chain.workbench.wizardPresetStart")}
          </span>
          {presets.map((preset) => (
            <button
              key={preset.id}
              data-testid="picker-preset-pill"
              onClick={() => applyPreset(preset)}
              className={cn(
                "rounded-full border px-3 py-1 text-[12px] font-medium transition-colors outline-none",
                presetActive(preset)
                  ? "border-accent-border bg-surface-active text-secondary"
                  : "border-glass-hairline bg-glass-soft text-muted hover:text-secondary",
              )}
            >
              {preset.name}
              <span className="ml-1 text-[10.5px] text-faint">
                {t("chain.workbench.presetCount", { count: preset.skills.length })}
              </span>
            </button>
          ))}
          <button
            data-testid="picker-preset-scratch"
            onClick={() => onChange(new Set())}
            className={cn(
              "rounded-full border px-3 py-1 text-[12px] font-medium transition-colors outline-none",
              selected.size === 0
                ? "border-accent-border bg-surface-active text-secondary"
                : "border-glass-hairline bg-glass-soft text-muted hover:text-secondary",
            )}
          >
            {t("chain.workbench.wizardPresetScratch")}
          </button>
        </div>
      )}

      <div className="relative mb-3">
        <Search className="absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-faint" />
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={t("chain.searchPlaceholder")}
          className="app-input h-9 w-full pl-9"
        />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-border-subtle">
        {groups.map((group) => (
          <div key={group.repo}>
            <div className="sticky top-0 bg-bg-secondary px-3 py-1.5 font-mono text-[11px] font-semibold text-muted">
              {group.repo}
              {multiRoot && (
                <span className="ml-2 font-sans font-normal text-faint">
                  {t("chain.rootSource", {
                    name: group.root.split("/").pop() || group.root,
                  })}
                </span>
              )}
            </div>
            {group.skills.map((skill) => (
              <label
                key={skill.path}
                className="flex cursor-pointer items-center gap-2.5 border-t border-border-subtle px-3 py-1.5 hover:bg-surface-hover"
              >
                <input
                  type="checkbox"
                  checked={selected.has(skill.path)}
                  onChange={() => toggleSkill(skill.path)}
                  className="accent-current"
                />
                <span className="font-mono text-[12px] text-secondary">{skill.name}</span>
              </label>
            ))}
          </div>
        ))}
        {groups.length === 0 && (
          <div className="px-3 py-6 text-center text-[12.5px] text-muted">—</div>
        )}
      </div>
    </div>
  );
}
