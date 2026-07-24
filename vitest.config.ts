import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Minimal test runner for the GUI flows: jsdom + React Testing Library, driving
// components through the real Tauri invocation adapter (with `invoke` mocked).
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
