import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";

// Boundary under test: the Tauri invocation adapter. The shared ChainProvider
// runs the real scan bindings on top of the mocked `invoke`.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));

import { invoke } from "@tauri-apps/api/core";
import { ChainStatusBar } from "./ChainStatusBar";
import { renderWithChain } from "../test/renderWithChain";
import type { ChainTopology } from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);

function topoWithGuard(guard: ChainTopology["guard"]): ChainTopology {
  return {
    warehouse_roots: [],
    projects_root: "/Users/x/Projects",
    repos: [],
    projects: [],
    guard,
    scanned_at: Date.now(),
  };
}

function mockScan(topology: ChainTopology | Error) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "chain_get_topology") {
      return topology instanceof Error
        ? Promise.reject(topology)
        : Promise.resolve(topology);
    }
    return Promise.resolve(undefined);
  });
}

describe("ChainStatusBar guard verdict", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("reports the guard as unverified when the scan fails", async () => {
    // "Clean" is a verdict about surfaces that were actually checked. With no
    // topology at all, the bar must say unknown — not render the green shield.
    mockScan(new Error("scan exploded"));
    renderWithChain(<ChainStatusBar />);

    await waitFor(() =>
      expect(screen.getByTestId("chain-status-bar").getAttribute("data-guard")).toBe("unknown"),
    );
    // The failure itself is named here, on every route — not only in the
    // workbench's own error panel.
    expect(screen.getByTestId("chain-scan-error").textContent).toContain("scan exploded");
  });

  it("declares clean only after a scan observes empty surfaces", async () => {
    mockScan(
      topoWithGuard([
        { agent: "Claude Code", path: "/u/.claude/skills", state: "empty", violations: [] },
      ]),
    );
    renderWithChain(<ChainStatusBar />);

    await waitFor(() =>
      expect(screen.getByTestId("chain-status-bar").getAttribute("data-guard")).toBe("clean"),
    );
  });

  it("raises a violation with its remediable skill", async () => {
    mockScan(
      topoWithGuard([
        {
          agent: "Codex",
          path: "/u/.codex/skills",
          state: "violation",
          violations: [
            {
              skill: "stray",
              path: "/u/.codex/skills/stray",
              final_target: "/u/.codex/skills/stray",
              is_link: false,
            },
          ],
        },
      ]),
    );
    renderWithChain(<ChainStatusBar />);

    await waitFor(() =>
      expect(screen.getByTestId("chain-status-bar").getAttribute("data-guard")).toBe("violation"),
    );
    expect(screen.getByTestId("guard-violation").textContent).toBe("stray");
  });
});
