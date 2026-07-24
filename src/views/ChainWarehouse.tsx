import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, GitBranch, Download, GitFork } from "lucide-react";
import { cn } from "../utils";
import {
  getChainTopology,
  getChainDuplicates,
  chainPlanPull,
  chainApplyPull,
  chainPlanForkSync,
  chainApplyForkSync,
} from "../lib/tauri";
import type {
  ChainDuplicateCheckout,
  ChainDuplicatesReport,
  ChainRepo,
  ChainTopology,
  ChainPullPlan,
  ChainPullPreview,
  ChainPullResult,
  ChainForkSyncPlan,
  ChainForkSyncPreview,
  ChainForkSyncResult,
} from "../lib/tauri";
import { HEALTH_TONE, TONE_BADGE, TONE_DOT, shortenPath, type ChainTone } from "../lib/chainUi";
import { ChainScanStatus } from "../components/ChainScanStatus";

/** Verdict tone for a pull skip/error reason code. */
function pullReasonTone(reason: string | null): ChainTone {
  switch (reason) {
    case "dirty":
    case "diverged":
    case "ahead":
    case "untracked_collision":
      return "warn";
    case "auth":
    case "network":
    case "fetch":
    case "checkout":
    case "scan_error":
      return "err";
    default:
      return "dim";
  }
}

/** Verdict tone for a fork-sync skip/error reason code. */
function forkReasonTone(reason: string | null): ChainTone {
  switch (reason) {
    case "dirty":
    case "diverged":
    case "untracked_collision":
      return "warn";
    case "auth":
    case "network":
    case "fetch":
    case "checkout":
      return "err";
    default:
      return "dim";
  }
}

interface Badge {
  text: string;
  tone: ChainTone;
  title?: string;
}

/**
 * Original Repositories work area (Issue #12): repository health (clean/dirty,
 * ahead/behind/diverged, missing tracking, scan error), the origin and optional
 * upstream remotes shown distinctly, and the registered projects that currently
 * depend on each repository. Selected repositories can be fast-forward pulled
 * (Issue #14) or fork-synchronized upstream → origin (Issue #15); both mutating
 * flows preview first and fast-forward only.
 */
export function ChainWarehouse() {
  const { t } = useTranslation();
  const [topo, setTopo] = useState<ChainTopology | null>(null);
  const [duplicates, setDuplicates] = useState<ChainDuplicatesReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Fast-forward pull (Issue #14): select repositories, preview their FF/skip
  // actions, apply, and show per-repo results. Mutating but fast-forward only.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [pullPlan, setPullPlan] = useState<ChainPullPlan | null>(null);
  const [pullResults, setPullResults] = useState<ChainPullResult[] | null>(null);
  const [pullBusy, setPullBusy] = useState(false);
  const [pullError, setPullError] = useState<string | null>(null);

  // Fork synchronization (Issue #15): preview upstream → origin fast-forwards
  // for selected forks, confirm, then apply — which fast-forward PUSHES to
  // origin. Never force-pushes, rebases, or rewrites history.
  const [forkPlan, setForkPlan] = useState<ChainForkSyncPlan | null>(null);
  const [forkResults, setForkResults] = useState<ChainForkSyncResult[] | null>(null);
  const [forkBusy, setForkBusy] = useState(false);
  const [forkError, setForkError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [topology, dupes] = await Promise.all([getChainTopology(), getChainDuplicates()]);
      setTopo(topology);
      setDuplicates(dupes);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const toggleSelect = useCallback((path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const previewPull = useCallback(async () => {
    setPullBusy(true);
    setPullError(null);
    setPullResults(null);
    try {
      const plan = await chainPlanPull([...selected]);
      setPullPlan(plan);
    } catch (e) {
      setPullError(String(e));
    } finally {
      setPullBusy(false);
    }
  }, [selected]);

  const applyPull = useCallback(async () => {
    if (!pullPlan) return;
    setPullBusy(true);
    setPullError(null);
    try {
      const outcome = await chainApplyPull(pullPlan);
      setPullResults(outcome.results);
      setPullPlan(null);
      setSelected(new Set());
      // Rescan so health, ahead/behind, and revisions reflect the fast-forward.
      await load();
    } catch (e) {
      setPullError(String(e));
    } finally {
      setPullBusy(false);
    }
  }, [pullPlan, load]);

  const previewForkSync = useCallback(async () => {
    setForkBusy(true);
    setForkError(null);
    setForkResults(null);
    try {
      // Read-only: preview never fetches or pushes.
      const plan = await chainPlanForkSync([...selected]);
      setForkPlan(plan);
    } catch (e) {
      setForkError(String(e));
    } finally {
      setForkBusy(false);
    }
  }, [selected]);

  const applyForkSync = useCallback(async () => {
    if (!forkPlan) return;
    setForkBusy(true);
    setForkError(null);
    try {
      // Explicit confirm only: this fast-forward PUSHES to origin.
      const outcome = await chainApplyForkSync(forkPlan);
      setForkResults(outcome.results);
      setForkPlan(null);
      setSelected(new Set());
      // Rescan so health, ahead/behind, and revisions reflect the sync.
      await load();
    } catch (e) {
      setForkError(String(e));
    } finally {
      setForkBusy(false);
    }
  }, [forkPlan, load]);

  useEffect(() => {
    void load();
  }, [load]);

  const roots = useMemo(() => topo?.warehouse_roots.map((r) => r.root) ?? [], [topo]);
  const multiRoot = (topo?.warehouse_roots.length ?? 0) > 1;

  // Roots that could not be scanned (missing / unreadable). A bad root yields no
  // repos, so without this banner it would vanish silently from the grouping —
  // the per-source error is surfaced explicitly instead (Issue #19, AC4).
  const badRoots = useMemo(
    () => topo?.warehouse_roots.filter((r) => r.status !== "ok") ?? [],
    [topo]
  );

  const trackingBadge = useCallback(
    (repo: ChainRepo): Badge => {
      const { state, ahead, behind } = repo.health;
      const tone = HEALTH_TONE[state];
      switch (state) {
        case "ahead":
          return { text: t("chain.health.ahead", { count: ahead }), tone };
        case "behind":
          return { text: t("chain.health.behind", { count: behind }), tone };
        case "diverged":
          return { text: t("chain.health.diverged", { ahead, behind }), tone };
        default:
          return { text: t(`chain.health.${state}`), tone };
      }
    },
    [t]
  );

  const badgesFor = useCallback(
    (repo: ChainRepo): Badge[] => {
      // A repo that could not be inspected reports only the scan error — the
      // other axes are meaningless until it can be read.
      if (repo.health.state === "scan_error") {
        return [
          {
            text: t("chain.health.scan_error"),
            tone: "err",
            title: repo.health.error ?? undefined,
          },
        ];
      }
      const badges: Badge[] = [];
      badges.push(
        repo.health.dirty
          ? { text: t("chain.dirty"), tone: "warn" }
          : { text: t("chain.clean"), tone: "dim" }
      );
      badges.push(trackingBadge(repo));
      return badges;
    },
    [t, trackingBadge]
  );

  const grouped = useMemo(() => {
    if (!topo) return [] as { root: string; repos: ChainRepo[] }[];
    if (!multiRoot) return [{ root: "", repos: topo.repos }];
    const order = topo.warehouse_roots.map((r) => r.root);
    const byRoot = new Map<string, ChainRepo[]>();
    for (const root of order) byRoot.set(root, []);
    for (const repo of topo.repos) {
      const bucket = byRoot.get(repo.root);
      if (bucket) bucket.push(repo);
      else byRoot.set(repo.root, [repo]);
    }
    return [...byRoot.entries()]
      .filter(([, repos]) => repos.length > 0)
      .map(([root, repos]) => ({ root, repos }));
  }, [topo, multiRoot]);

  const renderCheckout = (checkout: ChainDuplicateCheckout) => {
    const stateTone: ChainTone = checkout.dirty ? "warn" : HEALTH_TONE[checkout.state];
    return (
      <div
        key={checkout.path}
        className="rounded-lg border border-border-subtle bg-surface px-3 py-2"
      >
        <div className="flex flex-wrap items-center gap-2">
          <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[stateTone])} />
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted" title={checkout.path}>
            {shortenPath(checkout.path, roots, topo?.projects_root ?? "")}
          </span>
          <span
            className={cn(
              "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
              TONE_BADGE[stateTone]
            )}
          >
            {checkout.dirty ? t("chain.dirty") : t("chain.clean")}
          </span>
          <span className="rounded-full border border-border-subtle bg-surface-hover px-1.5 py-px font-mono text-[10.5px] font-medium text-muted">
            {checkout.revision
              ? `${t("chain.duplicateRevision")} ${checkout.revision}`
              : t("chain.duplicateNoRevision")}
          </span>
          <span className="rounded-full border border-border-subtle bg-surface-hover px-1.5 py-px text-[10.5px] font-medium text-muted">
            {checkout.referenced_by.length === 0
              ? t("chain.noRefs")
              : t("chain.referencedBy", { count: checkout.referenced_by.length })}
          </span>
        </div>
      </div>
    );
  };

  const renderRepo = (repo: ChainRepo) => {
    const managed = repo.source_kind === "managed";
    const dotTone: ChainTone =
      managed
        ? "ok"
        : repo.health.state === "scan_error"
        ? "err"
        : repo.health.dirty
          ? "warn"
          : HEALTH_TONE[repo.health.state];
    return (
      <div
        key={repo.path}
        className="rounded-lg border border-border-subtle bg-surface px-3.5 py-3"
      >
        <div className="flex flex-wrap items-center gap-2">
          {!managed && (
            <input
              type="checkbox"
              className="h-3.5 w-3.5 shrink-0 cursor-pointer accent-emerald-500"
              checked={selected.has(repo.path)}
              onChange={() => toggleSelect(repo.path)}
              aria-label={t("chain.pull.selectRepo", { name: repo.name })}
            />
          )}
          <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[dotTone])} />
          <span className="text-[13px] font-medium text-secondary">{repo.name}</span>
          {managed && (
            <span className="rounded-full border border-emerald-500/25 bg-emerald-500/10 px-1.5 py-px text-[10.5px] font-medium text-emerald-400">
              {t("chain.managedSource")}
            </span>
          )}
          {repo.health.branch && (
            <span className="flex items-center gap-1 font-mono text-[11px] text-muted">
              <GitBranch className="h-3 w-3" />
              {repo.health.branch}
            </span>
          )}
          <div className="flex flex-wrap gap-1">
            {!managed && badgesFor(repo).map((badge, i) => (
              <span
                key={i}
                title={badge.title}
                className={cn(
                  "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                  TONE_BADGE[badge.tone]
                )}
              >
                {badge.text}
              </span>
            ))}
            <span className="rounded-full border border-border-subtle bg-surface-hover px-1.5 py-px text-[10.5px] font-medium text-muted">
              {t("chain.skillsCount", { count: repo.skills.length })}
            </span>
          </div>
        </div>

        <div className="mt-1.5 truncate font-mono text-[11px] text-faint" title={repo.path}>
          {shortenPath(repo.path, roots, topo?.projects_root ?? "")}
        </div>

        {/* A repo that could not be inspected shows its error inline, not just in
            the badge tooltip, so the per-source failure is never concealed (AC4). */}
        {!managed && repo.health.state === "scan_error" && repo.health.error && (
          <div className="mt-1.5 break-all text-[11px] text-red-400">{repo.health.error}</div>
        )}

        {/* Remotes: origin and optional upstream shown distinctly. */}
        {!managed && <div className="mt-2 space-y-1">
          <div className="flex items-baseline gap-2 font-mono text-[11px]">
            <span className="w-16 shrink-0 font-sans text-tertiary">{t("chain.origin")}</span>
            <span className="min-w-0 flex-1 break-all text-muted">
              {repo.origin?.url || <span className="text-faint">{t("chain.noRemote")}</span>}
            </span>
          </div>
          {repo.upstream && (
            <div className="flex items-baseline gap-2 font-mono text-[11px]">
              <span className="w-16 shrink-0 font-sans text-tertiary">{t("chain.upstream")}</span>
              <span className="min-w-0 flex-1 break-all text-muted">{repo.upstream.url}</span>
            </div>
          )}
        </div>}

        {/* Reverse usage: which registered projects depend on this repo. */}
        <div className="mt-2 flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="text-[11px] font-medium text-tertiary">{t("chain.dependents")}</span>
          {repo.referenced_by.length === 0 ? (
            <span className="text-[11px] text-faint">{t("chain.noDependents")}</span>
          ) : (
            repo.referenced_by.map((ref) => (
              <span
                key={ref.path}
                title={shortenPath(ref.path, roots, topo?.projects_root ?? "")}
                className="rounded-full border border-emerald-500/25 bg-emerald-500/10 px-1.5 py-px text-[10.5px] font-medium text-emerald-400"
              >
                {ref.name}
              </span>
            ))
          )}
        </div>
      </div>
    );
  };

  const pullEligibleCount = useMemo(
    () => pullPlan?.items.filter((item) => item.action === "fast_forward").length ?? 0,
    [pullPlan]
  );

  const forkEligibleCount = useMemo(
    () => forkPlan?.items.filter((item) => item.action === "fast_forward").length ?? 0,
    [forkPlan]
  );

  const renderPullPreview = (item: ChainPullPreview) => {
    const eligible = item.action === "fast_forward";
    const tone: ChainTone = eligible ? "ok" : pullReasonTone(item.reason);
    return (
      <div
        key={item.path}
        className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-surface px-3 py-2"
      >
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[tone])} />
        <span className="text-[12px] font-medium text-secondary">{item.name}</span>
        {item.branch && (
          <span className="flex items-center gap-1 font-mono text-[11px] text-muted">
            <GitBranch className="h-3 w-3" />
            {item.branch}
          </span>
        )}
        <span
          className={cn(
            "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            TONE_BADGE[tone]
          )}
        >
          {eligible
            ? t("chain.pull.action.fast_forward", { count: item.behind })
            : t(`chain.pull.reason.${item.reason ?? "scan_error"}`, {
                defaultValue: item.reason ?? "",
              })}
        </span>
        <span className="ml-auto min-w-0 truncate font-mono text-[11px] text-faint" title={item.path}>
          {shortenPath(item.path, roots, topo?.projects_root ?? "")}
        </span>
      </div>
    );
  };

  const renderPullResult = (result: ChainPullResult) => {
    const tone: ChainTone =
      result.action === "updated"
        ? "ok"
        : result.action === "error"
          ? "err"
          : result.action === "skipped"
            ? pullReasonTone(result.reason)
            : "dim";
    return (
      <div
        key={result.path}
        className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-surface px-3 py-2"
      >
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[tone])} />
        <span className="text-[12px] font-medium text-secondary">{result.name}</span>
        <span
          className={cn(
            "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            TONE_BADGE[tone]
          )}
        >
          {t(`chain.pull.result.${result.action}`)}
        </span>
        {result.action === "updated" && result.from && result.to && (
          <span className="font-mono text-[11px] text-muted">
            {result.from} → {result.to}
          </span>
        )}
        {result.reason && result.action !== "updated" && (
          <span className="text-[11px] text-muted" title={result.message ?? undefined}>
            {t(`chain.pull.reason.${result.reason}`, { defaultValue: result.reason })}
          </span>
        )}
        <span className="ml-auto min-w-0 truncate font-mono text-[11px] text-faint" title={result.path}>
          {shortenPath(result.path, roots, topo?.projects_root ?? "")}
        </span>
      </div>
    );
  };

  const renderForkPreview = (item: ChainForkSyncPreview) => {
    const eligible = item.action === "fast_forward";
    const tone: ChainTone = eligible ? "ok" : forkReasonTone(item.reason);
    return (
      <div
        key={item.path}
        className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-surface px-3 py-2"
      >
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[tone])} />
        <span className="text-[12px] font-medium text-secondary">{item.name}</span>
        {/* Name the source → target explicitly (AC2). */}
        {item.source && item.target && (
          <span className="flex items-center gap-1 font-mono text-[11px] text-muted">
            <GitBranch className="h-3 w-3" />
            {item.source} → {item.target}
          </span>
        )}
        <span
          className={cn(
            "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            TONE_BADGE[tone]
          )}
        >
          {eligible
            ? t("chain.forkSync.action.fast_forward", { count: item.behind })
            : t(`chain.forkSync.reason.${item.reason ?? "ambiguous_branch"}`, {
                defaultValue: item.reason ?? "",
              })}
        </span>
        <span className="ml-auto min-w-0 truncate font-mono text-[11px] text-faint" title={item.path}>
          {shortenPath(item.path, roots, topo?.projects_root ?? "")}
        </span>
      </div>
    );
  };

  const renderForkResult = (result: ChainForkSyncResult) => {
    const tone: ChainTone =
      result.action === "synced"
        ? "ok"
        : result.action === "error"
          ? "err"
          : result.action === "skipped"
            ? forkReasonTone(result.reason)
            : "dim";
    return (
      <div
        key={result.path}
        className="flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-surface px-3 py-2"
      >
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[tone])} />
        <span className="text-[12px] font-medium text-secondary">{result.name}</span>
        <span
          className={cn(
            "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            TONE_BADGE[tone]
          )}
        >
          {t(`chain.forkSync.result.${result.action}`)}
        </span>
        {result.action === "synced" && result.from && result.to && (
          <span className="font-mono text-[11px] text-muted">
            {result.from} → {result.to}
          </span>
        )}
        {result.reason && result.action !== "synced" && (
          <span className="text-[11px] text-muted" title={result.message ?? undefined}>
            {t(`chain.forkSync.reason.${result.reason}`, { defaultValue: result.reason })}
          </span>
        )}
        <span className="ml-auto min-w-0 truncate font-mono text-[11px] text-faint" title={result.path}>
          {shortenPath(result.path, roots, topo?.projects_root ?? "")}
        </span>
      </div>
    );
  };

  return (
    <div className="app-page">
      <div className="app-page-header app-toolbar">
        <div>
          <h1 className="app-page-title">{t("chain.warehouseTitle")}</h1>
          <p className="app-page-subtitle">{t("chain.warehouseSubtitle")}</p>
          <ChainScanStatus scannedAt={topo?.scanned_at} loading={loading} />
        </div>
        <div className="flex items-center gap-2">
          {selected.size > 0 && (
            <button
              className="app-button-secondary"
              onClick={() => void previewPull()}
              disabled={pullBusy || forkBusy || loading}
            >
              <Download className="h-4 w-4" />
              {t("chain.pull.selected", { count: selected.size })}
            </button>
          )}
          {selected.size > 0 && (
            <button
              className="app-button-secondary"
              onClick={() => void previewForkSync()}
              disabled={forkBusy || pullBusy || loading}
            >
              <GitFork className="h-4 w-4" />
              {t("chain.forkSync.selected", { count: selected.size })}
            </button>
          )}
          <button className="app-button-secondary" onClick={() => void load()} disabled={loading}>
            <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
            {t("chain.rescan")}
          </button>
        </div>
      </div>

      {/* Per-source root errors: a missing/unreadable root contributes no repos,
          so surface it explicitly rather than dropping it silently (AC4). */}
      {badRoots.length > 0 && (
        <div
          data-testid="warehouse-root-errors"
          className="app-panel border-red-500/30 p-4"
        >
          <div className="app-section-title mb-2 text-red-400">{t("chain.rootsTitle")}</div>
          <div className="space-y-1">
            {badRoots.map((r) => (
              <div key={r.root} className="flex flex-wrap items-baseline gap-2 text-[12px]">
                <span className="font-mono text-[11.5px] text-red-400">
                  {shortenPath(r.root, [], topo?.projects_root ?? "")}
                </span>
                <span className="font-semibold text-red-400">
                  {t(r.status === "missing" ? "chain.rootMissing" : "chain.rootUnreadable")}
                </span>
                {r.error && <span className="min-w-0 flex-1 break-all text-muted">{r.error}</span>}
              </div>
            ))}
          </div>
        </div>
      )}

      {pullError && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.pull.failed")}: {pullError}
        </div>
      )}

      {pullPlan && (
        <div className="app-panel space-y-2 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <div className="app-section-title">{t("chain.pull.previewTitle")}</div>
              <p className="app-page-subtitle">{t("chain.pull.previewHint")}</p>
            </div>
            <div className="flex items-center gap-2">
              <button
                className="app-button-secondary"
                onClick={() => setPullPlan(null)}
                disabled={pullBusy}
              >
                {t("chain.pull.cancel")}
              </button>
              <button
                className="app-button-primary"
                onClick={() => void applyPull()}
                disabled={pullBusy || pullEligibleCount === 0}
              >
                <Download className={cn("h-4 w-4", pullBusy && "animate-pulse")} />
                {pullBusy
                  ? t("chain.pull.applying")
                  : t("chain.pull.apply", { count: pullEligibleCount })}
              </button>
            </div>
          </div>
          {pullEligibleCount === 0 && (
            <div className="text-[12px] text-amber-400">{t("chain.pull.noEligible")}</div>
          )}
          <div className="space-y-1.5">{pullPlan.items.map(renderPullPreview)}</div>
        </div>
      )}

      {pullResults && (
        <div className="app-panel space-y-2 p-4">
          <div className="flex items-center justify-between gap-2">
            <div className="app-section-title">{t("chain.pull.resultsTitle")}</div>
            <button className="app-button-secondary" onClick={() => setPullResults(null)}>
              {t("chain.pull.close")}
            </button>
          </div>
          <div className="space-y-1.5">{pullResults.map(renderPullResult)}</div>
        </div>
      )}

      {forkError && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.forkSync.failed")}: {forkError}
        </div>
      )}

      {forkPlan && (
        <div className="app-panel space-y-2 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <div className="app-section-title">{t("chain.forkSync.previewTitle")}</div>
              <p className="app-page-subtitle">{t("chain.forkSync.previewHint")}</p>
            </div>
            <div className="flex items-center gap-2">
              <button
                className="app-button-secondary"
                onClick={() => setForkPlan(null)}
                disabled={forkBusy}
              >
                {t("chain.forkSync.cancel")}
              </button>
              <button
                className="app-button-primary"
                onClick={() => void applyForkSync()}
                disabled={forkBusy || forkEligibleCount === 0}
              >
                <GitFork className={cn("h-4 w-4", forkBusy && "animate-pulse")} />
                {forkBusy
                  ? t("chain.forkSync.applying")
                  : t("chain.forkSync.apply", { count: forkEligibleCount })}
              </button>
            </div>
          </div>
          {/* Make the push-to-origin nature unmistakable before confirming. */}
          <div className="text-[12px] text-amber-400">{t("chain.forkSync.pushWarning")}</div>
          {forkEligibleCount === 0 && (
            <div className="text-[12px] text-amber-400">{t("chain.forkSync.noEligible")}</div>
          )}
          <div className="space-y-1.5">{forkPlan.items.map(renderForkPreview)}</div>
        </div>
      )}

      {forkResults && (
        <div className="app-panel space-y-2 p-4">
          <div className="flex items-center justify-between gap-2">
            <div className="app-section-title">{t("chain.forkSync.resultsTitle")}</div>
            <button className="app-button-secondary" onClick={() => setForkResults(null)}>
              {t("chain.forkSync.close")}
            </button>
          </div>
          <div className="space-y-1.5">{forkResults.map(renderForkResult)}</div>
        </div>
      )}

      {error && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.scanFailed")}: {error}
        </div>
      )}
      {loading && !topo && <div className="p-4 text-[13px] text-muted">{t("chain.scanning")}</div>}

      {topo && topo.repos.length === 0 && !loading && (
        <div className="app-panel-muted p-4 text-[13px] text-muted">{t("chain.warehouseEmpty")}</div>
      )}

      {topo &&
        grouped.map((group) => (
          <div key={group.root || "all"} className="space-y-2">
            {multiRoot && (
              <div className="app-section-title" title={group.root}>
                {shortenPath(group.root, [], topo.projects_root)}
                <span className="ml-2 font-normal text-muted">
                  {t("chain.reposCount", { count: group.repos.length })}
                </span>
              </div>
            )}
            <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
              {group.repos.map(renderRepo)}
            </div>
          </div>
        ))}

      {/* Duplicate checkouts: same remote identity across multiple checkouts.
          Read-only evidence for a human to choose an authority — never deleted. */}
      {duplicates && duplicates.groups.length > 0 && (
        <div className="space-y-2">
          <div className="app-section-title">
            {t("chain.duplicatesTitle")}
            <span className="ml-2 font-normal text-muted">
              {t("chain.reposCount", { count: duplicates.groups.length })}
            </span>
          </div>
          <p className="app-page-subtitle">{t("chain.duplicatesSubtitle")}</p>
          {duplicates.groups.map((group) => (
            <div
              key={group.identity}
              className="rounded-lg border border-border-subtle bg-surface px-3.5 py-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <span className="flex items-center gap-1 font-mono text-[12px] font-medium text-secondary">
                  <GitBranch className="h-3 w-3" />
                  {group.identity}
                </span>
                {group.guidance.map((code) => (
                  <span
                    key={code}
                    className={cn(
                      "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                      TONE_BADGE["dim"]
                    )}
                  >
                    {t(`chain.duplicateGuidance.${code}`, { defaultValue: code })}
                  </span>
                ))}
              </div>
              <div className="mt-2 space-y-1.5">{group.checkouts.map(renderCheckout)}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
