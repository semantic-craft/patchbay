import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, ShieldAlert, ShieldCheck, ShieldQuestion } from "lucide-react";
import { cn } from "../utils";
import { useChain } from "../context/ChainContext";
import { relativeScanTime, SCAN_STALE_MS } from "../lib/chainUi";
import type { ChainGuardViolation } from "../lib/tauri";
import { RemediateDialog } from "./RemediateDialog";

/**
 * The shell's chain bar: the global-surface guard on the left, scan freshness
 * and the one rescan control on the right. It sits above every route because
 * both of its jobs are properties of the machine rather than of a screen.
 *
 * The guard is the product's only hard rule — the global Agent surfaces must
 * stay empty — so a violation cannot be something you have to navigate to. It
 * is loud here and nowhere else; when the surfaces are clean it collapses to a
 * single quiet word so the green case costs no attention.
 *
 * "Clean" is a verdict, never a default: before the first topology lands — or
 * when the scan failed outright — the guard says so instead of pretending the
 * surfaces were checked. A failed RESCAN keeps the previous topology, so the
 * bar then shows the last real verdict plus the error and, in time, the stale
 * badge — a marked stale observation beats "unknown".
 */
export function ChainStatusBar() {
  const { t } = useTranslation();
  const { topo, loading, error, reload } = useChain();
  const [now, setNow] = useState(() => Date.now());
  const [remediate, setRemediate] = useState<{
    violation: ChainGuardViolation;
    agent: string;
  } | null>(null);

  // Keeps the relative label ("2m ago") honest without a rescan.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  const breached = topo?.guard.filter((surface) => surface.state === "violation") ?? [];
  const guardState: "unknown" | "clean" | "violation" =
    topo === null ? "unknown" : breached.length > 0 ? "violation" : "clean";
  const scannedAt = topo?.scanned_at;
  const relative = scannedAt ? relativeScanTime(scannedAt, now) : null;
  const stale = !loading && scannedAt !== undefined && now - scannedAt > SCAN_STALE_MS;

  return (
    <>
      <div
        data-testid="chain-status-bar"
        data-guard={guardState}
        className={cn(
          "flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-hairline px-5 py-2 text-[12.5px]",
          guardState === "violation"
            ? "bg-red-500/[0.07]"
            : "bg-bg-secondary",
        )}
      >
        {guardState === "unknown" ? (
          <span className="flex items-center gap-1.5 text-faint">
            <ShieldQuestion className="h-3.5 w-3.5" />
            {t("chain.guardUnknown")}
          </span>
        ) : guardState === "clean" ? (
          <span className="flex items-center gap-1.5 text-muted">
            <ShieldCheck className="h-3.5 w-3.5" />
            {t("chain.guardOk")}
          </span>
        ) : (
          <>
            <span className="flex items-center gap-1.5 font-semibold text-red-400">
              <ShieldAlert className="h-4 w-4" />
              {t("chain.guardBad")}
            </span>
            {breached.map((surface) => (
              <span key={surface.path} className="flex flex-wrap items-center gap-1">
                <span title={surface.path} className="font-mono text-[11.5px] text-tertiary">
                  {surface.agent}
                </span>
                {surface.violations.map((violation) => (
                  <button
                    key={violation.path}
                    data-testid="guard-violation"
                    onClick={() => setRemediate({ violation, agent: surface.agent })}
                    title={`${violation.final_target}${violation.is_link ? " (symlink)" : ""} — ${t(
                      "chain.remediate.action",
                    )}`}
                    className="rounded border border-red-500/40 px-1.5 py-px font-mono text-[11.5px] font-semibold text-red-400 outline-none transition-colors hover:bg-red-500/10"
                  >
                    {violation.skill}
                  </button>
                ))}
              </span>
            ))}
          </>
        )}

        {/* A failed scan is a property of the machine too — name it on every
            route instead of only inside the workbench. */}
        {error && (
          <span
            data-testid="chain-scan-error"
            title={error}
            className="min-w-0 max-w-[40%] truncate text-[11.5px] text-amber-400"
          >
            {t("chain.scanFailed")}: {error}
          </span>
        )}

        <div className="ml-auto flex items-center gap-2.5 text-[11.5px] text-faint">
          {loading ? (
            <span className="flex items-center gap-1.5 text-muted">
              <RefreshCw className="h-3 w-3 animate-spin" />
              {t("chain.freshness.refreshing")}
            </span>
          ) : (
            relative && (
              <span className="flex items-center gap-1.5">
                {t(relative.key, { count: relative.count })}
                {stale && (
                  <span className="rounded-full border border-amber-500/25 bg-amber-500/10 px-1.5 py-px font-medium text-amber-400">
                    {t("chain.freshness.stale")}
                  </span>
                )}
              </span>
            )
          )}
          <button
            onClick={() => void reload()}
            disabled={loading}
            title={t("chain.rescan")}
            aria-label={t("chain.rescan")}
            className="rounded-full p-1 text-muted outline-none transition-colors hover:text-secondary disabled:opacity-50"
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
          </button>
        </div>
      </div>

      <RemediateDialog
        open={remediate !== null}
        violation={remediate?.violation ?? null}
        agent={remediate?.agent ?? ""}
        projects={topo?.projects ?? []}
        onClose={() => setRemediate(null)}
        onDone={() => void reload()}
      />
    </>
  );
}
