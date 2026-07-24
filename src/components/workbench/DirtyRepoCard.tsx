import { useState } from "react";
import { useTranslation } from "react-i18next";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { ChevronDown, ChevronRight, FolderGit2, GitCompareArrows } from "lucide-react";
import { cn } from "../../utils";
import { chainRepoDirtyDiff } from "../../lib/tauri";
import type { ChainDirtyDiff, ChainRepo } from "../../lib/tauri";

interface DirtyRepoCardProps {
  repo: ChainRepo;
}

const STATUS_TAG: Record<string, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  typechange: "T",
  other: "?",
};

/**
 * 反哺提示卡（#34，原型 S2 第二张卡）：warehouse 原件仓库工作区不干净时的
 * 暖色提示——warning 级、不遮蔽断链卡、不阻塞主动线。「查看 diff」懒加载
 * 只读的 tracked 改动清单（文件 + 行数统计）；「整理提交」打开仓库目录，
 * 把接力棒交给你自己的提交工具链（#26：反哺自动化 out of scope）。
 */
export function DirtyRepoCard({ repo }: DirtyRepoCardProps) {
  const { t } = useTranslation();
  const [diff, setDiff] = useState<ChainDirtyDiff | null>(null);
  const [showDiff, setShowDiff] = useState(false);
  const [loading, setLoading] = useState(false);
  const Chevron = showDiff ? ChevronDown : ChevronRight;

  const toggleDiff = async () => {
    if (showDiff) {
      setShowDiff(false);
      return;
    }
    setShowDiff(true);
    if (diff) return;
    setLoading(true);
    try {
      setDiff(await chainRepoDirtyDiff(repo.path));
    } catch (e) {
      toast.error(String(e));
      setShowDiff(false);
    } finally {
      setLoading(false);
    }
  };

  const openRepo = async () => {
    try {
      await openPath(repo.path);
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div data-testid="dirty-repo-card" data-repo={repo.path} className="app-glass-card flex gap-3 px-4 py-3.5">
      <span className="w-1 shrink-0 self-stretch rounded-full bg-amber-400" />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13.5px] font-semibold text-primary">
            {t("chain.workbench.dirtyTitle", { name: repo.name })}
          </span>
          <span className="rounded-full border border-amber-500/25 bg-amber-500/10 px-1.5 py-px text-[10.5px] font-medium text-amber-400">
            {t("chain.workbench.dirtyBadge")}
          </span>
        </div>

        <p className="text-[12.5px] leading-relaxed text-tertiary">
          {t("chain.workbench.dirtyDesc")}
        </p>

        {showDiff && (
          <div data-testid="dirty-diff" className="space-y-0.5 font-mono text-[11.5px]">
            {loading && <div className="text-muted">{t("chain.workbench.dirtyLoading")}</div>}
            {diff?.files.length === 0 && !loading && (
              <div className="text-muted">{t("chain.workbench.dirtyEmpty")}</div>
            )}
            {diff?.files.map((file) => (
              <div key={file.path} className="flex items-center gap-2">
                <span
                  className={cn(
                    "w-3 shrink-0 text-center font-bold",
                    file.status === "deleted" ? "text-red-400" : "text-amber-400",
                  )}
                >
                  {STATUS_TAG[file.status] ?? "?"}
                </span>
                <span className="min-w-0 flex-1 truncate text-secondary">{file.path}</span>
                <span className="shrink-0 text-emerald-400">+{file.additions}</span>
                <span className="shrink-0 text-red-400">-{file.deletions}</span>
              </div>
            ))}
            {diff?.truncated && (
              <div className="text-faint">{t("chain.workbench.dirtyTruncated")}</div>
            )}
          </div>
        )}

        <div className="mt-1 flex flex-wrap gap-2">
          <button
            data-testid="dirty-toggle-diff"
            onClick={() => void toggleDiff()}
            aria-expanded={showDiff}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <Chevron className="h-3 w-3" />
            <GitCompareArrows className="h-3 w-3" />
            {t(showDiff ? "chain.workbench.dirtyDiffHide" : "chain.workbench.dirtyDiff")}
          </button>
          <button
            data-testid="dirty-open-repo"
            onClick={() => void openRepo()}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <FolderGit2 className="h-3 w-3" />
            {t("chain.workbench.dirtyOpenRepo")}
          </button>
        </div>
      </div>
    </div>
  );
}
