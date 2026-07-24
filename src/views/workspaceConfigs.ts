import type { ToolCategory } from "../lib/tauri";

/**
 * Drives the retained compatibility workspace for lobster agents.
 */
export interface WorkspaceConfig {
  category: ToolCategory;
  basePath: string;
  i18nKeys: {
    /** Heading shown on the "all agents" overview page. */
    title: string;
    /** Heading shown when no agents in this category are installed. */
    noAgents: string;
    /** Hint shown under the no-agents heading. */
    noAgentsHint: string;
  };
}

export const LOBSTER_WORKSPACE_CONFIG: WorkspaceConfig = {
  category: "lobster",
  basePath: "/lobster-workspace",
  i18nKeys: {
    title: "lobsterWorkspace.title",
    noAgents: "lobsterWorkspace.noAgents",
    noAgentsHint: "lobsterWorkspace.noAgentsHint",
  },
};
