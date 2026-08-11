import { useEffect } from "react";
import { Link, Outlet, useLocation, useNavigate } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { StatusBanner } from "./StatusBanner";
import { CommandPalette } from "./CommandPalette";
import { ChainStatusBar } from "./ChainStatusBar";
import { useApp } from "../context/AppContext";
import { useChain } from "../context/ChainContext";
import { useTranslation } from "react-i18next";
import { useDragWindow } from "../hooks/useDragWindow";
import { cn } from "../utils";

const ISLAND_TABS = [
  { path: "/", labelKey: "home.tabWorkbench" },
  { path: "/chain", labelKey: "home.tabChain" },
  { path: "/doctor", labelKey: "home.tabDoctor" },
  { path: "/sources", labelKey: "home.tabWarehouse" },
] as const;

export function Layout() {
  const { t } = useTranslation();
  const { appError, refreshAppData } = useApp();
  const { reload: rescanChain } = useChain();
  const onDrag = useDragWindow();
  const navigate = useNavigate();
  const location = useLocation();
  const tabbedRoute = ISLAND_TABS.some((tab) => tab.path === location.pathname);
  const project = new URLSearchParams(location.search).get("project");

  // Cmd+, to open Settings
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) return;
        e.preventDefault();
        navigate("/settings");
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "r") {
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) return;
        e.preventDefault();
        // One refresh gesture, both data planes — the registry and the scan.
        refreshAppData();
        void rescanChain();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [navigate, refreshAppData, rescanChain]);

  return (
    <div className="relative flex h-full w-full overflow-hidden text-primary">
      {/* Full-width top drag bar — spans sidebar + content. It reads as part of
          the canvas, so it carries no fill or divider of its own. --titlebar-h
          is 0 on Windows, where the OS supplies a caption bar. */}
      <div
        onMouseDown={onDrag}
        className="absolute inset-x-0 top-0 z-50 h-[var(--titlebar-h)]"
      />
      <Sidebar />
      {/* 内容区：右侧留出画布边距，岛在其中浮起。侧栏一侧不留边距——
          侧栏本身就坐在画布上，岛的左缘即是二者的分界。 */}
      <div className="relative flex min-w-[600px] flex-1 flex-col overflow-hidden">
        <div className="flex-1 overflow-y-auto pb-5 pl-1 pr-5 pt-[calc(var(--titlebar-h)+16px)] scrollbar-hide">
          <div className="mx-auto flex min-h-full max-w-[1200px] flex-col gap-3">
            {appError ? (
              <StatusBanner
                compact
                title={t("common.dataOutOfDate")}
                description={appError}
                actionLabel={t("common.retry")}
                onAction={refreshAppData}
                tone="danger"
              />
            ) : null}
            {/* 一扇窗一座岛：每条路由的内容都落在这块浮起的白面上，
                页签、页头与列表都在岛内，画布只负责托住它。 */}
            {/* flex-auto，不是 flex-1：basis 取内容高度，内容短时撑满一屏，
                内容长时随之增高——否则 overflow-hidden 会把列表裁掉。 */}
            <div className="app-island flex flex-auto flex-col overflow-hidden">
              {tabbedRoute ? (
                <div className="app-island-tabs shrink-0">
                  {ISLAND_TABS.map((tab) => {
                    const href = project
                      ? `${tab.path}?project=${encodeURIComponent(project)}`
                      : tab.path;
                    return (
                      <Link
                        key={tab.path}
                        to={href}
                        className={cn("app-tab", location.pathname === tab.path && "app-tab-active")}
                      >
                        {t(tab.labelKey)}
                      </Link>
                    );
                  })}
                </div>
              ) : null}
              <ChainStatusBar />
              <Outlet />
            </div>
          </div>
        </div>
      </div>
      <CommandPalette />
    </div>
  );
}
