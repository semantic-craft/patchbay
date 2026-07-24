import { useEffect } from "react";
import { Outlet, useNavigate } from "react-router-dom";
import { Sidebar } from "./Sidebar";
import { StatusBanner } from "./StatusBanner";
import { CommandPalette } from "./CommandPalette";
import { useApp } from "../context/AppContext";
import { useTranslation } from "react-i18next";
import { useDragWindow } from "../hooks/useDragWindow";

export function Layout() {
  const { t } = useTranslation();
  const { appError, refreshAppData } = useApp();
  const onDrag = useDragWindow();
  const navigate = useNavigate();

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
        refreshAppData();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [navigate, refreshAppData]);

  return (
    <div className="app-glass-shell relative flex h-full w-full overflow-hidden text-primary">
      {/* Full-width top drag bar — spans sidebar + content, with bottom divider.
          It reads as part of the shell's glass, so it carries no fill of its own.
          --titlebar-h is 0 on Windows, where the OS supplies a caption bar, so
          this collapses to just the hairline dividing chrome from content. */}
      <div
        onMouseDown={onDrag}
        className="absolute inset-x-0 top-0 z-50 h-[var(--titlebar-h)] border-b border-glass-hairline"
      />
      <Sidebar />
      <div className="relative flex min-w-[600px] flex-1 flex-col overflow-hidden">
        <div className="flex-1 overflow-y-auto px-5 pb-5 pt-[calc(var(--titlebar-h)+20px)] scrollbar-hide">
          <div className="mx-auto flex min-h-full max-w-[1200px] flex-col gap-4">
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
            <Outlet />
          </div>
        </div>
      </div>
      <CommandPalette />
    </div>
  );
}
