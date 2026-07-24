import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import { TONE_DOT } from "../../lib/chainUi";

interface CollapsedLinksProps {
  count: number;
  /** Row label key; the attention state says「其余 N 条正常」instead (#30). */
  labelKey?: string;
  children: ReactNode;
}

/** Workbench 折叠行：全绿时把链接列表收起来，一行交代「N 条正常」并可展开回完整列表。 */
export function CollapsedLinks({ count, labelKey = "chain.workbench.allNormal", children }: CollapsedLinksProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const Chevron = expanded ? ChevronDown : ChevronRight;

  return (
    <>
      <button
        data-testid="collapsed-links"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
        className="app-glass-card flex w-full items-center gap-2.5 px-4 py-3 text-left text-[12.5px] text-tertiary outline-none"
      >
        <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${TONE_DOT.ok}`} />
        {t(labelKey, { count })}
        <span className="ml-auto flex items-center gap-1 text-muted">
          {t(expanded ? "chain.workbench.collapse" : "chain.workbench.expand")}
          <Chevron className="h-3.5 w-3.5" />
        </span>
      </button>
      {expanded && children}
    </>
  );
}
