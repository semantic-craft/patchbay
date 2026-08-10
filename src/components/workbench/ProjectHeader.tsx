import { useTranslation } from "react-i18next";
import { FolderOpen, Plus } from "lucide-react";
import type { ChainProject } from "../../lib/tauri";

interface ProjectHeaderProps {
  project: ChainProject | null;
  onPickFolder: () => void;
  onLink: () => void;
}

/**
 * The workbench header: which project you are looking at, and the one action
 * this screen exists for.
 *
 * It replaced a page title plus a 51-pill project selector that duplicated the
 * sidebar and ate the whole first screen. The sidebar owns selection now, so
 * the header only has to answer "where am I" — hence the name and the path,
 * nothing else.
 */
export function ProjectHeader({ project, onPickFolder, onLink }: ProjectHeaderProps) {
  const { t } = useTranslation();

  return (
    <div className="app-page-header app-toolbar">
      <div className="min-w-0">
        <h1 className="app-page-title truncate">
          {project ? project.name : t("chain.noProjectSelected")}
        </h1>
        <p className="mt-1 truncate font-mono text-[11.5px] text-muted" title={project?.path}>
          {project?.path ?? t("chain.noProjectHint")}
        </p>
      </div>
      <div className="flex shrink-0 gap-2">
        <button className="app-button-secondary" onClick={onPickFolder}>
          <FolderOpen className="h-4 w-4" />
          {t("chain.pickProject")}
        </button>
        {project && (
          <button className="app-button-primary" onClick={onLink}>
            <Plus className="h-4 w-4" />
            {t("chain.linkButton")}
          </button>
        )}
      </div>
    </div>
  );
}
