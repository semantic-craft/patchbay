import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// Boundary under test: the Tauri invocation adapter. We mock `invoke` and let
// the real chain bindings + the Original Repositories view run on top of it.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { ChainWarehouse } from "./ChainWarehouse";
import type {
  ChainRepo,
  ChainTopology,
  ChainDuplicatesReport,
  ChainPullPlan,
} from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);

const DIRTY_REPO: ChainRepo = {
  name: "toolkit",
  path: "/wh/toolkit",
  source_kind: "checkout",
  root: "/wh",
  health: { dirty: true, state: "up_to_date", ahead: 0, behind: 0, branch: "main", error: null },
  origin: { name: "origin", url: "git@github.com:org/toolkit.git" },
  upstream: null,
  skills: [],
  referenced_by: [],
};

const TOPO: ChainTopology = {
  warehouse_roots: [{ root: "/wh", status: "ok", error: null, repo_count: 1 }],
  projects_root: "/Users/x/Projects",
  repos: [DIRTY_REPO],
  projects: [],
  guard: [],
  scanned_at: 0,
};

const NO_DUPES: ChainDuplicatesReport = { groups: [], scanned_at: 0 };

// A dirty repo is ineligible for a fast-forward: the plan skips it with reason.
const PULL_PLAN: ChainPullPlan = {
  items: [
    {
      path: "/wh/toolkit",
      name: "toolkit",
      branch: "main",
      upstream: "origin/main",
      ahead: 0,
      behind: 3,
      dirty: true,
      action: "skip",
      reason: "dirty",
    },
  ],
  scanned_at: 0,
};

describe("ChainWarehouse", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "chain_duplicate_checkouts":
          return Promise.resolve(NO_DUPES);
        case "chain_plan_pull":
          return Promise.resolve(PULL_PLAN);
        default:
          return Promise.resolve(undefined);
      }
    });
  });

  it("shows a dirty repository as skipped in the pull preview", async () => {
    render(<ChainWarehouse />);

    // Select the dirty repo, which reveals the pull action.
    const checkbox = await screen.findByLabelText("Select toolkit for pull");
    fireEvent.click(checkbox);

    fireEvent.click(screen.getByRole("button", { name: "Pull 1" }));
    expect(mockInvoke).toHaveBeenCalledWith("chain_plan_pull", { repoPaths: ["/wh/toolkit"] });

    // The preview classifies the dirty repo as a skip, and offers nothing to
    // fast-forward — its per-repo reason is surfaced, never silently dropped.
    await screen.findByText("Fast-forward preview");
    expect(screen.getByText("dirty")).toBeDefined();
    expect(
      screen.getByText("No selected repository is eligible for a fast-forward."),
    ).toBeDefined();
    // Apply stays disabled: nothing is eligible.
    const apply = screen.getByRole("button", { name: /Fast-forward 0/ });
    expect(apply).toHaveProperty("disabled", true);
  });

  it("shows Patchbay Central as managed and never offers raw Git actions", async () => {
    const central: ChainRepo = {
      ...DIRTY_REPO,
      name: "Patchbay Central",
      path: "/central",
      root: "/central",
      source_kind: "managed",
      health: { dirty: false, state: "up_to_date", ahead: 0, behind: 0, branch: null, error: null },
      origin: null,
    };
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "chain_get_topology") return Promise.resolve({ ...TOPO, repos: [central] });
      if (cmd === "chain_duplicate_checkouts") return Promise.resolve(NO_DUPES);
      return Promise.resolve(undefined);
    });

    render(<ChainWarehouse />);
    await screen.findByText("Patchbay Central");
    expect(screen.getByText("managed default")).toBeDefined();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByText("origin")).toBeNull();
  });
});
