import { cn } from "../../utils";
import type { ChainProject } from "../../lib/tauri";

interface ProjectSwitcherProps {
  projects: ChainProject[];
  activePath: string | undefined;
  onSelect: (path: string) => void;
}

/** Workbench 项目切换区块：注册项目的胶囊选择器。 */
export function ProjectSwitcher({ projects, activePath, onSelect }: ProjectSwitcherProps) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {projects.map((p) => {
        const active = activePath === p.path;
        return (
          <button
            key={p.path}
            onClick={() => onSelect(p.path)}
            className={cn(
              "rounded-full border px-3 py-1.5 text-[12.5px] font-medium transition-colors outline-none",
              active
                ? "border-accent-border bg-glass-strong text-secondary"
                : "border-glass-hairline text-muted hover:bg-glass-soft hover:text-tertiary"
            )}
          >
            {p.name}
          </button>
        );
      })}
    </div>
  );
}
