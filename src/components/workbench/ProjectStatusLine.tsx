import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { cn } from "../../utils";
import { TONE_BADGE } from "../../lib/chainUi";
import type { ChainProject } from "../../lib/tauri";

interface ProjectStatusLineProps {
  project: ChainProject;
  /** Total link rows in this project — the headline number. */
  count: number;
  /** True when Doctor has a report and it names nothing to handle. */
  green: boolean;
}

/**
 * One line for "what is this project's chain right now": the link count, the
 * verdict, and every Agent surface with its state.
 *
 * This used to be two stacked cards — a 200px tall ✓ badge that carried a
 * single number, then a 42px strip that carried everything else. The verdict
 * and the evidence for it belong on the same line, and the evidence is the part
 * worth the pixels.
 */
export function ProjectStatusLine({ project, count, green }: ProjectStatusLineProps) {
  const { t } = useTranslation();

  return (
    <div
      data-testid="workbench-status-line"
      data-state={green ? "green" : "unknown"}
      className="app-glass-card flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2.5 text-[12.5px]"
    >
      {green && (
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-emerald-500/30 bg-emerald-500/10 text-emerald-400">
          <Check className="h-3 w-3" strokeWidth={3} />
        </span>
      )}
      <span className="font-medium text-secondary">
        {t("chain.workbench.linkCount", { count })}
      </span>

      <span className="h-3 w-px bg-glass-hairline" />

      <span className="font-mono text-[11.5px] text-muted">
        {project.agents_dir
          ? `.agents/skills · ${t("chain.entriesCount", {
              count: project.agents_dir.entries.length,
            })}`
          : t("chain.noAgg")}
      </span>

      {project.surfaces
        .filter((surface) => surface.kind !== "absent")
        .map((surface) => (
          <span
            key={surface.agent}
            className="flex items-center gap-1.5 font-mono text-[11.5px] text-muted"
          >
            {surface.agent}
            {surface.kind === "dir_link" ? (
              <span
                className={cn(
                  "rounded-full border px-1.5 py-px font-sans text-[10.5px] font-medium",
                  surface.dir_link_ok ? TONE_BADGE.ok : TONE_BADGE.err,
                )}
              >
                {t(surface.dir_link_ok ? "chain.dirLinkOk" : "chain.dirLinkBad")}
              </span>
            ) : (
              <span className="text-tertiary">
                {t("chain.entriesCount", { count: surface.entries.length })}
              </span>
            )}
          </span>
        ))}
    </div>
  );
}
