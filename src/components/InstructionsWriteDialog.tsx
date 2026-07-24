import { useState } from "react";
import { ArrowLeft, FilePenLine, ShieldAlert, ShieldCheck, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { instructionsApplyInit, instructionsApplyNormalize } from "../lib/tauri";
import type {
  InstructionsInitItem,
  InstructionsInitOutcome,
  InstructionsInitPlan,
  InstructionsNormalizeItem,
  InstructionsNormalizeOutcome,
  InstructionsNormalizePlan,
} from "../lib/tauri";
import { TONE_BADGE } from "../lib/chainUi";
import { cn } from "../utils";

interface Props {
  open: boolean;
  operation: "normalize" | "init";
  projectName: string;
  projectPath: string;
  plan: InstructionsNormalizePlan | InstructionsInitPlan | null;
  onClose: () => void;
  onApplied: () => Promise<void> | void;
}

type WriteItem = InstructionsNormalizeItem | InstructionsInitItem;
type WriteOutcome = InstructionsNormalizeOutcome | InstructionsInitOutcome;

const ACTION_TONE: Record<WriteItem["action"], keyof typeof TONE_BADGE> = {
  create: "ok",
  rewrite: "ok",
  replace_link: "ok",
  noop: "dim",
  conflict: "warn",
};

function itemKey(item: WriteItem) {
  return "fingerprint" in item ? `${item.path}:${item.fingerprint}` : `${item.path}:${item.kind}`;
}

function ItemRow({
  item,
  operation,
  preview,
}: {
  item: WriteItem;
  operation: Props["operation"];
  preview: boolean;
}) {
  const { t } = useTranslation();
  const detail = "rule" in item ? item.rule : item.kind;
  return (
    <div className="space-y-2 rounded-lg border border-border-subtle bg-bg-secondary p-3">
      <div className="flex flex-wrap items-baseline gap-2 font-mono text-[11.5px]">
        <span
          className={cn(
            "shrink-0 rounded-full border px-1.5 py-px font-sans text-[10.5px] font-medium",
            TONE_BADGE[ACTION_TONE[item.action]]
          )}
        >
          {item.action}
        </span>
        <span className="text-faint">{detail}</span>
        <span className="break-all text-secondary">{item.path}</span>
        {"snapshot" in item && item.snapshot && (
          <span className="font-sans text-[10.5px] text-muted">
            {t("instructions.normalize.snapshot")}
          </span>
        )}
      </div>
      {item.message && <div className="break-words text-[11.5px] text-amber-400">{item.message}</div>}
      {preview && item.after_content !== undefined && (
        <div>
          <div className="mb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted">
            {t(`instructions.${operation}.afterContent`)}
          </div>
          <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words rounded border border-border-subtle bg-bg-primary p-2 font-mono text-[11px] leading-relaxed text-tertiary">
            {item.after_content}
          </pre>
        </div>
      )}
    </div>
  );
}

export function InstructionsWriteDialog({
  open,
  operation,
  projectName,
  projectPath,
  plan,
  onClose,
  onApplied,
}: Props) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);
  const [outcome, setOutcome] = useState<WriteOutcome | null>(null);

  if (!open || !plan) return null;

  const close = () => {
    setOutcome(null);
    onClose();
  };

  const apply = async () => {
    setLoading(true);
    try {
      const result =
        operation === "normalize"
          ? await instructionsApplyNormalize(projectPath, plan as InstructionsNormalizePlan)
          : await instructionsApplyInit(projectPath, plan as InstructionsInitPlan);
      setOutcome(result);
      const succeeded = result.verified && result.items.every((item) => item.action !== "conflict");
      if (succeeded) toast.success(t(`instructions.${operation}.verified`));
      else toast.warning(t(`instructions.${operation}.unverified`));
      await onApplied();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  const succeeded = outcome?.verified && outcome.items.every((item) => item.action !== "conflict");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={close} />
      <div className="relative flex max-h-[80vh] w-full max-w-2xl flex-col rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-primary">
            <FilePenLine className="h-4 w-4 text-accent" />
            {t(`instructions.${operation}.title`, { project: projectName })}
          </h2>
          <button onClick={close} className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary">
            <X className="h-4 w-4" />
          </button>
        </div>

        {!outcome ? (
          <>
            <div className="mb-2">
              <div className="app-section-title">{t(`instructions.${operation}.planTitle`)}</div>
              <p className="mt-0.5 text-[12px] text-muted">{t(`instructions.${operation}.planHint`)}</p>
            </div>
            <div className="min-h-0 flex-1 space-y-2 overflow-y-auto">
              {plan.items.map((item) => (
                <ItemRow key={itemKey(item)} item={item} operation={operation} preview />
              ))}
              {"unsupported" in plan &&
                plan.unsupported.map((fingerprint) => (
                  <div key={fingerprint} className="rounded-lg border border-amber-500/25 bg-amber-500/10 p-3 font-mono text-[11px] text-amber-400">
                    {t("instructions.normalize.unsupported", { fingerprint })}
                  </div>
                ))}
              {plan.items.length === 0 && (!("unsupported" in plan) || plan.unsupported.length === 0) && (
                <div className="rounded-lg border border-border-subtle p-4 text-center text-[12px] text-muted">
                  {t(`instructions.${operation}.empty`)}
                </div>
              )}
            </div>
            <div className="mt-4 flex justify-between gap-2">
              <button onClick={close} className="app-button-secondary">
                <ArrowLeft className="h-4 w-4" />
                {t("common.cancel")}
              </button>
              <button onClick={() => void apply()} disabled={loading} className="app-button-primary">
                {loading ? t(`instructions.${operation}.applying`) : t(`instructions.${operation}.apply`)}
              </button>
            </div>
          </>
        ) : (
          <>
            <div
              className={cn(
                "mb-3 flex items-center gap-2 rounded-lg border px-3 py-2 text-[12px]",
                succeeded
                  ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-400"
                  : "border-amber-500/25 bg-amber-500/10 text-amber-400"
              )}
            >
              {succeeded ? <ShieldCheck className="h-4 w-4" /> : <ShieldAlert className="h-4 w-4" />}
              {succeeded
                ? t(`instructions.${operation}.verified`)
                : t(`instructions.${operation}.unverified`)}
              {"snapshot_id" in outcome && outcome.snapshot_id && (
                <span className="ml-auto font-mono text-[10.5px]">{outcome.snapshot_id}</span>
              )}
            </div>
            <div className="min-h-0 flex-1 space-y-2 overflow-y-auto">
              {outcome.items.map((item) => (
                <ItemRow key={itemKey(item)} item={item} operation={operation} preview={false} />
              ))}
            </div>
            <div className="mt-4 flex justify-end">
              <button onClick={close} className="app-button-secondary">
                {t(`instructions.${operation}.close`)}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
