import { BrowserRouter, Routes, Route } from "react-router-dom";
import { Toaster } from "sonner";
import { AppProvider } from "./context/AppContext";
import { ChainProvider } from "./context/ChainContext";
import { ThemeProvider, useThemeContext } from "./context/ThemeContext";
import { HelpDialog } from "./components/HelpDialog";
import { CloseActionGuard } from "./components/CloseActionGuard";
import { AppUpdateNotifier } from "./components/AppUpdateNotifier";
import { Layout } from "./components/Layout";
import { ChainProjects } from "./views/ChainProjects";
import { ChainDoctor } from "./views/ChainDoctor";
import { ChainWarehouse } from "./views/ChainWarehouse";
import { Settings } from "./views/Settings";
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

export function AppRoutes() {
  return (
    <Routes>
      <Route element={<Layout />}>
        {/* The workbench IS the main screen — one job, no shell around it. */}
        <Route path="/" element={<ChainProjects />} />
        <Route path="/doctor" element={<ChainDoctor />} />
        <Route path="/sources" element={<ChainWarehouse />} />
        <Route path="/fleet" element={<Fleet />} />
        <Route path="/settings" element={<Settings />} />
      </Route>
    </Routes>
  );
}

function App() {
  return (
    <ThemeProvider>
      <AppProvider>
        <ChainProvider>
          <BrowserRouter>
            <AppRoutes />
            <HelpDialog />
            <CloseActionGuard />
          </BrowserRouter>
          <ThemedToaster />
          <AppUpdateNotifier />
        </ChainProvider>
      </AppProvider>
    </ThemeProvider>
  );
}

export default App;
