import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { cn } from "../utils";
import { ChainProjects } from "./ChainProjects";
import { ChainOverview } from "./ChainOverview";
import { ChainDoctor } from "./ChainDoctor";
import { ChainWarehouse } from "./ChainWarehouse";

/**
 * 应用主屏。动线是「选项目 → 挂技能」，所以工作台是默认分区；链路图、诊断
 * 和开发源都是出问题时才看的东西，收在同一屏的分区里，不再占据导航。
 */
const SECTIONS = [
  { key: "workbench", labelKey: "home.tabWorkbench" },
  { key: "chain", labelKey: "home.tabChain" },
  { key: "doctor", labelKey: "home.tabDoctor" },
  { key: "warehouse", labelKey: "home.tabWarehouse" },
] as const;

type SectionKey = (typeof SECTIONS)[number]["key"];

function isSectionKey(value: string | null): value is SectionKey {
  return SECTIONS.some((section) => section.key === value);
}

export function Home() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const requested = searchParams.get("tab");
  const active: SectionKey = isSectionKey(requested) ? requested : "workbench";

  // 工作台是默认分区，所以它不写进 URL —— `?project=` 深链保持原样可用。
  const select = (key: SectionKey) => {
    const next = new URLSearchParams(searchParams);
    if (key === "workbench") next.delete("tab");
    else next.set("tab", key);
    setSearchParams(next, { replace: true });
  };

  return (
    <div className="flex min-h-full flex-col gap-4">
      <div className="app-segmented self-start">
        {SECTIONS.map((section) => (
          <button
            key={section.key}
            onClick={() => select(section.key)}
            className={cn(
              "app-segmented-button",
              active === section.key && "app-segmented-button-active",
            )}
          >
            {t(section.labelKey)}
          </button>
        ))}
      </div>

      {active === "workbench" && <ChainProjects />}
      {active === "chain" && <ChainOverview />}
      {active === "doctor" && <ChainDoctor />}
      {active === "warehouse" && <ChainWarehouse />}
    </div>
  );
}
