import { BrowserRouter, Navigate, Routes, Route, useLocation } from "react-router-dom";
import { Toaster } from "sonner";
import { AppProvider } from "./context/AppContext";
import { ThemeProvider, useThemeContext } from "./context/ThemeContext";
import { HelpDialog } from "./components/HelpDialog";
import { CloseActionGuard } from "./components/CloseActionGuard";
import { FirstRunRestoreDialog } from "./components/FirstRunRestoreDialog";
import { AppUpdateNotifier } from "./components/AppUpdateNotifier";
import { Layout } from "./components/Layout";
import { MySkills } from "./views/MySkills";
import { WorkspaceView } from "./views/WorkspaceView";
import { LOBSTER_WORKSPACE_CONFIG } from "./views/workspaceConfigs";
import { InstallSkills } from "./views/InstallSkills";
import { Settings } from "./views/Settings";
import { ProjectDetail } from "./views/ProjectDetail";
import { Backup } from "./views/Backup";
import { ChainOverview } from "./views/ChainOverview";
import { ChainProjects } from "./views/ChainProjects";
import { ChainWarehouse } from "./views/ChainWarehouse";
import { ChainDoctor } from "./views/ChainDoctor";
import { Fleet } from "./views/Fleet";

function ThemedToaster() {
  const { resolvedTheme } = useThemeContext();
  return (
    <Toaster
      theme={resolvedTheme}
      position="bottom-right"
      toastOptions={{
        style: {
          background: "var(--color-surface)",
          border: "1px solid var(--color-border)",
          color: "var(--color-text-primary)",
        },
      }}
    />
  );
}

/**
 * Legacy Project Links URL. The workbench moved to "/"; keep the query string
 * so `?project=` deep links keep selecting the same project.
 */
function LegacyProjectsRedirect() {
  const location = useLocation();
  return <Navigate to={{ pathname: "/", search: location.search }} replace />;
}

export function AppRoutes() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route path="/" element={<ChainProjects />} />
        <Route path="/my-skills" element={<MySkills />} />
        <Route path="/global-workspace" element={<Navigate to="/chain/doctor" replace />} />
        <Route path="/global-workspace/:agentKey" element={<Navigate to="/chain/doctor" replace />} />
        <Route path="/lobster-workspace" element={<WorkspaceView config={LOBSTER_WORKSPACE_CONFIG} />} />
        <Route path="/lobster-workspace/:agentKey" element={<WorkspaceView config={LOBSTER_WORKSPACE_CONFIG} />} />
        <Route path="/install" element={<InstallSkills />} />
        <Route path="/chain" element={<Navigate to="/" replace />} />
        <Route path="/chain/overview" element={<ChainOverview />} />
        <Route path="/chain/projects" element={<LegacyProjectsRedirect />} />
        <Route path="/chain/warehouse" element={<ChainWarehouse />} />
        <Route path="/chain/doctor" element={<ChainDoctor />} />
        <Route path="/fleet" element={<Fleet />} />
        <Route path="/backup" element={<Backup />} />
        <Route path="/project/:id" element={<ProjectDetail />} />
        <Route path="/settings" element={<Settings />} />
      </Route>
    </Routes>
  );
}

function App() {
  return (
    <ThemeProvider>
      <AppProvider>
        <BrowserRouter>
          <AppRoutes />
          <HelpDialog />
          <CloseActionGuard />
          <FirstRunRestoreDialog />
        </BrowserRouter>
        <ThemedToaster />
        <AppUpdateNotifier />
      </AppProvider>
    </ThemeProvider>
  );
}

export default App;
