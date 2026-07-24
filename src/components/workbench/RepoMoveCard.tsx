import { useTranslation } from "react-i18next";
import { ListTree, Stethoscope, Wrench, X } from "lucide-react";
import type { ChainRepoMove } from "../../lib/tauri";
import { LivePanel, useLiveRepair } from "./liveRepair";

interface RepoMoveCardProps {
  group: ChainRepoMove;
  /** The group's members scoped to the selected project (what this card
   * repairs — no cross-project surprise writes). */
  fingerprints: string[];
  skills: string[];
  onViewDiagnosis: () => void;
  /** Fall back to per-finding evidence cards for this group. */
  onItemize: () => void;
  onRepaired: () => void;
}

/**
 * 风暴聚合卡（#33，原型 S5）：多条断链共指同一仓库根、新根含全部同名技能时，
 * 呈现为一个根因（仓库被移动）而不是一堆症状。「让 Agent 全部修复」批量走
 * #32 直播（preferRoot 锚定检测出的新根），一次 apply 一条 journal（#31
 * 整体撤销）；「逐条处理」退回单条证据卡。
 */
export function RepoMoveCard({
  group,
  fingerprints,
  skills,
  onViewDiagnosis,
  onItemize,
  onRepaired,
}: RepoMoveCardProps) {
  const { t } = useTranslation();
  const liveRepair = useLiveRepair(fingerprints, onRepaired, group.new_root);
  const live = liveRepair.live;

  return (
    <div
      data-testid="repo-move-card"
      data-old-root={group.old_root}
      className="app-glass-card flex gap-3 px-4 py-3.5"
    >
      <span className="w-1 shrink-0 self-stretch rounded-full bg-red-400" />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13.5px] font-semibold text-primary">
            {t("chain.workbench.repoMoveTitle", { name: group.repo_name })}
          </span>
          <span className="rounded-full border border-red-500/25 bg-red-500/10 px-1.5 py-px text-[10.5px] font-medium text-red-400">
            {t("chain.workbench.repoMoveBadge", { count: fingerprints.length })}
          </span>
        </div>

        <p className="text-[12.5px] leading-relaxed text-tertiary">
          {t("chain.workbench.repoMoveDesc", {
            count: fingerprints.length,
            oldRoot: group.old_root,
            newRoot: group.new_root,
          })}
        </p>

        <div
          data-testid="repo-move-skills"
          className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[11.5px] text-muted"
        >
          {skills.map((skill) => (
            <span key={skill} className="flex items-center gap-1">
              <X className="h-3 w-3 text-red-400" />
              {skill}
            </span>
          ))}
        </div>

        <div className="mt-1 flex flex-wrap gap-2">
          <button
            data-testid="repo-move-repair"
            onClick={() => void liveRepair.run()}
            disabled={live !== null}
            className="app-button-primary h-7 px-3 text-[12px]"
          >
            <Wrench className="h-3 w-3" />
            {t("chain.workbench.repoMoveRepairAll")}
          </button>
          <button
            data-testid="repo-move-diagnose"
            onClick={onViewDiagnosis}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <Stethoscope className="h-3 w-3" />
            {t("chain.workbench.cardDiagnose")}
          </button>
          <button
            data-testid="repo-move-itemize"
            onClick={onItemize}
            disabled={live !== null}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <ListTree className="h-3 w-3" />
            {t("chain.workbench.repoMoveItemize")}
          </button>
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
      </div>
    </div>
  );
}
