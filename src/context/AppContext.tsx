/* eslint-disable react-refresh/only-export-components */
import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Project, ToolInfo } from "../lib/tauri";
import * as api from "../lib/tauri";
import i18n from "../i18n";
import { applyTextSize } from "../lib/textScale";

interface AppState {
  tools: ToolInfo[];
  projects: Project[];
  loading: boolean;
  appError: string | null;
  helpOpen: boolean;
  refreshAppData: () => Promise<void>;
  refreshTools: () => Promise<void>;
  refreshProjects: () => Promise<void>;
  clearAppError: () => void;
  openHelp: () => void;
  closeHelp: () => void;
}

const AppContext = createContext<AppState | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [appError, setAppError] = useState<string | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);

  const setTranslatedError = useCallback((key: string) => {
    setAppError(i18n.t("common.loadFailed", { item: i18n.t(key) }));
  }, []);

  const refreshTools = useCallback(async () => {
    try {
      const t = await api.getToolStatus();
      setTools(t);
      setAppError(null);
    } catch (e) {
      console.error("Failed to load tools:", e);
      setTranslatedError("common.agents");
    }
  }, [setTranslatedError]);

  const refreshProjects = useCallback(async () => {
    try {
      const p = await api.getProjects();
      setProjects(p);
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }, []);

  const refreshAppData = useCallback(async () => {
    setLoading(true);
    await Promise.all([refreshTools(), refreshProjects()]);
    setLoading(false);
  }, [refreshProjects, refreshTools]);

  useEffect(() => {
    async function init() {
      // Both events log performance.now() (ms since timeOrigin) so the
      // reader can compute duration as done - start.
      api.logStartupEvent("refresh_app_data_start", performance.now()).catch(() => {});
      await refreshAppData();
      api.logStartupEvent("refresh_app_data_done", performance.now()).catch(() => {});
      const savedSize = await api.getSettings("text_size").catch(() => null);
      if (savedSize) {
        applyTextSize(savedSize);
      }
    }
    init();
  }, [refreshAppData]);

  useEffect(() => {
    let refreshTimer: ReturnType<typeof setTimeout> | null = null;

    const unlistenPromise = listen("app-files-changed", () => {
      if (refreshTimer) {
        clearTimeout(refreshTimer);
      }
      refreshTimer = setTimeout(() => {
        refreshAppData().catch((error) => {
          console.error("Failed to refresh after filesystem change:", error);
        });
      }, 500);
    });

    return () => {
      if (refreshTimer) {
        clearTimeout(refreshTimer);
      }
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          console.error("Failed to unlisten app-files-changed:", error);
        });
    };
  }, [refreshAppData]);

  return (
    <AppContext.Provider
      value={{
        tools,
        projects,
        loading,
        appError,
        helpOpen,
        refreshAppData,
        refreshTools,
        refreshProjects,
        clearAppError: () => setAppError(null),
        openHelp: () => setHelpOpen(true),
        closeHelp: () => setHelpOpen(false),
      }}
    >
      {children}
    </AppContext.Provider>
  );
}

export function useApp() {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp must be used within AppProvider");
  return ctx;
}
