import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { relativeScanTime, SCAN_STALE_MS } from "../lib/chainUi";

/**
 * Shared scan-freshness indicator for the four chain work areas (Issue #19,
 * AC3). Shows "scanned … ago" from the last successful scan's `scannedAt`
 * (epoch ms), a spinning "refreshing…" while a reload is in flight, and a
 * "stale" badge once the last scan ages past {@link SCAN_STALE_MS}. Renders
 * nothing before the first successful scan — the view's own "Scanning…"
 * placeholder covers that.
 */
export function ChainScanStatus({
  scannedAt,
  loading,
}: {
  scannedAt: number | undefined;
  loading: boolean;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());

  // Re-render periodically so the relative label ("2m ago") keeps up without a
  // rescan. 30s granularity matches the minute-level buckets.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  if (!scannedAt) return null;

  const relative = relativeScanTime(scannedAt, now);
  const stale = !loading && now - scannedAt > SCAN_STALE_MS;

  return (
    <div className="mt-1 flex items-center gap-1.5 text-[11.5px] text-faint">
      {loading ? (
        <>
          <RefreshCw className="h-3 w-3 animate-spin text-muted" />
          <span className="text-muted">{t("chain.freshness.refreshing")}</span>
        </>
      ) : (
        <>
          <span>{t(relative.key, { count: relative.count })}</span>
          {stale && (
            <span className="rounded-full border border-amber-500/25 bg-amber-500/10 px-1.5 py-px font-medium text-amber-400">
              {t("chain.freshness.stale")}
            </span>
          )}
        </>
      )}
    </div>
  );
}
