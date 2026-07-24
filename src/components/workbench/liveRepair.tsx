/* eslint-disable react-refresh/only-export-components -- hook + panel share
   the live-repair contract; splitting them would separate one seam. */
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Check, Hand, Pause, Play, RotateCcw, X } from "lucide-react";
import { cn } from "../../utils";
import { chainRepairLive, chainRepairLiveControl } from "../../lib/tauri";
import type { ChainLiveEvent } from "../../lib/tauri";

/** The live run's fixed narration script (issue #32, prototype S3). */
export const LIVE_STEPS = ["check", "locate", "rebuild", "verify"] as const;

interface LiveStep {
  status: "start" | "done" | "failed";
  detail: string | null;
}

export interface LiveState {
  runId: string;
  steps: Partial<Record<string, LiveStep>>;
  paused: boolean;
  /** Terminal error — the panel offers retry. */
  failed: string | null;
}

/**
 * 直播修复的共享状态机（#32；#33 的聚合卡复用）：开跑、按事件打勾、
 * 暂停/接管、失败重试。单卡传单 fingerprint，风暴卡传整组 + preferRoot
 * （锚定检测出的新根，同分同名不靠路径排序碰运气）。
 */
export function useLiveRepair(
  fingerprints: string[],
  onRepaired: () => void,
  preferRoot?: string,
) {
  const { t } = useTranslation();
  const [live, setLive] = useState<LiveState | null>(null);

  const run = async () => {
    const runId = `run-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    setLive({ runId, steps: {}, paused: false, failed: null });
    const unlisten = await listen<ChainLiveEvent>("chain-repair-live", (event) => {
      const payload = event.payload;
      if (payload.run_id !== runId) return;
      setLive((cur) =>
        cur && cur.runId === runId
          ? {
              ...cur,
              steps: {
                ...cur.steps,
                [payload.step]: {
                  status: payload.status as LiveStep["status"],
                  detail: payload.detail,
                },
              },
            }
          : cur,
      );
    });
    try {
      const out = await chainRepairLive(fingerprints, runId, preferRoot);
      if (out.aborted) {
        toast.info(t("chain.workbench.liveTakenOver"));
        setLive(null);
      } else if (out.outcome?.verified) {
        toast.success(t("chain.doctor.repairVerified"));
        onRepaired();
      } else {
        toast.warning(t("chain.doctor.repairUnverified"));
        onRepaired();
      }
    } catch (e) {
      setLive((cur) => (cur && cur.runId === runId ? { ...cur, failed: String(e) } : cur));
    } finally {
      unlisten();
    }
  };

  const togglePause = async () => {
    if (!live) return;
    const action = live.paused ? "resume" : "pause";
    try {
      await chainRepairLiveControl(live.runId, action);
      setLive((cur) => (cur ? { ...cur, paused: !cur.paused } : cur));
    } catch (e) {
      toast.error(String(e));
    }
  };

  const takeover = async () => {
    if (!live) return;
    try {
      // The invoke in flight resolves `aborted`; the panel closes there.
      await chainRepairLiveControl(live.runId, "takeover");
    } catch (e) {
      toast.error(String(e));
    }
  };

  return { live, run, togglePause, takeover, close: () => setLive(null) };
}

/** 执行直播面板（#32，原型 S3）：四步逐条打勾，事件驱动；进行中可暂停/
 * 接管，失败态可重试。完成后调用方重扫，#31 的修复记录卡自然衔接。 */
export function LivePanel({
  live,
  onTogglePause,
  onTakeover,
  onRetry,
  onClose,
}: {
  live: LiveState;
  onTogglePause: () => void;
  onTakeover: () => void;
  onRetry: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const doneCount = LIVE_STEPS.filter((step) => live.steps[step]?.status === "done").length;

  return (
    <div
      data-testid="live-panel"
      className="mt-1 space-y-2 rounded-[10px] border border-accent-border bg-accent/[0.04] p-2.5 text-[11.5px]"
    >
      <div className="flex items-center gap-2">
        <span className="app-section-title">{t("chain.workbench.liveTitle")}</span>
        <span
          data-testid="live-progress"
          className={cn(
            "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
            live.failed
              ? "border-red-500/25 bg-red-500/10 text-red-400"
              : "border-amber-500/25 bg-amber-500/10 text-amber-400",
          )}
        >
          {live.failed
            ? t("chain.workbench.liveFailed")
            : live.paused
              ? t("chain.workbench.livePaused")
              : t("chain.workbench.liveProgress", { done: doneCount, total: LIVE_STEPS.length })}
        </span>
      </div>

      <ul className="space-y-1.5">
        {LIVE_STEPS.map((step, index) => {
          const state = live.steps[step];
          const status = state?.status ?? "idle";
          return (
            <li
              key={step}
              data-testid={`live-step-${step}`}
              data-status={status}
              className="flex items-start gap-2"
            >
              <span
                className={cn(
                  "mt-px flex h-4 w-4 shrink-0 items-center justify-center rounded-full border text-[9px] font-bold",
                  status === "done" && "border-emerald-500/40 bg-emerald-500/10 text-emerald-400",
                  status === "failed" && "border-red-500/40 bg-red-500/10 text-red-400",
                  status === "start" && "animate-pulse border-accent-border bg-accent/10 text-accent",
                  status === "idle" && "border-border-subtle text-faint",
                )}
              >
                {status === "done" ? (
                  <Check className="h-2.5 w-2.5" />
                ) : status === "failed" ? (
                  <X className="h-2.5 w-2.5" />
                ) : (
                  index + 1
                )}
              </span>
              <div className="min-w-0 flex-1">
                <span className={cn(status === "idle" ? "text-faint" : "text-secondary")}>
                  {t(`chain.workbench.liveStep.${step}`)}
                </span>
                {state?.detail &&
                  state.detail.split("\n").map((line, i) => (
                    <div key={i} className="break-all font-mono text-[10.5px] text-muted">
                      {line}
                    </div>
                  ))}
              </div>
            </li>
          );
        })}
      </ul>

      {live.failed && <div className="text-red-400">{live.failed}</div>}

      <div className="flex items-center gap-1.5 border-t border-border-subtle pt-2">
        <span className="text-[10.5px] text-faint">{t("chain.workbench.liveNote")}</span>
        <span className="ml-auto flex gap-1.5">
          {live.failed ? (
            <>
              <button
                data-testid="live-retry"
                onClick={onRetry}
                className="flex items-center gap-1 rounded-full border border-accent-border bg-accent/10 px-2.5 py-0.5 font-medium text-accent transition-colors hover:bg-accent/15"
              >
                <RotateCcw className="h-3 w-3" />
                {t("chain.workbench.liveRetry")}
              </button>
              <button
                data-testid="live-close"
                onClick={onClose}
                className="rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 font-medium text-muted transition-colors hover:border-border hover:text-secondary"
              >
                {t("chain.workbench.liveClose")}
              </button>
            </>
          ) : (
            <>
              <button
                data-testid="live-pause"
                onClick={onTogglePause}
                className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 font-medium text-muted transition-colors hover:border-border hover:text-secondary"
              >
                {live.paused ? <Play className="h-3 w-3" /> : <Pause className="h-3 w-3" />}
                {live.paused
                  ? t("chain.workbench.liveResume")
                  : t("chain.workbench.livePause")}
              </button>
              <button
                data-testid="live-takeover"
                onClick={onTakeover}
                className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-hover px-2.5 py-0.5 font-medium text-muted transition-colors hover:border-border hover:text-secondary"
              >
                <Hand className="h-3 w-3" />
                {t("chain.workbench.liveTakeover")}
              </button>
            </>
          )}
        </span>
      </div>
    </div>
  );
}
