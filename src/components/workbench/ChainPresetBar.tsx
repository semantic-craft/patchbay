import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Package, Pencil, Plus, Trash2, X } from "lucide-react";
import { cn } from "../../utils";
import { chainPresetDelete, chainPresetRename, chainPresetSave } from "../../lib/tauri";
import type { ChainPreset, ChainPresetSkill } from "../../lib/tauri";
import { ConfirmDialog } from "../ConfirmDialog";

interface ChainPresetBarProps {
  presets: ChainPreset[];
  /** The selected project's current deduped skill references — the material
   * 「把当前 N 个技能存为 Preset」saves. */
  currentSkills: ChainPresetSkill[];
  onChanged: () => void;
}

type NameDialog =
  | { mode: "save" }
  | { mode: "rename"; preset: ChainPreset };

/**
 * Preset 栏（#35，原型 S1）：全绿态的装配套装一览。本票只管「沉淀与管理」
 * ——展示既有套装、把当前链接集合命名保存、重命名/删除；套装的应用（批量
 * 建链）归 #36 接入向导。
 */
export function ChainPresetBar({ presets, currentSkills, onChanged }: ChainPresetBarProps) {
  const { t } = useTranslation();
  const [dialog, setDialog] = useState<NameDialog | null>(null);
  const [deleting, setDeleting] = useState<ChainPreset | null>(null);

  const remove = async () => {
    if (!deleting) return;
    try {
      await chainPresetDelete(deleting.id);
      toast.success(t("chain.workbench.presetDeleted", { name: deleting.name }));
      onChanged();
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div
      data-testid="chain-preset-bar"
      className="app-glass-card flex flex-wrap items-center gap-2 px-4 py-2.5"
    >
      <span className="flex items-center gap-1.5 text-[11.5px] font-semibold text-muted">
        <Package className="h-3.5 w-3.5" />
        Preset
      </span>

      {presets.map((preset) => (
        <span
          key={preset.id}
          data-testid="preset-pill"
          className="group flex items-center gap-1.5 rounded-full border border-glass-hairline bg-glass-strong px-3 py-1 text-[12px] font-medium text-secondary"
        >
          {preset.name}
          <span className="text-[10.5px] text-muted">
            {t("chain.workbench.presetCount", { count: preset.skills.length })}
          </span>
          <button
            data-testid="preset-rename"
            title={t("chain.workbench.presetRename")}
            onClick={() => setDialog({ mode: "rename", preset })}
            className="invisible rounded p-0.5 text-faint transition-colors hover:text-secondary group-hover:visible"
          >
            <Pencil className="h-3 w-3" />
          </button>
          <button
            data-testid="preset-delete"
            title={t("chain.workbench.presetDelete")}
            onClick={() => setDeleting(preset)}
            className="invisible rounded p-0.5 text-faint transition-colors hover:text-red-400 group-hover:visible"
          >
            <Trash2 className="h-3 w-3" />
          </button>
        </span>
      ))}

      <button
        data-testid="preset-save-current"
        onClick={() => setDialog({ mode: "save" })}
        disabled={currentSkills.length === 0}
        className="flex items-center gap-1 rounded-full border border-glass-hairline bg-glass-soft px-3 py-1 text-[12px] font-medium text-muted transition-colors hover:text-secondary disabled:opacity-50"
      >
        <Plus className="h-3 w-3" />
        {t("chain.workbench.presetSaveCurrent", { count: currentSkills.length })}
      </button>

      {dialog && (
        <PresetNameDialog
          title={
            dialog.mode === "save"
              ? t("chain.workbench.presetSaveTitle")
              : t("chain.workbench.presetRenameTitle", { name: dialog.preset.name })
          }
          initial={dialog.mode === "rename" ? dialog.preset.name : ""}
          onClose={() => setDialog(null)}
          onSubmit={async (name) => {
            if (dialog.mode === "save") {
              await chainPresetSave(name, currentSkills);
              toast.success(t("chain.workbench.presetSaved", { name }));
            } else {
              await chainPresetRename(dialog.preset.id, name);
            }
            onChanged();
          }}
        />
      )}

      <ConfirmDialog
        open={deleting !== null}
        message={t("chain.workbench.presetDeleteConfirm", { name: deleting?.name ?? "" })}
        onClose={() => setDeleting(null)}
        onConfirm={remove}
      />
    </div>
  );
}

/** 命名对话框：保存与重命名共用（输入 + 确认，错误就地吐 toast）。 */
function PresetNameDialog({
  title,
  initial,
  onClose,
  onSubmit,
}: {
  title: string;
  initial: string;
  onClose: () => void;
  onSubmit: (name: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(initial);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    if (!name.trim()) return;
    setBusy(true);
    try {
      await onSubmit(name.trim());
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div
        data-testid="preset-name-dialog"
        className="app-glass-card w-[360px] space-y-3 p-4"
      >
        <div className="flex items-center justify-between">
          <span className="text-[13.5px] font-semibold text-primary">{title}</span>
          <button onClick={onClose} className="rounded p-1 text-faint hover:text-secondary">
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
        <input
          data-testid="preset-name-input"
          autoFocus
          value={name}
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
            if (event.key === "Escape") onClose();
          }}
          placeholder={t("chain.workbench.presetNamePlaceholder")}
          className="w-full rounded-[6px] border border-border-subtle bg-background px-3 py-2 text-[13px] text-secondary placeholder-faint transition-all focus:border-border focus:outline-none"
        />
        <div className="flex justify-end gap-2">
          <button onClick={onClose} className="app-button-secondary h-7 px-3 text-[12px]">
            {t("common.cancel")}
          </button>
          <button
            data-testid="preset-name-confirm"
            onClick={() => void submit()}
            disabled={busy || !name.trim()}
            className={cn("app-button-primary h-7 px-3 text-[12px]", busy && "opacity-60")}
          >
            {t("common.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
