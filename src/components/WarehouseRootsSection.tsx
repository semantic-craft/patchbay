import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as dialogOpen } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { FolderPlus, Trash2, ChevronUp, ChevronDown, AlertTriangle } from "lucide-react";
import { cn } from "../utils";
import { getWarehouseRoots, setWarehouseRoots } from "../lib/tauri";
import type { ChainRootConfig } from "../lib/tauri";

const iconButtonClass =
  "rounded-md p-1.5 text-tertiary hover:bg-surface-hover hover:text-secondary disabled:opacity-40 disabled:hover:bg-transparent";

/**
 * Settings editor for the ordered collection of Original Repository roots.
 * Add / remove / reorder persist immediately through the chain service, and
 * each root shows a readability status so unreadable roots are never silent.
 */
export function WarehouseRootsSection() {
  const { t } = useTranslation();
  const [roots, setRoots] = useState<ChainRootConfig[]>([]);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setRoots(await getWarehouseRoots());
    } catch (e) {
      toast.error(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const persist = useCallback(async (paths: string[]) => {
    setBusy(true);
    try {
      setRoots(await setWarehouseRoots(paths));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const currentPaths = () => roots.map((r) => r.path);

  const handleAdd = async () => {
    const selected = await dialogOpen({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    if (roots.some((r) => r.path === selected)) return;
    await persist([...currentPaths(), selected]);
  };

  const handleRemove = (index: number) => {
    const next = currentPaths();
    next.splice(index, 1);
    void persist(next);
  };

  const move = (index: number, delta: number) => {
    const next = currentPaths();
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    void persist(next);
  };

  return (
    <section>
      <div className="mb-2">
        <h2 className="app-section-title">{t("chain.rootsTitle")}</h2>
        <p className="mt-0.5 text-[12.5px] text-muted">{t("chain.rootsHint")}</p>
      </div>

      <div className="space-y-1.5">
        {roots.map((root, index) => {
          const bad = root.status !== "ok";
          return (
            <div
              key={root.path}
              className="flex items-center gap-2 rounded-lg border border-border-subtle bg-surface px-3 py-2"
            >
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-[12.5px] text-secondary">{root.path}</div>
                {bad && (
                  <div className="mt-0.5 flex items-center gap-1 text-[11.5px] font-medium text-red-400">
                    <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                    {t(root.status === "missing" ? "chain.rootMissing" : "chain.rootUnreadable")}
                    {root.error && (
                      <span className="truncate font-normal text-muted">· {root.error}</span>
                    )}
                  </div>
                )}
              </div>
              <button
                type="button"
                className={iconButtonClass}
                title={t("chain.rootMoveUp")}
                aria-label={t("chain.rootMoveUp")}
                disabled={busy || index === 0}
                onClick={() => move(index, -1)}
              >
                <ChevronUp className="h-4 w-4" />
              </button>
              <button
                type="button"
                className={iconButtonClass}
                title={t("chain.rootMoveDown")}
                aria-label={t("chain.rootMoveDown")}
                disabled={busy || index === roots.length - 1}
                onClick={() => move(index, 1)}
              >
                <ChevronDown className="h-4 w-4" />
              </button>
              <button
                type="button"
                className={cn(iconButtonClass, "hover:text-red-400")}
                title={t("chain.rootRemove")}
                aria-label={t("chain.rootRemove")}
                disabled={busy}
                onClick={() => handleRemove(index)}
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          );
        })}
        {roots.length === 0 && (
          <div className="px-1 py-2 text-[12.5px] text-muted">{t("chain.rootsEmpty")}</div>
        )}
      </div>

      <button
        type="button"
        className="app-button-secondary mt-2.5"
        disabled={busy}
        onClick={() => void handleAdd()}
      >
        <FolderPlus className="h-4 w-4" />
        {t("chain.rootAdd")}
      </button>
    </section>
  );
}
