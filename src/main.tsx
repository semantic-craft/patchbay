import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { i18nReady } from "./i18n";
import { logStartupEvent } from "./lib/tauri";
import { applyPlatformAttribute } from "./lib/platform";
import { applyWindowGlassAttribute } from "./lib/windowGlass";
import "./index.css";
import App from "./App.tsx";

// Synchronous and before first paint: the titlebar offsets derive from it, so
// deferring would flash 28px of dead space on Windows.
applyPlatformAttribute();
void applyWindowGlassAttribute();

await i18nReady;
logStartupEvent("i18n_ready", performance.now()).catch(() => {});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
logStartupEvent("root_rendered", performance.now()).catch(() => {});
