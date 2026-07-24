import { useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import {
  CloudUpload,
  Download,
  FolderOpen,
  GitBranch,
  Layers,
  LayoutDashboard,
  Link2,
  MonitorSmartphone,
  Plus,
  Settings,
  Stethoscope,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import { useApp } from "../context/AppContext";
import { useDoctorReport } from "../lib/doctorStore";
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
  // Latest Doctor report, published by the workbench after each scan. `null`
  // until a scan lands — then no dot is shown, because no health is known.
  const doctorReport = useDoctorReport();
  const [showAddProject, setShowAddProject] = useState(false);
  const [deleteProjectTarget, setDeleteProjectTarget] = useState<{ id: string; name: string } | null>(null);
  const selectedProjectPath = new URLSearchParams(location.search).get("project");

  // 「视图」小节：链路的几个观察面，项目工作台是主屏。
  const viewItems = [
    { name: t("sidebar.workbench"), path: "/", icon: FolderOpen },
    { name: t("sidebar.chainOverview"), path: "/chain/overview", icon: LayoutDashboard },
    { name: t("sidebar.chainWarehouse"), path: "/chain/warehouse", icon: GitBranch },
    { name: t("sidebar.chainDoctor"), path: "/chain/doctor", icon: Stethoscope },
    { name: t("sidebar.fleet"), path: "/fleet", icon: MonitorSmartphone },
  ];
  const libraryItems = [
    { name: t("sidebar.mySkills"), path: "/my-skills", icon: Layers },
    { name: t("sidebar.installSkills"), path: "/install", icon: Download },
    { name: t("sidebar.backup"), path: "/backup", icon: CloudUpload },
  ];

  const openProject = (path: string) => {
    navigate(`/?project=${encodeURIComponent(path)}`);
  };

  const handleDeleteProject = async () => {
    if (!deleteProjectTarget) return;
    await api.removeProject(deleteProjectTarget.id);
    await refreshProjects();
    if (location.pathname === "/chain/projects") navigate("/");
    toast.success(t("project.removed"));
  };

  return (
    <>
      <div className="app-glass-chrome relative z-10 flex h-full w-[220px] flex-shrink-0 select-none flex-col border-r border-glass-hairline">
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

        <div className="shrink-0 px-2.5">
          <div className="mb-1 px-2.5 text-[12px] font-semibold tracking-[0.01em] text-muted">
            {t("sidebar.views")}
          </div>
          <div className="space-y-0.5">
            {viewItems.map((item) => {
              const Icon = item.icon;
              const isActive = location.pathname === item.path;
              return (
                <Link
                  key={item.path}
                  to={item.path}
                  className={cn(
                    "flex items-center gap-2.5 rounded-[5px] px-2.5 py-[7px] text-sm font-medium transition-colors outline-none",
                    isActive
                      ? "bg-glass-strong text-primary"
                      : "text-tertiary hover:bg-glass-soft hover:text-secondary",
                  )}
                >
                  <Icon className={cn("h-4 w-4 shrink-0", isActive ? "text-accent" : "text-muted")} />
                  {item.name}
                </Link>
              );
            })}
          </div>
          <div className="mx-0.5 my-2.5 border-t border-glass-hairline" />
          <div className="space-y-0.5">
            {libraryItems.map((item) => {
              const Icon = item.icon;
              const isActive = location.pathname === item.path;
              return (
                <Link
                  key={item.path}
                  to={item.path}
                  className={cn(
                    "flex items-center gap-2.5 rounded-[5px] px-2.5 py-[7px] text-sm font-medium transition-colors outline-none",
                    isActive
                      ? "bg-glass-strong text-primary"
                      : "text-tertiary hover:bg-glass-soft hover:text-secondary",
                  )}
                >
                  <Icon className={cn("h-4 w-4 shrink-0", isActive ? "text-accent" : "text-muted")} />
                  {item.name}
                </Link>
              );
            })}
          </div>
        </div>

        <div className="mx-3 mb-2.5 mt-3.5 border-t border-glass-hairline" />

        <div className="min-h-0 flex-1 overflow-y-auto px-2.5 scrollbar-hide">
          <div className="mb-1.5 flex items-center gap-1 px-2.5">
            <span className="min-w-0 flex-1 truncate whitespace-nowrap text-[12px] font-semibold tracking-[0.01em] text-muted">
              {t("sidebar.projects")}
            </span>
          </div>

          <div className="space-y-0.5">
            {projects.map((project) => {
              const isActive = location.pathname === "/" && selectedProjectPath === project.path;
              const health = projectHealth(doctorReport, project.path);
              return (
                <div
                  key={project.id}
                  className={cn(
                    "group relative flex items-center rounded-[5px] transition-colors",
                    isActive ? "bg-glass-strong" : "hover:bg-glass-soft",
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
                      className={cn(
                        "flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded border",
                        isActive
                          ? "border-accent/30 bg-accent/10 text-accent"
                          : "border-glass-hairline bg-glass-soft text-muted",
                      )}
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
          </div>

          <button
            onClick={() => setShowAddProject(true)}
            className="mt-1 flex w-full items-center gap-2 rounded-[5px] px-2.5 py-[7px] text-sm text-muted transition-colors outline-none hover:bg-glass-soft hover:text-secondary"
          >
            <Plus className="h-3.5 w-3.5" />
            {t("sidebar.addProject")}
          </button>
        </div>

        <div className="shrink-0 border-t border-glass-hairline p-2.5">
          <Link
            to="/settings"
            className={cn(
              "flex items-center gap-2.5 rounded-[5px] px-2.5 py-[7px] text-sm font-medium transition-colors outline-none",
              location.pathname === "/settings"
                ? "bg-glass-strong text-primary"
                : "text-tertiary hover:bg-glass-soft hover:text-secondary",
            )}
          >
            <Settings className={cn("h-4 w-4 shrink-0", location.pathname === "/settings" ? "text-accent" : "text-muted")} />
            {t("sidebar.settings")}
          </Link>
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
