import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "../../utils";
import { TONE_BADGE } from "../../lib/chainUi";
import { INSTRUCTIONS_STATE_TONE, formatBytes, formatTokens } from "../../lib/instructionsUi";
import type { InstructionsEntry, InstructionsProject } from "../../lib/tauri";

interface InstructionsPanelProps {
  instrProject: InstructionsProject;
  planning: "normalize" | "init" | null;
  onNormalize: () => void;
  onInit: () => void;
}

/**
 * Instructions governance for the selected project: the canonical `AGENTS.md`
 * and each Agent's resident cost.
 *
 * It is a summary row by default. This is real but low-frequency work — it has
 * nothing to do with picking Skills — and as an always-open panel it took more
 * of the workbench than the project's own status did.
 */
export function InstructionsPanel({ instrProject, planning, onNormalize, onInit }: InstructionsPanelProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const Chevron = expanded ? ChevronDown : ChevronRight;

  // The headline number: what every Agent in this project carries in total.
  const totalTokens = useMemo(
    () => instrProject.resident.reduce((sum, r) => sum + r.est_tokens, 0),
    [instrProject],
  );

  // Widest agent resident total, so every agent's stacked bar shares one scale
  // and their costs are comparable at a glance.
  const instrMaxBytes = useMemo(
    () => Math.max(1, ...instrProject.resident.map((r) => r.project_bytes + r.global_bytes)),
    [instrProject]
  );

  // An agent's representative entry for the state badge. Claude may expose two
  // (root `CLAUDE.md` and `.claude/CLAUDE.md`); the root entry is primary.
  const entryFor = (agent: string): InstructionsEntry | undefined => {
    const es = instrProject.entries.filter((e) => e.agent === agent);
    return es.find((e) => !e.path.includes("/.claude/")) ?? es[0];
  };

  return (
    <div data-testid="instructions-panel" className="app-glass-card overflow-hidden">
      <button
        data-testid="instructions-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
        className="flex w-full flex-wrap items-center gap-x-3 gap-y-1.5 px-4 py-3 text-left text-[12.5px] outline-none"
      >
        <span className="app-section-title">{t("instructions.panelTitle")}</span>
        {instrProject.canonical.exists ? (
          <>
            <span
              className={cn(
                "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                TONE_BADGE.ok
              )}
            >
              AGENTS.md
            </span>
            <span className="font-mono text-[11.5px] text-muted">
              {formatBytes(instrProject.canonical.bytes)} ·{" "}
              {t("instructions.lines", { count: instrProject.canonical.lines })}
            </span>
          </>
        ) : (
          <span
            className={cn(
              "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
              TONE_BADGE.warn
            )}
          >
            {t("instructions.canonicalMissing")}
          </span>
        )}
        <span className="font-mono text-[11.5px] text-muted">
          {t("instructions.residentSummary", {
            count: instrProject.resident.length,
            tokens: formatTokens(totalTokens),
          })}
        </span>
        <span className="ml-auto flex items-center gap-1 text-muted">
          {t(expanded ? "chain.workbench.collapse" : "chain.workbench.expand")}
          <Chevron className="h-3.5 w-3.5" />
        </span>
      </button>

      {!expanded ? null : (
      <div className="space-y-3 border-t border-glass-hairline p-4">
      <div className="flex flex-wrap items-center justify-end gap-3">
          <span className="flex items-center gap-3 text-[10.5px] text-muted">
            <span className="flex items-center gap-1">
              <span className="h-2 w-2 rounded-sm bg-emerald-400/70" />
              {t("instructions.projectSegment")}
            </span>
            <span className="flex items-center gap-1">
              <span className="h-2 w-2 rounded-sm bg-gray-400/50" />
              {t("instructions.globalSegment")}
            </span>
          </span>
          <button
            onClick={onNormalize}
            disabled={planning !== null}
            className="rounded border border-border-subtle px-2 py-0.5 text-[11px] font-medium text-muted outline-none transition-colors hover:text-secondary disabled:opacity-50"
          >
            {planning === "normalize"
              ? t("instructions.normalize.planning")
              : t("instructions.normalize.action")}
          </button>
          <button
            onClick={onInit}
            disabled={planning !== null}
            className="rounded border border-border-subtle px-2 py-0.5 text-[11px] font-medium text-muted outline-none transition-colors hover:text-secondary disabled:opacity-50"
          >
            {planning === "init" ? t("instructions.init.planning") : t("instructions.init.action")}
          </button>
      </div>

      {instrProject.resident.length === 0 ? (
        <div className="text-[12px] text-muted">{t("instructions.noAgents")}</div>
      ) : (
        <div className="space-y-1.5">
          {instrProject.resident.map((r) => {
            const state = entryFor(r.agent)?.state ?? "missing";
            // The agent column has to fit the longest installed agent key
            // ("antigravity", 11 mono chars); 64px clipped it into the badge.
            return (
              <div key={r.agent} className="grid grid-cols-[86px_86px_1fr_auto] items-center gap-2.5">
                <span className="font-mono text-[11.5px] text-tertiary">{r.agent}</span>
                <span
                  className={cn(
                    "justify-self-start rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                    TONE_BADGE[INSTRUCTIONS_STATE_TONE[state]]
                  )}
                >
                  {t(`instructions.state.${state}`)}
                </span>
                <div
                  className="flex h-1.5 w-full overflow-hidden rounded-full bg-surface-hover"
                  title={`${t("instructions.projectSegment")} ${formatBytes(
                    r.project_bytes
                  )} · ${t("instructions.globalSegment")} ${formatBytes(r.global_bytes)}`}
                >
                  <div
                    className="bg-emerald-400/70"
                    style={{ width: `${(r.project_bytes / instrMaxBytes) * 100}%` }}
                  />
                  <div
                    className="bg-gray-400/50"
                    style={{ width: `${(r.global_bytes / instrMaxBytes) * 100}%` }}
                  />
                </div>
                <span className="justify-self-end font-mono text-[11px] tabular-nums text-muted">
                  {formatTokens(r.est_tokens)}
                </span>
              </div>
            );
          })}
        </div>
      )}
      </div>
      )}
    </div>
  );
}
