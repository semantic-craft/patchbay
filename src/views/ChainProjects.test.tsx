import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

// Boundary under test: the Tauri invocation adapter. We mock `invoke` and let
// the real chain bindings + the Project Links view run on top of it. The folder
// picker and toast surface are stubbed — neither is the subject here. The
// event channel (`chain-repair-live`, #32) is mocked as a captured handler
// list so tests can inject scripted step sequences.
const { liveListeners } = vi.hoisted(() => ({
  liveListeners: [] as Array<(event: { payload: unknown }) => void>,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((_name: string, cb: (event: { payload: unknown }) => void) => {
    liveListeners.push(cb);
    return Promise.resolve(() => {});
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn(() => Promise.resolve()) }));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}));

import { invoke } from "@tauri-apps/api/core";
import { Route, Routes } from "react-router-dom";
import { ChainProjects } from "./ChainProjects";
import type {
  ChainApplyOutcome,
  ChainCandidatesReport,
  ChainDoctorReport,
  ChainFinding,
  ChainJournalRecord,
  ChainLinkPlan,
  ChainLiveEvent,
  ChainPreset,
  ChainProject,
  ChainRepairOutcome,
  ChainRepairPlan,
  ChainRepo,
  ChainTopology,
  ChainTracedEntry,
  ChainUndoOutcome,
  ChainUnlinkPlan,
  ChainUnlinkOutcome,
} from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);

/** Deliver one live step event to every registered listener, as the Tauri
 * event channel would. Components filter by run_id themselves. */
function emitLive(payload: ChainLiveEvent) {
  act(() => {
    for (const listener of [...liveListeners]) listener({ payload });
  });
}

/** The runId the view generated for its live run, read back from the invoke. */
function liveRunId(): string {
  const call = mockInvoke.mock.calls.find(([cmd]) => cmd === "chain_repair_live");
  return (call?.[1] as { runId: string }).runId;
}

function renderView(initialEntry = "/chain/projects") {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <ChainProjects />
    </MemoryRouter>,
  );
}

const ENTRY: ChainTracedEntry = {
  name: "alpha",
  entry_path: "/proj/.claude/skills/alpha",
  hops: [],
  final_target: "/wh/repo/skills/alpha",
  status: "link_repo",
  repo: "repo",
};

const PROJECT: ChainProject = {
  name: "proj",
  path: "/proj",
  agents_dir: null,
  surfaces: [
    {
      agent: "claude",
      path: "/proj/.claude/skills",
      kind: "per_entry",
      dir_link_target: null,
      dir_link_ok: false,
      entries: [ENTRY],
    },
  ],
};

const TOPO: ChainTopology = {
  warehouse_roots: [{ root: "/wh", status: "ok", error: null, repo_count: 1 }],
  projects_root: "/Users/x/Projects",
  repos: [],
  projects: [PROJECT],
  guard: [],
  scanned_at: 0,
};

/** A broken-link finding scoped to `projectPath` — findings carry their project
 * among the affected objects, which is how the workbench scopes the global
 * Doctor report to the selected project. */
function finding(projectPath: string, overrides: Partial<ChainFinding> = {}): ChainFinding {
  return {
    rule: "chain.broken_link",
    deviation: "broken",
    severity: "violation",
    evidence: {
      entry_path: `${projectPath}/.claude/skills/alpha`,
      hops: [],
      final_target: "/wh/repo/skills/alpha",
      topology_status: "broken",
    },
    affected: [
      { kind: "skill", name: "alpha", path: `${projectPath}/.claude/skills/alpha` },
      { kind: "project", name: "proj", path: projectPath },
    ],
    actions: ["repair"],
    fingerprint: `fp-${projectPath}`,
    ...overrides,
  };
}

function doctorReport(...projectPaths: string[]): ChainDoctorReport {
  const findings = projectPaths.map(finding);
  return { findings, ignored: [], total: findings.length, scanned_at: 0 };
}

/** The attention state renders evidence cards and collapses the healthy rest
 * of the link list behind one row (#30); the flows below that act on the full
 * list expand that row first, exactly like the user would. */
const DOCTOR_ATTENTION = doctorReport("/proj");

/** Candidate evidence for the broken finding: a same-name Skill scanned in a
 * second repo, above the relink threshold, plus a git rename clue. */
const CANDIDATES: ChainCandidatesReport = {
  candidates: {
    "fp-/proj": [
      {
        path: "/wh/repo2/skills/alpha",
        name: "alpha",
        score: 98,
        reason: "git_rename",
        renamed_at: 1_752_000_000,
      },
    ],
  },
  scanned_at: 0,
};

const REPAIR_PLAN: ChainRepairPlan = {
  items: [
    {
      fingerprint: "fp-/proj",
      rule: "chain.broken_link",
      deviation: "broken",
      project: "/proj",
      path: "/proj/.agents/skills/alpha",
      kind: "relink_broken",
      action: "create",
      old_target: null,
      new_target: "/wh/repo2/skills/alpha",
      message: null,
    },
    {
      fingerprint: "fp-/proj",
      rule: "chain.broken_link",
      deviation: "broken",
      project: "/proj",
      path: "/proj/.claude/skills/alpha",
      kind: "relink_broken",
      action: "repoint",
      old_target: "/wh/repo/skills/alpha",
      new_target: "../../.agents/skills/alpha",
      message: null,
    },
  ],
  evidence: {},
  snapshot: [],
  unsupported: [],
  scanned_at: 0,
};

const REPAIR_OUTCOME: ChainRepairOutcome = {
  results: REPAIR_PLAN.items,
  verified: true,
  scanned_at: 1,
  journal_id: 7,
};

/** A journaled repair for /proj (#31): the applied relink items verbatim. */
const JOURNAL_RECORD: ChainJournalRecord = {
  id: 7,
  created_at: 1_752_000_000,
  projects: ["/proj"],
  fingerprints: ["fp-/proj"],
  items: REPAIR_PLAN.items,
  verified: true,
  status: "applied",
  dismissed: false,
};

const UNDO_OUTCOME: ChainUndoOutcome = {
  results: REPAIR_PLAN.items,
  verified: true,
  scanned_at: 2,
};

const UNLINK_PLAN: ChainUnlinkPlan = {
  project: "/proj",
  skill: "alpha",
  agents: ["claude"],
  items: [
    {
      name: "alpha",
      path: "/proj/.claude/skills/alpha",
      scope: "surface",
      agent: "claude",
      kind: "per_agent_entry",
      action: "remove",
      message: null,
    },
  ],
  evidence: {},
  affected_agents: ["claude"],
  shared_surface: false,
};

const UNLINK_OUTCOME: ChainUnlinkOutcome = {
  report: [{ name: "alpha", path: "/proj/.claude/skills/alpha", action: "removed", message: null }],
  verified: true,
  still_linked: [],
  removed_from: ["claude"],
};

const INSTRUCTIONS_REPORT = {
  projects: [
    {
      path: "/proj",
      canonical: {
        exists: true,
        path: "/proj/AGENTS.md",
        bytes: 12,
        lines: 1,
        est_tokens: 3,
      },
      entries: [
        {
          agent: "claude",
          state: "body",
          path: "/proj/CLAUDE.md",
          bytes: 18,
          est_tokens: 5,
        },
      ],
      resident: [{ agent: "claude", project_bytes: 30, global_bytes: 0, est_tokens: 8 }],
      unmanaged: [],
    },
  ],
  globals: [],
  agents: ["claude"],
  scanned_at: 1,
};

const NORMALIZE_PLAN = {
  items: [
    {
      fingerprint: "fp-dual-body",
      rule: "instructions.dual_body",
      project: "/proj",
      path: "/proj/CLAUDE.md",
      action: "rewrite",
      before: { state: "file", sha256: "before-sha" },
      after_content: "@AGENTS.md\n\n<!-- patchbay:append claude -->\n\nClaude-only notes\n",
      snapshot: true,
      depends_on: {
        path: "/proj/AGENTS.md",
        before: { state: "file", sha256: "canonical-sha" },
      },
      message: null,
    },
    {
      fingerprint: "fp-conflict",
      rule: "instructions.missing_entry",
      project: "/proj",
      path: "/proj/CLAUDE.md",
      action: "conflict",
      before: { state: "dir" },
      snapshot: false,
      message: "path is occupied by a directory",
    },
  ],
  unsupported: [],
  scanned_at: 2,
};

const NORMALIZE_OUTCOME = {
  items: NORMALIZE_PLAN.items,
  snapshot_id: "instructions-2",
  verified: true,
  scanned_at: 3,
};

const INIT_PLAN = {
  items: [
    {
      path: "/proj/AGENTS.md",
      kind: "canonical",
      action: "create",
      before: { state: "absent" },
      after_content:
        "# proj\n\n## Overview\n\n<one line: what this project is>\n\n## Commands\n\n<build / test / lint commands agents should run>\n\n## Conventions\n\n<key conventions; keep this file short and link out for detail>\n",
      message: null,
    },
    {
      path: "/proj/CLAUDE.md",
      kind: "entry",
      action: "create",
      before: { state: "absent" },
      after_content: "@AGENTS.md\n",
      message: null,
    },
  ],
  scanned_at: 4,
};

const INIT_OUTCOME = {
  items: INIT_PLAN.items,
  verified: true,
  scanned_at: 5,
};

describe("ChainProjects", () => {
  beforeEach(() => {
    liveListeners.length = 0;
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_plan_unlink":
          return Promise.resolve(UNLINK_PLAN);
        case "chain_apply_unlink":
          return Promise.resolve(UNLINK_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });
  });

  it("wires the unlink action through plan then confirmed apply", async () => {
    renderView();

    // The attention state collapses the list; expanding restores the table.
    fireEvent.click(await screen.findByTestId("collapsed-links"));
    await screen.findByText("alpha");

    // Unlink previews first: the row action plans the Agent-scoped unlink.
    fireEvent.click(screen.getByRole("button", { name: "Unlink" }));
    expect(mockInvoke).toHaveBeenCalledWith("chain_plan_unlink", {
      projectPath: "/proj",
      skillName: "alpha",
      agents: ["claude"],
    });

    // The guarded confirmation opens before anything is written.
    await screen.findByText("Remove links");
    const confirmButtons = screen.getAllByRole("button", { name: "Unlink" });
    fireEvent.click(confirmButtons[confirmButtons.length - 1]);

    // Only on confirm is the previewed plan applied.
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_apply_unlink", { plan: UNLINK_PLAN }),
    );
  });

  it("previews the complete normalize plan before apply and rescans afterward", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "instructions_plan_normalize":
          return Promise.resolve(NORMALIZE_PLAN);
        case "instructions_apply_normalize":
          return Promise.resolve(NORMALIZE_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Normalize" }));
    expect(mockInvoke).toHaveBeenCalledWith("instructions_plan_normalize", {
      projectPath: "/proj",
      fingerprints: [],
    });

    expect(await screen.findByText("rewrite")).toBeTruthy();
    expect(screen.getAllByText("/proj/CLAUDE.md")).toHaveLength(2);
    expect(screen.getByText(/Claude-only notes/)).toBeTruthy();
    expect(screen.getByText("path is occupied by a directory")).toBeTruthy();
    expect(mockInvoke).not.toHaveBeenCalledWith("instructions_apply_normalize", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("instructions_apply_normalize", {
        projectPath: "/proj",
        plan: NORMALIZE_PLAN,
      }),
    );
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([cmd]) => cmd === "instructions_scan"),
      ).toHaveLength(2),
    );
    expect(screen.getByText("Normalize was not fully verified")).toBeTruthy();
  });

  it("previews the complete init scaffold before apply and rescans afterward", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "instructions_plan_init":
          return Promise.resolve(INIT_PLAN);
        case "instructions_apply_init":
          return Promise.resolve(INIT_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Init" }));
    expect(mockInvoke).toHaveBeenCalledWith("instructions_plan_init", {
      projectPath: "/proj",
      docsDir: false,
    });

    expect(await screen.findAllByText("create")).toHaveLength(2);
    expect(screen.getByText(/one line: what this project is/)).toBeTruthy();
    expect(mockInvoke).not.toHaveBeenCalledWith("instructions_apply_init", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("instructions_apply_init", {
        projectPath: "/proj",
        plan: INIT_PLAN,
      }),
    );
    expect(await screen.findByText("Init verified")).toBeTruthy();
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([cmd]) => cmd === "instructions_scan"),
      ).toHaveLength(2),
    );
  });

  it("renders the green state with the link list collapsed when Doctor finds nothing", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // The ✓ card is the whole of the main area: link count and last scan.
    const green = await screen.findByTestId("workbench-green");
    expect(green.textContent).toContain("All 1 links healthy");
    expect(green.textContent).toContain("nothing needs you");

    // No link-table noise — the list is behind one collapsed row.
    expect(screen.queryByText("alpha")).toBeNull();
    expect(screen.queryByRole("button", { name: "Unlink" })).toBeNull();
    const row = screen.getByTestId("collapsed-links");
    expect(row.textContent).toContain("1 normal");
    expect(row.getAttribute("aria-expanded")).toBe("false");

    // The entry card stays: green hides what needs handling, not the project.
    expect(screen.getByText(/\.agents\/skills|no \.agents\/skills/)).toBeTruthy();

    // Expanding restores the full list, unlink action included.
    fireEvent.click(row);
    expect(await screen.findByText("alpha")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Unlink" })).toBeTruthy();
    expect(screen.getByTestId("collapsed-links").getAttribute("aria-expanded")).toBe("true");
  });

  it("keeps the full link list when Doctor is unreachable (unknown state)", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        // No report, no health assertion: the workbench must not claim green
        // OR attention — it falls back to the full list.
        case "chain_doctor_report":
          return Promise.reject(new Error("doctor unavailable"));
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    expect(await screen.findByText("alpha")).toBeTruthy();
    expect(screen.queryByTestId("workbench-green")).toBeNull();
    expect(screen.queryByTestId("workbench-attention")).toBeNull();
    expect(screen.queryByTestId("collapsed-links")).toBeNull();
  });

  it("renders severity-ordered evidence cards and collapses the healthy rest", async () => {
    // Two findings for the project: an advice-level direct link and a
    // violation-level broken link, deliberately delivered worst-LAST so the
    // client-side ordering (not the wire order) is what's asserted.
    const direct = finding("/proj", {
      rule: "chain.direct_link",
      deviation: "direct",
      severity: "advice",
      fingerprint: "fp-direct",
      evidence: {
        entry_path: "/proj/.claude/skills/beta",
        hops: [],
        final_target: "/wh/repo/skills/beta",
        topology_status: "direct",
      },
      affected: [
        { kind: "skill", name: "beta", path: "/proj/.claude/skills/beta" },
        { kind: "project", name: "proj", path: "/proj" },
      ],
    });
    const report: ChainDoctorReport = {
      findings: [direct, finding("/proj")],
      ignored: [],
      total: 2,
      scanned_at: 0,
    };
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(report);
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    const attention = await screen.findByTestId("workbench-attention");
    const cards = within(attention).getAllByTestId("evidence-card");
    expect(cards.map((card) => card.getAttribute("data-deviation"))).toEqual([
      "broken",
      "direct",
    ]);

    // Candidate evidence is located once for the report's broken findings...
    expect(mockInvoke).toHaveBeenCalledWith("chain_locate_candidates", {
      fingerprints: ["fp-/proj"],
    });
    // ...and printed on the broken card: name, match score, git rename clue.
    const candidate = await screen.findByTestId("card-candidate");
    expect(candidate.textContent).toContain("alpha");
    expect(candidate.textContent).toContain("98% match");
    expect(candidate.textContent).toContain("rename");

    // The healthy remainder is one collapsed row, not a full table.
    expect(screen.getByTestId("collapsed-links").textContent).toContain("0 others normal");
    expect(screen.queryByTestId("workbench-green")).toBeNull();
  });

  it("narrates the live repair from scripted events and hands off to the record card", async () => {
    let resolveLive!: (value: unknown) => void;
    let doctorCalls = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          // The finding disappears after the repair's rescan.
          doctorCalls += 1;
          return Promise.resolve(doctorCalls === 1 ? DOCTOR_ATTENTION : doctorReport());
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        // The live invoke stays in flight until the test resolves it.
        case "chain_repair_live":
          return new Promise((resolve) => {
            resolveLive = resolve;
          });
        // After the repair the journal carries the record (#31 handoff).
        case "chain_repair_journal":
          return Promise.resolve(doctorCalls >= 2 ? [JOURNAL_RECORD] : []);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // The primary action starts the live run directly — no preview confirm.
    fireEvent.click(await screen.findByTestId("card-repair"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repair_live", {
        fingerprints: ["fp-/proj"],
        runId: expect.any(String),
        preferRoot: null,
      }),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("chain_plan_repair", expect.anything());
    const runId = liveRunId();

    // Scripted step events drive the tick-off.
    emitLive({ run_id: runId, seq: 1, step: "check", status: "start", detail: null });
    await waitFor(() =>
      expect(screen.getByTestId("live-step-check").getAttribute("data-status")).toBe("start"),
    );
    emitLive({
      run_id: runId,
      seq: 2,
      step: "check",
      status: "done",
      detail: "/proj/.claude/skills/alpha → /wh/repo/skills/alpha · broken",
    });
    emitLive({
      run_id: runId,
      seq: 3,
      step: "locate",
      status: "done",
      detail: "/wh/repo2/skills/alpha · 98%",
    });
    await waitFor(() =>
      expect(screen.getByTestId("live-step-locate").getAttribute("data-status")).toBe("done"),
    );
    // The evidence line rides the event, and later steps are still pending.
    expect(screen.getByTestId("live-panel").textContent).toContain("98%");
    expect(screen.getByTestId("live-step-rebuild").getAttribute("data-status")).toBe("idle");

    // Events for another run are ignored.
    emitLive({ run_id: "someone-else", seq: 9, step: "rebuild", status: "done", detail: null });
    expect(screen.getByTestId("live-step-rebuild").getAttribute("data-status")).toBe("idle");

    // Terminal outcome: verified → rescan → the record card takes the slot
    // (prototype S3 → S4 handoff), all-green collapsed row below.
    resolveLive({ aborted: false, outcome: REPAIR_OUTCOME });
    expect(await screen.findByTestId("repair-record")).toBeTruthy();
    expect(screen.queryByTestId("workbench-attention")).toBeNull();
  });

  it("pauses, resumes and takes over a live run", async () => {
    let resolveLive!: (value: unknown) => void;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_repair_live":
          return new Promise((resolve) => {
            resolveLive = resolve;
          });
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByTestId("card-repair"));
    await screen.findByTestId("live-panel");
    const runId = liveRunId();

    // Pause → control command; the button flips to resume.
    fireEvent.click(screen.getByTestId("live-pause"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repair_live_control", {
        runId,
        action: "pause",
      }),
    );
    expect(await screen.findByText("Resume")).toBeTruthy();
    fireEvent.click(screen.getByTestId("live-pause"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repair_live_control", {
        runId,
        action: "resume",
      }),
    );

    // Takeover: the control fires and the in-flight invoke resolves aborted —
    // the panel closes and the manual path is available again.
    fireEvent.click(screen.getByTestId("live-takeover"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repair_live_control", {
        runId,
        action: "takeover",
      }),
    );
    resolveLive({ aborted: true, outcome: null });
    await waitFor(() => expect(screen.queryByTestId("live-panel")).toBeNull());
    expect(screen.getByTestId("card-manual")).toBeTruthy();
    // Nothing was applied and nothing rescanned into green.
    expect(screen.queryByTestId("workbench-green")).toBeNull();
  });

  it("shows the failed state and retries the live run", async () => {
    const attempts: Array<(reason: unknown) => void> = [];
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_repair_live":
          return new Promise((_resolve, reject) => {
            attempts.push(reject);
          });
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByTestId("card-repair"));
    await screen.findByTestId("live-panel");
    act(() => attempts[0]("scan failed"));

    // Failed state: the reason shows and retry starts a fresh run.
    const retry = await screen.findByTestId("live-retry");
    expect(screen.getByTestId("live-panel").textContent).toContain("scan failed");
    fireEvent.click(retry);
    await waitFor(() => expect(attempts).toHaveLength(2));
    const runIds = mockInvoke.mock.calls
      .filter(([cmd]) => cmd === "chain_repair_live")
      .map(([, args]) => (args as { runId: string }).runId);
    expect(new Set(runIds).size).toBe(2);
  });

  it("offers removal, not repair, for a broken finding without a candidate", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        // Nowhere plausible to point: the fingerprint is absent from the map.
        case "chain_locate_candidates":
          return Promise.resolve({ candidates: {}, scanned_at: 0 });
        case "chain_plan_repair":
          return Promise.resolve(REPAIR_PLAN);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    await screen.findByTestId("evidence-card");
    expect(screen.queryByTestId("card-repair")).toBeNull();
    const remove = screen.getByTestId("card-remove");

    // The danger path still goes through the same guarded plan → preview flow.
    fireEvent.click(remove);
    expect(mockInvoke).toHaveBeenCalledWith("chain_plan_repair", {
      fingerprints: ["fp-/proj"],
    });
  });

  it("routes manual handling into the existing unlink flow", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_plan_unlink":
          return Promise.resolve(UNLINK_PLAN);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByTestId("card-manual"));
    // The finding's entry lives on the claude surface, so the unlink is
    // scoped to that Agent — same parameters the row action would send.
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_plan_unlink", {
        projectPath: "/proj",
        skillName: "alpha",
        agents: ["claude"],
      }),
    );
  });

  it("renders the repair record card in place of the green status card", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_repair_journal":
          return Promise.resolve([JOURNAL_RECORD]);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // Prototype S4: the record card takes the status slot; the collapsed
    // "N normal" row doubles as the all-green indicator.
    const card = await screen.findByTestId("repair-record");
    expect(card.textContent).toContain("Repair record");
    expect(card.textContent).toContain("completed");
    expect(screen.queryByTestId("workbench-green")).toBeNull();
    expect(screen.getByTestId("collapsed-links")).toBeTruthy();

    // The diff is the journaled edits: link path plus before → after targets.
    fireEvent.click(screen.getByTestId("record-toggle-diff"));
    const diff = screen.getByTestId("record-diff");
    expect(diff.textContent).toContain("/proj/.agents/skills/alpha");
    expect(diff.textContent).toContain("/wh/repo2/skills/alpha");
    expect(diff.textContent).toContain("../../.agents/skills/alpha");
  });

  it("undoes a journaled repair and reloads so the state falls back", async () => {
    let undone = false;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        // After the undo the restored fault is back in the report.
        case "chain_doctor_report":
          return Promise.resolve(undone ? DOCTOR_ATTENTION : doctorReport());
        case "chain_repair_journal":
          return Promise.resolve(
            undone ? [{ ...JOURNAL_RECORD, status: "undone" }] : [JOURNAL_RECORD],
          );
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_undo_repair":
          undone = true;
          return Promise.resolve(UNDO_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByTestId("record-undo"));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("chain_undo_repair", { id: 7 }));

    // The reload drops the spent record and the restored fault surfaces as
    // an evidence card again (undo rolled the state back).
    expect(await screen.findByTestId("workbench-attention")).toBeTruthy();
    expect(screen.queryByTestId("repair-record")).toBeNull();
  });

  it("dismisses the record card persistently without undoing", async () => {
    let dismissed = false;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_repair_journal":
          return Promise.resolve(
            dismissed ? [{ ...JOURNAL_RECORD, dismissed: true }] : [JOURNAL_RECORD],
          );
        case "chain_dismiss_repair_record":
          dismissed = true;
          return Promise.resolve(undefined);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    fireEvent.click(await screen.findByTestId("record-dismiss"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_dismiss_repair_record", { id: 7 }),
    );

    // No undo happened; the green ✓ card returns once the record is hidden.
    expect(mockInvoke).not.toHaveBeenCalledWith("chain_undo_repair", expect.anything());
    expect(await screen.findByTestId("workbench-green")).toBeTruthy();
    expect(screen.queryByTestId("repair-record")).toBeNull();
  });

  it("shows the preset bar in the green state and saves the current skills", async () => {
    let saved: unknown = null;
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_presets_list":
          return Promise.resolve(
            saved
              ? [{ id: 1, name: "写作全套", skills: [saved], created_at: 0 }]
              : [
                  {
                    id: 1,
                    name: "工程基础",
                    skills: [
                      { name: "tdd", path: "/wh/repo/skills/tdd", repo: "repo" },
                      { name: "grilling", path: "/wh/repo/skills/grilling", repo: "repo" },
                    ],
                    created_at: 0,
                  },
                ],
          );
        case "chain_preset_save":
          saved = args;
          return Promise.resolve({ id: 2, name: "写作全套", skills: [], created_at: 1 });
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // The bar lives in the green state: existing presets with their counts.
    const bar = await screen.findByTestId("chain-preset-bar");
    expect(within(bar).getByTestId("preset-pill").textContent).toContain("工程基础");
    expect(within(bar).getByTestId("preset-pill").textContent).toContain("2");

    // Save current: the project's single deduped skill reference crosses the
    // invoke seam with its resolved Original and repo.
    fireEvent.click(within(bar).getByTestId("preset-save-current"));
    fireEvent.change(await screen.findByTestId("preset-name-input"), {
      target: { value: "写作全套" },
    });
    fireEvent.click(screen.getByTestId("preset-name-confirm"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_preset_save", {
        name: "写作全套",
        skills: [{ name: "alpha", path: "/wh/repo/skills/alpha", repo: "repo" }],
      }),
    );
    // The bar refreshes from the store, not from optimistic state.
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([cmd]) => cmd === "chain_presets_list"),
      ).toHaveLength(2),
    );
  });

  it("renames and deletes presets from the bar and hides it outside green", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_presets_list":
          return Promise.resolve([
            {
              id: 7,
              name: "工程基础",
              skills: [{ name: "tdd", path: "/wh/repo/skills/tdd", repo: "repo" }],
              created_at: 0,
            },
          ]);
        case "chain_preset_rename":
        case "chain_preset_delete":
          return Promise.resolve(undefined);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();
    const bar = await screen.findByTestId("chain-preset-bar");

    // Rename via the shared name dialog.
    fireEvent.click(within(bar).getByTestId("preset-rename"));
    fireEvent.change(await screen.findByTestId("preset-name-input"), {
      target: { value: "工程进阶" },
    });
    fireEvent.click(screen.getByTestId("preset-name-confirm"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_preset_rename", {
        id: 7,
        name: "工程进阶",
      }),
    );

    // Delete goes through the guarded confirm (default danger label).
    fireEvent.click(within(bar).getByTestId("preset-delete"));
    const confirmButtons = await screen.findAllByRole("button", { name: "Delete" });
    fireEvent.click(confirmButtons[confirmButtons.length - 1]);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_preset_delete", { id: 7 }),
    );
  });

  it("keeps the preset bar out of the attention state", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_presets_list":
          return Promise.resolve([
            { id: 1, name: "工程基础", skills: [], created_at: 0 },
          ]);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    await screen.findByTestId("evidence-card");
    expect(screen.queryByTestId("chain-preset-bar")).toBeNull();
  });

  it("shows the amber feedback card for a dirty referenced repo without hiding fault cards", async () => {
    // A dirty repo the project references, delivered alongside a broken
    // finding — the hint must coexist below the fault, not replace it.
    const dirtyTopo: ChainTopology = {
      ...TOPO,
      repos: [
        {
          name: "xw-writing",
          path: "/wh/xw-writing",
          source_kind: "checkout",
          root: "/wh",
          health: {
            dirty: true,
            state: "up_to_date",
            ahead: 0,
            behind: 0,
            branch: "main",
            error: null,
          },
          origin: null,
          upstream: null,
          skills: [],
          referenced_by: [{ name: "proj", path: "/proj" }],
        },
        {
          name: "clean-repo",
          path: "/wh/clean",
          source_kind: "checkout",
          root: "/wh",
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
          referenced_by: [{ name: "proj", path: "/proj" }],
        },
        {
          name: "other-dirty",
          path: "/wh/other",
          source_kind: "checkout",
          root: "/wh",
          health: {
            dirty: true,
            state: "up_to_date",
            ahead: 0,
            behind: 0,
            branch: "main",
            error: null,
          },
          origin: null,
          upstream: null,
          skills: [],
          referenced_by: [{ name: "elsewhere", path: "/elsewhere" }],
        },
      ],
    };
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(dirtyTopo);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_repo_dirty_diff":
          return Promise.resolve({
            repo: "/wh/xw-writing",
            files: [
              { path: "skills/zotero/SKILL.md", status: "modified", additions: 12, deletions: 3 },
              { path: "skills/zotero/refs/new.md", status: "added", additions: 40, deletions: 0 },
            ],
            truncated: false,
          });
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // Only the repo THIS project references gets a card; the fault card
    // stays above it, unobscured.
    const cards = await screen.findAllByTestId("dirty-repo-card");
    expect(cards).toHaveLength(1);
    expect(cards[0].getAttribute("data-repo")).toBe("/wh/xw-writing");
    expect(screen.getByTestId("evidence-card")).toBeTruthy();
    const attention = screen.getByTestId("workbench-attention");
    // DOM order: the fault area precedes the hint card.
    expect(
      attention.compareDocumentPosition(cards[0]) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    // 查看 diff lazily fetches the read-only evidence and lists the files.
    fireEvent.click(within(cards[0]).getByTestId("dirty-toggle-diff"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repo_dirty_diff", {
        repoPath: "/wh/xw-writing",
      }),
    );
    const diff = await within(cards[0]).findByTestId("dirty-diff");
    expect(diff.textContent).toContain("skills/zotero/SKILL.md");
    expect(diff.textContent).toContain("+12");
    expect(diff.textContent).toContain("-3");

    // 整理提交 hands off to the existing opener capability.
    const { openPath } = await import("@tauri-apps/plugin-opener");
    fireEvent.click(within(cards[0]).getByTestId("dirty-open-repo"));
    await waitFor(() => expect(vi.mocked(openPath)).toHaveBeenCalledWith("/wh/xw-writing"));
  });

  it("keeps the feedback card in the green state without breaking the all-clear", async () => {
    const dirtyTopo: ChainTopology = {
      ...TOPO,
      repos: [
        {
          name: "xw-writing",
          path: "/wh/xw-writing",
          source_kind: "checkout",
          root: "/wh",
          health: {
            dirty: true,
            state: "up_to_date",
            ahead: 0,
            behind: 0,
            branch: "main",
            error: null,
          },
          origin: null,
          upstream: null,
          skills: [],
          referenced_by: [{ name: "proj", path: "/proj" }],
        },
      ],
    };
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(dirtyTopo);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // Dirty is a hint, not a health deviation: the ✓ card and collapsed
    // list stay, with the amber card alongside.
    expect(await screen.findByTestId("dirty-repo-card")).toBeTruthy();
    expect(screen.getByTestId("workbench-green")).toBeTruthy();
    expect(screen.getByTestId("collapsed-links")).toBeTruthy();
  });

  it("aggregates a repo-move storm into one card and batch-repairs with the detected root", async () => {
    // Two broken findings whose repo moved: beta is delivered worst-LAST so
    // the card, not the wire order, owns the presentation.
    const alpha = finding("/proj");
    const beta = finding("/proj", {
      fingerprint: "fp-beta",
      evidence: {
        entry_path: "/proj/.claude/skills/beta",
        hops: [],
        final_target: "/wh/repo/skills/beta",
        topology_status: "broken",
      },
      affected: [
        { kind: "skill", name: "beta", path: "/proj/.claude/skills/beta" },
        { kind: "project", name: "proj", path: "/proj" },
      ],
    });
    const stormReport: ChainDoctorReport = {
      findings: [alpha, beta],
      ignored: [],
      total: 2,
      scanned_at: 0,
    };
    const group: ChainRepoMove = {
      old_root: "/wh/repo",
      new_root: "/wh/repo-v2",
      repo_name: "repo-v2",
      skills: ["alpha", "beta"],
      fingerprints: ["fp-/proj", "fp-beta"],
      entry_paths: ["/proj/.claude/skills/alpha", "/proj/.claude/skills/beta"],
    };
    let resolveLive!: (value: unknown) => void;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(stormReport);
        case "chain_repo_moves":
          return Promise.resolve({ groups: [group], scanned_at: 0 });
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        case "chain_repair_live":
          return new Promise((resolve) => {
            resolveLive = resolve;
          });
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // One root cause, not two symptoms: the storm card shows the cause and
    // the affected list; the member evidence cards are absorbed.
    const card = await screen.findByTestId("repo-move-card");
    expect(card.textContent).toContain("repo-v2");
    expect(card.textContent).toContain("/wh/repo");
    expect(within(card).getByTestId("repo-move-skills").textContent).toContain("alpha");
    expect(within(card).getByTestId("repo-move-skills").textContent).toContain("beta");
    expect(screen.queryByTestId("evidence-card")).toBeNull();

    // Batch repair: ONE live run over the whole group, anchored to the
    // detected new root so same-name ties cannot drift elsewhere.
    fireEvent.click(within(card).getByTestId("repo-move-repair"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_repair_live", {
        fingerprints: ["fp-/proj", "fp-beta"],
        runId: expect.any(String),
        preferRoot: "/wh/repo-v2",
      }),
    );
    resolveLive({ aborted: true, outcome: null });
  });

  it("itemizes a storm back into individual evidence cards", async () => {
    const alpha = finding("/proj");
    const beta = finding("/proj", {
      fingerprint: "fp-beta",
      evidence: {
        entry_path: "/proj/.claude/skills/beta",
        hops: [],
        final_target: "/wh/repo/skills/beta",
        topology_status: "broken",
      },
      affected: [
        { kind: "skill", name: "beta", path: "/proj/.claude/skills/beta" },
        { kind: "project", name: "proj", path: "/proj" },
      ],
    });
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve({
            findings: [alpha, beta],
            ignored: [],
            total: 2,
            scanned_at: 0,
          });
        case "chain_repo_moves":
          return Promise.resolve({
            groups: [
              {
                old_root: "/wh/repo",
                new_root: "/wh/repo-v2",
                repo_name: "repo-v2",
                skills: ["alpha", "beta"],
                fingerprints: ["fp-/proj", "fp-beta"],
                entry_paths: [],
              },
            ],
            scanned_at: 0,
          });
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    const card = await screen.findByTestId("repo-move-card");
    fireEvent.click(within(card).getByTestId("repo-move-itemize"));

    // The aggregate dissolves into per-finding evidence cards.
    await waitFor(() =>
      expect(screen.queryByTestId("repo-move-card")).toBeNull(),
    );
    expect(screen.getAllByTestId("evidence-card")).toHaveLength(2);
  });

  it("keeps a lone broken link as a plain evidence card, not a storm", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        // A stale group whose OTHER member is no longer a finding here.
        case "chain_repo_moves":
          return Promise.resolve({
            groups: [
              {
                old_root: "/wh/repo",
                new_root: "/wh/repo-v2",
                repo_name: "repo-v2",
                skills: ["alpha", "gone"],
                fingerprints: ["fp-/proj", "fp-gone"],
                entry_paths: [],
              },
            ],
            scanned_at: 0,
          });
        case "chain_locate_candidates":
          return Promise.resolve(CANDIDATES);
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    // Scoped to this project the group has one member — below the storm
    // threshold, so the plain evidence card renders instead.
    expect(await screen.findByTestId("evidence-card")).toBeTruthy();
    expect(screen.queryByTestId("repo-move-card")).toBeNull();
  });

  it("jumps to the full diagnosis from the evidence card", async () => {
    render(
      <MemoryRouter initialEntries={["/chain/projects"]}>
        <Routes>
          <Route path="/chain/doctor" element={<div data-testid="doctor-page" />} />
          <Route path="*" element={<ChainProjects />} />
        </Routes>
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByTestId("card-diagnose"));
    expect(await screen.findByTestId("doctor-page")).toBeTruthy();
  });

  it("stays green when the only finding belongs to another project", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        // Doctor reports globally; only /proj's own findings concern this
        // workbench, so another project's finding leaves it green.
        case "chain_doctor_report":
          return Promise.resolve(doctorReport("/other"));
        default:
          return Promise.resolve(undefined);
      }
    });

    renderView();

    expect(await screen.findByTestId("workbench-green")).toBeTruthy();
  });

  it("opens the project selected by the sidebar query parameter", async () => {
    const beta: ChainProject = {
      ...PROJECT,
      name: "beta-project",
      path: "/beta project",
      surfaces: [
        {
          ...PROJECT.surfaces[0],
          path: "/beta project/.claude/skills",
          entries: [{ ...ENTRY, name: "beta-skill", entry_path: "/beta project/.claude/skills/beta-skill" }],
        },
      ],
    };
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "chain_get_topology") {
        return Promise.resolve({ ...TOPO, projects: [PROJECT, beta] });
      }
      if (cmd === "instructions_scan") return Promise.resolve(INSTRUCTIONS_REPORT);
      if (cmd === "chain_doctor_report") return Promise.resolve(doctorReport("/beta project"));
      return Promise.resolve(undefined);
    });

    renderView("/chain/projects?project=%2Fbeta%20project");

    // beta's finding puts it in the attention state; the selected project is
    // identified by its own link list behind the collapsed row.
    fireEvent.click(await screen.findByTestId("collapsed-links"));
    expect(await screen.findByText("beta-skill")).toBeTruthy();
  });

  // ── #36 onboarding wizard ──

  /** A warehouse repo fixture for the wizard's source list and skill picker. */
  function repo(name: string, skills: Array<{ name: string; path: string }>): ChainRepo {
    return {
      name,
      path: `/wh/${name}`,
      source_kind: "checkout",
      root: "/wh",
      health: { dirty: false, state: "up_to_date", ahead: 0, behind: 0, branch: "main", error: null },
      origin: null,
      upstream: null,
      skills,
      referenced_by: [],
    };
  }

  const REPO_MP = repo("mp", [
    { name: "grilling", path: "/wh/mp/skills/grilling" },
    { name: "tdd", path: "/wh/mp/skills/tdd" },
  ]);
  const REPO_XW = repo("xw", [{ name: "zotero", path: "/wh/xw/skills/zotero" }]);

  /** A freshly enrolled project: no aggregate, no surfaces, zero links. */
  const EMPTY_PROJECT: ChainProject = {
    name: "helios",
    path: "/helios",
    agents_dir: null,
    surfaces: [],
  };

  const TOPO_ONBOARD: ChainTopology = {
    warehouse_roots: [{ root: "/wh", status: "ok", error: null, repo_count: 2 }],
    projects_root: "/Users/x/Projects",
    repos: [REPO_MP, REPO_XW],
    projects: [EMPTY_PROJECT],
    guard: [],
    scanned_at: 0,
  };

  const PRESET_ENG: ChainPreset = {
    id: 1,
    name: "工程基础",
    skills: [
      { name: "grilling", path: "/wh/mp/skills/grilling", repo: "mp" },
      { name: "tdd", path: "/wh/mp/skills/tdd", repo: "mp" },
    ],
    created_at: 0,
  };

  const ONBOARD_PLAN: ChainLinkPlan = {
    project: "/helios",
    agg_dir: "/helios/.agents/skills",
    originals: ["/wh/mp/skills/grilling", "/wh/xw/skills/zotero"],
    agents: ["claude", "codex", "copilot"],
    skills: [],
    entries: [],
    evidence: {},
  };

  const ONBOARD_OUTCOME: ChainApplyOutcome = {
    report: { agg_dir: "/helios/.agents/skills", skills: [], entries: [] },
    verified: true,
    observed: ["grilling", "zotero"],
    missing: [],
  };

  function mockOnboarding() {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO_ONBOARD);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_presets_list":
          return Promise.resolve([PRESET_ENG]);
        case "chain_plan_link":
          return Promise.resolve(ONBOARD_PLAN);
        case "chain_apply_link":
          return Promise.resolve(ONBOARD_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });
  }

  it("auto-enters the three-step wizard for a zero-link project and walks back and forth", async () => {
    mockOnboarding();
    renderView();

    // The wizard replaces the whole green-state block: no ✓ card, no preset
    // bar, no collapsed link row — onboarding is the state.
    const wizard = await screen.findByTestId("onboarding-wizard");
    expect(screen.queryByTestId("workbench-green")).toBeNull();
    expect(screen.queryByTestId("chain-preset-bar")).toBeNull();
    expect(screen.queryByTestId("collapsed-links")).toBeNull();

    // Step 1: every scanned source repo is offered, preselected.
    expect(within(wizard).getAllByTestId("wizard-source")).toHaveLength(2);
    expect(within(wizard).getByTestId("wizard-summary").textContent).toContain(
      "2 sources selected",
    );

    // Forward to the skill picker, back to the sources, forward again.
    fireEvent.click(within(wizard).getByTestId("wizard-next"));
    expect(within(wizard).getByTestId("skill-picker")).toBeTruthy();
    fireEvent.click(within(wizard).getByTestId("wizard-back"));
    expect(within(wizard).getAllByTestId("wizard-source")).toHaveLength(2);
    fireEvent.click(within(wizard).getByTestId("wizard-next"));

    // Deselecting every skill keeps the wizard on step 2: nothing to create.
    expect(
      (within(wizard).getByTestId("wizard-next") as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("seeds from a preset, adjusts the selection, and submits one batch plan/apply", async () => {
    mockOnboarding();
    renderView();

    const wizard = await screen.findByTestId("onboarding-wizard");
    fireEvent.click(within(wizard).getByTestId("wizard-next"));

    // Preset 起步: one click selects the preset's references.
    fireEvent.click(within(wizard).getByTestId("picker-preset-pill"));
    expect(
      (within(wizard).getByRole("checkbox", { name: "grilling" }) as HTMLInputElement).checked,
    ).toBe(true);
    expect(
      (within(wizard).getByRole("checkbox", { name: "tdd" }) as HTMLInputElement).checked,
    ).toBe(true);

    // …then adjust freely: drop tdd, add zotero from the other source.
    fireEvent.click(within(wizard).getByRole("checkbox", { name: "tdd" }));
    fireEvent.click(within(wizard).getByRole("checkbox", { name: "zotero" }));
    expect(within(wizard).getByTestId("wizard-summary").textContent).toContain(
      "Will create 2 links",
    );

    // Step 3: agents default to claude+codex; add copilot. The summary names
    // both halves of the batch: N links + M agent entries.
    fireEvent.click(within(wizard).getByTestId("wizard-next"));
    fireEvent.click(within(wizard).getByRole("button", { name: "copilot" }));
    expect(within(wizard).getByTestId("wizard-summary").textContent).toContain(
      "Will create 2 links + 3 agent entries",
    );

    // One confirmation = one batch plan + one apply of that exact plan.
    fireEvent.click(within(wizard).getByTestId("wizard-apply"));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_plan_link", {
        projectPath: "/helios",
        skillPaths: ["/wh/mp/skills/grilling", "/wh/xw/skills/zotero"],
        agents: ["claude", "codex", "copilot"],
      }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_apply_link", { plan: ONBOARD_PLAN }),
    );

    // Success lands back on the normal workbench: a fresh scan reloads it.
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([cmd]) => cmd === "chain_get_topology").length,
      ).toBeGreaterThanOrEqual(2),
    );
  });

  it("keeps the wizard behind evidence cards when the project has findings", async () => {
    // A project whose only link is broken: zero healthy rows would never
    // happen with rows>0, but an attention state must always win over
    // onboarding — faults are not hidden by a wizard.
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve(TOPO);
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(DOCTOR_ATTENTION);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderView();

    expect(await screen.findByTestId("workbench-attention")).toBeTruthy();
    expect(screen.queryByTestId("onboarding-wizard")).toBeNull();
  });

  it("reuses the shared picking flow — preset start included — in the link dialog", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "chain_get_topology":
          return Promise.resolve({ ...TOPO, repos: [REPO_MP] });
        case "instructions_scan":
          return Promise.resolve(INSTRUCTIONS_REPORT);
        case "chain_doctor_report":
          return Promise.resolve(doctorReport());
        case "chain_presets_list":
          return Promise.resolve([PRESET_ENG]);
        case "chain_plan_link":
          return Promise.resolve(ONBOARD_PLAN);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderView();

    // The existing project is green with links, so no wizard — the same
    // picker arrives through「＋ 链接技能」instead.
    fireEvent.click(await screen.findByRole("button", { name: "＋ Link skills" }));
    const picker = await screen.findByTestId("skill-picker");
    fireEvent.click(within(picker).getByTestId("picker-preset-pill"));
    fireEvent.click(screen.getByRole("button", { name: "Preview plan" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("chain_plan_link", {
        projectPath: "/proj",
        skillPaths: ["/wh/mp/skills/grilling", "/wh/mp/skills/tdd"],
        agents: ["claude", "codex"],
      }),
    );
  });
});
