import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

// The sidebar's health dots read the Doctor report the workbench publishes
// (#30). AppContext is stubbed to just the registry projects — its refresh
// machinery is not the subject here.
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

import { Sidebar } from "./Sidebar";
import { publishDoctorReport } from "../lib/doctorStore";
import type { ChainDoctorReport } from "../lib/tauri";

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

function renderSidebar() {
  return render(
    <MemoryRouter>
      <Sidebar />
    </MemoryRouter>,
  );
}

describe("Sidebar health dots", () => {
  beforeEach(() => {
    publishDoctorReport(null);
  });

  it("shows no dot before any Doctor report exists", () => {
    renderSidebar();
    expect(screen.queryByTestId("project-health")).toBeNull();
  });

  it("colors each project by its own findings once a report lands", () => {
    publishDoctorReport(REPORT);
    renderSidebar();

    const dots = screen.getAllByTestId("project-health");
    expect(dots).toHaveLength(2);
    // /proj has a violation finding; /other is green in the same report.
    expect(dots[0].getAttribute("data-state")).toBe("attention");
    expect(dots[0].className).toContain("bg-red-400");
    expect(dots[1].getAttribute("data-state")).toBe("green");
    expect(dots[1].className).toContain("bg-emerald-400");
  });
});
