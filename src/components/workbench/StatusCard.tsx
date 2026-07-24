import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { relativeScanTime } from "../../lib/chainUi";

interface StatusCardProps {
  count: number;
  scannedAt: number;
}

/** Workbench 全绿状态卡：链路无需处理时的唯一主区内容——一个 ✓、条数与上次扫描时间。 */
export function StatusCard({ count, scannedAt }: StatusCardProps) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());

  // Match ChainScanStatus's cadence so the two relative labels never disagree.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  const relative = relativeScanTime(scannedAt, now);

  return (
    <div
      data-testid="workbench-green"
      className="app-glass-card flex flex-col items-center gap-2.5 px-5 py-12 text-center"
    >
      <span className="flex h-11 w-11 items-center justify-center rounded-full border border-emerald-500/30 bg-emerald-500/10 text-emerald-400">
        <Check className="h-5 w-5" strokeWidth={3} />
      </span>
      <span className="text-[15px] font-semibold text-primary">
        {t("chain.workbench.greenTitle", { count })}
      </span>
      <span className="text-[12.5px] text-tertiary">
        {t(relative.key, { count: relative.count })} · {t("chain.workbench.greenSubtitle")}
      </span>
    </div>
  );
}
