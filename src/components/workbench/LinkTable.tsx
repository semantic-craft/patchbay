import { useTranslation } from "react-i18next";
import { cn } from "../../utils";
import { STATUS_TONE, TONE_BADGE, shortenPath } from "../../lib/chainUi";
import type { ChainTopology, ChainTracedEntry } from "../../lib/tauri";

export interface LinkRow {
  location: string;
  entry: ChainTracedEntry;
}

/** Entry statuses backed by a symlink — these can be safely unlinked. */
const UNLINKABLE = new Set(["direct", "link_repo", "via_agents", "external", "broken"]);

interface LinkTableProps {
  rows: LinkRow[];
  topo: ChainTopology;
  onUnlink: (name: string, location: string) => void;
}

/** Workbench 链接列表区块：项目所有软链条目与卸载动作。 */
export function LinkTable({ rows, topo, onUnlink }: LinkTableProps) {
  const { t } = useTranslation();
  return (
    <div className="app-panel overflow-x-auto">
      <table className="w-full min-w-[900px] border-collapse text-left">
        <thead>
          <tr className="border-b border-border-subtle">
            {[
              t("chain.location"),
              t("chain.skill"),
              t("chain.chainCol"),
              t("chain.statusCol"),
              t("chain.repoCol"),
              "",
            ].map((label, i) => (
              <th
                key={i}
                className="px-4 py-2.5 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted"
              >
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ location, entry }) => (
            <tr key={entry.entry_path} className="border-b border-border-subtle last:border-b-0">
              <td className="px-4 py-2 font-mono text-[11.5px] text-muted">{location}</td>
              <td className="px-4 py-2 font-mono text-[12px] font-medium text-secondary">
                {entry.name}
              </td>
              <td className="max-w-[380px] break-all px-4 py-2 font-mono text-[11px] text-muted">
                {entry.hops.length > 0
                  ? entry.hops
                      .map((h) => shortenPath(h, topo.warehouse_roots.map((r) => r.root), topo.projects_root))
                      .join(" → ")
                  : shortenPath(entry.final_target, topo.warehouse_roots.map((r) => r.root), topo.projects_root)}
              </td>
              <td className="px-4 py-2">
                <span
                  className={cn(
                    "inline-block rounded-full border px-2 py-px text-[11px] font-medium",
                    TONE_BADGE[STATUS_TONE[entry.status]]
                  )}
                >
                  {t(`chain.status.${entry.status}`)}
                </span>
              </td>
              <td className="px-4 py-2 font-mono text-[11.5px] text-tertiary">{entry.repo ?? "—"}</td>
              <td className="px-4 py-2 text-right">
                {UNLINKABLE.has(entry.status) && (
                  <button
                    onClick={() => onUnlink(entry.name, location)}
                    className="rounded border border-border-subtle px-2 py-0.5 text-[11px] font-medium text-muted transition-colors outline-none hover:border-red-500/40 hover:text-red-400"
                  >
                    {t("chain.unlink")}
                  </button>
                )}
              </td>
            </tr>
          ))}
          {rows.length === 0 && (
            <tr>
              <td colSpan={6} className="px-4 py-6 text-center text-[12.5px] text-muted">
                {t("chain.noAgg")}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
