import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { Download, FolderDown, MonitorSmartphone, Plus, RefreshCw, Settings2, Trash2, Upload } from "lucide-react";
import { toast } from "sonner";
import { cn } from "../utils";
import {
  fleetApplyBootstrap,
  fleetApplyPull,
  fleetApplyPush,
  fleetAutoStatus,
  fleetDiscover,
  fleetManifestApply,
  fleetManifestGet,
  fleetManifestPreview,
  fleetPlanBootstrap,
  fleetPlanPull,
  fleetPlanPush,
  fleetReport,
  fleetSetRepoAutoSync,
  fleetStatus,
  setSettings,
} from "../lib/tauri";
import type {
  FleetAutoRoundResult,
  FleetAutoRoundStatus,
  FleetBootstrapPlan,
  FleetCell,
  FleetDiscovery,
  FleetManifestChange,
  FleetManifestRepo,
  FleetManifestSnapshot,
  FleetManifestUpdatePlan,
  FleetMachineColumn,
  FleetPullPlan,
  FleetPushPlan,
  FleetStatus,
} from "../lib/tauri";
import { TONE_BADGE, TONE_DOT, type ChainTone } from "../lib/chainUi";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { REPORT_STALE_MS, reportAge } from "./fleetAge";

const HUB_NOTE_KEY: Record<string, string> = {
  hub_unreachable: "fleet.hubUnreachable",
  branch_missing_on_hub: "fleet.hubMissingBranch",
  hub_head_not_local: "fleet.hubNotLocal",
  branch_off_manifest: "fleet.branchOffManifest",
};

function manifestRepoSummary(repo: FleetManifestRepo): string {
  return `${repo.hub} / ${repo.authority} / ${repo.branch}`;
}

function manifestChangeDetail(
  change: FleetManifestChange,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  if (change.action === "add" && change.after) {
    return t("fleet.manifest.diffAdd", {
      name: change.repo,
      after: manifestRepoSummary(change.after),
    });
  }
  if (change.action === "remove" && change.before) {
    return t("fleet.manifest.diffRemove", {
      name: change.repo,
      before: manifestRepoSummary(change.before),
    });
  }
  return t("fleet.manifest.diffUpdate", {
    name: change.repo,
    before: change.before ? manifestRepoSummary(change.before) : "?",
    after: change.after ? manifestRepoSummary(change.after) : "?",
  });
}

function Badge({ tone, children }: { tone: ChainTone; children: React.ReactNode }) {
  return (
    <span
      className={cn(
        "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
        TONE_BADGE[tone],
      )}
    >
      {children}
    </span>
  );
}

/** One matrix cell: `branch@head` plus dirty / ahead / behind verdict badges.
 * The red tone is reserved for the design's hard flag — the authority machine
 * itself out of sync with the hub. */
function Cell({ cell, isAuthority }: { cell: FleetCell | undefined; isAuthority: boolean }) {
  const { t } = useTranslation();
  if (!cell) {
    return <span className="text-[12px] text-muted">{t("fleet.notReported")}</span>;
  }
  if (!cell.present) {
    return <Badge tone="dim">{t("fleet.missing")}</Badge>;
  }
  const divergence = (cell.ahead ?? 0) + (cell.behind ?? 0);
  const dirty = cell.dirty ?? 0;
  const divergenceTone: ChainTone = divergence === 0 ? "ok" : isAuthority ? "err" : "warn";
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="font-mono text-[12px] text-primary">
        {cell.detached ? t("fleet.detached") : cell.branch}
        {cell.head ? `@${cell.head}` : ""}
      </span>
      {cell.detached && <Badge tone="warn">{t("fleet.detached")}</Badge>}
      {dirty > 0 && <Badge tone="warn">{t("fleet.dirty", { count: dirty })}</Badge>}
      {cell.ahead == null || cell.behind == null ? (
        <Badge tone="dim">
          {cell.note && HUB_NOTE_KEY[cell.note] ? t(HUB_NOTE_KEY[cell.note]) : t("fleet.unknown")}
        </Badge>
      ) : (
        <>
          {cell.ahead > 0 && <Badge tone={divergenceTone}>↑ {t("fleet.ahead", { count: cell.ahead })}</Badge>}
          {cell.behind > 0 && <Badge tone={divergenceTone}>↓ {t("fleet.behind", { count: cell.behind })}</Badge>}
          {divergence === 0 && <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT.ok)} />}
        </>
      )}
    </div>
  );
}

function MachineHeader({ machine, now }: { machine: FleetMachineColumn; now: number }) {
  const { t } = useTranslation();
  const stale =
    machine.reported_at != null && now - Date.parse(machine.reported_at) > REPORT_STALE_MS;
  const age = machine.reported_at ? reportAge(machine.reported_at, now) : null;
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-primary">
        {machine.display_name || machine.id}
        {machine.is_self && (
          <span className="ml-1 normal-case tracking-normal text-muted">({t("fleet.self")})</span>
        )}
      </span>
      {age && (
        <span
          className={cn(
            "text-[10.5px] font-normal normal-case tracking-normal",
            stale ? "text-amber-400" : "text-muted",
          )}
        >
          {t(age.key, { count: age.count })}
          {stale && ` · ${t("fleet.stale")}`}
        </span>
      )}
    </div>
  );
}

export function Fleet() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<FleetStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [reporting, setReporting] = useState(false);
  const [planningRepo, setPlanningRepo] = useState<string | null>(null);
  const [pushPlan, setPushPlan] = useState<FleetPushPlan | null>(null);
  const [pullPlan, setPullPlan] = useState<FleetPullPlan | null>(null);
  const [autoStatus, setAutoStatus] = useState<FleetAutoRoundStatus | null>(null);
  const [savingAutoMode, setSavingAutoMode] = useState(false);
  const [savingAutoRepo, setSavingAutoRepo] = useState<string | null>(null);
  const [manifestSnapshot, setManifestSnapshot] = useState<FleetManifestSnapshot | null>(null);
  const [manifestDraft, setManifestDraft] = useState<FleetManifestRepo[]>([]);
  const [discovery, setDiscovery] = useState<FleetDiscovery | null>(null);
  const [editingManifest, setEditingManifest] = useState(false);
  const [loadingManifest, setLoadingManifest] = useState(false);
  const [manifestPlan, setManifestPlan] = useState<FleetManifestUpdatePlan | null>(null);
  const [bootstrapPlan, setBootstrapPlan] = useState<FleetBootstrapPlan | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextStatus, nextAutoStatus] = await Promise.all([
        fleetStatus(),
        fleetAutoStatus(),
      ]);
      setStatus(nextStatus);
      setAutoStatus(nextAutoStatus);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const unlistenPromise = listen<FleetAutoRoundResult>(
      "fleet-auto-round-completed",
      (event) => {
        if (event.payload.ok) {
          toast.success(
            t("fleet.auto.completed", {
              pulled: event.payload.pulled.length,
              pushed: event.payload.pushed.length,
            }),
          );
        } else {
          const reason = event.payload.attention[0]?.reason || t("fleet.unknown");
          toast.warning(t("fleet.auto.attentionNotification", { reason }));
        }
        void load();
      },
    );
    return () => {
      void unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [load, t]);

  const toggleAutoMode = useCallback(async () => {
    if (!autoStatus) return;
    const enabled = !autoStatus.enabled;
    setSavingAutoMode(true);
    try {
      await setSettings("fleet_auto_mode", enabled ? "on" : "off");
      setAutoStatus((current) => (current ? { ...current, enabled } : current));
    } catch (e) {
      toast.error(`${t("fleet.auto.saveFailed")}: ${String(e)}`);
    } finally {
      setSavingAutoMode(false);
    }
  }, [autoStatus, t]);

  const toggleRepoAutoSync = useCallback(
    async (repo: string, enabled: boolean) => {
      setSavingAutoRepo(repo);
      try {
        await fleetSetRepoAutoSync(repo, enabled);
        setStatus((current) =>
          current
            ? {
                ...current,
                repos: current.repos.map((row) =>
                  row.name === repo ? { ...row, auto_sync: enabled } : row,
                ),
              }
            : current,
        );
      } catch (e) {
        toast.error(`${t("fleet.auto.repoSaveFailed")}: ${String(e)}`);
      } finally {
        setSavingAutoRepo(null);
      }
    },
    [t],
  );

  const report = useCallback(async () => {
    setReporting(true);
    try {
      await fleetReport();
      toast.success(t("fleet.reportDone"));
      await load();
    } catch (e) {
      toast.error(`${t("fleet.reportFailed")}: ${String(e)}`);
    } finally {
      setReporting(false);
    }
  }, [load, t]);

  const startPush = useCallback(
    async (repo: string) => {
      setPlanningRepo(repo);
      try {
        const plan = await fleetPlanPush([repo]);
        const item = plan.items[0];
        if (!item || item.status !== "ready") {
          const detail = item?.reason_code
            ? t(`fleet.push.reason.${item.reason_code}`, {
                defaultValue: item.message || item.reason_code,
              })
            : item?.message || t("fleet.unknown");
          toast.error(
            `${t("fleet.push.failed")}: ${detail}`,
          );
          return;
        }
        setPushPlan(plan);
      } catch (e) {
        toast.error(`${t("fleet.push.failed")}: ${String(e)}`);
      } finally {
        setPlanningRepo(null);
      }
    },
    [t],
  );

  const confirmPush = useCallback(async () => {
    if (!pushPlan) return;
    try {
      const outcome = await fleetApplyPush(pushPlan);
      if (outcome.ok) {
        toast.success(t("fleet.push.done"));
      } else {
        const failed = outcome.items.find(
          (item) => item.action !== "pushed" && item.action !== "up_to_date",
        );
        const detail = failed?.reason_code
          ? t(`fleet.push.reason.${failed.reason_code}`, {
              defaultValue: failed.message || failed.reason_code,
            })
          : failed?.message || t("fleet.unknown");
        const notification = `${t("fleet.push.failed")}: ${detail}`;
        if (failed?.action === "conflict") toast.warning(notification);
        else toast.error(notification);
      }
      await load();
    } catch (e) {
      toast.error(`${t("fleet.push.failed")}: ${String(e)}`);
    }
  }, [load, pushPlan, t]);

  const startPull = useCallback(
    async (repo: string) => {
      setPlanningRepo(repo);
      try {
        const plan = await fleetPlanPull([repo]);
        const item = plan.items[0];
        if (!item || item.status !== "ready") {
          const detail = item?.reason_code
            ? t(`fleet.pull.reason.${item.reason_code}`, {
                defaultValue: item.message || item.reason_code,
              })
            : item?.message || t("fleet.unknown");
          toast.error(`${t("fleet.pull.failed")}: ${detail}`);
          return;
        }
        setPullPlan(plan);
      } catch (e) {
        toast.error(`${t("fleet.pull.failed")}: ${String(e)}`);
      } finally {
        setPlanningRepo(null);
      }
    },
    [t],
  );

  const confirmPull = useCallback(async () => {
    if (!pullPlan) return;
    try {
      const outcome = await fleetApplyPull(pullPlan);
      if (outcome.ok) {
        toast.success(t("fleet.pull.done"));
      } else {
        const failed = outcome.items.find((item) => item.action !== "pulled");
        const detail = failed?.reason_code
          ? t(`fleet.pull.reason.${failed.reason_code}`, {
              defaultValue: failed.message || failed.reason_code,
            })
          : failed?.message || t("fleet.unknown");
        const notification = `${t("fleet.pull.failed")}: ${detail}`;
        if (failed?.action === "conflict") toast.warning(notification);
        else toast.error(notification);
      }
      await load();
    } catch (e) {
      toast.error(`${t("fleet.pull.failed")}: ${String(e)}`);
    }
  }, [load, pullPlan, t]);

  const openManifestEditor = useCallback(async () => {
    setLoadingManifest(true);
    try {
      const [snapshot, found] = await Promise.all([fleetManifestGet(), fleetDiscover()]);
      setManifestSnapshot(snapshot);
      setManifestDraft(snapshot.manifest.repos.map((repo) => ({ ...repo })));
      setDiscovery(found);
      setEditingManifest(true);
    } catch (e) {
      toast.error(`${t("fleet.manifest.loadFailed")}: ${String(e)}`);
    } finally {
      setLoadingManifest(false);
    }
  }, [t]);

  const updateManifestRepo = useCallback(
    (index: number, field: "hub" | "authority" | "branch", value: string) => {
      setManifestDraft((current) =>
        current.map((repo, repoIndex) =>
          repoIndex === index ? { ...repo, [field]: value } : repo,
        ),
      );
    },
    [],
  );

  const addDiscoveredRepo = useCallback(
    (name: string) => {
      if (!manifestSnapshot) return;
      const hub = Object.keys(manifestSnapshot.manifest.hubs)[0] ?? "";
      setManifestDraft((current) =>
        current.some((repo) => repo.name === name)
          ? current
          : [
              ...current,
              { name, hub, authority: manifestSnapshot.machine, branch: "main" },
            ],
      );
    },
    [manifestSnapshot],
  );

  const previewManifest = useCallback(async () => {
    if (!manifestSnapshot) return;
    try {
      setManifestPlan(await fleetManifestPreview(manifestSnapshot, manifestDraft));
    } catch (e) {
      toast.error(`${t("fleet.manifest.previewFailed")}: ${String(e)}`);
    }
  }, [manifestDraft, manifestSnapshot, t]);

  const confirmManifest = useCallback(async () => {
    if (!manifestPlan) return;
    try {
      const outcome = await fleetManifestApply(manifestPlan);
      if (outcome.ok) {
        toast.success(t("fleet.manifest.saved"));
        setEditingManifest(false);
        setManifestSnapshot(null);
      } else {
        toast.warning(`${t("fleet.manifest.conflict")}: ${outcome.message || t("fleet.unknown")}`);
        setEditingManifest(false);
      }
      await load();
    } catch (e) {
      toast.error(`${t("fleet.manifest.saveFailed")}: ${String(e)}`);
    }
  }, [load, manifestPlan, t]);
  const startBootstrap = useCallback(
    async (repo: string) => {
      setPlanningRepo(repo);
      try {
        const plan = await fleetPlanBootstrap([repo]);
        const item = plan.items[0];
        if (!item || item.status !== "ready") {
          const detail = item?.reason_code
            ? t(`fleet.bootstrap.reason.${item.reason_code}`, {
                defaultValue: item.message || item.reason_code,
              })
            : item?.message || t("fleet.unknown");
          toast.error(`${t("fleet.bootstrap.failed")}: ${detail}`);
          return;
        }
        setBootstrapPlan(plan);
      } catch (e) {
        toast.error(`${t("fleet.bootstrap.failed")}: ${String(e)}`);
      } finally {
        setPlanningRepo(null);
      }
    },
    [t],
  );

  const confirmBootstrap = useCallback(async () => {
    if (!bootstrapPlan) return;
    try {
      const outcome = await fleetApplyBootstrap(bootstrapPlan);
      if (outcome.ok) {
        toast.success(t("fleet.bootstrap.done"));
      } else {
        const failed = outcome.items.find((item) => item.action !== "bootstrapped");
        const detail = failed?.reason_code
          ? t(`fleet.bootstrap.reason.${failed.reason_code}`, {
              defaultValue: failed.message || failed.reason_code,
            })
          : failed?.message || t("fleet.unknown");
        const notification = `${t("fleet.bootstrap.failed")}: ${detail}`;
        if (failed?.action === "conflict") toast.warning(notification);
        else toast.error(notification);
      }
      await load();
    } catch (e) {
      toast.error(`${t("fleet.bootstrap.failed")}: ${String(e)}`);
    }
  }, [bootstrapPlan, load, t]);

  const now = Date.now();

  return (
    <div className="app-page">
      <div className="app-page-header app-toolbar">
        <div>
          <h1 className="app-page-title flex items-center gap-2">
            <MonitorSmartphone className="h-5 w-5 text-accent" /> {t("fleet.title")}
          </h1>
          <p className="app-page-subtitle">{t("fleet.subtitle")}</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            className="app-button-secondary"
            onClick={() => void openManifestEditor()}
            disabled={loading || loadingManifest}
          >
            <Settings2 className={cn("h-4 w-4", loadingManifest && "animate-pulse")} />{" "}
            {loadingManifest ? t("fleet.manifest.loading") : t("fleet.manifest.manage")}
          </button>
          <button
            className="app-button-secondary"
            onClick={() => void report()}
            disabled={loading || reporting}
          >
            <Upload className={cn("h-4 w-4", reporting && "animate-pulse")} /> {t("fleet.report")}
          </button>
          <button
            className="app-button-secondary"
            onClick={() => void load()}
            disabled={loading}
          >
            <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} /> {t("fleet.refresh")}
          </button>
        </div>
      </div>

      {autoStatus && (
        <div className="app-panel p-4">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h2 className="text-[14px] font-semibold text-secondary">
                {t("fleet.auto.title")}
              </h2>
              <p className="mt-1 text-[12px] leading-5 text-muted">
                {t("fleet.auto.description")}
              </p>
              <p className="mt-2 text-[12px] text-muted">
                {autoStatus.last_round ? (
                  autoStatus.last_round.ok ? (
                    t("fleet.auto.lastSuccess", {
                      pulled: autoStatus.last_round.pulled.length,
                      pushed: autoStatus.last_round.pushed.length,
                    })
                  ) : (
                    t("fleet.auto.lastAttention", {
                      reason: autoStatus.last_round.attention[0]?.reason || t("fleet.unknown"),
                    })
                  )
                ) : (
                  t("fleet.auto.neverRun")
                )}
                {autoStatus.in_backoff && ` · ${t("fleet.auto.backoff")}`}
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-label={t("fleet.auto.globalLabel")}
              aria-checked={autoStatus.enabled}
              onClick={() => void toggleAutoMode()}
              disabled={savingAutoMode}
              className={cn(
                "relative mt-0.5 inline-flex h-4 w-7 shrink-0 items-center rounded-full outline-none transition-colors focus-visible:ring-2 focus-visible:ring-accent",
                autoStatus.enabled ? "bg-emerald-500" : "bg-zinc-300 dark:bg-zinc-600",
                savingAutoMode ? "cursor-wait opacity-60" : "cursor-pointer",
              )}
            >
              <span
                className={cn(
                  "inline-flex h-3 w-3 rounded-full bg-white shadow transition-transform",
                  autoStatus.enabled ? "translate-x-3.5" : "translate-x-0.5",
                )}
              />
            </button>
          </div>
        </div>
      )}

      {error && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("fleet.scanFailed")}: {error}
        </div>
      )}
      {loading && !status && <div className="p-4 text-[13px] text-muted">{t("fleet.scanning")}</div>}

      {status?.meta_warning && (
        <div className="app-panel border-amber-500/30 p-3 text-[12.5px] text-amber-400">
          {t("fleet.metaStale", { error: status.meta_warning })}
        </div>
      )}
      {status && status.warnings.length > 0 && (
        <div className="app-panel-muted p-3 text-[12px] text-muted">
          <span className="font-medium">{t("fleet.warnings")}: </span>
          {status.warnings.join("; ")}
        </div>
      )}

      {editingManifest && manifestSnapshot && (
        <div className="app-panel p-4">
          <div className="mb-3 flex items-start justify-between gap-4">
            <div>
              <h2 className="text-[14px] font-semibold text-primary">
                {t("fleet.manifest.title")}
              </h2>
              <p className="mt-1 text-[12px] text-muted">{t("fleet.manifest.removeHint")}</p>
            </div>
            <button
              className="app-button-secondary px-2.5 py-1.5 text-[12px]"
              onClick={() => setEditingManifest(false)}
            >
              {t("common.cancel")}
            </button>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full min-w-[680px] border-collapse text-left">
              <thead>
                <tr className="border-b border-border-subtle text-[11px] uppercase tracking-[0.05em] text-muted">
                  <th className="px-2 py-2">{t("fleet.repo")}</th>
                  <th className="px-2 py-2">{t("fleet.manifest.hub")}</th>
                  <th className="px-2 py-2">{t("fleet.authority")}</th>
                  <th className="px-2 py-2">{t("fleet.manifest.branch")}</th>
                  <th className="px-2 py-2" />
                </tr>
              </thead>
              <tbody>
                {manifestDraft.map((repo, index) => (
                  <tr key={repo.name} className="border-b border-border-subtle last:border-0">
                    <td className="px-2 py-2 text-[13px] font-medium text-primary">{repo.name}</td>
                    <td className="px-2 py-2">
                      <select
                        className="app-input min-w-28"
                        aria-label={t("fleet.manifest.hubFor", { name: repo.name })}
                        value={repo.hub}
                        onChange={(event) => updateManifestRepo(index, "hub", event.target.value)}
                      >
                        {Object.keys(manifestSnapshot.manifest.hubs).map((hub) => (
                          <option key={hub} value={hub}>{hub}</option>
                        ))}
                      </select>
                    </td>
                    <td className="px-2 py-2">
                      <select
                        className="app-input min-w-28"
                        aria-label={t("fleet.manifest.authorityFor", { name: repo.name })}
                        value={repo.authority}
                        onChange={(event) =>
                          updateManifestRepo(index, "authority", event.target.value)
                        }
                      >
                        <option value="shared">{t("fleet.authorityShared")}</option>
                        {manifestSnapshot.known_machines.map((machine) => (
                          <option key={machine} value={machine}>{machine}</option>
                        ))}
                      </select>
                    </td>
                    <td className="px-2 py-2">
                      <input
                        className="app-input min-w-28"
                        aria-label={t("fleet.manifest.branchFor", { name: repo.name })}
                        value={repo.branch}
                        onChange={(event) =>
                          updateManifestRepo(index, "branch", event.target.value)
                        }
                      />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <button
                        className="app-button-secondary px-2 py-1 text-[11px]"
                        aria-label={t("fleet.manifest.removeFor", { name: repo.name })}
                        onClick={() =>
                          setManifestDraft((current) =>
                            current.filter((_, repoIndex) => repoIndex !== index),
                          )
                        }
                      >
                        <Trash2 className="h-3.5 w-3.5" /> {t("fleet.manifest.remove")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {discovery &&
            discovery.unlisted.filter(
              (found) => !manifestDraft.some((repo) => repo.name === found.name),
            ).length > 0 && (
              <div className="mt-4 border-t border-border-subtle pt-3">
                <div className="mb-2 text-[12px] font-medium text-secondary">
                  {t("fleet.manifest.discovered")}
                </div>
                <div className="flex flex-wrap gap-2">
                  {discovery.unlisted
                    .filter((found) => !manifestDraft.some((repo) => repo.name === found.name))
                    .map((found) => (
                      <button
                        key={found.name}
                        className="app-button-secondary px-2.5 py-1.5 text-[12px]"
                        aria-label={t("fleet.manifest.addFor", { name: found.name })}
                        onClick={() => addDiscoveredRepo(found.name)}
                      >
                        <Plus className="h-3.5 w-3.5" /> {found.name}
                      </button>
                    ))}
                </div>
              </div>
            )}

          <div className="mt-4 flex justify-end">
            <button className="app-button-primary" onClick={() => void previewManifest()}>
              {t("fleet.manifest.preview")}
            </button>
          </div>
        </div>
      )}

      {status && status.repos.length === 0 && !loading && (
        <div className="app-panel-muted p-4 text-[13px] text-muted">{t("fleet.empty")}</div>
      )}

      {status && status.repos.length > 0 && (
        <div className="app-panel overflow-x-auto">
          <table className="w-full min-w-[720px] border-collapse text-left">
            <thead>
              <tr className="border-b border-border-subtle">
                <th className="px-4 py-2.5 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted">
                  {t("fleet.repo")}
                </th>
                {status.machines.map((machine) => (
                  <th
                    key={machine.id}
                    className="px-4 py-2.5 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted"
                  >
                    <MachineHeader machine={machine} now={now} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {status.repos.map((row) => (
                <tr key={row.name} className="border-b border-border-subtle last:border-b-0 align-top">
                  <td className="px-4 py-2.5">
                    <div className="flex flex-col gap-0.5">
                      <span className="text-[13px] font-medium text-primary">{row.name}</span>
                      <span className="text-[11px] text-muted">
                        {t("fleet.authority")}:{" "}
                        {row.authority === "shared" ? t("fleet.authorityShared") : row.authority}
                        {" · "}
                        <span className="font-mono">
                          {row.hub}
                          {row.hub_head ? `@${row.hub_head}` : ""}
                        </span>
                        {row.hub_note && HUB_NOTE_KEY[row.hub_note] && (
                          <span className="text-amber-400"> · {t(HUB_NOTE_KEY[row.hub_note])}</span>
                        )}
                      </span>
                      <button
                        type="button"
                        role="switch"
                        aria-label={t("fleet.auto.repoLabel", { name: row.name })}
                        aria-checked={row.auto_sync}
                        onClick={() => void toggleRepoAutoSync(row.name, !row.auto_sync)}
                        disabled={savingAutoRepo !== null}
                        className={cn(
                          "mt-1 inline-flex w-fit items-center gap-1.5 text-[11px]",
                          row.auto_sync ? "text-emerald-500" : "text-muted",
                          savingAutoRepo === row.name && "cursor-wait opacity-60",
                        )}
                      >
                        <span
                          className={cn(
                            "h-2.5 w-2.5 rounded-full border",
                            row.auto_sync
                              ? "border-emerald-500 bg-emerald-500"
                              : "border-border-subtle bg-surface",
                          )}
                        />
                        {row.auto_sync ? t("fleet.auto.repoOn") : t("fleet.auto.repoOff")}
                      </button>
                    </div>
                  </td>
                  {status.machines.map((machine) => (
                    <td
                      key={machine.id}
                      className={cn("px-4 py-2.5", !machine.is_self && "opacity-75")}
                    >
                      <div className="flex flex-col items-start gap-2">
                        <Cell
                          cell={row.cells[machine.id]}
                          isAuthority={row.authority === machine.id}
                        />
                        {machine.is_self &&
                          row.cells[machine.id]?.present === false && (
                            <button
                              className="app-button-secondary px-2 py-1 text-[11px]"
                              aria-label={t("fleet.bootstrap.actionFor", { name: row.name })}
                              onClick={() => void startBootstrap(row.name)}
                              disabled={planningRepo !== null}
                            >
                              <FolderDown
                                className={cn(
                                  "h-3.5 w-3.5",
                                  planningRepo === row.name && "animate-pulse",
                                )}
                              />
                              {planningRepo === row.name
                                ? t("fleet.bootstrap.planning")
                                : t("fleet.bootstrap.action")}
                            </button>
                          )}
                        {machine.is_self &&
                          row.cells[machine.id]?.present === true &&
                          (row.authority === status.machine || row.authority === "shared") && (
                            <button
                              className="app-button-secondary px-2 py-1 text-[11px]"
                              aria-label={t("fleet.push.actionFor", { name: row.name })}
                              onClick={() => void startPush(row.name)}
                              disabled={planningRepo !== null}
                            >
                              <Upload
                                className={cn(
                                  "h-3.5 w-3.5",
                                  planningRepo === row.name && "animate-pulse",
                                )}
                              />
                              {planningRepo === row.name
                                ? t("fleet.push.planning")
                                : t("fleet.push.action")}
                            </button>
                          )}
                        {machine.is_self &&
                          row.cells[machine.id]?.present === true &&
                          (row.authority !== status.machine || row.authority === "shared") && (
                            <button
                              className="app-button-secondary px-2 py-1 text-[11px]"
                              aria-label={t("fleet.pull.actionFor", { name: row.name })}
                              onClick={() => void startPull(row.name)}
                              disabled={planningRepo !== null}
                            >
                              <Download
                                className={cn(
                                  "h-3.5 w-3.5",
                                  planningRepo === row.name && "animate-pulse",
                                )}
                              />
                              {planningRepo === row.name
                                ? t("fleet.pull.planning")
                                : t("fleet.pull.action")}
                            </button>
                          )}
                      </div>
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <ConfirmDialog
        open={pushPlan !== null}
        title={t("fleet.push.previewTitle")}
        message={t("fleet.push.previewMessage", { name: pushPlan?.items[0]?.repo ?? "" })}
        details={pushPlan?.items.flatMap((item) =>
          item.evidence
            ? [
                `${item.evidence.branch}@${item.evidence.head_oid.slice(0, 7)}`,
                item.evidence.remote_url,
              ]
            : [],
        )}
        confirmLabel={t("fleet.push.action")}
        tone="warning"
        onClose={() => setPushPlan(null)}
        onConfirm={confirmPush}
      />
      <ConfirmDialog
        open={pullPlan !== null}
        title={t("fleet.pull.previewTitle")}
        message={t("fleet.pull.previewMessage", { name: pullPlan?.items[0]?.repo ?? "" })}
        details={pullPlan?.items.flatMap((item) =>
          item.evidence
            ? [
                `${item.evidence.branch}@${item.evidence.head_oid.slice(0, 7)} → ${item.evidence.target_oid.slice(0, 7)}`,
                item.evidence.hub_url,
              ]
            : [],
        )}
        confirmLabel={t("fleet.pull.action")}
        tone="warning"
        onClose={() => setPullPlan(null)}
        onConfirm={confirmPull}
      />
      <ConfirmDialog
        open={manifestPlan !== null}
        title={t("fleet.manifest.previewTitle")}
        message={t("fleet.manifest.previewMessage")}
        details={manifestPlan?.changes.map((change) => manifestChangeDetail(change, t))}
        confirmLabel={t("fleet.manifest.save")}
        tone="warning"
        onClose={() => setManifestPlan(null)}
        onConfirm={confirmManifest}
      />
      <ConfirmDialog
        open={bootstrapPlan !== null}
        title={t("fleet.bootstrap.previewTitle")}
        message={t("fleet.bootstrap.previewMessage", {
          name: bootstrapPlan?.items[0]?.repo ?? "",
        })}
        details={bootstrapPlan?.items.flatMap((item) =>
          item.evidence
            ? [
                `${item.evidence.branch}@${item.evidence.target_oid.slice(0, 7)}`,
                item.evidence.target_path,
                item.evidence.hub_url,
              ]
            : [],
        )}
        confirmLabel={t("fleet.bootstrap.action")}
        tone="warning"
        onClose={() => setBootstrapPlan(null)}
        onConfirm={confirmBootstrap}
      />
    </div>
  );
}
