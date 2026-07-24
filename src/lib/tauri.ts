import { invoke } from "@tauri-apps/api/core";

// ── Types ──

export type ToolCategory = "coding" | "lobster";

export interface ToolInfo {
  key: string;
  display_name: string;
  installed: boolean;
  skills_dir: string;
  enabled: boolean;
  is_custom: boolean;
  has_path_override: boolean;
  project_relative_skills_dir: string | null;
  has_project_path_override: boolean;
  category: ToolCategory;
}

export interface ManagedSkill {
  id: string;
  name: string;
  description: string | null;
  source_type: string;
  source_ref: string | null;
  source_ref_resolved: string | null;
  source_subpath: string | null;
  source_branch: string | null;
  source_revision: string | null;
  remote_revision: string | null;
  update_status: string;
  last_checked_at: number | null;
  last_check_error: string | null;
  central_path: string;
  enabled: boolean;
  created_at: number;
  updated_at: number;
  status: string;
  targets: SkillTarget[];
  preset_ids: string[];
  tags: string[];
}

export interface SkillTarget {
  id: string;
  skill_id: string;
  tool: string;
  target_path: string;
  mode: string;
  status: string;
  synced_at: number | null;
}

export interface SkillToolToggle {
  tool: string;
  display_name: string;
  installed: boolean;
  globally_enabled: boolean;
  enabled: boolean;
}

export interface SkillDocument {
  skill_id: string;
  filename: string;
  content: string;
  central_path: string;
}

export interface SourceSkillDocument {
  skill_id: string;
  filename: string;
  content: string;
  source_label: string;
  revision: string;
}

export type SkillSourceDiffStatus = "added" | "removed" | "modified";
export type SkillSourceDiffContentKind =
  | "text"
  | "binary"
  | "too_large"
  | "permission_only";

export interface SkillSourceDiffEntry {
  relative_path: string;
  status: SkillSourceDiffStatus;
  content_kind: SkillSourceDiffContentKind;
  original_text: string | null;
  updated_text: string | null;
  executable_before: boolean;
  executable_after: boolean;
}

export interface SkillSourceDiff {
  skill_id: string;
  source_label: string;
  revision: string;
  entries: SkillSourceDiffEntry[];
}

export interface Preset {
  id: string;
  name: string;
  description: string | null;
  icon: string | null;
  sort_order: number;
  skill_count: number;
  created_at: number;
  updated_at: number;
}

export interface DiscoveredGroup {
  name: string;
  fingerprint: string | null;
  locations: { id: string; tool: string; found_path: string }[];
  imported: boolean;
  found_at: number;
}

export interface ScanResult {
  tools_scanned: number;
  skills_found: number;
  groups: DiscoveredGroup[];
}

export interface SkillsShSkill {
  id: string;
  skill_id: string;
  name: string;
  source: string;
  installs: number;
}

export interface SyncHealth {
  in_sync: number;
  project_newer: number;
  center_newer: number;
  diverged: number;
  project_only: number;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  workspace_type: "project" | "linked";
  linked_agent_name: string | null;
  supports_skill_toggle: boolean;
  sort_order: number;
  skill_count: number;
  sync_health: SyncHealth;
  created_at: number;
  updated_at: number;
}

export interface ProjectAgentTarget {
  key: string;
  display_name: string;
  enabled: boolean;
  installed: boolean;
  is_custom: boolean;
}

export interface ProjectSkill {
  name: string;
  dir_name: string;
  relative_path: string;
  description: string | null;
  path: string;
  files: string[];
  enabled: boolean;
  agent: string;
  agent_display_name: string;
  tags: string[];
  in_center: boolean;
  sync_status: "project_only" | "in_sync" | "project_newer" | "center_newer" | "diverged";
  center_skill_id: string | null;
}

export interface ProjectSkillDocument {
  skill_name: string;
  filename: string;
  content: string;
}

// ── Tools ──

export const getToolStatus = () => invoke<ToolInfo[]>("get_tool_status");

export const setToolEnabled = (key: string, enabled: boolean) =>
  invoke<void>("set_tool_enabled", { key, enabled });

export const setAllToolsEnabled = (enabled: boolean) =>
  invoke<void>("set_all_tools_enabled", { enabled });

export const getToolOrder = () => invoke<string[]>("get_tool_order_cmd");

export const setToolOrder = (order: string[]) =>
  invoke<void>("set_tool_order_cmd", { order });

export const setCustomToolPath = (key: string, path: string) =>
  invoke<void>("set_custom_tool_path", { key, path });

export const resetCustomToolPath = (key: string) =>
  invoke<void>("reset_custom_tool_path", { key });

export const setCustomToolProjectPath = (
  key: string,
  projectRelativeSkillsDir: string | null,
) =>
  invoke<void>("set_custom_tool_project_path", {
    key,
    projectRelativeSkillsDir,
  });

export const resetCustomToolProjectPath = (key: string) =>
  invoke<void>("reset_custom_tool_project_path", { key });

export const addCustomTool = (
  key: string,
  displayName: string,
  skillsDir: string,
  projectRelativeSkillsDir?: string,
) =>
  invoke<void>("add_custom_tool", {
    key,
    displayName,
    skillsDir,
    projectRelativeSkillsDir: projectRelativeSkillsDir ?? null,
  });

export const removeCustomTool = (key: string) =>
  invoke<void>("remove_custom_tool", { key });

// ── Skills ──

export const getManagedSkills = () =>
  invoke<ManagedSkill[]>("get_managed_skills");

export const getSkillsForPreset = (presetId: string) =>
  invoke<ManagedSkill[]>("get_skills_for_preset", {
    presetId,
  });

export const getSkillDocument = (skillId: string) =>
  invoke<SkillDocument>("get_skill_document", { skillId });

export const getSourceSkillDocument = (skillId: string) =>
  invoke<SourceSkillDocument>("get_source_skill_document", { skillId });

export const getSkillSourceDiff = (skillId: string) =>
  invoke<SkillSourceDiff>("get_skill_source_diff", { skillId });

export const deleteManagedSkill = (skillId: string) =>
  invoke<void>("delete_managed_skill", { skillId });

export interface BatchDeleteSkillsResult {
  deleted: number;
  failed: string[];
}

export const deleteManagedSkills = (skillIds: string[]) =>
  invoke<BatchDeleteSkillsResult>("delete_managed_skills", { skillIds });

export const installLocal = (sourcePath: string, name?: string) =>
  invoke<void>("install_local", { sourcePath, name: name || null });

export const installGit = (repoUrl: string, name?: string) =>
  invoke<void>("install_git", { repoUrl, name: name || null });

export interface GitSkillPreview {
  /** Path relative to the resolved scan root, using `/` separators. Stable key. */
  rel_path: string;
  name: string;
  description: string | null;
}

export interface GitPreviewResult {
  temp_dir: string;
  skills: GitSkillPreview[];
}

export interface SkillInstallItem {
  rel_path: string;
  name: string;
}

export const previewGitInstall = (repoUrl: string) =>
  invoke<GitPreviewResult>("preview_git_install", { repoUrl });

export const confirmGitInstall = (repoUrl: string, tempDir: string, items: SkillInstallItem[]) =>
  invoke<void>("confirm_git_install", { repoUrl, tempDir, items });

export const cancelGitPreview = (tempDir: string) =>
  invoke<void>("cancel_git_preview", { tempDir });

export const installFromSkillssh = (source: string, skillId: string) =>
  invoke<void>("install_from_skillssh", { source, skillId });

export const cancelInstall = (key: string) =>
  invoke<boolean>("cancel_install", { key });

export const checkSkillUpdate = (skillId: string, force?: boolean) =>
  invoke<ManagedSkill>("check_skill_update", {
    skillId,
    force: force ?? false,
  });

export const checkAllSkillUpdates = (force?: boolean) =>
  invoke<void>("check_all_skill_updates", {
    force: force ?? false,
  });

export interface UpdateSkillResult {
  skill: ManagedSkill;
  /** False when a monorepo commit didn't touch this skill's subdirectory. */
  content_changed: boolean;
}

export const updateSkill = (skillId: string) =>
  invoke<UpdateSkillResult>("update_skill", { skillId });

export interface BatchUpdateSkillsResult {
  refreshed: number;
  unchanged: number;
  failed: string[];
}

export const batchUpdateSkills = (skillIds: string[]) =>
  invoke<BatchUpdateSkillsResult>("batch_update_skills", { skillIds });

export const reimportLocalSkill = (skillId: string) =>
  invoke<ManagedSkill>("reimport_local_skill", { skillId });

export const relinkLocalSkillSource = (skillId: string, sourcePath: string) =>
  invoke<ManagedSkill>("relink_local_skill_source", { skillId, sourcePath });

export const detachLocalSkillSource = (skillId: string) =>
  invoke<ManagedSkill>("detach_local_skill_source", { skillId });

export interface BatchImportResult {
  imported: number;
  skipped: number;
  errors: string[];
}

export const batchImportFolder = (folderPath: string) =>
  invoke<BatchImportResult>("batch_import_folder", { folderPath });

export const getAllTags = () => invoke<string[]>("get_all_tags");

export const setSkillTags = (skillId: string, tags: string[]) =>
  invoke<void>("set_skill_tags", { skillId, tags });

export const renameTag = (oldName: string, newName: string) =>
  invoke<void>("rename_tag", { oldName, newName });

export const deleteTag = (name: string) =>
  invoke<void>("delete_tag", { name });

// ── Sync ──

export const syncSkillToTool = (skillId: string, tool: string) =>
  invoke<void>("sync_skill_to_tool", { skillId, tool });

export const unsyncSkillFromTool = (skillId: string, tool: string) =>
  invoke<void>("unsync_skill_from_tool", { skillId, tool });

export const getSkillToolToggles = (skillId: string, presetId: string) =>
  invoke<SkillToolToggle[]>("get_skill_tool_toggles", { skillId, presetId });

export const setSkillToolToggle = (
  skillId: string,
  presetId: string,
  tool: string,
  enabled: boolean
) =>
  invoke<void>("set_skill_tool_toggle", { skillId, presetId, tool, enabled });

// ── Scan ──

export const scanLocalSkills = () => invoke<ScanResult>("scan_local_skills");

export const importExistingSkill = (sourcePath: string, name?: string) =>
  invoke<void>("import_existing_skill", { sourcePath, name: name || null });

export const importAllDiscovered = () =>
  invoke<void>("import_all_discovered");

// ── Browse ──

export const fetchLeaderboard = (board: string) =>
  invoke<SkillsShSkill[]>("fetch_leaderboard", { board });

export const searchSkillssh = (query: string, limit?: number) =>
  invoke<SkillsShSkill[]>("search_skillssh", {
    query,
    limit: limit ?? null,
  });

// ── Settings ──

export const getSettings = (key: string) =>
  invoke<string | null>("get_settings", { key });

export const setSettings = (key: string, value: string) =>
  invoke<void>("set_settings", { key, value });

export const getCentralRepoPath = () =>
  invoke<string>("get_central_repo_path");

export const getCentralRepoPathOverride = () =>
  invoke<string | null>("get_central_repo_path_override");

export const getCentralRepoWarnings = () =>
  invoke<string[]>("get_central_repo_warnings");

export const setCentralRepoPath = (path?: string | null) =>
  invoke<string>("set_central_repo_path", { path: path ?? null });

export const appExit = () => invoke<void>("app_exit");

export const hideToTray = () => invoke<void>("hide_to_tray");

export const openCentralRepoFolder = () =>
  invoke<void>("open_central_repo_folder");

export interface AppUpdateInfo {
  has_update: boolean;
  current_version: string;
  latest_version: string;
  release_url: string;
}

export const checkAppUpdate = () =>
  invoke<AppUpdateInfo>("check_app_update");

export interface DiagnosticInfo {
  app_version: string;
  os: string;
  os_version: string;
  arch: string;
  central_repo_path: string;
  central_repo_path_overridden: boolean;
}

export const getDiagnosticInfo = () =>
  invoke<DiagnosticInfo>("get_diagnostic_info");

export interface LogExcerpt {
  log_path: string;
  excerpt: string;
  line_count: number;
  has_warnings: boolean;
}

export const getRecentLogExcerpt = () =>
  invoke<LogExcerpt>("get_recent_log_excerpt");

export interface LogExportResult {
  zip_path: string;
  file_count: number;
}

export const exportLogsZip = () =>
  invoke<LogExportResult>("export_logs_zip");

export interface PanicInfo {
  timestamp: string;
  message: string;
}

export const checkLastPanic = () =>
  invoke<PanicInfo | null>("check_last_panic");

export const clearLastPanic = () =>
  invoke<void>("clear_last_panic");

/**
 * Diagnostic-only: write a named startup event with elapsed ms (from
 * performance.timeOrigin) into the backend log file. Used to correlate
 * WebView2 boot and frontend boot timing with Rust-side startup logs
 * when debugging slow launches (see issue #153).
 */
export const logStartupEvent = (label: string, elapsedMs: number) =>
  invoke<void>("log_startup_event", { label, elapsedMs: Math.round(elapsedMs) });

// ── Window glass (#37) ──

/**
 * Which native material ended up behind the webview: macOS 26 real glass,
 * NSVisualEffectView vibrancy on older macOS, or none (apply failed or
 * non-macOS — the CSS wallpaper must then stay opaque).
 */
export type WindowGlassTier = "liquid-glass" | "vibrancy" | "none";

export const getWindowGlassStatus = () =>
  invoke<WindowGlassTier>("window_glass_status");

// ── Git Backup ──

export type GitUpstreamHealth =
  | "healthy"
  | "no_remote"
  | "no_upstream"
  | "unrelated_histories"
  | "detached";

export interface GitBackupStatus {
  is_repo: boolean;
  remote_url: string | null;
  branch: string | null;
  has_changes: boolean;
  changed_skill_count: number;
  ahead: number;
  behind: number;
  last_commit: string | null;
  last_commit_time: string | null;
  current_snapshot_tag: string | null;
  restored_from_tag: string | null;
  upstream_health: GitUpstreamHealth;
}

export interface GitBackupVersion {
  tag: string;
  commit: string;
  message: string;
  committed_at: string;
  /** Device name of the machine that made this backup (empty for old commits). */
  author: string;
}

export interface GitBackupSizeReport {
  total_bytes: number;
  /** `excluded`: oversized and kept out of the backup (§3.6); false = already tracked, warning only. */
  oversized: { name: string; bytes: number; excluded: boolean }[];
  skill_limit_bytes: number;
  repo_warn_bytes: number;
}

export const gitBackupStatus = () =>
  invoke<GitBackupStatus>("git_backup_status");

export const gitBackupFetch = () => invoke<void>("git_backup_fetch");

export const gitBackupInit = () => invoke<void>("git_backup_init");

/** Returns the sanitized URL actually configured (credentials moved to the OS keychain). */
export const gitBackupSetRemote = (url: string) =>
  invoke<string>("git_backup_set_remote", { url });

/** Strip embedded credentials into the OS keychain; returns the URL safe to persist. */
export const gitBackupSanitizeRemoteUrl = (url: string) =>
  invoke<string>("git_backup_sanitize_remote_url", { url });

export interface GithubBackupConnectResult {
  url: string;
  login: string;
  repo_created: boolean;
  /** False when a pre-existing PUBLIC repository was connected. */
  repo_private: boolean;
  remote_has_content: boolean;
}

/** GitHub guided connect (PAT): validates the token, finds or creates the
 * private backup repo, stores the token in the OS keychain, saves the URL. */
export const githubBackupConnect = (token: string, repoName: string) =>
  invoke<GithubBackupConnectResult>("github_backup_connect", { token, repoName });

export interface GithubDeviceFlowStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface GithubDevicePollResult {
  status: "pending" | "slow_down" | "repository_identified" | "connected";
  result: GithubBackupConnectResult | null;
  next_flow: GithubDeviceFlowStart | null;
  repository_id: number | null;
}

export const githubDeviceFlowStart = () =>
  invoke<GithubDeviceFlowStart>("github_device_flow_start");

/** One poll; on authorization the backend completes the whole connect and the
 * OAuth token never reaches the webview. */
export const githubDeviceFlowPoll = (
  deviceCode: string,
  repoName: string,
  repositoryId: number | null,
) =>
  invoke<GithubDevicePollResult>("github_device_flow_poll", {
    deviceCode,
    repoName,
    repositoryId,
  });

/** Migrate token-in-URL remotes to the OS keychain. Returns the sanitized URL if migrated. */
export const gitBackupMigrateCredentials = () =>
  invoke<string | null>("git_backup_migrate_credentials");

export const gitBackupSizeReport = () =>
  invoke<GitBackupSizeReport>("git_backup_size_report");

/** This machine's device name (§4.3): saved setting or persisted hostname default. */
export const backupDeviceName = () => invoke<string>("backup_device_name");

/** Rename this device; only affects future backups. Returns the sanitized name. */
export const backupSetDeviceName = (name: string) =>
  invoke<string>("backup_set_device_name", { name });

export const gitBackupRemoveRemote = () =>
  invoke<void>("git_backup_remove_remote");

export const gitBackupCommit = (message: string) =>
  invoke<void>("git_backup_commit", { message });

export const gitBackupPush = () => invoke<void>("git_backup_push");

export interface MergeUpdatedSkill {
  skill_id: string;
  path: string;
  /** Device (commit author) that last touched this skill on the remote. */
  from_device: string;
}

/** Outcome of a sync merge (merge-engine design §8). With the default
 * system engine only `engine` is meaningful. */
export interface MergeSummary {
  engine: "object" | "system";
  up_to_date: boolean;
  fast_forward: boolean;
  updated: MergeUpdatedSkill[];
  kept_local: string[];
  new_conflicts: string[];
  pending_total: number;
  old_client_warning: string | null;
  legacy_fallback: boolean;
}

export const gitBackupPull = () => invoke<MergeSummary>("git_backup_pull");

/** Outcome of the one-transaction sync (commit → merge → snapshot → push,
 * with automatic retry when another device pushes concurrently). */
export interface SyncOutcome {
  committed: boolean;
  merge: MergeSummary | null;
  pushed: boolean;
  snapshot_tag: string | null;
}

export const gitBackupSync = (message: string) =>
  invoke<SyncOutcome>("git_backup_sync", { message });

/** One "needs attention" sync conflict (merge-engine design §4). */
export interface PendingConflict {
  skill_id: string;
  theirs_commit: string;
  theirs_path: string | null;
  detected_at: number;
}

export const gitBackupPendingConflicts = () =>
  invoke<PendingConflict[]>("git_backup_pending_conflicts");

export type ResolveConflictAction = "keep_local" | "use_remote" | "keep_both";

/** Resolve a pending conflict; returns the safety snapshot tag. */
export const gitBackupResolveConflict = (
  skillId: string,
  action: ResolveConflictAction,
) => invoke<string>("git_backup_resolve_conflict", { skillId, action });

export const gitBackupClone = (url: string) =>
  invoke<void>("git_backup_clone", { url });

export const gitBackupReclone = (url: string) =>
  invoke<void>("git_backup_reclone", { url });

export const gitBackupCreateSnapshot = () =>
  invoke<string>("git_backup_create_snapshot");

export const gitBackupListVersions = (limit?: number) =>
  invoke<GitBackupVersion[]>("git_backup_list_versions", {
    limit: typeof limit === "number" ? limit : null,
  });

/** Returns the safety-point tag that captured the pre-restore state. */
export const gitBackupRestoreVersion = (tag: string) =>
  invoke<string>("git_backup_restore_version", { tag });

// ── Presets ──

export const getPresets = () => invoke<Preset[]>("get_presets");

export const getActivePreset = () =>
  invoke<Preset | null>("get_active_preset");

export const createPreset = (name: string, description?: string, icon?: string) =>
  invoke<Preset>("create_preset", {
    name,
    description: description || null,
    icon: icon || null,
  });

export const updatePreset = (
  id: string,
  name: string,
  description?: string,
  icon?: string
) =>
  invoke<void>("update_preset", {
    id,
    name,
    description: description || null,
    icon: icon || null,
  });

export const deletePreset = (id: string) =>
  invoke<void>("delete_preset", { id });

export const addSkillToPreset = (skillId: string, presetId: string) =>
  invoke<void>("add_skill_to_preset", { skillId, presetId });

export const removeSkillFromPreset = (skillId: string, presetId: string) =>
  invoke<void>("remove_skill_from_preset", { skillId, presetId });

export const reorderPresets = (ids: string[]) =>
  invoke<void>("reorder_presets", { ids });

export const reorderProjects = (ids: string[]) =>
  invoke<void>("reorder_projects", { ids });

export const getPresetSkillOrder = (presetId: string) =>
  invoke<string[]>("get_preset_skill_order", { presetId });

export const reorderPresetSkills = (presetId: string, skillIds: string[]) =>
  invoke<void>("reorder_preset_skills", { presetId, skillIds });

// ── Projects ──

export const getProjects = () => invoke<Project[]>("get_projects");

export const addProject = (path: string) =>
  invoke<Project>("add_project", { path });

export const addLinkedWorkspace = (name: string, path: string, disabledPath?: string) =>
  invoke<Project>("add_linked_workspace", {
    name,
    path,
    disabledPath: disabledPath ?? null,
  });

export const removeProject = (id: string) =>
  invoke<void>("remove_project", { id });

export const scanProjects = (root: string) =>
  invoke<string[]>("scan_projects", { root });

export const getProjectAgentTargets = (projectId: string) =>
  invoke<ProjectAgentTarget[]>("get_project_agent_targets", { projectId });

export const getProjectSkills = (projectId: string) =>
  invoke<ProjectSkill[]>("get_project_skills", { projectId });

export const getProjectSkillDocument = (projectId: string, skillRelativePath: string, agent: string) =>
  invoke<ProjectSkillDocument>("get_project_skill_document", { projectId, skillRelativePath, agent });

export const importProjectSkillToCenter = (projectId: string, skillRelativePath: string, agent: string) =>
  invoke<void>("import_project_skill_to_center", { projectId, skillRelativePath, agent });

export const exportSkillToProject = (skillId: string, projectId: string, agents?: string[]) =>
  invoke<void>("export_skill_to_project", { skillId, projectId, agents: agents ?? null });

export const updateProjectSkillToCenter = (projectId: string, skillRelativePath: string, agent: string) =>
  invoke<void>("update_project_skill_to_center", { projectId, skillRelativePath, agent });

export const updateProjectSkillFromCenter = (projectId: string, skillRelativePath: string, agent: string) =>
  invoke<void>("update_project_skill_from_center", { projectId, skillRelativePath, agent });

export const toggleProjectSkill = (projectId: string, skillRelativePath: string, agent: string, enabled: boolean) =>
  invoke<void>("toggle_project_skill", { projectId, skillRelativePath, agent, enabled });

export const deleteProjectSkill = (projectId: string, skillRelativePath: string, agent: string) =>
  invoke<void>("delete_project_skill", { projectId, skillRelativePath, agent });

export const slugifySkillNames = (names: string[]) =>
  invoke<string[]>("slugify_skill_names", { names });

// ── Agent Local Workspace ──

export const getGlobalLocalSkills = (agent: string) =>
  invoke<ProjectSkill[]>("get_global_local_skills", { agent });

export const getGlobalLocalSkillDocument = (agent: string, skillRelativePath: string) =>
  invoke<ProjectSkillDocument>("get_global_local_skill_document", { agent, skillRelativePath });

export const importGlobalLocalSkillToCenter = (agent: string, skillRelativePath: string) =>
  invoke<void>("import_global_local_skill_to_center", { agent, skillRelativePath });

export const updateGlobalLocalSkillFromCenter = (agent: string, skillRelativePath: string) =>
  invoke<void>("update_global_local_skill_from_center", { agent, skillRelativePath });

export const deleteGlobalLocalSkill = (agent: string, skillRelativePath: string) =>
  invoke<void>("delete_global_local_skill", { agent, skillRelativePath });

// ── Chain (three-tier links, xw fork) ──

export type ChainEntryStatus =
  | "link_repo"
  | "private"
  | "via_agents"
  | "direct"
  | "copy"
  | "internal"
  | "external"
  | "broken";

export interface ChainTracedEntry {
  name: string;
  entry_path: string;
  hops: string[];
  final_target: string;
  status: ChainEntryStatus;
  repo: string | null;
}

export interface ChainAggregateDir {
  path: string;
  entries: ChainTracedEntry[];
}

export interface ChainAgentSurface {
  agent: string;
  path: string;
  kind: "dir_link" | "per_entry" | "absent";
  dir_link_target: string | null;
  dir_link_ok: boolean;
  entries: ChainTracedEntry[];
}

export interface ChainProject {
  name: string;
  path: string;
  agents_dir: ChainAggregateDir | null;
  surfaces: ChainAgentSurface[];
}

export interface ChainRepoSkill {
  name: string;
  path: string;
}

/** A configured Git remote's identity (origin / upstream). */
export interface ChainRepoRemote {
  name: string;
  url: string;
}

/** Read-only Git health: working-tree cleanliness plus the current branch's
 * position against its configured upstream tracking branch. */
export type ChainRepoState =
  | "up_to_date"
  | "ahead"
  | "behind"
  | "diverged"
  | "no_upstream"
  | "detached"
  | "scan_error";

export interface ChainRepoHealth {
  dirty: boolean;
  state: ChainRepoState;
  ahead: number;
  behind: number;
  branch: string | null;
  /** Reason, present only when state === "scan_error". */
  error: string | null;
}

/** A registered project depending on a repo, by canonical identity. */
export interface ChainProjectRef {
  name: string;
  path: string;
}

export interface ChainRepo {
  name: string;
  path: string;
  /** Patchbay central library or an optional developer Git checkout. */
  source_kind: "managed" | "checkout";
  /** Warehouse root this repo was found under — its source root. */
  root: string;
  health: ChainRepoHealth;
  /** origin remote identity, when configured. */
  origin: ChainRepoRemote | null;
  /** upstream remote identity, shown distinctly from origin; null when absent. */
  upstream: ChainRepoRemote | null;
  skills: ChainRepoSkill[];
  referenced_by: ChainProjectRef[];
}

export interface ChainGuardViolation {
  skill: string;
  path: string;
  final_target: string;
  is_link: boolean;
}

export interface ChainGuardSurface {
  agent: string;
  path: string;
  state: "empty" | "absent" | "violation";
  violations: ChainGuardViolation[];
}

/** Per-root scan status so a missing/unreadable root shows explicitly. */
export interface ChainRootStatus {
  root: string;
  status: "ok" | "missing" | "unreadable";
  error: string | null;
  repo_count: number;
}

export interface ChainTopology {
  /** Ordered, de-duplicated Original Repository roots with per-root status. */
  warehouse_roots: ChainRootStatus[];
  projects_root: string;
  repos: ChainRepo[];
  projects: ChainProject[];
  guard: ChainGuardSurface[];
  scanned_at: number;
}

export const getChainTopology = () => invoke<ChainTopology>("chain_get_topology");

export interface ChainRegisteredProject {
  name: string;
  path: string;
}

/**
 * Enrol a chosen folder as a registered project for ongoing chain management.
 * Persists it so it appears in the topology and survives rescans and restarts.
 */
export const chainRegisterProject = (path: string) =>
  invoke<ChainRegisteredProject>("chain_register_project", { path });

/** A configured root plus a lightweight readability status, for settings. */
export interface ChainRootConfig {
  path: string;
  status: "ok" | "missing" | "unreadable";
  error: string | null;
}

export const getWarehouseRoots = () => invoke<ChainRootConfig[]>("chain_get_warehouse_roots");

export const setWarehouseRoots = (roots: string[]) =>
  invoke<ChainRootConfig[]>("chain_set_warehouse_roots", { roots });

export interface ChainOpResult {
  name: string;
  path: string;
  action: "created" | "exists" | "removed" | "absent" | "skipped" | "conflict" | "error";
  message: string | null;
}

export interface ChainLinkReport {
  agg_dir: string;
  skills: ChainOpResult[];
  entries: ChainOpResult[];
}

/** On-disk state of a target, captured at plan time and re-checked at apply. */
export type ChainEntryEvidence =
  | { state: "absent" }
  | { state: "symlink"; target: string }
  | { state: "dir" }
  | { state: "file" };

/** One previewed target in a link plan, shown before anything is written. */
export interface ChainPlanItem {
  name: string;
  path: string;
  action: "created" | "exists" | "conflict" | "error";
  scope: "aggregate" | "surface";
  message: string | null;
}

/** A previewed, guarded link operation. Produced by plan, consumed by apply. */
export interface ChainLinkPlan {
  project: string;
  agg_dir: string;
  originals: string[];
  agents: string[];
  skills: ChainPlanItem[];
  entries: ChainPlanItem[];
  evidence: Record<string, ChainEntryEvidence>;
}

/** Apply result plus proof, from a rescan, that the chain is really on disk. */
export interface ChainApplyOutcome {
  report: ChainLinkReport;
  verified: boolean;
  observed: string[];
  missing: string[];
}

/** Preview linking Skills into a project without writing anything. */
export const chainPlanLink = (projectPath: string, skillPaths: string[], agents: string[]) =>
  invoke<ChainLinkPlan>("chain_plan_link", { projectPath, skillPaths, agents });

/** Apply a previewed plan; refuses targets that changed since the preview. */
export const chainApplyLink = (plan: ChainLinkPlan) =>
  invoke<ChainApplyOutcome>("chain_apply_link", { plan });

export const chainUnlinkSkill = (projectPath: string, skillName: string) =>
  invoke<ChainOpResult[]>("chain_unlink_skill", { projectPath, skillName });

/** One previewed unlink action, Agent-scope aware. */
export interface ChainUnlinkItem {
  name: string;
  path: string;
  scope: "surface" | "aggregate";
  /** Agent key for surface items; null for the shared aggregate. */
  agent: string | null;
  kind: "per_agent_entry" | "shared_surface" | "aggregate";
  action: "remove" | "retain" | "shared" | "conflict" | "absent";
  message: string | null;
}

/** A previewed, guarded unlink operation. Produced by plan, consumed by apply. */
export interface ChainUnlinkPlan {
  project: string;
  skill: string;
  agents: string[];
  items: ChainUnlinkItem[];
  evidence: Record<string, ChainEntryEvidence>;
  /** Every Agent that would lose access to the Skill if applied. */
  affected_agents: string[];
  /** True when the operation removes the shared aggregate (affects every
   * dir-link Agent) or targets a dir-link surface — needs explicit confirm. */
  shared_surface: boolean;
}

/** Apply result plus proof, from a rescan, that access was removed as intended. */
export interface ChainUnlinkOutcome {
  report: ChainOpResult[];
  verified: boolean;
  still_linked: string[];
  removed_from: string[];
}

/** Preview an Agent-aware unlink without writing anything. Empty `agents`
 * previews unlinking from every Agent currently exposing the Skill. */
export const chainPlanUnlink = (projectPath: string, skillName: string, agents: string[]) =>
  invoke<ChainUnlinkPlan>("chain_plan_unlink", { projectPath, skillName, agents });

/** Apply a previewed unlink plan; removes only validated symlinks. */
export const chainApplyUnlink = (plan: ChainUnlinkPlan) =>
  invoke<ChainUnlinkOutcome>("chain_apply_unlink", { plan });

// ── Chain Global Guard remediation ──

/** A previewed remediation of a global Skill violation into a project. */
export interface ChainRemediationPlan {
  global_path: string;
  skill: string;
  agent: string;
  final_target: string;
  is_link: boolean;
  project: string;
  agents: string[];
  link_plan: ChainLinkPlan | null;
  remove_global: boolean;
  global_evidence: ChainEntryEvidence;
  guidance: string | null;
}

/** Result of applying a remediation: the project link plus whether the global
 * entry was retired, or guidance when it was left in place. */
export interface ChainRemediationOutcome {
  link: ChainApplyOutcome | null;
  global_removed: boolean;
  verified: boolean;
  guidance: string | null;
  scanned_at: number;
  guard: ChainGuardSurface[];
}

/** Preview remediating a global violation into a registered project + Agents. */
export const chainPlanRemediate = (globalPath: string, projectPath: string, agents: string[]) =>
  invoke<ChainRemediationPlan>("chain_plan_remediate", { globalPath, projectPath, agents });

/** Apply a remediation: verify the project chain before retiring the global entry. */
export const chainApplyRemediate = (plan: ChainRemediationPlan) =>
  invoke<ChainRemediationOutcome>("chain_apply_remediate", { plan });

// ── Chain Doctor (read-only findings) ──

export type ChainSeverity = "violation" | "warning" | "advice" | "notice";

export type ChainDeviation =
  | "broken"
  | "direct"
  | "copy"
  | "project_private"
  | "legacy"
  | "orphan";

export interface ChainAffectedObject {
  /** "skill" | "project" | "repo" | "surface" */
  kind: string;
  name: string;
  path: string;
}

/** The same hop-by-hop chain Link Topology shows, carried on the finding. */
export interface ChainEvidence {
  entry_path: string;
  hops: string[];
  final_target: string;
  topology_status: string;
}

export interface ChainFinding {
  /** Stable rule identifier, e.g. "chain.direct_link". */
  rule: string;
  deviation: ChainDeviation;
  severity: ChainSeverity;
  evidence: ChainEvidence;
  affected: ChainAffectedObject[];
  /** Stable action codes Patchbay could offer; read-only Doctor runs none. */
  actions: string[];
  /** Hash over rule + material evidence, for future ignore records. */
  fingerprint: string;
}

/** The two ways a finding can be classified out of the visible set. */
export type ChainDecisionKind = "ignored" | "project_private";

/**
 * A persisted decision to hide a Doctor finding, keyed by rule + evidence
 * fingerprint so a materially changed chain is reconsidered.
 */
export interface ChainFindingDecision {
  rule: string;
  fingerprint: string;
  /** "ignored" (generic accept) | "project_private" (legitimate physical Skill). */
  kind: ChainDecisionKind;
  note: string | null;
  created_at: number;
}

export interface ChainDoctorReport {
  findings: ChainFinding[];
  /** Findings hidden by a persisted ignore/project-private decision, returned
   * unfiltered so the "Ignored" panel is complete and each can be restored. */
  ignored: ChainFinding[];
  /** Visible findings before any filter — lets the UI show "N of M". */
  total: number;
  scanned_at: number;
}

export interface ChainDoctorFilter {
  severities: ChainSeverity[];
  deviations: ChainDeviation[];
}

/** Read-only diagnosis. Omitting the filter returns every visible finding. */
export const chainDoctorReport = (filter?: ChainDoctorFilter) =>
  invoke<ChainDoctorReport>("chain_doctor_report", { filter: filter ?? null });

/**
 * Hide a Doctor finding by persisting a decision keyed on its rule and evidence
 * fingerprint. `kind` is "ignored" (generic accept) or "project_private"
 * (classify a legitimate physical Skill). Never touches Skill contents.
 */
export const chainIgnoreFinding = (
  rule: string,
  fingerprint: string,
  kind: ChainDecisionKind,
  note: string | null,
) => invoke<void>("chain_ignore_finding", { rule, fingerprint, kind, note });

/** Restore a previously hidden finding, removing its persisted decision. */
export const chainRestoreFinding = (rule: string, fingerprint: string) =>
  invoke<void>("chain_restore_finding", { rule, fingerprint });

// ── Chain broken-link candidates (read-only evidence, issue #30) ──

/** One place a broken link's dead target may have gone, with evidence. */
export interface ChainRepairCandidate {
  /** Absolute path of the candidate Original Skill directory. */
  path: string;
  /** The candidate's directory name. */
  name: string;
  /** Confidence 0–100: 98 git rename, 95 same name, else name similarity. */
  score: number;
  /** "git_rename" | "same_name" | "similar_name" */
  reason: string;
  /** Commit time (unix seconds) of the rename; git_rename only. */
  renamed_at: number | null;
}

/** Mirror of the Rust `candidates::RELINK_THRESHOLD`: the planner only relinks
 * a broken entry to a candidate at or above this score — below it, the plan
 * falls back to removing the dangling link, so the card must not promise a
 * rebuild. */
export const CHAIN_RELINK_THRESHOLD = 75;

/** Per-fingerprint candidates from one fresh scan. A requested fingerprint
 * with no current broken finding or no plausible target is absent. */
export interface ChainCandidatesReport {
  candidates: Record<string, ChainRepairCandidate[]>;
  scanned_at: number;
}

/** Read-only candidate location for broken findings: where each dead target
 * likely went. Evidence for the workbench card; nothing is planned or written. */
export const chainLocateCandidates = (fingerprints: string[]) =>
  invoke<ChainCandidatesReport>("chain_locate_candidates", { fingerprints });

// ── Chain Doctor repair (mutating: normalize noncanonical chains) ──

/** One planned or applied repair edit for a single link of a Doctor finding. */
export interface ChainRepairItem {
  /** Fingerprint of the Doctor finding this item repairs. */
  fingerprint: string;
  rule: string;
  /** "broken" | "direct" | "legacy" */
  deviation: string;
  /** Registered project root the edited link lives in. */
  project: string;
  path: string;
  /** "ensure_aggregate" | "repoint_entry" | "relink_broken" | "remove_broken" */
  kind: string;
  /** "create" | "repoint" | "remove" | "exists" | "conflict" | "skip" | "error" */
  action: string;
  old_target: string | null;
  new_target: string | null;
  message: string | null;
}

/** One recoverable pre-change record: a changed link with its prior target. */
export interface ChainRepairSnapshotEntry {
  path: string;
  target: string;
}

/** A previewed, guarded repair operation. Produced by plan, consumed by apply. */
export interface ChainRepairPlan {
  items: ChainRepairItem[];
  /** Target path -> on-disk state captured at plan time (re-checked at apply). */
  evidence: Record<string, ChainEntryEvidence>;
  /** Pre-change target of every existing link the apply would change (recovery). */
  snapshot: ChainRepairSnapshotEntry[];
  /** Requested fingerprints that map to no current supported finding. */
  unsupported: string[];
  scanned_at: number;
}

/** Apply result plus proof, from a rescan, that the chain is normalized. */
export interface ChainRepairOutcome {
  results: ChainRepairItem[];
  /** True only when the apply was clean and the rescan confirmed the chain. */
  verified: boolean;
  scanned_at: number;
  /** Repair-journal record id when the apply wrote anything (issue #31). */
  journal_id: number | null;
}

// ── Chain live repair (step events + pause/takeover, issue #32) ──

/** One narrated step transition on the `chain-repair-live` event channel. */
export interface ChainLiveEvent {
  run_id: string;
  /** Per-run monotonic counter for dropping stale deliveries. */
  seq: number;
  /** "check" | "locate" | "rebuild" | "verify" */
  step: string;
  /** "start" | "done" | "failed" */
  status: string;
  /** Raw evidence lines (paths, scores, edits); labels are the frontend's. */
  detail: string | null;
}

/** Terminal result of a live run. `aborted` means a takeover stopped the run
 * BEFORE rebuild — zero writes; otherwise `outcome` carries the journaled
 * apply. */
export interface ChainLiveOutcome {
  aborted: boolean;
  outcome: ChainRepairOutcome | null;
}

/** Start a narrated live repair. Progress streams as `chain-repair-live`
 * events (filter by `run_id`); the invoke resolves with the terminal outcome.
 * `preferRoot` (a detected repo-move destination, #33) pins same-name ties
 * to the detected new location. */
export const chainRepairLive = (
  fingerprints: string[],
  runId: string,
  preferRoot?: string,
) =>
  invoke<ChainLiveOutcome>("chain_repair_live", {
    fingerprints,
    runId,
    preferRoot: preferRoot ?? null,
  });

// ── Chain assembly presets (issue #35) ──

/** One warehouse skill reference inside a chain preset. */
export interface ChainPresetSkill {
  name: string;
  /** Absolute path of the Original the chain resolved to at save time. */
  path: string;
  /** Repository display name, when known. */
  repo: string | null;
}

/** A named chain assembly preset (consumed by the #36 wizard as a batch-link
 * starting point). */
export interface ChainPreset {
  id: number;
  name: string;
  skills: ChainPresetSkill[];
  created_at: number;
}

export const chainPresetsList = () =>
  invoke<ChainPreset[]>("chain_presets_list");

export const chainPresetSave = (name: string, skills: ChainPresetSkill[]) =>
  invoke<ChainPreset>("chain_preset_save", { name, skills });

export const chainPresetRename = (id: number, name: string) =>
  invoke<void>("chain_preset_rename", { id, name });

export const chainPresetDelete = (id: number) =>
  invoke<void>("chain_preset_delete", { id });

// ── Chain dirty-repo feedback evidence (issue #34) ──

/** One tracked file with uncommitted changes (staged + unstaged vs HEAD). */
export interface ChainDirtyFile {
  path: string;
  /** "added" | "modified" | "deleted" | "renamed" | "typechange" | "other" */
  status: string;
  additions: number;
  deletions: number;
}

/** A repository's uncommitted tracked changes; untracked files excluded
 * (mirrors the health `dirty` flag's `git status -uno` semantics). */
export interface ChainDirtyDiff {
  repo: string;
  files: ChainDirtyFile[];
  truncated: boolean;
}

/** Per-file evidence for the feedback card's diff panel. Read-only; the
 * repository must be part of the current topology. */
export const chainRepoDirtyDiff = (repoPath: string) =>
  invoke<ChainDirtyDiff>("chain_repo_dirty_diff", { repoPath });

// ── Chain common-cause analysis (repo-move storms, issue #33) ──

/** One detected repository move: the root cause plus its blast radius. */
export interface ChainRepoMove {
  /** The vanished root every dead target shares. */
  old_root: string;
  /** The scanned location holding same-name skills for every member. */
  new_root: string;
  /** Display name of the destination repository. */
  repo_name: string;
  skills: string[];
  /** Fingerprints of the aggregated broken findings. */
  fingerprints: string[];
  entry_paths: string[];
}

export interface ChainRepoMoveReport {
  groups: ChainRepoMove[];
  scanned_at: number;
}

/** Whole-repository moves detected behind broken-link storms. Read-only. */
export const chainRepoMoves = () =>
  invoke<ChainRepoMoveReport>("chain_repo_moves");

/** Steer a live run: "pause" | "resume" | "takeover". */
export const chainRepairLiveControl = (runId: string, action: string) =>
  invoke<void>("chain_repair_live_control", { runId, action });

// ── Chain repair journal (durable record + one-click undo, issue #31) ──

/** A persisted repair record: the applied items verbatim ARE the undo
 * material (each carries path/old_target/new_target). */
export interface ChainJournalRecord {
  id: number;
  /** Unix seconds of the apply. */
  created_at: number;
  /** Distinct registered project roots the writing items edited. */
  projects: string[];
  /** Distinct fingerprints of the findings the writing items repaired. */
  fingerprints: string[];
  items: ChainRepairItem[];
  /** Apply-time verification flag. */
  verified: boolean;
  /** "applied" | "undone" */
  status: string;
  /** Record card hidden by the user; history retained. */
  dismissed: boolean;
}

/** The result of undoing a journaled repair. `verified` is true only when
 * every inverse landed AND the rescan shows the original findings back. */
export interface ChainUndoOutcome {
  results: ChainRepairItem[];
  verified: boolean;
  scanned_at: number;
}

/** The repair journal, newest first. */
export const chainRepairJournal = (limit?: number) =>
  invoke<ChainJournalRecord[]>("chain_repair_journal", { limit: limit ?? null });

/** One-click undo of a journaled repair: per-item guarded inverse replay,
 * audited, then rescanned and verified. */
export const chainUndoRepair = (id: number) =>
  invoke<ChainUndoOutcome>("chain_undo_repair", { id });

/** Hide a repair record's workbench card without deleting the history. */
export const chainDismissRepairRecord = (id: number) =>
  invoke<void>("chain_dismiss_repair_record", { id });

/** Preview repairs for the given Doctor findings (by fingerprint) without writing.
 * The service re-scans so the plan is built from current evidence. */
export const chainPlanRepair = (fingerprints: string[]) =>
  invoke<ChainRepairPlan>("chain_plan_repair", { fingerprints });

/** Apply a previewed repair plan; refuses items that changed since the preview. */
export const chainApplyRepair = (plan: ChainRepairPlan) =>
  invoke<ChainRepairOutcome>("chain_apply_repair", { plan });

// ── Chain duplicate checkouts (read-only) ──

/** One checkout that shares a normalized remote identity with another. */
export interface ChainDuplicateCheckout {
  /** Directory name (not the identity — same names with different remotes never group). */
  name: string;
  path: string;
  /** Short HEAD sha, or null when HEAD cannot be read. */
  revision: string | null;
  dirty: boolean;
  state: ChainRepoState;
  branch: string | null;
  origin: ChainRepoRemote | null;
  upstream: ChainRepoRemote | null;
  referenced_by: ChainProjectRef[];
}

/** Two or more checkouts resolving to one normalized remote identity. */
export interface ChainDuplicateGroup {
  /** Normalized remote identity, e.g. "github.com/org/repo". */
  identity: string;
  checkouts: ChainDuplicateCheckout[];
  /** Stable, non-localized advisory codes (all_clean, some_dirty,
   * none_referenced, some_unreferenced, differing_revisions). Never a
   * delete/merge recommendation. */
  guidance: string[];
}

export interface ChainDuplicatesReport {
  /** Duplicate groups, sorted by identity. Empty when nothing is duplicated. */
  groups: ChainDuplicateGroup[];
  scanned_at: number;
}

/** Read-only detection of duplicate Original Repository checkouts. */
export const getChainDuplicates = () =>
  invoke<ChainDuplicatesReport>("chain_duplicate_checkouts");

// ── Chain fast-forward pull (mutating: fast-forward only) ──

/** Why a repository was withheld from a fast-forward. Stable, non-localized. */
export type ChainPullSkipReason =
  | "dirty"
  | "diverged"
  | "ahead"
  | "up_to_date"
  | "no_upstream"
  | "detached"
  | "scan_error";

/** One repository's read-only fast-forward classification, produced by preview. */
export interface ChainPullPreview {
  path: string;
  name: string;
  branch: string | null;
  /** Upstream tracking branch shorthand, e.g. "origin/main". */
  upstream: string | null;
  ahead: number;
  behind: number;
  dirty: boolean;
  /** "fast_forward" when eligible, "skip" otherwise. */
  action: "fast_forward" | "skip";
  /** Stable reason code when action === "skip". */
  reason: ChainPullSkipReason | null;
}

/** A previewed, guarded pull. Produced by preview, consumed unchanged by apply. */
export interface ChainPullPlan {
  items: ChainPullPreview[];
  scanned_at: number;
}

/** The outcome of attempting one repository's update. */
export interface ChainPullResult {
  path: string;
  name: string;
  /** "updated" | "skipped" | "up_to_date" | "error". */
  action: "updated" | "skipped" | "up_to_date" | "error";
  /** Short HEAD sha before the attempt. */
  from: string | null;
  /** Short HEAD sha after the attempt. */
  to: string | null;
  /** Stable skip code ("dirty", "untracked_collision", …) or error code
   * ("auth", "network", "fetch", "checkout", "scan_error"). */
  reason: string | null;
  /** Free-form detail, present for errors. */
  message: string | null;
}

/** Apply outcome plus a fresh timestamp from the confirming post-pull rescan. */
export interface ChainPullOutcome {
  results: ChainPullResult[];
  scanned_at: number;
}

/** Preview fast-forward-only pulls for the given repositories. Read-only: it
 * neither fetches nor mutates — clean & behind repos are eligible, every other
 * state is a skip with a precise reason. */
export const chainPlanPull = (repoPaths: string[]) =>
  invoke<ChainPullPlan>("chain_plan_pull", { repoPaths });

/** Apply a previewed pull plan; fast-forwards eligible repos only and never
 * resets, stashes, force-updates, merges, or auto-resolves conflicts. */
export const chainApplyPull = (plan: ChainPullPlan) =>
  invoke<ChainPullOutcome>("chain_apply_pull", { plan });

// ── Chain fork synchronization (mutating: fast-forward push to origin only) ──

/** Why a fork was withheld from synchronization. Stable, non-localized. */
export type ChainForkSyncSkipReason =
  | "no_origin"
  | "no_upstream"
  | "detached"
  | "ambiguous_branch"
  | "up_to_date"
  | "diverged"
  | "dirty"
  | "untracked_collision"
  | "auth"
  | "network"
  | "fetch"
  | "checkout";

/** One fork's read-only fork-sync classification (upstream → origin), produced
 * by preview. Every field is populated so the preview names source, target,
 * branch, and lag even for a skipped fork. */
export interface ChainForkSyncPreview {
  path: string;
  name: string;
  branch: string | null;
  /** origin fetch URL, when configured. */
  origin: string | null;
  /** upstream fetch URL, when configured. */
  upstream: string | null;
  /** Synchronization source, e.g. "upstream/main". */
  source: string | null;
  /** Synchronization target, e.g. "origin/main". */
  target: string | null;
  /** Commits origin/<branch> is AHEAD of upstream (must be 0 to sync). */
  ahead: number;
  /** Commits origin/<branch> is BEHIND upstream. */
  behind: number;
  /** Whether upstream/<branch> strictly descends from origin/<branch>. */
  fast_forwardable: boolean;
  /** "fast_forward" when eligible, "skip" otherwise. */
  action: "fast_forward" | "skip";
  /** Stable reason code when action === "skip". */
  reason: ChainForkSyncSkipReason | null;
}

/** A previewed, guarded fork-sync. Produced by preview, consumed unchanged by apply. */
export interface ChainForkSyncPlan {
  items: ChainForkSyncPreview[];
  scanned_at: number;
}

/** The outcome of attempting one fork's synchronization. */
export interface ChainForkSyncResult {
  path: string;
  name: string;
  /** "synced" | "skipped" | "up_to_date" | "error". */
  action: "synced" | "skipped" | "up_to_date" | "error";
  /** Short origin/<branch> sha before the attempt. */
  from: string | null;
  /** Short sha after the attempt (equal to the upstream tip). */
  to: string | null;
  /** Stable skip code or error code ("auth", "network", "fetch", "checkout"). */
  reason: string | null;
  /** Free-form detail, present for errors. */
  message: string | null;
}

/** Apply outcome plus a fresh timestamp from the confirming post-sync rescan. */
export interface ChainForkSyncOutcome {
  results: ChainForkSyncResult[];
  scanned_at: number;
}

/** Preview fast-forward-only fork synchronizations (upstream → origin) for the
 * given repositories. Read-only: it neither fetches nor pushes — a fork strictly
 * behind its upstream is eligible, every other state is a skip with a precise
 * reason. */
export const chainPlanForkSync = (repoPaths: string[]) =>
  invoke<ChainForkSyncPlan>("chain_plan_fork_sync", { repoPaths });

/** Apply a previewed fork-sync plan; advances origin to upstream by fast-forward
 * push only and never force-pushes, rebases, merges, or rewrites history. */
export const chainApplyForkSync = (plan: ChainForkSyncPlan) =>
  invoke<ChainForkSyncOutcome>("chain_apply_fork_sync", { plan });

// ── Instructions (AGENTS.md governance) ──

/**
 * One installed agent's entry shape into a project.
 * - `wrapper` — first line `@AGENTS.md`, nothing else (the compliant form)
 * - `wrapper_plus` — wrapper plus an agent-specific append layer
 * - `symlink` — entry is a symlink to the canonical body (compliant variant)
 * - `body` — real content lives in the entry file (dual-body / missing-canonical)
 * - `missing` — the agent has no entry into this project
 * - `native` — the agent reads the canonical `AGENTS.md` directly, no entry file
 */
export type InstructionsEntryState =
  | "wrapper"
  | "wrapper_plus"
  | "symlink"
  | "body"
  | "missing"
  | "native";

/** The canonical body `<project>/AGENTS.md`. */
export interface InstructionsCanonical {
  exists: boolean;
  path: string;
  bytes: number;
  lines: number;
  est_tokens: number;
}

/** One installed agent's primary entry into a project. */
export interface InstructionsEntry {
  agent: string;
  state: InstructionsEntryState;
  path: string;
  bytes: number;
  est_tokens: number;
}

/** One agent's resident-set cost, split project-side / global-side, with a
 * combined token estimate (design §2). */
export interface InstructionsResident {
  agent: string;
  project_bytes: number;
  global_bytes: number;
  est_tokens: number;
}

/** A personal/unmanaged-layer file, reported for cost visibility only. */
export interface InstructionsUnmanaged {
  agent: string;
  path: string;
  bytes: number;
  est_tokens: number;
}

/** One project's instructions surface. */
export interface InstructionsProject {
  path: string;
  canonical: InstructionsCanonical;
  entries: InstructionsEntry[];
  resident: InstructionsResident[];
  unmanaged: InstructionsUnmanaged[];
}

/** A machine-level global surface and the installed agents that read it. */
export interface InstructionsGlobalFile {
  path: string;
  bytes: number;
  est_tokens: number;
  readers: string[];
}

/** Full scan payload (design §5): one entry per scanned project, the machine's
 * global surfaces, the installed-agent set, and a scan timestamp. */
export interface InstructionsScanReport {
  projects: InstructionsProject[];
  globals: InstructionsGlobalFile[];
  agents: string[];
  scanned_at: number;
}

/**
 * Read-only instructions scan. With `project`, scans exactly that path (its
 * `projects[0]` is that project); without it, every registered project. Shares
 * the CLI's `InstructionsService` — the GUI never re-derives sizes or tokens.
 */
export const instructionsScan = (project?: string) =>
  invoke<InstructionsScanReport>("instructions_scan", { project: project ?? null });

// ── Instructions Doctor (read-only findings + persisted ignore decisions) ──

export type InstructionsRule =
  | "instructions.uninitialized"
  | "instructions.missing_canonical"
  | "instructions.dual_body"
  | "instructions.duplicate_content"
  | "instructions.missing_entry"
  | "instructions.symlink_entry"
  | "instructions.broken_import"
  | "instructions.import_in_canonical"
  | "instructions.oversized_body"
  | "instructions.hard_cap_risk"
  | "instructions.skill_missing"
  | "instructions.skill_unmentioned"
  | "instructions.entry_gitignored"
  | "instructions.global_cost";

export interface InstructionsLocation {
  path: string;
  line: number;
}

/** Instructions evidence replaces chain hops with paths, metrics, and concrete
 * source locations. Values in `metrics` are rendered as returned by the service;
 * the GUI does not derive diagnosis facts. */
export interface InstructionsEvidence {
  primary_path: string;
  counterpart_path?: string;
  metrics: Record<string, unknown>;
  locations: InstructionsLocation[];
}

export interface InstructionsAffectedObject {
  /** "project" | "canonical" | "entry" | "skill" | "agent" | "global" */
  kind: string;
  name: string;
  path: string;
}

export interface InstructionsFinding {
  rule: InstructionsRule;
  severity: ChainSeverity;
  evidence: InstructionsEvidence;
  affected: InstructionsAffectedObject[];
  actions: string[];
  fingerprint: string;
}

export interface InstructionsDoctorReport {
  findings: InstructionsFinding[];
  ignored: InstructionsFinding[];
  total: number;
  scanned_at: number;
}

export interface InstructionsDoctorFilter {
  severities: ChainSeverity[];
  rules: InstructionsRule[];
}

/** Read-only diagnosis from the shared InstructionsService. */
export const instructionsDoctorReport = (
  filter?: InstructionsDoctorFilter,
  project?: string,
) =>
  invoke<InstructionsDoctorReport>("instructions_doctor_report", {
    filter: filter ?? null,
    project: project ?? null,
  });

/** Persist an `ignored` instructions decision; instructions has no
 * project-private decision kind. */
export const instructionsIgnoreFinding = (
  rule: string,
  fingerprint: string,
  note: string | null,
) => invoke<void>("instructions_ignore_finding", { rule, fingerprint, note });

/** Remove a persisted instructions ignore decision. */
export const instructionsRestoreFinding = (rule: string, fingerprint: string) =>
  invoke<void>("instructions_restore_finding", { rule, fingerprint });

/** On-disk evidence captured during preview and re-checked before apply. */
export type InstructionsWriteEvidence =
  | { state: "absent" }
  | { state: "symlink"; target: string }
  | { state: "dir" }
  | { state: "file"; sha256: string };

/** A read input whose content the previewed output depends on. */
export interface InstructionsSourceGuard {
  path: string;
  before: InstructionsWriteEvidence;
}

/** One previewed or applied normalize file action. */
export interface InstructionsNormalizeItem {
  fingerprint: string;
  rule: string;
  project: string;
  path: string;
  action: "create" | "rewrite" | "replace_link" | "noop" | "conflict";
  before: InstructionsWriteEvidence;
  after_content?: string;
  snapshot: boolean;
  depends_on?: InstructionsSourceGuard;
  message?: string;
}

/** Evidence-carrying normalize preview consumed unchanged by apply. */
export interface InstructionsNormalizePlan {
  items: InstructionsNormalizeItem[];
  unsupported: string[];
  scanned_at: number;
}

/** Normalize apply results plus the service's fresh-rescan verdict. */
export interface InstructionsNormalizeOutcome {
  items: InstructionsNormalizeItem[];
  snapshot_id: string | null;
  verified: boolean;
  scanned_at: number;
}

/** Preview every fixable instructions finding in one project without writing. */
export const instructionsPlanNormalize = (projectPath: string, fingerprints: string[] = []) =>
  invoke<InstructionsNormalizePlan>("instructions_plan_normalize", {
    projectPath,
    fingerprints,
  });

/** Apply exactly the guarded normalize preview and return the rescan verdict. */
export const instructionsApplyNormalize = (
  projectPath: string,
  plan: InstructionsNormalizePlan,
) => invoke<InstructionsNormalizeOutcome>("instructions_apply_normalize", { projectPath, plan });

/** One previewed or applied create-only init target. */
export interface InstructionsInitItem {
  path: string;
  kind: "canonical" | "entry" | "docs_dir";
  action: "create" | "noop" | "conflict";
  before: InstructionsWriteEvidence;
  after_content?: string;
  message?: string;
}

/** Create-only init preview consumed unchanged by apply. */
export interface InstructionsInitPlan {
  items: InstructionsInitItem[];
  scanned_at: number;
}

/** Init apply results plus the service's verification verdict. */
export interface InstructionsInitOutcome {
  items: InstructionsInitItem[];
  verified: boolean;
  scanned_at: number;
}

/** Preview an instructions scaffold without writing. */
export const instructionsPlanInit = (projectPath: string, docsDir = false) =>
  invoke<InstructionsInitPlan>("instructions_plan_init", { projectPath, docsDir });

/** Apply exactly the create-only init preview. */
export const instructionsApplyInit = (projectPath: string, plan: InstructionsInitPlan) =>
  invoke<InstructionsInitOutcome>("instructions_apply_init", { projectPath, plan });

// ── Fleet (multi-machine repo sync, P0 read-only) ─────────────────────────

/** One repo's state on one machine (self column live, others reported). */
export interface FleetCell {
  name: string;
  present: boolean;
  branch: string | null;
  head: string | null;
  dirty: number | null;
  detached: boolean;
  ahead: number | null;
  behind: number | null;
  note?: string | null;
}

export interface FleetMachineColumn {
  id: string;
  display_name: string | null;
  is_self: boolean;
  /** RFC3339; null for the live-measured self column. */
  reported_at: string | null;
}

export interface FleetRepoRow {
  name: string;
  hub: string;
  authority: string;
  branch: string;
  auto_sync: boolean;
  hub_head: string | null;
  hub_note: string | null;
  cells: Record<string, FleetCell>;
}

/** Status matrix: rows = manifest repos, columns = machines (design §6). */
export interface FleetStatus {
  machine: string;
  meta_url: string;
  meta_state: "fresh" | "stale";
  meta_warning: string | null;
  projects_root: string;
  scanned_at: number;
  machines: FleetMachineColumn[];
  repos: FleetRepoRow[];
  warnings: string[];
}

export interface FleetDiscoveredRepo {
  name: string;
  path: string;
  origin: string | null;
}

export interface FleetDiscovery {
  machine: string;
  projects_root: string;
  scanned_at: number;
  unlisted: FleetDiscoveredRepo[];
}

export interface FleetManifestRepo {
  name: string;
  hub: string;
  authority: string;
  branch: string;
}

export interface FleetManifest {
  fleet: { projects_root: string | null };
  hubs: Record<string, { url: string; host_machine: string | null }>;
  repos: FleetManifestRepo[];
}

export interface FleetManifestSnapshot {
  machine: string;
  meta_head: string;
  manifest_digest: string;
  manifest: FleetManifest;
  known_machines: string[];
}

export interface FleetManifestChange {
  action: "add" | "update" | "remove";
  repo: string;
  before: FleetManifestRepo | null;
  after: FleetManifestRepo | null;
}

export interface FleetManifestUpdatePlan {
  machine: string;
  meta_head: string;
  manifest_digest: string;
  planned_at: number;
  manifest: FleetManifest;
  changes: FleetManifestChange[];
}

export interface FleetManifestUpdateOutcome {
  ok: boolean;
  action: "updated" | "unchanged" | "conflict";
  pushed: boolean;
  commit: string | null;
  manifest_digest: string;
  changes: FleetManifestChange[];
  message: string | null;
}

type FleetManifestUpdateResponse =
  | { mode: "preview"; plan: FleetManifestUpdatePlan }
  | { mode: "apply"; outcome: FleetManifestUpdateOutcome };

export interface FleetReportResult {
  ok: boolean;
  machine: string;
  meta_url: string;
  action: string;
  pushed: boolean;
  commit: string | null;
}

export interface FleetAutoRoundAttention {
  repo: string;
  reason: string;
  message: string;
}

export interface FleetAutoRoundResult {
  ok: boolean;
  finished_at: number;
  pulled: string[];
  pushed: string[];
  attention: FleetAutoRoundAttention[];
}

export interface FleetAutoRoundStatus {
  enabled: boolean;
  in_backoff: boolean;
  next_round_at: number | null;
  consecutive_failures: number;
  last_round: FleetAutoRoundResult | null;
}

export interface FleetAutoSyncUpdateOutcome {
  repo: string;
  enabled: boolean;
  pushed: boolean;
  commit: string | null;
}

export interface FleetPushEvidence {
  head_oid: string;
  dirty_count: number;
  branch: string;
  remote_url: string;
}

export interface FleetPushPlanItem {
  repo: string;
  status: "ready" | "refused";
  reason_code: string | null;
  message: string | null;
  evidence: FleetPushEvidence | null;
}

export interface FleetPushPlan {
  ok: boolean;
  machine: string;
  planned_at: number;
  items: FleetPushPlanItem[];
}

export interface FleetPushResult {
  repo: string;
  action: "pushed" | "up_to_date" | "refused" | "conflict" | "error";
  reason_code: string | null;
  message: string | null;
  before_head: string | null;
  after_head: string | null;
}

export interface FleetPushOutcome {
  ok: boolean;
  machine: string;
  items: FleetPushResult[];
}

export const fleetStatus = () => invoke<FleetStatus>("fleet_status");

export const fleetAutoStatus = () =>
  invoke<FleetAutoRoundStatus>("fleet_auto_status");

export const fleetSetRepoAutoSync = (repo: string, enabled: boolean) =>
  invoke<FleetAutoSyncUpdateOutcome>("fleet_set_repo_auto_sync", { repo, enabled });
export const fleetDiscover = () => invoke<FleetDiscovery>("fleet_discover");

export const fleetManifestGet = () =>
  invoke<FleetManifestSnapshot>("fleet_manifest_get");

export const fleetManifestPreview = async (
  base: FleetManifestSnapshot,
  repos: FleetManifestRepo[],
) => {
  const response = await invoke<FleetManifestUpdateResponse>("fleet_manifest_update", {
    request: { mode: "preview", base, repos },
  });
  if (response.mode !== "preview") throw new Error("invalid fleet manifest preview response");
  return response.plan;
};

export const fleetManifestApply = async (plan: FleetManifestUpdatePlan) => {
  const response = await invoke<FleetManifestUpdateResponse>("fleet_manifest_update", {
    request: { mode: "apply", plan },
  });
  if (response.mode !== "apply") throw new Error("invalid fleet manifest apply response");
  return response.outcome;
};

export const fleetPlanPush = (repos: string[]) =>
  invoke<FleetPushPlan>("fleet_plan_push", { repos });

export const fleetApplyPush = (plan: FleetPushPlan) =>
  invoke<FleetPushOutcome>("fleet_apply_push", { plan });

export interface FleetPullEvidence {
  head_oid: string;
  target_oid: string;
  dirty_count: number;
  branch: string;
  remote_url: string | null;
  hub_url: string;
}

export interface FleetPullPlanItem {
  repo: string;
  status: "ready" | "refused";
  reason_code: string | null;
  message: string | null;
  evidence: FleetPullEvidence | null;
}

export interface FleetPullPlan {
  ok: boolean;
  machine: string;
  manifest_digest: string;
  planned_at: number;
  items: FleetPullPlanItem[];
}

export interface FleetPullResult {
  repo: string;
  action: "pulled" | "refused" | "conflict" | "error";
  reason_code: string | null;
  message: string | null;
  before_head: string | null;
  after_head: string | null;
}

export interface FleetPullOutcome {
  ok: boolean;
  machine: string;
  items: FleetPullResult[];
}

export const fleetPlanPull = (repos: string[]) =>
  invoke<FleetPullPlan>("fleet_plan_pull", { repos });

export const fleetApplyPull = (plan: FleetPullPlan) =>
  invoke<FleetPullOutcome>("fleet_apply_pull", { plan });

export interface FleetBootstrapEvidence {
  target_path: string;
  hub_name: string;
  hub_url: string;
  branch: string;
  target_oid: string;
}

export interface FleetBootstrapPlanItem {
  repo: string;
  status: "ready" | "refused";
  reason_code: string | null;
  message: string | null;
  evidence: FleetBootstrapEvidence | null;
}

export interface FleetBootstrapPlan {
  ok: boolean;
  machine: string;
  manifest_digest: string;
  planned_at: number;
  items: FleetBootstrapPlanItem[];
}

export interface FleetBootstrapResult {
  repo: string;
  action: "bootstrapped" | "refused" | "conflict" | "error";
  reason_code: string | null;
  message: string | null;
  after_head: string | null;
}

export interface FleetBootstrapOutcome {
  ok: boolean;
  machine: string;
  items: FleetBootstrapResult[];
}

export const fleetPlanBootstrap = (repos: string[]) =>
  invoke<FleetBootstrapPlan>("fleet_plan_bootstrap", { repos });

export const fleetApplyBootstrap = (plan: FleetBootstrapPlan) =>
  invoke<FleetBootstrapOutcome>("fleet_apply_bootstrap", { plan });

/** Push this machine's status report to the fleet meta repo. */
export const fleetReport = () => invoke<FleetReportResult>("fleet_report");
