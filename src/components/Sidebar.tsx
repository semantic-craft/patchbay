import { useMemo, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import {
  Link2,
  MonitorSmartphone,
  Plus,
  Search,
  Settings,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import { useApp } from "../context/AppContext";
import { useChain } from "../context/ChainContext";
import { entityColor } from "../lib/entityColor";
import { projectHealth } from "../lib/workbenchState";
import type { ChainSeverity } from "../lib/tauri";
import { AddProjectDialog } from "./AddProjectDialog";
import { ConfirmDialog } from "./ConfirmDialog";
import * as api from "../lib/tauri";

/** 项目健康点配色：按该项目 findings 的最高 severity 上色（#30）。 */
const HEALTH_DOT: Record<ChainSeverity, string> = {
  violation: "bg-red-400",
  warning: "bg-amber-400",
  advice: "bg-blue-400",
  notice: "bg-gray-400",
};

export function Sidebar() {
  const { t } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();
  const { projects, refreshProjects } = useApp();
  // The shared scan's Doctor report. `null` until the first scan lands — then
  // no dot is shown, because no health is known.
  const { doctor: doctorReport } = useChain();
  const [query, setQuery] = useState("");
  const [showAddProject, setShowAddProject] = useState(false);
  const [deleteProjectTarget, setDeleteProjectTarget] = useState<{
    id: string;
    name: string;
    path: string;
  } | null>(null);
  const selectedProjectPath = new URLSearchParams(location.search).get("project");

  // 诊断与技能源在岛头页签里；侧栏底部只保留跨工作台的目的地。
  const footerItems = [
    { name: t("sidebar.fleet"), path: "/fleet", icon: MonitorSmartphone },
    { name: t("sidebar.settings"), path: "/settings", icon: Settings },
  ];

  // 51 个项目时列表本身就是导航负担，给它一个过滤框。
  const visibleProjects = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return projects;
    return projects.filter((project) => project.name.toLowerCase().includes(q));
  }, [projects, query]);

  const openProject = (path: string) => {
    navigate(`/?project=${encodeURIComponent(path)}`);
  };

  const handleDeleteProject = async () => {
    if (!deleteProjectTarget) return;
    await api.removeProject(deleteProjectTarget.id);
    await refreshProjects();
    if (selectedProjectPath === deleteProjectTarget.path) navigate("/");
    toast.success(t("project.removed"));
  };

  return (
    <>
      <div className="relative z-10 flex h-full w-[220px] flex-shrink-0 select-none flex-col">
        {/* Clears the overlay titlebar plus a little breathing room. On Windows
            --titlebar-h is 0, leaving just the breathing room under the native
            caption bar. */}
        <div className="h-[calc(var(--titlebar-h)+10px)] shrink-0" />
        <div className="flex shrink-0 items-center gap-3 px-3 pb-2.5">
          <img src="/icons/32x32.png" alt="logo" className="h-[24px] w-[24px] shrink-0" />
          <span className="truncate text-[16px] font-semibold leading-[22px] tracking-tight text-secondary">
            {t("app.name")}
          </span>
        </div>

        <div className="shrink-0 px-2.5 pb-2">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3 w-3 -translate-y-1/2 text-faint" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("sidebar.filterProjects")}
              aria-label={t("sidebar.filterProjects")}
              className="h-7 w-full rounded-[5px] border border-border-subtle bg-surface pl-7 pr-2 text-[12.5px] text-secondary outline-none transition-colors placeholder:text-faint focus:border-accent-border"
            />
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-2.5 scrollbar-hide">
          <div className="space-y-0.5">
            {visibleProjects.map((project) => {
              const isActive = location.pathname === "/" && selectedProjectPath === project.path;
              const health = projectHealth(doctorReport, project.path);
              return (
                <div
                  key={project.id}
                  className={cn(
                    "group relative flex items-center rounded-[5px] transition-colors",
                    isActive ? "bg-surface-active" : "hover:bg-surface-hover",
                  )}
                >
                  <button
                    onClick={() => openProject(project.path)}
                    className={cn(
                      "flex min-w-0 flex-1 items-center gap-2 px-2.5 py-[7px] text-left text-sm leading-5 outline-none",
                      isActive ? "font-medium text-primary" : "text-tertiary group-hover:text-secondary",
                    )}
                  >
                    <span
                      className="flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded"
                      style={entityColor(project.name)}
                    >
                      <Link2 className="h-3 w-3" />
                    </span>
                    <span className="flex-1 truncate">{project.name}</span>
                    {health.state !== "unknown" && (
                      <span
                        data-testid="project-health"
                        data-state={health.state}
                        className={cn(
                          "h-1.5 w-1.5 shrink-0 rounded-full",
                          health.worst ? HEALTH_DOT[health.worst] : "bg-emerald-400",
                        )}
                      />
                    )}
                  </button>
                  <button
                    onClick={(event) => {
                      event.stopPropagation();
                      setDeleteProjectTarget(project);
                    }}
                    className="invisible absolute right-1 rounded p-1 text-faint opacity-0 transition hover:text-red-400 group-hover:visible group-hover:opacity-100"
                    title={t("common.delete")}
                  >
                    <Trash2 className="h-3 w-3" />
                  </button>
                </div>
              );
            })}
            {visibleProjects.length === 0 && (
              <div className="px-2.5 py-3 text-[12.5px] text-faint">
                {t("sidebar.noProjectMatch")}
              </div>
            )}
          </div>

          <button
            onClick={() => setShowAddProject(true)}
            className="mt-1 flex w-full items-center gap-2 rounded-[5px] px-2.5 py-[7px] text-sm text-muted transition-colors outline-none hover:bg-surface-hover hover:text-secondary"
          >
            <Plus className="h-3.5 w-3.5" />
            {t("sidebar.addProject")}
          </button>
        </div>

        <div className="shrink-0 space-y-0.5 border-t border-hairline p-2.5">
          {footerItems.map((item) => {
            const Icon = item.icon;
            const isActive = location.pathname === item.path;
            return (
              <Link
                key={item.path}
                to={item.path}
                className={cn(
                  "flex items-center gap-2.5 rounded-[5px] px-2.5 py-[7px] text-sm font-medium transition-colors outline-none",
                  isActive
                    ? "bg-surface-active text-primary"
                    : "text-tertiary hover:bg-surface-hover hover:text-secondary",
                )}
              >
                <Icon className={cn("h-4 w-4 shrink-0", isActive ? "text-accent" : "text-muted")} />
                {item.name}
              </Link>
            );
          })}
        </div>
      </div>

      <AddProjectDialog
        open={showAddProject}
        onClose={() => setShowAddProject(false)}
        onAdded={async () => {
          await refreshProjects();
          toast.success(t("project.workspaceAdded"));
        }}
      />

      <ConfirmDialog
        open={deleteProjectTarget !== null}
        message={t("project.removeConfirm", { name: deleteProjectTarget?.name || "" })}
        onClose={() => setDeleteProjectTarget(null)}
        onConfirm={handleDeleteProject}
      />
    </>
  );
}
