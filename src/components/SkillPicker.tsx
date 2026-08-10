import { useMemo, useState } from "react";
import { Search } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import type { ChainPreset, ChainRepo } from "../lib/tauri";

/** The Agent surfaces a chain link can enter through — the shared list the
 * link editor and the onboarding wizard both offer. */
export const CHAIN_AGENTS = ["claude", "codex", "copilot", "opencode", "qoderwork"] as const;

interface SkillPickerProps {
  repos: ChainRepo[];
  /** Selected Original paths — the exact values `chain_plan_link` consumes. */
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  /** Original paths the project already links. Rendered as the baseline the
   * selection is a diff against; empty for a project with no chain yet. */
  linked?: Set<string>;
  /** Preset 起步 pills（#35 套装）；缺省或空列表时不渲染该行。 */
  presets?: ChainPreset[];
}

/**
 * The Skill whitelist editor: which Originals this project exposes.
 *
 * It is a two-pane inventory rather than a flat add-only checklist. The left
 * rail is the source repositories — the browsing job that used to require
 * leaving for a separate area — and the right pane is their Skills with the
 * project's CURRENT selection already checked. That is the point: the picker
 * shows state, so unchecking is how you remove a Skill, and the whitelist has
 * exactly one screen instead of an add dialog plus a remove table.
 */
export function SkillPicker({
  repos,
  selected,
  onChange,
  linked = new Set(),
  presets = [],
}: SkillPickerProps) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  // null = every repository. The rail narrows the right pane; it never
  // changes the selection, so scoping can never silently drop a choice.
  const [scope, setScope] = useState<string | null>(null);

  const linkedCount = useMemo(
    () => (repo: ChainRepo) => repo.skills.filter((skill) => selected.has(skill.path)).length,
    [selected],
  );

  const groups = useMemo(() => {
    const q = search.trim().toLowerCase();
    return repos
      .filter((repo) => scope === null || repo.path === scope)
      .map((repo) => ({
        repo: repo.name,
        skills: repo.skills.filter((skill) => !q || skill.name.toLowerCase().includes(q)),
      }))
      .filter((group) => group.skills.length > 0);
  }, [repos, search, scope]);

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

      <div className="flex min-h-0 flex-1 gap-2">
        {/* Source rail: the repository inventory, in the picker instead of in a
            separate area you had to leave the decision to visit. */}
        <div
          data-testid="picker-sources"
          className="w-[176px] shrink-0 overflow-y-auto rounded-lg border border-border-subtle"
        >
          <SourceRow
            label={t("chain.sourceAll")}
            count={repos.reduce((total, repo) => total + linkedCount(repo), 0)}
            active={scope === null}
            onClick={() => setScope(null)}
          />
          {repos.map((repo) => (
            <SourceRow
              key={repo.path}
              data-testid="picker-source"
              label={repo.name}
              count={linkedCount(repo)}
              dirty={repo.health.dirty}
              active={scope === repo.path}
              onClick={() => setScope(scope === repo.path ? null : repo.path)}
            />
          ))}
          {repos.length === 0 && (
            <div className="px-3 py-6 text-center text-[12px] text-muted">
              {t("chain.workbench.wizardNoSources")}
            </div>
          )}
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-border-subtle">
          {groups.map((group) => (
            <div key={group.repo}>
              <div className="sticky top-0 bg-bg-secondary px-3 py-1.5 font-mono text-[11px] font-semibold text-muted">
                {group.repo}
              </div>
              {group.skills.map((skill) => {
                const checked = selected.has(skill.path);
                const wasLinked = linked.has(skill.path);
                return (
                  <label
                    key={skill.path}
                    data-testid="picker-skill"
                    data-changed={checked !== wasLinked ? (checked ? "add" : "remove") : undefined}
                    className="flex cursor-pointer items-center gap-2.5 border-t border-border-subtle px-3 py-1.5 hover:bg-surface-hover"
                  >
                    <input
                      type="checkbox"
                      // Named explicitly: the row also carries a change badge,
                      // which would otherwise leak into the accessible name.
                      aria-label={skill.name}
                      checked={checked}
                      onChange={() => toggleSkill(skill.path)}
                      className="accent-current"
                    />
                    <span className="font-mono text-[12px] text-secondary">{skill.name}</span>
                    {checked !== wasLinked && (
                      <span
                        className={cn(
                          "ml-auto rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                          checked
                            ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-400"
                            : "border-red-500/25 bg-red-500/10 text-red-400",
                        )}
                      >
                        {t(checked ? "chain.willLink" : "chain.willUnlink")}
                      </span>
                    )}
                  </label>
                );
              })}
            </div>
          ))}
          {groups.length === 0 && (
            <div className="px-3 py-6 text-center text-[12.5px] text-muted">—</div>
          )}
        </div>
      </div>
    </div>
  );
}

function SourceRow({
  label,
  count,
  dirty,
  active,
  onClick,
  ...rest
}: {
  label: string;
  count: number;
  dirty?: boolean;
  active: boolean;
  onClick: () => void;
} & Record<string, unknown>) {
  return (
    <button
      {...rest}
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-1.5 border-b border-border-subtle px-2.5 py-1.5 text-left text-[12px] outline-none last:border-b-0 transition-colors",
        active ? "bg-surface-active text-secondary" : "text-muted hover:bg-surface-hover",
      )}
    >
      {dirty && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400" />}
      <span className="min-w-0 flex-1 truncate font-mono">{label}</span>
      {count > 0 && <span className="shrink-0 tabular-nums text-[11px] text-accent">{count}</span>}
    </button>
  );
}
