import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";

// The sidebar's health dots read the Doctor report from the shared chain scan.
// AppContext is stubbed to just the registry projects — its refresh machinery
// is not the subject here.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));
vi.mock("../context/AppContext", () => ({
  useApp: () => ({
    projects: [
      { id: "1", name: "proj", path: "/proj" },
      { id: "2", name: "other", path: "/other" },
    ],
    refreshProjects: vi.fn(),
  }),
}));

import { invoke } from "@tauri-apps/api/core";
import { Sidebar } from "./Sidebar";
import { renderWithChain } from "../test/renderWithChain";
import type { ChainDoctorReport } from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);

const REPORT: ChainDoctorReport = {
  findings: [
    {
      rule: "chain.broken_link",
      deviation: "broken",
      severity: "violation",
      evidence: {
        entry_path: "/proj/.claude/skills/alpha",
        hops: [],
        final_target: "/wh/repo/skills/alpha",
        topology_status: "broken",
      },
      affected: [
        { kind: "skill", name: "alpha", path: "/proj/.claude/skills/alpha" },
        { kind: "project", name: "proj", path: "/proj" },
      ],
      actions: ["repair"],
      fingerprint: "fp-1",
    },
  ],
  ignored: [],
  total: 1,
  scanned_at: 0,
};

function renderSidebar(report: ChainDoctorReport | null) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "chain_get_topology") {
      return Promise.resolve({
        warehouse_roots: [],
        projects_root: "/Users/x/Projects",
        repos: [],
        projects: [],
        guard: [],
        scanned_at: 0,
      });
    }
    if (cmd === "chain_doctor_report") {
      return report ? Promise.resolve(report) : Promise.reject(new Error("unavailable"));
    }
    return Promise.resolve(undefined);
  });
  return renderWithChain(<Sidebar />);
}

describe("Sidebar health dots", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("shows no dot while no Doctor report exists", async () => {
    renderSidebar(null);
    await waitFor(() => expect(screen.getByText("proj")).toBeTruthy());
    expect(screen.queryByTestId("project-health")).toBeNull();
  });

  it("colors each project by its own findings once a report lands", async () => {
    renderSidebar(REPORT);

    const dots = await screen.findAllByTestId("project-health");
    expect(dots).toHaveLength(2);
    // /proj has a violation finding; /other is green in the same report.
    expect(dots[0].getAttribute("data-state")).toBe("attention");
    expect(dots[0].className).toContain("bg-red-400");
    expect(dots[1].getAttribute("data-state")).toBe("green");
    expect(dots[1].className).toContain("bg-emerald-400");
  });
});
