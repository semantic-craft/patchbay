import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "../../utils";
import type { ChainFinding } from "../../lib/tauri";

const DOT: Record<string, string> = {
  advice: "bg-blue-400",
  notice: "bg-gray-400",
};

interface QuietFindingsProps {
  findings: ChainFinding[];
  onViewDiagnosis: () => void;
}

/**
 * The advice/notice findings, folded into one row.
 *
 * These describe intended states — a project-private Skill is the usual case —
 * so they must not take an evidence card or knock the project out of green.
 * They still have to be reachable, so the row names how many there are and
 * expands to the same one-line-each summary Diagnostics would show.
 */
export function QuietFindings({ findings, onViewDiagnosis }: QuietFindingsProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const Chevron = expanded ? ChevronDown : ChevronRight;

  if (findings.length === 0) return null;

  return (
    <div data-testid="quiet-findings" className="app-glass-card overflow-hidden">
      <button
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
        className="flex w-full items-center gap-2.5 px-4 py-3 text-left text-[12.5px] text-tertiary outline-none"
      >
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-gray-400" />
        {t("chain.workbench.quietCount", { count: findings.length })}
        <span className="ml-auto flex items-center gap-1 text-muted">
          {t(expanded ? "chain.workbench.collapse" : "chain.workbench.expand")}
          <Chevron className="h-3.5 w-3.5" />
        </span>
      </button>

      {expanded && (
        <div className="border-t border-glass-hairline">
          {findings.map((finding) => (
            <div
              key={finding.fingerprint}
              data-testid="quiet-finding"
              className="flex items-center gap-2.5 border-b border-glass-hairline px-4 py-2 text-[12px] last:border-b-0"
            >
              <span
                className={cn(
                  "h-1.5 w-1.5 shrink-0 rounded-full",
                  DOT[finding.severity] ?? "bg-gray-400",
                )}
              />
              <span className="shrink-0 text-secondary">
                {t(`chain.doctor.deviation.${finding.deviation}`)}
              </span>
              <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-muted">
                {finding.affected.find((object) => object.kind === "skill")?.name ?? ""}
              </span>
            </div>
          ))}
          <button
            onClick={onViewDiagnosis}
            className="w-full px-4 py-2 text-left text-[11.5px] text-muted outline-none transition-colors hover:text-secondary"
          >
            {t("chain.workbench.cardDiagnose")}
          </button>
        </div>
      )}
    </div>
  );
}
