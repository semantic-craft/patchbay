import { describe, expect, it, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Fleet } from "./Fleet";
import { reportAge } from "./fleetAge";
import type {
  FleetManifestSnapshot,
  FleetManifestUpdatePlan,
  FleetBootstrapOutcome,
  FleetBootstrapPlan,
  FleetPullOutcome,
  FleetPullPlan,
  FleetAutoRoundResult,
  FleetAutoRoundStatus,
  FleetPushOutcome,
  FleetPushPlan,
  FleetStatus,
} from "../lib/tauri";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockToast = vi.mocked(toast);
let autoRoundHandler: ((event: { payload: FleetAutoRoundResult }) => void) | null = null;

const AUTO_STATUS: FleetAutoRoundStatus = {
  enabled: false,
  in_backoff: false,
  next_round_at: null,
  consecutive_failures: 0,
  last_round: null,
};

const STATUS: FleetStatus = {
  machine: "alpha",
  meta_url: "/hub/_patchbay-fleet.git",
  meta_state: "fresh",
  meta_warning: null,
  projects_root: "/Users/me/Projects",
  scanned_at: Date.now(),
  machines: [
    { id: "alpha", display_name: "Alpha", is_self: true, reported_at: null },
    {
      id: "gamma",
      display_name: "Demo Mac",
      is_self: false,
      reported_at: new Date(Date.now() - 3 * 60 * 1000).toISOString(),
    },
  ],
  repos: [
    {
      name: "patchbay",
      hub: "alpha",
      authority: "alpha",
      branch: "main",
      auto_sync: false,
      hub_head: "4553c0b",
      hub_note: null,
      cells: {
        alpha: {
          name: "patchbay",
          present: true,
          branch: "main",
          head: "4553c0b",
          dirty: 2,
          detached: false,
          ahead: 1,
          behind: 0,
        },
        gamma: {
          name: "patchbay",
          present: true,
          branch: "main",
          head: "374aef3",
          dirty: 0,
          detached: false,
          ahead: 0,
          behind: 3,
        },
      },
    },
    {
      name: "prompt-optimizer",
      hub: "alpha",
      authority: "gamma",
      branch: "main",
      auto_sync: false,
      hub_head: null,
      hub_note: "hub_unreachable",
      cells: {
        alpha: {
          name: "prompt-optimizer",
          present: false,
          branch: null,
          head: null,
          dirty: null,
          detached: false,
          ahead: null,
          behind: null,
        },
      },
    },
  ],
  warnings: [],
};

const PUSH_PLAN: FleetPushPlan = {
  ok: true,
  machine: "alpha",
  planned_at: Date.now(),
  items: [
    {
      repo: "patchbay",
      status: "ready",
      reason_code: null,
      message: null,
      evidence: {
        head_oid: "613873f000000000000000000000000000000000",
        dirty_count: 0,
        branch: "main",
        remote_url: "/hub/patchbay.git",
      },
    },
  ],
};

const PUSH_OUTCOME: FleetPushOutcome = {
  ok: true,
  machine: "alpha",
  items: [
    {
      repo: "patchbay",
      action: "pushed",
      reason_code: null,
      message: null,
      before_head: "4553c0b000000000000000000000000000000000",
      after_head: "613873f000000000000000000000000000000000",
    },
  ],
};

const PULL_STATUS: FleetStatus = {
  ...STATUS,
  repos: [{ ...STATUS.repos[0], authority: "gamma" }],
};

const PULL_PLAN: FleetPullPlan = {
  ok: true,
  machine: "alpha",
  manifest_digest: "manifest-v1",
  planned_at: Date.now(),
  items: [
    {
      repo: "patchbay",
      status: "ready",
      reason_code: null,
      message: null,
      evidence: {
        head_oid: "613873f000000000000000000000000000000000",
        target_oid: "da6c7a1000000000000000000000000000000000",
        dirty_count: 0,
        branch: "main",
        remote_url: "alpha:git-mirrors/projects/patchbay.git",
        hub_url: "alpha:git-mirrors/projects/patchbay.git",
      },
    },
  ],
};

const PULL_OUTCOME: FleetPullOutcome = {
  ok: true,
  machine: "alpha",
  items: [
    {
      repo: "patchbay",
      action: "pulled",
      reason_code: null,
      message: null,
      before_head: "613873f000000000000000000000000000000000",
      after_head: "da6c7a1000000000000000000000000000000000",
    },
  ],
};

const MANIFEST_SNAPSHOT: FleetManifestSnapshot = {
  machine: "alpha",
  meta_head: "meta-head-v1",
  manifest_digest: "manifest-v1",
  known_machines: ["gamma", "alpha"],
  manifest: {
    fleet: { projects_root: "~/Projects" },
    hubs: {
      alpha: { url: "alpha:mirrors", host_machine: "alpha" },
      backup: { url: "backup:mirrors", host_machine: "gamma" },
    },
    repos: [
      { name: "patchbay", hub: "alpha", authority: "alpha", branch: "main" },
    ],
  },
};

const MANIFEST_PLAN: FleetManifestUpdatePlan = {
  machine: "alpha",
  meta_head: "meta-head-v1",
  manifest_digest: "manifest-v1",
  planned_at: Date.now(),
  manifest: {
    ...MANIFEST_SNAPSHOT.manifest,
    repos: [
      { name: "patchbay", hub: "backup", authority: "gamma", branch: "stable" },
      { name: "stray", hub: "alpha", authority: "alpha", branch: "main" },
    ],
  },
  changes: [
    {
      action: "update",
      repo: "patchbay",
      before: MANIFEST_SNAPSHOT.manifest.repos[0],
      after: { name: "patchbay", hub: "backup", authority: "gamma", branch: "stable" },
    },
    {
      action: "add",
      repo: "stray",
      before: null,
      after: { name: "stray", hub: "alpha", authority: "alpha", branch: "main" },
    },
  ],
};

const BOOTSTRAP_PLAN: FleetBootstrapPlan = {
  ok: true,
  machine: "alpha",
  manifest_digest: "manifest-v1",
  planned_at: Date.now(),
  items: [
    {
      repo: "prompt-optimizer",
      status: "ready",
      reason_code: null,
      message: null,
      evidence: {
        target_path: "/Users/me/Projects/prompt-optimizer",
        hub_name: "alpha",
        hub_url: "alpha:git-mirrors/projects/prompt-optimizer.git",
        branch: "main",
        target_oid: "385f5f0000000000000000000000000000000000",
      },
    },
  ],
};

const BOOTSTRAP_OUTCOME: FleetBootstrapOutcome = {
  ok: true,
  machine: "alpha",
  items: [
    {
      repo: "prompt-optimizer",
      action: "bootstrapped",
      reason_code: null,
      message: null,
      after_head: "385f5f0000000000000000000000000000000000",
    },
  ],
};

function renderFleet() {
  return render(
    <MemoryRouter>
      <Fleet />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  mockInvoke.mockReset();
  mockToast.success.mockReset();
  mockToast.warning.mockReset();
  mockToast.error.mockReset();
  autoRoundHandler = null;
  mockListen.mockReset();
  mockListen.mockImplementation((_event, handler) => {
    autoRoundHandler = handler as (event: { payload: FleetAutoRoundResult }) => void;
    return Promise.resolve(() => {});
  });
  mockInvoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "fleet_status":
        return Promise.resolve(STATUS);
      case "fleet_plan_push":
        return Promise.resolve(PUSH_PLAN);
      case "fleet_apply_push":
        return Promise.resolve(PUSH_OUTCOME);
      case "fleet_auto_status":
        return Promise.resolve(AUTO_STATUS);
      case "fleet_set_repo_auto_sync":
      case "set_settings":
        return Promise.resolve(undefined);
      case "fleet_manifest_get":
        return Promise.resolve(MANIFEST_SNAPSHOT);
      case "fleet_discover":
        return Promise.resolve({
          machine: "alpha",
          projects_root: "/Users/me/Projects",
          scanned_at: Date.now(),
          unlisted: [{ name: "stray", path: "/Users/me/Projects/stray", origin: null }],
        });
      case "fleet_manifest_update": {
        const request = (args as { request: { mode: string } }).request;
        return request.mode === "preview"
          ? Promise.resolve({ mode: "preview", plan: MANIFEST_PLAN })
          : Promise.resolve({
              mode: "apply",
              outcome: {
                ok: true,
                action: "updated",
                pushed: true,
                commit: "abc1234",
                manifest_digest: "manifest-v2",
                changes: MANIFEST_PLAN.changes,
                message: null,
              },
            });
      }
      default:
        return Promise.resolve(undefined);
    }
  });

  mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
    switch (cmd) {
      case "fleet_status":
        return Promise.resolve(STATUS);
      case "fleet_plan_push":
        return Promise.resolve(PUSH_PLAN);
      case "fleet_apply_push":
        return Promise.resolve(PUSH_OUTCOME);
      case "fleet_auto_status":
        return Promise.resolve(AUTO_STATUS);
      case "fleet_set_repo_auto_sync":
      case "set_settings":
        return Promise.resolve(undefined);
      case "fleet_manifest_get":
        return Promise.resolve(MANIFEST_SNAPSHOT);
      case "fleet_discover":
        return Promise.resolve({
          machine: "alpha",
          projects_root: "/Users/me/Projects",
          scanned_at: Date.now(),
          unlisted: [{ name: "stray", path: "/Users/me/Projects/stray", origin: null }],
        });
      case "fleet_manifest_update": {
        const request = (args as { request: { mode: string } }).request;
        return request.mode === "preview"
          ? Promise.resolve({ mode: "preview", plan: MANIFEST_PLAN })
          : Promise.resolve({
              mode: "apply",
              outcome: {
                ok: true,
                action: "updated",
                pushed: true,
                commit: "abc1234",
                manifest_digest: "manifest-v2",
                changes: MANIFEST_PLAN.changes,
                message: null,
              },
            });
      }
      default:
        return Promise.resolve(undefined);
    }
  });
});

describe("Fleet", () => {
  it("renders the matrix: repo rows × machine columns", async () => {
    renderFleet();
    expect(await screen.findByText("patchbay")).toBeDefined();
    expect(screen.getByText("prompt-optimizer")).toBeDefined();
    // Machine columns show display names; self column is marked.
    expect(screen.getByText("Alpha")).toBeDefined();
    expect(screen.getByText("Demo Mac")).toBeDefined();
    expect(screen.getByText("(this machine)")).toBeDefined();
    // Reported column shows its age.
    expect(screen.getByText("3m ago")).toBeDefined();
  });

  it("shows dirty, ahead/behind, missing and hub notes", async () => {
    renderFleet();
    expect(await screen.findByText("dirty 2")).toBeDefined();
    expect(screen.getByText(/1 ahead/)).toBeDefined();
    expect(screen.getByText(/3 behind/)).toBeDefined();
    // prompt-optimizer is absent locally and its hub is unreachable.
    expect(screen.getByText("missing")).toBeDefined();
    expect(screen.getByText(/hub unreachable/)).toBeDefined();
    // gamma never reported prompt-optimizer → unknown-not-absent wording.
    expect(screen.getByText("not reported")).toBeDefined();
  });

  it("surfaces a stale meta cache warning", async () => {
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === "fleet_status"
        ? Promise.resolve({
            ...STATUS,
            meta_state: "stale",
            meta_warning: "ssh: connect refused",
          })
        : Promise.resolve(undefined),
    );
    renderFleet();
    expect(await screen.findByText(/ssh: connect refused/)).toBeDefined();
  });

  it("shows the error state when the scan fails", async () => {
    mockInvoke.mockImplementation(() => Promise.reject("fleet_meta_url is not configured"));
    renderFleet();
    expect(await screen.findByText(/fleet_meta_url is not configured/)).toBeDefined();
  });

  it("renders the global and per-repo opt-ins with the last-round state", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_auto_status") {
        return Promise.resolve({
          ...AUTO_STATUS,
          in_backoff: true,
          next_round_at: Date.now() + 60_000,
          consecutive_failures: 1,
          last_round: {
            ok: false,
            finished_at: Date.now() - 30_000,
            pulled: [],
            pushed: [],
            attention: [
              { repo: "patchbay", reason: "repo_dirty", message: "dirty" },
            ],
          },
        } satisfies FleetAutoRoundStatus);
      }
      return Promise.resolve(undefined);
    });
    renderFleet();

    expect(
      (await screen.findByRole("switch", { name: "Automatic fleet rounds" })).getAttribute(
        "aria-checked",
      ),
    ).toBe("false");
    expect(
      screen
        .getByRole("switch", { name: "Automatically sync patchbay" })
        .getAttribute("aria-checked"),
    ).toBe("false");
    expect(screen.getByText(/repo_dirty/)).toBeDefined();
    expect(screen.getByText(/backoff/i)).toBeDefined();
  });

  it("persists global and per-repo opt-ins through their owned APIs", async () => {
    renderFleet();
    fireEvent.click(
      await screen.findByRole("switch", { name: "Automatic fleet rounds" }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("set_settings", {
        key: "fleet_auto_mode",
        value: "on",
      }),
    );

    fireEvent.click(
      screen.getByRole("switch", { name: "Automatically sync patchbay" }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("fleet_set_repo_auto_sync", {
        repo: "patchbay",
        enabled: true,
      }),
    );
  });

  it("keeps a failed repo toggle off and never reports success", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_auto_status") return Promise.resolve(AUTO_STATUS);
      if (cmd === "fleet_set_repo_auto_sync") return Promise.reject("push rejected");
      return Promise.resolve(undefined);
    });
    renderFleet();
    const toggle = await screen.findByRole("switch", {
      name: "Automatically sync patchbay",
    });
    fireEvent.click(toggle);

    await waitFor(() =>
      expect(mockToast.error).toHaveBeenCalledWith(expect.stringContaining("push rejected")),
    );
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(mockToast.success).not.toHaveBeenCalled();
  });

  it("shows an attention notification for a failed background round, not success", async () => {
    renderFleet();
    await screen.findByText("patchbay");
    await waitFor(() => expect(autoRoundHandler).not.toBeNull());

    act(() => {
      autoRoundHandler!({
        payload: {
          ok: false,
          finished_at: Date.now(),
          pulled: [],
          pushed: [],
          attention: [
            { repo: "patchbay", reason: "diverged", message: "diverged" },
          ],
        },
      });
    });

    expect(mockToast.warning).toHaveBeenCalledWith(expect.stringContaining("diverged"));
    expect(mockToast.success).not.toHaveBeenCalled();
  });

  it("pushes an authority repo through preview then confirmed apply", async () => {
    renderFleet();

    fireEvent.click(await screen.findByRole("button", { name: "Push patchbay" }));
    expect(screen.queryByRole("button", { name: "Push prompt-optimizer" })).toBeNull();
    expect(mockInvoke).toHaveBeenCalledWith("fleet_plan_push", { repos: ["patchbay"] });
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_push", expect.anything());

    expect(await screen.findByText("Push repository")).toBeDefined();
    expect(screen.getByText("main@613873f")).toBeDefined();
    expect(screen.getByText("/hub/patchbay.git")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Push" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("fleet_apply_push", { plan: PUSH_PLAN }),
    );
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("does not confirm or apply a refused push preview", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_plan_push") {
        return Promise.resolve({
          ...PUSH_PLAN,
          ok: false,
          items: [
            {
              repo: "patchbay",
              status: "refused",
              reason_code: "repo_dirty",
              message: "local repo has uncommitted changes",
              evidence: null,
            },
          ],
        } satisfies FleetPushPlan);
      }
      return Promise.resolve(undefined);
    });
    renderFleet();

    fireEvent.click(await screen.findByRole("button", { name: "Push patchbay" }));

    await waitFor(() =>
      expect(mockToast.error).toHaveBeenCalledWith(expect.stringContaining("uncommitted changes")),
    );
    expect(screen.queryByText("Push repository")).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_push", expect.anything());
  });

  it("reports an apply-time plan conflict and still refreshes the matrix", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "fleet_status":
          return Promise.resolve(STATUS);
        case "fleet_plan_push":
          return Promise.resolve(PUSH_PLAN);
        case "fleet_apply_push":
          return Promise.resolve({
            ok: false,
            machine: "alpha",
            items: [
              {
                repo: "patchbay",
                action: "conflict",
                reason_code: "plan_conflict",
                message: "repository evidence changed after preview",
                before_head: null,
                after_head: null,
              },
            ],
          } satisfies FleetPushOutcome);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderFleet();
    fireEvent.click(await screen.findByRole("button", { name: "Push patchbay" }));
    fireEvent.click(await screen.findByRole("button", { name: "Push" }));

    await waitFor(() =>
      expect(mockToast.warning).toHaveBeenCalledWith(expect.stringContaining("since preview")),
    );
    expect(mockToast.success).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("pulls a non-authority repo through preview and sends the exact plan on confirm", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "fleet_status":
          return Promise.resolve(PULL_STATUS);
        case "fleet_plan_pull":
          return Promise.resolve(PULL_PLAN);
        case "fleet_apply_pull":
          return Promise.resolve(PULL_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderFleet();

    fireEvent.click(await screen.findByRole("button", { name: "Pull patchbay" }));
    expect(screen.queryByRole("button", { name: "Push patchbay" })).toBeNull();
    expect(mockInvoke).toHaveBeenCalledWith("fleet_plan_pull", { repos: ["patchbay"] });
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_pull", expect.anything());
    expect(await screen.findByText("Pull repository")).toBeDefined();
    expect(screen.getByText("main@613873f → da6c7a1")).toBeDefined();
    expect(screen.getByText("alpha:git-mirrors/projects/patchbay.git")).toBeDefined();

    fireEvent.click(screen.getByRole("button", { name: "Pull" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("fleet_apply_pull", { plan: PULL_PLAN }),
    );
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("does not open confirmation or apply when pull preview refuses", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") return Promise.resolve(PULL_STATUS);
      if (cmd === "fleet_plan_pull") {
        return Promise.resolve({
          ...PULL_PLAN,
          ok: false,
          items: [
            {
              repo: "patchbay",
              status: "refused",
              reason_code: "repo_dirty",
              message: "local repo has uncommitted changes",
              evidence: null,
            },
          ],
        } satisfies FleetPullPlan);
      }
      return Promise.resolve(undefined);
    });
    renderFleet();

    fireEvent.click(await screen.findByRole("button", { name: "Pull patchbay" }));

    await waitFor(() =>
      expect(mockToast.error).toHaveBeenCalledWith(expect.stringContaining("uncommitted changes")),
    );
    expect(screen.queryByText("Pull repository")).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_pull", expect.anything());
  });

  it("reports pull apply conflict without success and refreshes the matrix", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "fleet_status":
          return Promise.resolve(PULL_STATUS);
        case "fleet_plan_pull":
          return Promise.resolve(PULL_PLAN);
        case "fleet_apply_pull":
          return Promise.resolve({
            ok: false,
            machine: "alpha",
            items: [
              {
                repo: "patchbay",
                action: "conflict",
                reason_code: "plan_conflict",
                message: "repository evidence changed after preview",
                before_head: PULL_PLAN.items[0].evidence!.head_oid,
                after_head: PULL_PLAN.items[0].evidence!.head_oid,
              },
            ],
          } satisfies FleetPullOutcome);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderFleet();
    fireEvent.click(await screen.findByRole("button", { name: "Pull patchbay" }));
    fireEvent.click(await screen.findByRole("button", { name: "Pull" }));

    await waitFor(() =>
      expect(mockToast.warning).toHaveBeenCalledWith(expect.stringContaining("since preview")),
    );
    expect(mockToast.success).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("edits fields, adopts discovery, previews the diff, and applies the exact plan", async () => {
    renderFleet();
    fireEvent.click(await screen.findByRole("button", { name: "Manage manifest" }));

    expect(await screen.findByText("Manage fleet manifest")).toBeDefined();
    fireEvent.change(screen.getByLabelText("Hub patchbay"), { target: { value: "backup" } });
    fireEvent.change(screen.getByLabelText("Authority patchbay"), {
      target: { value: "gamma" },
    });
    fireEvent.change(screen.getByLabelText("Branch patchbay"), {
      target: { value: "stable" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add stray" }));
    fireEvent.click(screen.getByRole("button", { name: "Preview changes" }));

    expect(mockInvoke).toHaveBeenCalledWith("fleet_manifest_update", {
      request: {
        mode: "preview",
        base: MANIFEST_SNAPSHOT,
        repos: MANIFEST_PLAN.manifest.repos,
      },
    });
    expect(await screen.findByText("Update patchbay: alpha / alpha / main → backup / gamma / stable")).toBeDefined();
    expect(screen.getByText("Add stray: alpha / alpha / main")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("fleet_manifest_update", {
        request: { mode: "apply", plan: MANIFEST_PLAN },
      }),
    );
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("bootstraps a missing repo through preview and sends the exact plan on confirm", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "fleet_status":
          return Promise.resolve(STATUS);
        case "fleet_plan_bootstrap":
          return Promise.resolve(BOOTSTRAP_PLAN);
        case "fleet_apply_bootstrap":
          return Promise.resolve(BOOTSTRAP_OUTCOME);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderFleet();

    fireEvent.click(
      await screen.findByRole("button", { name: "Bootstrap prompt-optimizer" }),
    );
    expect(mockInvoke).toHaveBeenCalledWith("fleet_plan_bootstrap", {
      repos: ["prompt-optimizer"],
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_bootstrap", expect.anything());
    expect(await screen.findByText("Bootstrap repository")).toBeDefined();
    expect(screen.getByText("main@385f5f0")).toBeDefined();
    expect(screen.getByText("/Users/me/Projects/prompt-optimizer")).toBeDefined();

    fireEvent.click(screen.getByRole("button", { name: "Bootstrap" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("fleet_apply_bootstrap", {
        plan: BOOTSTRAP_PLAN,
      }),
    );
  });

  // Losing your own repository is exactly when you need it back, so the
  // authority machine gets the same Bootstrap action as anyone else. (Pull
  // stays gated — that one could let the hub overwrite the source of truth.)
  it("offers bootstrap for a missing repository even on its authority machine", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") {
        return Promise.resolve({
          ...STATUS,
          repos: [
            STATUS.repos[0],
            { ...STATUS.repos[1], authority: STATUS.machine },
          ],
        } satisfies FleetStatus);
      }
      return Promise.resolve(undefined);
    });

    renderFleet();

    await screen.findByText("prompt-optimizer");
    expect(
      screen.getByRole("button", { name: "Bootstrap prompt-optimizer" }),
    ).toBeDefined();
    expect(
      screen.queryByRole("button", { name: "Pull prompt-optimizer" }),
    ).toBeNull();
  });

  it("does not open confirmation or apply when bootstrap preview refuses", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_plan_bootstrap") {
        return Promise.resolve({
          ...BOOTSTRAP_PLAN,
          ok: false,
          items: [
            {
              repo: "prompt-optimizer",
              status: "refused",
              reason_code: "target_exists",
              message: "bootstrap target already exists",
              evidence: null,
            },
          ],
        } satisfies FleetBootstrapPlan);
      }
      return Promise.resolve(undefined);
    });
    renderFleet();

    fireEvent.click(
      await screen.findByRole("button", { name: "Bootstrap prompt-optimizer" }),
    );
    await waitFor(() =>
      expect(mockToast.error).toHaveBeenCalledWith(expect.stringContaining("already exists")),
    );
    expect(screen.queryByText("Bootstrap repository")).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("fleet_apply_bootstrap", expect.anything());
  });

  it("reports bootstrap apply conflict without success and refreshes the matrix", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "fleet_status":
          return Promise.resolve(STATUS);
        case "fleet_plan_bootstrap":
          return Promise.resolve(BOOTSTRAP_PLAN);
        case "fleet_apply_bootstrap":
          return Promise.resolve({
            ok: false,
            machine: "alpha",
            items: [
              {
                repo: "prompt-optimizer",
                action: "conflict",
                reason_code: "plan_conflict",
                message: "hub branch changed after preview",
                after_head: null,
              },
            ],
          } satisfies FleetBootstrapOutcome);
        default:
          return Promise.resolve(undefined);
      }
    });
    renderFleet();
    fireEvent.click(
      await screen.findByRole("button", { name: "Bootstrap prompt-optimizer" }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Bootstrap" }));

    await waitFor(() =>
      expect(mockToast.warning).toHaveBeenCalledWith(expect.stringContaining("since preview")),
    );
    expect(mockToast.success).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === "fleet_status")).toHaveLength(2),
    );
  });

  it("cancels a manifest diff with zero apply writes", async () => {
    renderFleet();
    fireEvent.click(await screen.findByRole("button", { name: "Manage manifest" }));
    await screen.findByText("Manage fleet manifest");
    fireEvent.click(screen.getByRole("button", { name: "Remove patchbay" }));

    const removePlan: FleetManifestUpdatePlan = {
      ...MANIFEST_PLAN,
      manifest: { ...MANIFEST_SNAPSHOT.manifest, repos: [] },
      changes: [
        {
          action: "remove",
          repo: "patchbay",
          before: MANIFEST_SNAPSHOT.manifest.repos[0],
          after: null,
        },
      ],
    };
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_manifest_get") return Promise.resolve(MANIFEST_SNAPSHOT);
      if (cmd === "fleet_discover") return Promise.resolve({ unlisted: [] });
      if (cmd === "fleet_manifest_update") {
        const request = (args as { request: { mode: string } }).request;
        if (request.mode === "preview") return Promise.resolve({ mode: "preview", plan: removePlan });
      }
      return Promise.resolve(undefined);
    });
    fireEvent.click(screen.getByRole("button", { name: "Preview changes" }));
    expect(await screen.findByText("Remove patchbay: alpha / alpha / main")).toBeDefined();
    fireEvent.click(screen.getAllByRole("button", { name: "Cancel" })[1]);

    expect(
      mockInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "fleet_manifest_update" &&
          (args as { request?: { mode?: string } })?.request?.mode === "apply",
      ),
    ).toBe(false);
  });

  it("reports manifest apply conflict without a success toast", async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "fleet_status") return Promise.resolve(STATUS);
      if (cmd === "fleet_manifest_get") return Promise.resolve(MANIFEST_SNAPSHOT);
      if (cmd === "fleet_discover") return Promise.resolve({ unlisted: [] });
      if (cmd === "fleet_manifest_update") {
        const request = (args as { request: { mode: string } }).request;
        return request.mode === "preview"
          ? Promise.resolve({ mode: "preview", plan: MANIFEST_PLAN })
          : Promise.resolve({
              mode: "apply",
              outcome: {
                ok: false,
                action: "conflict",
                pushed: false,
                commit: null,
                manifest_digest: "manifest-v2",
                changes: MANIFEST_PLAN.changes,
                message: "remote metadata changed after preview",
              },
            });
      }
      return Promise.resolve(undefined);
    });
    renderFleet();
    fireEvent.click(await screen.findByRole("button", { name: "Manage manifest" }));
    await screen.findByText("Manage fleet manifest");
    fireEvent.click(screen.getByRole("button", { name: "Preview changes" }));
    fireEvent.click(await screen.findByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(mockToast.warning).toHaveBeenCalledWith(expect.stringContaining("remote metadata")),
    );
    expect(mockToast.success).not.toHaveBeenCalledWith("Manifest saved");
  });
});

describe("reportAge", () => {
  it("buckets ages like the chain freshness helper", () => {
    const now = Date.parse("2026-07-18T12:00:00Z");
    expect(reportAge("2026-07-18T11:59:55Z", now)).toEqual({
      key: "fleet.age.justNow",
      count: 0,
    });
    expect(reportAge("2026-07-18T11:59:20Z", now)).toEqual({
      key: "fleet.age.secondsAgo",
      count: 40,
    });
    expect(reportAge("2026-07-18T11:30:00Z", now)).toEqual({
      key: "fleet.age.minutesAgo",
      count: 30,
    });
    expect(reportAge("2026-07-10T12:00:00Z", now)).toEqual({
      key: "fleet.age.daysAgo",
      count: 8,
    });
  });
});
