import { useTranslation } from "react-i18next";
import { cn } from "../../utils";
import { TONE_BADGE } from "../../lib/chainUi";
import type { ChainProject } from "../../lib/tauri";

interface SurfacesCardProps {
  project: ChainProject;
}

/** Workbench 入口卡区块：聚合目录与各 Agent 面的入口状态摘要。 */
export function SurfacesCard({ project }: SurfacesCardProps) {
  const { t } = useTranslation();
  return (
    <div className="app-glass-card flex flex-wrap items-center gap-x-5 gap-y-1.5 px-4 py-2.5 text-[12.5px]">
      <span className="font-mono text-[11.5px] text-muted">
        {project.agents_dir
          ? `.agents/skills · ${t("chain.entriesCount", { count: project.agents_dir.entries.length })}`
          : t("chain.noAgg")}
      </span>
      {project.surfaces
        .filter((s) => s.kind !== "absent")
        .map((s) => (
          <span key={s.agent} className="flex items-center gap-1.5 font-mono text-[11.5px] text-muted">
            {s.agent}
            {s.kind === "dir_link" ? (
              <span
                className={cn(
                  "rounded-full border px-1.5 py-px font-sans text-[10.5px] font-medium",
                  s.dir_link_ok ? TONE_BADGE.ok : TONE_BADGE.err
                )}
              >
                {t(s.dir_link_ok ? "chain.dirLinkOk" : "chain.dirLinkBad")}
              </span>
            ) : (
              <span className="text-tertiary">{t("chain.entriesCount", { count: s.entries.length })}</span>
            )}
          </span>
        ))}
    </div>
  );
}
