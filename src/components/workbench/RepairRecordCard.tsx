import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ChevronDown, ChevronRight, RotateCcw, ShieldCheck, X } from "lucide-react";
import { cn } from "../../utils";
import { chainDismissRepairRecord, chainUndoRepair } from "../../lib/tauri";
import type { ChainJournalRecord, ChainRepairItem } from "../../lib/tauri";

interface RepairRecordCardProps {
  record: ChainJournalRecord;
  /** Reload the workbench after an undo — the state falls back to the
   * restored fault (or green) from the fresh report. */
  onUndone: () => void;
  onDismissed: () => void;
}

/**
 * 修复记录卡（#31）：apply 留痕后的原型 S4 形态。记录里的条目就是全部事实
 * （软链编辑没有内容 diff）：查看 diff 展开逐项 path: old → new；「撤销」
 * 走 journal 的逐项护栏逆操作，verified 才算成功；「关闭」持久隐藏卡片。
 */
export function RepairRecordCard({ record, onUndone, onDismissed }: RepairRecordCardProps) {
  const { t } = useTranslation();
  const [showDiff, setShowDiff] = useState(false);
  const [busy, setBusy] = useState<"undo" | "dismiss" | null>(null);
  const Chevron = showDiff ? ChevronDown : ChevronRight;

  const undo = async () => {
    setBusy("undo");
    try {
      const outcome = await chainUndoRepair(record.id);
      if (outcome.verified) {
        toast.success(t("chain.workbench.recordUndone"));
      } else {
        // Skipped/conflicting inverses carry their reason per item; the
        // toast points at the report rather than claiming success.
        toast.warning(t("chain.workbench.recordUndoUnverified"));
      }
      onUndone();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  const dismiss = async () => {
    setBusy("dismiss");
    try {
      await chainDismissRepairRecord(record.id);
      onDismissed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div data-testid="repair-record" className="app-glass-card flex gap-3 px-4 py-3.5">
      <span
        className={cn(
          "w-1 shrink-0 self-stretch rounded-full",
          record.verified ? "bg-emerald-400" : "bg-amber-400",
        )}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13.5px] font-semibold text-primary">
            {t("chain.workbench.recordTitle")}
          </span>
          <span
            className={cn(
              "flex items-center gap-1 rounded-full border px-1.5 py-px text-[10.5px] font-medium",
              record.verified
                ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-400"
                : "border-amber-500/25 bg-amber-500/10 text-amber-400",
            )}
          >
            <ShieldCheck className="h-3 w-3" />
            {record.verified
              ? t("chain.workbench.recordVerified")
              : t("chain.workbench.recordUnverified")}
          </span>
          <span className="text-[11.5px] text-faint">
            {new Date(record.created_at * 1000).toLocaleString()}
          </span>
          <button
            data-testid="record-dismiss"
            onClick={() => void dismiss()}
            disabled={busy !== null}
            title={t("chain.workbench.recordClose")}
            className="ml-auto rounded p-1 text-faint transition-colors hover:text-secondary disabled:opacity-50"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>

        <p className="text-[12.5px] text-tertiary">
          {t("chain.workbench.recordSubtitle", { count: record.items.length })}
        </p>

        {showDiff && (
          <ul data-testid="record-diff" className="space-y-1 font-mono text-[11.5px]">
            {record.items.map((item, i) => (
              <DiffLine key={i} item={item} />
            ))}
          </ul>
        )}

        <div className="mt-1 flex flex-wrap gap-2">
          <button
            data-testid="record-toggle-diff"
            onClick={() => setShowDiff((cur) => !cur)}
            aria-expanded={showDiff}
            className="app-button-secondary h-7 px-3 text-[12px]"
          >
            <Chevron className="h-3 w-3" />
            {t(showDiff ? "chain.workbench.recordDiffHide" : "chain.workbench.recordDiff")}
          </button>
          <button
            data-testid="record-undo"
            onClick={() => void undo()}
            disabled={busy !== null}
            className="flex items-center gap-1.5 rounded-full border border-amber-500/25 bg-amber-500/10 px-3 py-1 text-[12px] font-medium text-amber-400 transition-colors hover:bg-amber-500/15 disabled:opacity-50"
          >
            <RotateCcw className="h-3 w-3" />
            {busy === "undo"
              ? t("chain.workbench.recordUndoing")
              : t("chain.workbench.recordUndo")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** One applied edit as a diff line: what kind of link, what was done, and the
 * link's target before → after. Non-writing items render their action only. */
function DiffLine({ item }: { item: ChainRepairItem }) {
  const { t } = useTranslation();
  return (
    <li className="flex flex-wrap items-center gap-1.5">
      <span className="text-tertiary">{t(`chain.doctor.repairKind.${item.kind}`, item.kind)}</span>
      <span
        className={cn(
          "rounded-full border px-1.5 py-px text-[10.5px]",
          item.action === "conflict" || item.action === "error"
            ? "border-red-500/25 bg-red-500/10 text-red-400"
            : item.action === "skip" || item.action === "exists"
              ? "border-border-subtle bg-surface-hover text-muted"
              : "border-accent-border bg-accent/10 text-accent",
        )}
      >
        {t(`chain.doctor.repairAction.${item.action}`, item.action)}
      </span>
      <span className="min-w-0 flex-1 break-all text-muted">
        {item.path}
        {(item.old_target || item.new_target) && (
          <span className="text-faint">
            {" "}
            : {item.old_target ?? "∅"} → {item.new_target ?? "∅"}
          </span>
        )}
      </span>
    </li>
  );
}
