import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ChainProject, ChainTopology, ChainTracedEntry } from "./lib/tauri";

// Boundary under test: the Tauri invocation adapter. We mock `invoke` and let
// the real route table, Layout, Sidebar, and chain bindings run on top of it.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));

import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

function entry(name: string, projectPath: string): ChainTracedEntry {
  return {
    name,
    entry_path: `${projectPath}/.agents/skills/${name}`,
    hops: [],
    final_target: `/wh/repo/skills/${name}`,
    status: "link_repo",
    repo: "repo",
  };
}

const PROJ_A: ChainProject = {
  name: "proj",
  path: "/proj",
  agents_dir: { path: "/proj/.agents/skills", entries: [entry("alpha-skill", "/proj")] },
  surfaces: [],
};

const PROJ_B: ChainProject = {
  name: "beta-proj",
  path: "/proj2",
  agents_dir: { path: "/proj2/.agents/skills", entries: [entry("beta-skill", "/proj2")] },
  surfaces: [],
};

const TOPO: ChainTopology = {
  warehouse_roots: [{ root: "/wh", status: "ok", error: null, repo_count: 1 }],
  projects_root: "/Users/x/Projects",
  repos: [],
  projects: [PROJ_A, PROJ_B],
  guard: [],
  scanned_at: 0,
};

// These tests identify the selected project by the entries in its link list,
// which the workbench renders in full only when Doctor is unreachable (#30
// collapses it in both the green and the attention state). Keep Doctor
// offline so routing stays the subject here; the workbench states are
// covered in ChainProjects.test.tsx.

function dispatch(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "get_presets":
      return Promise.resolve([]);
    case "get_active_preset":
      return Promise.resolve(null);
    case "get_tool_status":
      return Promise.resolve([]);
    case "get_managed_skills":
      return Promise.resolve([]);
    case "get_projects":
      return Promise.resolve([]);
    case "get_settings":
      return Promise.resolve(args && args.key === "language" ? "en" : null);
    case "log_startup_event":
      return Promise.resolve(undefined);
    case "chain_get_topology":
      return Promise.resolve(TOPO);
    case "chain_doctor_report":
      return Promise.reject(new Error("doctor offline"));
    case "chain_duplicate_checkouts":
      return Promise.resolve({ groups: [], scanned_at: 0 });
    case "instructions_scan":
      // The workbench tolerates a failing instructions scan; keep the fixture
      // minimal by not modelling the report at all.
      return Promise.reject(new Error("scan offline"));
    default:
      return Promise.reject(new Error(`unmocked command: ${cmd}`));
  }
}

// This environment's global localStorage is Node's non-functional stub; the
// i18n module and AppContext read it at import/mount time, so substitute a
// working in-memory Storage before importing the app modules.
const store = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
});

// The app i18n module resolves its language at import time via
// get_settings("language"); the dispatcher pins it to English before the
// route table is imported.
mockInvoke.mockImplementation(dispatch as never);

const { AppRoutes } = await import("./App");
const { AppProvider } = await import("./context/AppContext");

function renderApp(initialEntry: string) {
  return render(
    <AppProvider>
      <MemoryRouter initialEntries={[initialEntry]}>
        <AppRoutes />
      </MemoryRouter>
    </AppProvider>,
  );
}

beforeEach(() => {
  mockInvoke.mockClear();
});

describe("AppRoutes", () => {
  it("boots to the workbench at / with the views section in the sidebar", async () => {
    renderApp("/");
    expect(await screen.findByRole("heading", { name: "Project Links" })).toBeTruthy();
    // First registered project is selected by default.
    expect(await screen.findByText("alpha-skill")).toBeTruthy();
    // Sidebar views section.
    expect(screen.getByText("Views")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Workbench" })).toBeTruthy();
  });

  it("navigates to topology and development sources from the sidebar", async () => {
    renderApp("/");
    await screen.findByRole("heading", { name: "Project Links" });

    fireEvent.click(screen.getByRole("link", { name: "Topology" }));
    expect(await screen.findByRole("heading", { name: "Link Topology" })).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "Development Sources" }));
    expect(await screen.findByRole("heading", { name: "Skill Sources" })).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "Workbench" }));
    expect(await screen.findByRole("heading", { name: "Project Links" })).toBeTruthy();
  });

  it("redirects legacy /chain/projects deep links to the workbench, keeping ?project=", async () => {
    renderApp("/chain/projects?project=%2Fproj2");
    expect(await screen.findByRole("heading", { name: "Project Links" })).toBeTruthy();
    // The preserved query selects the second project, not the default first.
    expect(await screen.findByText("beta-skill")).toBeTruthy();
    expect(screen.queryByText("alpha-skill")).toBeNull();
  });
});
