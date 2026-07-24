import { useTranslation } from "react-i18next";
import { RefreshCw, FolderOpen, Plus } from "lucide-react";
import { cn } from "../../utils";
import { ChainScanStatus } from "../ChainScanStatus";

interface WorkbenchHeaderProps {
  loading: boolean;
  scannedAt: number | undefined;
  hasProject: boolean;
  onPickFolder: () => void;
  onRescan: () => void;
  onLink: () => void;
}

/** Workbench 头部区块：标题、扫描状态与主操作。 */
export function WorkbenchHeader({
  loading,
  scannedAt,
  hasProject,
  onPickFolder,
  onRescan,
  onLink,
}: WorkbenchHeaderProps) {
  const { t } = useTranslation();
  return (
    <div className="app-page-header app-toolbar">
      <div>
        <h1 className="app-page-title">{t("chain.projectsTitle")}</h1>
        <p className="app-page-subtitle">{t("chain.projectsSubtitle")}</p>
        <ChainScanStatus scannedAt={scannedAt} loading={loading} />
      </div>
      <div className="flex gap-2">
        <button className="app-button-secondary" onClick={onPickFolder}>
          <FolderOpen className="h-4 w-4" />
          {t("chain.pickProject")}
        </button>
        <button className="app-button-secondary" onClick={onRescan} disabled={loading}>
          <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
          {t("chain.rescan")}
        </button>
        {hasProject && (
          <button className="app-button-primary" onClick={onLink}>
            <Plus className="h-4 w-4" />
            {t("chain.linkButton")}
          </button>
        )}
      </div>
    </div>
  );
}
