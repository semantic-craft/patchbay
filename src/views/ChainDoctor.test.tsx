import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";

// Boundary under test: the Tauri invocation adapter. We mock `invoke` and let
// the real `chainDoctorReport` binding + the Doctor view run on top of it.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { ChainDoctor } from "./ChainDoctor";
import type {
  ChainDoctorReport,
  ChainFinding,
  ChainSeverity,
  ChainDeviation,
  ChainRepairPlan,
  InstructionsDoctorReport,
  InstructionsFinding,
  InstructionsRule,
} from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);

function mkFinding(
  rule: string,
  deviation: ChainDeviation,
  severity: ChainSeverity,
  entryPath: string,
  finalTarget: string,
): ChainFinding {
  const name = entryPath.split("/").pop() ?? entryPath;
  return {
    rule,
    deviation,
    severity,
    evidence: {
      entry_path: entryPath,
      hops: finalTarget === entryPath ? [] : [finalTarget],
      final_target: finalTarget,
      topology_status: deviation,
    },
    affected: [{ kind: "skill", name, path: entryPath }],
    actions: ["repair"],
    fingerprint: `fp-${entryPath}`,
  };
}

const POPULATED: ChainDoctorReport = {
  total: 5,
  scanned_at: 0,
  ignored: [],
  findings: [
    mkFinding("chain.broken_link", "broken", "violation", "/p/.claude/skills/dead", "/nowhere"),
    mkFinding("chain.broken_link", "broken", "violation", "/p/.codex/skills/dead2", "/gone"),
    mkFinding("chain.direct_link", "direct", "advice", "/p/.claude/skills/direct-one", "/wh/repo/skills/direct-one"),
    mkFinding("chain.unmanaged_copy", "copy", "warning", "/p/.claude/skills/copy-one", "/p/.claude/skills/copy-one"),
    mkFinding("chain.orphan_original", "orphan", "notice", "/wh/lonely", "/wh/lonely"),
  ],
};

function mkInstructionsFinding(
  rule: InstructionsRule,
  severity: ChainSeverity,
  primaryPath: string,
  counterpartPath?: string,
): InstructionsFinding {
  return {
    rule,
    severity,
    evidence: {
      primary_path: primaryPath,
      ...(counterpartPath ? { counterpart_path: counterpartPath } : {}),
      metrics: { bytes: 8193, readers: ["claude", "codex"] },
      locations: [{ path: primaryPath, line: 12 }],
    },
    affected: [{ kind: "canonical", name: "AGENTS.md", path: primaryPath }],
    actions: rule === "instructions.broken_import" ? ["init"] : ["normalize"],
    fingerprint: `ifp-${rule}`,
  };
}

const INSTRUCTIONS_POPULATED: InstructionsDoctorReport = {
  total: 2,
  scanned_at: 1,
  ignored: [],
  findings: [
    mkInstructionsFinding(
      "instructions.broken_import",
      "violation",
      "/p/CLAUDE.md",
      "/p/AGENTS.md",
    ),
    mkInstructionsFinding("instructions.missing_entry", "warning", "/p/AGENTS.md"),
  ],
};

const INSTRUCTIONS_EMPTY: InstructionsDoctorReport = {
  total: 0,
  scanned_at: 0,
  ignored: [],
  findings: [],
};

function mockDoctorReports(
  chain: ChainDoctorReport = POPULATED,
  instructions: InstructionsDoctorReport = INSTRUCTIONS_EMPTY,
) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "chain_doctor_report") return Promise.resolve(chain);
    if (cmd === "instructions_doctor_report") return Promise.resolve(instructions);
    return Promise.resolve(undefined);
  });
}

function deviations(): string[] {
  return screen
    .getAllByTestId("finding")
    .map((row) => row.getAttribute("data-deviation") ?? "");
}

describe("ChainDoctor", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockDoctorReports();
  });

  it("renders a populated report and filters by type and severity", async () => {
    render(<ChainDoctor />);

    // All findings render once the read-only report resolves.
    const rows = await screen.findAllByTestId("finding");
    expect(rows).toHaveLength(5);

    // Filter by deviation type: only the two broken links remain.
    fireEvent.click(screen.getByTestId("dev-broken"));
    expect(deviations()).toEqual(["broken", "broken"]);

    // Toggling the same chip off restores the full set.
    fireEvent.click(screen.getByTestId("dev-broken"));
    expect(screen.getAllByTestId("finding")).toHaveLength(5);

    // Filter by severity: the two violations are exactly the broken links.
    fireEvent.click(screen.getByTestId("sev-violation"));
    expect(deviations()).toEqual(["broken", "broken"]);
  });

  it("merges chain deviations with instructions rules under shared severity filters", async () => {
    mockDoctorReports(POPULATED, INSTRUCTIONS_POPULATED);
    render(<ChainDoctor />);

    expect(await screen.findAllByTestId("finding")).toHaveLength(7);
    expect(mockInvoke).toHaveBeenCalledWith("instructions_doctor_report", {
      filter: null,
      project: null,
    });
    expect(screen.getAllByTestId(/^dev-/)).toHaveLength(6);
    expect(screen.getAllByTestId(/^rule-/)).toHaveLength(14);

    // An instructions rule chip is part of the shared type axis: selecting it
    // hides every chain deviation and every other instructions rule.
    fireEvent.click(screen.getByTestId("rule-broken_import"));
    let rows = screen.getAllByTestId("finding");
    expect(rows).toHaveLength(1);
    expect(rows[0].getAttribute("data-module")).toBe("instructions");
    expect(rows[0].getAttribute("data-rule")).toBe("instructions.broken_import");

    // Severity is shared across modules: two broken chain findings plus one
    // broken-import instructions finding are all violations.
    fireEvent.click(screen.getByTestId("rule-broken_import"));
    fireEvent.click(screen.getByTestId("sev-violation"));
    rows = screen.getAllByTestId("finding");
    expect(rows).toHaveLength(3);
    expect(rows.map((row) => row.getAttribute("data-module"))).toEqual([
      "chain",
      "chain",
      "instructions",
    ]);
  });

  it("renders instructions paths, metrics, and locations in the shared finding row", async () => {
    mockDoctorReports(POPULATED, INSTRUCTIONS_POPULATED);
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const row = screen
      .getAllByTestId("finding")
      .find((candidate) => candidate.getAttribute("data-rule") === "instructions.broken_import");
    expect(row).toBeDefined();

    fireEvent.click(within(row!).getByRole("button"));
    const evidence = within(row!).getByTestId("instructions-evidence");
    expect(evidence.textContent).toContain("/p/CLAUDE.md");
    expect(evidence.textContent).toContain("/p/AGENTS.md");
    expect(evidence.textContent).toContain("bytes=8193");
    expect(evidence.textContent).toContain("/p/CLAUDE.md:12");
  });

  it("reveals the same chain evidence when a finding is opened", async () => {
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const directRow = screen
      .getAllByTestId("finding")
      .find((row) => row.getAttribute("data-deviation") === "direct");
    expect(directRow).toBeDefined();

    fireEvent.click(within(directRow!).getByRole("button"));

    const evidence = within(directRow!).getByTestId("evidence");
    expect(evidence.textContent).toContain("/wh/repo/skills/direct-one");
    expect(evidence.textContent).toContain("chain.direct_link");
  });

  it("inspects a violation finding and shows its chain evidence", async () => {
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    // A violation is the highest-severity finding class; open the first one and
    // confirm its evidence (entry, rule, and resolved target) is on screen.
    const violationRow = screen
      .getAllByTestId("finding")
      .find((row) => row.getAttribute("data-severity") === "violation");
    expect(violationRow).toBeDefined();
    expect(violationRow!.getAttribute("data-deviation")).toBe("broken");

    fireEvent.click(within(violationRow!).getByRole("button"));

    const evidence = within(violationRow!).getByTestId("evidence");
    expect(evidence.textContent).toContain("/p/.claude/skills/dead");
    expect(evidence.textContent).toContain("/nowhere");
    expect(evidence.textContent).toContain("chain.broken_link");
  });

  it("previews a normalization repair for a repairable finding", async () => {
    const plan: ChainRepairPlan = {
      items: [
        {
          fingerprint: "fp-/p/.claude/skills/direct-one",
          rule: "chain.direct_link",
          deviation: "direct",
          project: "/p",
          path: "/p/.agents/skills/direct-one",
          kind: "repoint_entry",
          action: "repoint",
          old_target: "/wh/repo/skills/direct-one",
          new_target: "/p/.agents/skills/direct-one",
          message: null,
        },
      ],
      evidence: {},
      snapshot: [],
      unsupported: [],
      scanned_at: 0,
    };
    // The report load returns POPULATED; only the repair preview returns a plan.
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "chain_plan_repair") return Promise.resolve(plan);
      if (cmd === "instructions_doctor_report") return Promise.resolve(INSTRUCTIONS_EMPTY);
      return Promise.resolve(POPULATED);
    });

    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const directRow = screen
      .getAllByTestId("finding")
      .find((row) => row.getAttribute("data-deviation") === "direct");
    fireEvent.click(within(directRow!).getByRole("button"));
    fireEvent.click(within(directRow!).getByTestId("repair"));

    // Repair previews by fingerprint via chain_plan_repair (never applies yet).
    expect(mockInvoke).toHaveBeenCalledWith("chain_plan_repair", {
      fingerprints: ["fp-/p/.claude/skills/direct-one"],
    });

    // The previewed edits render, and Apply is offered but not yet invoked.
    const items = await within(directRow!).findByTestId("repair-items");
    expect(items.textContent).toContain("/p/.agents/skills/direct-one");
    expect(within(directRow!).getByTestId("repair-confirm")).toBeDefined();
    expect(mockInvoke).not.toHaveBeenCalledWith("chain_apply_repair", expect.anything());
  });

  it("shows a clean state when the report has no findings", async () => {
    const emptyChain = { findings: [], ignored: [], total: 0, scanned_at: 0 };
    mockDoctorReports(emptyChain, INSTRUCTIONS_EMPTY);
    render(<ChainDoctor />);

    await screen.findByTestId("doctor-clean");
    expect(screen.queryAllByTestId("finding")).toHaveLength(0);
    expect(screen.queryByTestId("sev-violation")).toBeNull();
    expect(screen.queryByTestId("ignored-section")).toBeNull();
  });

  it("ignores a finding from its expanded panel", async () => {
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const directRow = screen
      .getAllByTestId("finding")
      .find((row) => row.getAttribute("data-deviation") === "direct");
    fireEvent.click(within(directRow!).getByRole("button"));
    fireEvent.click(within(directRow!).getByTestId("ignore"));

    // The generic accept is persisted with kind "ignored" and a null note.
    expect(mockInvoke).toHaveBeenCalledWith("chain_ignore_finding", {
      rule: "chain.direct_link",
      fingerprint: "fp-/p/.claude/skills/direct-one",
      kind: "ignored",
      note: null,
    });
  });

  it("routes instructions ignore through the instructions decision binding", async () => {
    mockDoctorReports(POPULATED, INSTRUCTIONS_POPULATED);
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const row = screen
      .getAllByTestId("finding")
      .find((candidate) => candidate.getAttribute("data-rule") === "instructions.broken_import");
    fireEvent.click(within(row!).getByRole("button"));
    fireEvent.click(within(row!).getByTestId("ignore"));

    expect(mockInvoke).toHaveBeenCalledWith("instructions_ignore_finding", {
      rule: "instructions.broken_import",
      fingerprint: "ifp-instructions.broken_import",
      note: null,
    });
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "chain_ignore_finding",
      expect.objectContaining({ rule: "instructions.broken_import" }),
    );
  });

  it("classifies a copy finding as project-private", async () => {
    render(<ChainDoctor />);
    await screen.findAllByTestId("finding");

    const copyRow = screen
      .getAllByTestId("finding")
      .find((row) => row.getAttribute("data-deviation") === "copy");
    fireEvent.click(within(copyRow!).getByRole("button"));
    // A copy finding offers the "Mark project-private" classification…
    fireEvent.click(within(copyRow!).getByTestId("mark-private"));
    expect(mockInvoke).toHaveBeenCalledWith("chain_ignore_finding", {
      rule: "chain.unmanaged_copy",
      fingerprint: "fp-/p/.claude/skills/copy-one",
      kind: "project_private",
      note: null,
    });
  });

  it("lists ignored findings and restores them", async () => {
    const withIgnored: ChainDoctorReport = {
      total: 1,
      scanned_at: 0,
      findings: [
        mkFinding("chain.direct_link", "direct", "advice", "/p/.claude/skills/keep", "/wh/repo/skills/keep"),
      ],
      ignored: [
        mkFinding("chain.unmanaged_copy", "copy", "warning", "/p/.claude/skills/old", "/p/.claude/skills/old"),
      ],
    };
    mockDoctorReports(withIgnored, INSTRUCTIONS_EMPTY);

    render(<ChainDoctor />);
    await screen.findByTestId("ignored-section");
    expect(screen.getAllByTestId("ignored-finding")).toHaveLength(1);

    fireEvent.click(screen.getByTestId("restore"));
    expect(mockInvoke).toHaveBeenCalledWith("chain_restore_finding", {
      rule: "chain.unmanaged_copy",
      fingerprint: "fp-/p/.claude/skills/old",
    });
  });

  it("restores instructions decisions without crossing into chain storage", async () => {
    const ignoredInstructions: InstructionsDoctorReport = {
      total: 0,
      scanned_at: 0,
      findings: [],
      ignored: [
        mkInstructionsFinding(
          "instructions.missing_entry",
          "warning",
          "/p/AGENTS.md",
        ),
      ],
    };
    mockDoctorReports(
      { findings: [], ignored: [], total: 0, scanned_at: 0 },
      ignoredInstructions,
    );

    render(<ChainDoctor />);
    const row = await screen.findByTestId("ignored-finding");
    expect(row.getAttribute("data-module")).toBe("instructions");
    fireEvent.click(within(row).getByTestId("restore"));

    expect(mockInvoke).toHaveBeenCalledWith("instructions_restore_finding", {
      rule: "instructions.missing_entry",
      fingerprint: "ifp-instructions.missing_entry",
    });
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "chain_restore_finding",
      expect.objectContaining({ rule: "instructions.missing_entry" }),
    );
  });
});
