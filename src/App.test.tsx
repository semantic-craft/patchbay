import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type {
  ChainProject,
  ChainRepo,
  ChainTopology,
  ChainTracedEntry,
  InstructionsScanReport,
} from "./lib/tauri";

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

const INSTRUCTIONS: InstructionsScanReport = {
  projects: [],
  globals: [{ path: "/global/AGENTS.md", bytes: 40, est_tokens: 10, readers: ["codex"] }],
  agents: ["codex"],
  scanned_at: 0,
};

let topology = TOPO;
let instructions: InstructionsScanReport | null = null;

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
      return Promise.resolve(topology);
    case "chain_doctor_report":
      return Promise.reject(new Error("doctor offline"));
    case "chain_duplicate_checkouts":
      return Promise.resolve({ groups: [], scanned_at: 0 });
    case "instructions_scan":
      return instructions
        ? Promise.resolve(instructions)
        : Promise.reject(new Error("scan offline"));
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
const { ChainProvider } = await import("./context/ChainContext");

function renderApp(initialEntry: string) {
  return render(
    <AppProvider>
      <ChainProvider>
        <MemoryRouter initialEntries={[initialEntry]}>
          <AppRoutes />
        </MemoryRouter>
      </ChainProvider>
    </AppProvider>,
  );
}

beforeEach(() => {
  mockInvoke.mockClear();
  topology = TOPO;
  instructions = null;
});

describe("AppRoutes", () => {
  it("boots straight into the selected project's workbench", async () => {
    renderApp("/");
    // The header names the project, not the screen — the workbench IS "/".
    expect(await screen.findByRole("heading", { name: "proj" })).toBeTruthy();
    // First registered project is selected by default.
    expect(await screen.findByText("alpha-skill")).toBeTruthy();
    // The island header carries the workbench's four sections; the sidebar
    // footer keeps only destinations outside that workbench.
    expect(screen.getByRole("link", { name: "Workbench" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Chain" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Doctor" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Sources" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Fleet" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "Settings" })).toBeTruthy();
  });

  it("keeps the global-surface guard visible on every route", async () => {
    renderApp("/fleet");
    expect(await screen.findByTestId("chain-status-bar")).toBeTruthy();
  });

  it("routes to the low-frequency areas from the island tabs", async () => {
    renderApp("/");
    await screen.findByRole("heading", { name: "proj" });

    fireEvent.click(screen.getByRole("link", { name: "Sources" }));
    expect(await screen.findByRole("heading", { name: "Skill Sources" })).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "Doctor" }));
    expect(await screen.findByRole("heading", { name: "Diagnostics" })).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "Chain" }));
    expect(await screen.findByRole("heading", { name: "Link Topology" })).toBeTruthy();
  });

  it("honours a ?project= deep link on the main screen", async () => {
    renderApp("/?project=%2Fproj2");
    // The query selects the second project, not the default first.
    expect(await screen.findByText("beta-skill")).toBeTruthy();
    expect(screen.queryByText("alpha-skill")).toBeNull();
  });

  it("keeps canonical node identities and graph-width wires for duplicate names", async () => {
    const repo = (path: string): ChainRepo => ({
      name: "repo",
      path,
      source_kind: "checkout",
      root: path.split("/").slice(0, -1).join("/"),
      health: {
        dirty: false,
        state: "up_to_date",
        ahead: 0,
        behind: 0,
        branch: "main",
        error: null,
      },
      origin: null,
      upstream: null,
      skills: [],
      referenced_by: [],
    });
    const project = (path: string, repoPath: string, dirLinkOk: boolean): ChainProject => ({
      name: "duplicate",
      path,
      agents_dir: {
        path: `${path}/.agents/skills`,
        entries: [
          {
            ...entry("skill", path),
            final_target: `${repoPath}/skills/skill`,
          },
        ],
      },
      surfaces: [
        {
          agent: "codex",
          path: `${path}/.codex/skills`,
          kind: "dir_link",
          dir_link_target: `${path}/.agents/skills`,
          dir_link_ok: dirLinkOk,
          entries: [],
        },
      ],
    });
    topology = {
      ...TOPO,
      repos: [repo("/wh/a/repo"), repo("/wh/b/repo")],
      projects: [
        project("/projects/a/duplicate", "/wh/a/repo", false),
        project("/projects/b/duplicate", "/wh/b/repo", true),
      ],
    };

    renderApp("/chain");
    await screen.findByRole("heading", { name: "Link Topology" });

    const graph = screen.getByTestId("chain-graph");
    const ids = [...graph.querySelectorAll("[data-node-id]")].map((node) =>
      node.getAttribute("data-node-id"),
    );
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual(
      expect.arrayContaining([
        "repo:/wh/a/repo",
        "repo:/wh/b/repo",
        "agg:/projects/a/duplicate",
        "agg:/projects/b/duplicate",
        "surf:/projects/a/duplicate:codex",
        "surf:/projects/b/duplicate:codex",
      ]),
    );

    const edges = [...graph.querySelectorAll("[data-edge-from]")];
    expect(
      edges.map((edge) => [
        edge.getAttribute("data-edge-from"),
        edge.getAttribute("data-edge-to"),
      ]),
    ).toEqual(
      expect.arrayContaining([
        ["repo:/wh/a/repo", "agg:/projects/a/duplicate"],
        ["repo:/wh/b/repo", "agg:/projects/b/duplicate"],
      ]),
    );
    expect(screen.getAllByText("dir link broken")).toHaveLength(2);
    expect(graph.classList.contains("min-w-max")).toBe(true);
  });

  it("preserves the selected project when the cost bar opens the workbench", async () => {
    instructions = INSTRUCTIONS;
    renderApp("/chain?project=%2Fproj2");
    await screen.findByRole("heading", { name: "Link Topology" });

    fireEvent.click(await screen.findByRole("button", { name: /codex/ }));

    expect(await screen.findByText("beta-skill")).toBeTruthy();
    expect(screen.queryByText("alpha-skill")).toBeNull();
  });
});
