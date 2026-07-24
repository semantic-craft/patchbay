use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::core::{chain, error::AppError, skill_store::SkillStore};

#[tauri::command]
pub async fn chain_get_topology(
    store: State<'_, Arc<SkillStore>>,
) -> Result<chain::ChainTopology, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::ChainService::new(&store).scan()).await?
}

/// A project just enrolled for chain management. Only the fields the topology
/// view needs cross the wire, matching the chain module's purpose-built result
/// types rather than leaking the full persistence record.
#[derive(Serialize)]
pub struct RegisteredProject {
    pub name: String,
    pub path: String,
}

/// Enrol a chosen folder as a registered project for ongoing chain management.
/// Persists it so it appears in the topology and survives rescans and restarts;
/// idempotent for a directory that is already registered.
/// Read-only Doctor report: topology deviations as stable, filterable findings.
/// Delegates to the Chain Service — the command layer holds no diagnosis rules.
/// An omitted filter returns every finding.
#[tauri::command]
pub async fn chain_doctor_report(
    store: State<'_, Arc<SkillStore>>,
    filter: Option<chain::doctor::DoctorFilter>,
) -> Result<chain::doctor::DoctorReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let filter = filter.unwrap_or_default();
        chain::ChainService::new(&store).doctor(&filter)
    })
    .await?
}

/// Read-only duplicate-checkout report: Original Repository checkouts that
/// resolve to the same remote identity, with evidence and advisory-only
/// guidance. Delegates to the Chain Service; the command layer decides nothing
/// and never deletes or merges.
#[tauri::command]
pub async fn chain_duplicate_checkouts(
    store: State<'_, Arc<SkillStore>>,
) -> Result<chain::duplicates::DuplicatesReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).duplicate_checkouts()
    })
    .await?
}

#[tauri::command]
pub async fn chain_register_project(
    store: State<'_, Arc<SkillStore>>,
    path: String,
) -> Result<RegisteredProject, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let record = chain::ChainService::new(&store).enrol_project(Path::new(&path))?;
        Ok(RegisteredProject {
            name: record.name,
            path: record.path,
        })
    })
    .await?
}

/// Preview linking Skills into a project: returns the plan (target paths,
/// intended actions, conflicts, and the on-disk evidence apply re-checks)
/// without writing anything. The project must already be registered.
#[tauri::command]
pub async fn chain_plan_link(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    skill_paths: Vec<String>,
    agents: Vec<String>,
) -> Result<chain::ops::LinkPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = PathBuf::from(&project_path);
        let originals: Vec<PathBuf> = skill_paths.iter().map(PathBuf::from).collect();
        chain::ChainService::new(&store).plan_link(&project, &originals, &agents)
    })
    .await?
}

/// Apply a previewed link plan. Re-validates the write boundary, refuses any
/// target whose evidence changed since the preview, and rescans to confirm the
/// requested chain before reporting success.
#[tauri::command]
pub async fn chain_apply_link(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::ops::LinkPlan,
) -> Result<chain::service::ApplyOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::ChainService::new(&store).apply_link(&plan))
        .await?
}

#[tauri::command]
pub async fn chain_unlink_skill(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    skill_name: String,
) -> Result<Vec<chain::ops::OpResult>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = PathBuf::from(&project_path);
        chain::ChainService::new(&store).unlink(&project, &skill_name)
    })
    .await?
}

/// Preview an Agent-aware unlink: returns the plan distinguishing per-Agent
/// entry links from shared-directory-surface operations, the Agents that would
/// lose access, and the evidence apply re-checks — without writing anything.
/// An empty `agents` list previews unlinking from every Agent exposing the Skill.
#[tauri::command]
pub async fn chain_plan_unlink(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    skill_name: String,
    agents: Vec<String>,
) -> Result<chain::ops::UnlinkPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = PathBuf::from(&project_path);
        chain::ChainService::new(&store).plan_unlink(&project, &skill_name, &agents)
    })
    .await?
}

/// Apply a previewed unlink plan. Removes only validated symlinks (never a
/// physical directory or Original), audits the operation, and rescans to
/// confirm the Skill was removed where intended and preserved elsewhere.
#[tauri::command]
pub async fn chain_apply_unlink(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::ops::UnlinkPlan,
) -> Result<chain::service::UnlinkOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).apply_unlink(&plan)
    })
    .await?
}

/// Read-only candidate location for broken findings (issue #30): where each
/// dead target likely went — same-name/near-name Skills across the topology
/// plus a bounded Git rename probe. Evidence for the workbench card only;
/// nothing is planned or written.
#[tauri::command]
pub async fn chain_locate_candidates(
    store: State<'_, Arc<SkillStore>>,
    fingerprints: Vec<String>,
) -> Result<chain::candidates::CandidatesReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).locate_candidates(&fingerprints)
    })
    .await?
}

/// Preview repairs for the given Doctor findings, identified by fingerprint,
/// without writing anything: returns the smallest edit per finding, the on-disk
/// evidence apply re-checks, a recoverable snapshot, and any unsupported
/// fingerprints. The service re-scans so the plan is built from current evidence.
#[tauri::command]
pub async fn chain_plan_repair(
    store: State<'_, Arc<SkillStore>>,
    fingerprints: Vec<String>,
) -> Result<chain::repair::RepairPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).plan_repair(&fingerprints)
    })
    .await?
}

/// Apply a previewed repair plan. Re-validates the write boundary and TOCTOU
/// evidence per item, never touches a physical entry, audits each item, then
/// rescans to verify the normalized chain before reporting success.
#[tauri::command]
pub async fn chain_apply_repair(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::repair::RepairPlan,
) -> Result<chain::repair::RepairOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).apply_repair(&plan)
    })
    .await?
}

/// Chain assembly presets (issue #35), name order.
#[tauri::command]
pub async fn chain_presets_list(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<chain::preset::ChainPreset>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::preset::list(&store)).await?
}

/// Save the given warehouse skill references as a new named preset.
#[tauri::command]
pub async fn chain_preset_save(
    store: State<'_, Arc<SkillStore>>,
    name: String,
    skills: Vec<chain::preset::ChainPresetSkill>,
) -> Result<chain::preset::ChainPreset, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::preset::save(&store, &name, &skills))
        .await?
}

#[tauri::command]
pub async fn chain_preset_rename(
    store: State<'_, Arc<SkillStore>>,
    id: i64,
    name: String,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::preset::rename(&store, id, &name)).await?
}

#[tauri::command]
pub async fn chain_preset_delete(
    store: State<'_, Arc<SkillStore>>,
    id: i64,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::preset::delete(&store, id)).await?
}

/// The uncommitted tracked changes of one scanned repository (issue #34):
/// per-file status and line stats, read-only, for the feedback card's diff
/// panel. The repository must be part of the current topology.
#[tauri::command]
pub async fn chain_repo_dirty_diff(
    store: State<'_, Arc<SkillStore>>,
    repo_path: String,
) -> Result<chain::repo_health::DirtyDiff, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).repo_dirty_diff(&repo_path)
    })
    .await?
}

/// Common-cause analysis (issue #33): whole-repository moves detected behind
/// broken-link storms — the root cause plus its blast radius, read-only.
#[tauri::command]
pub async fn chain_repo_moves(
    store: State<'_, Arc<SkillStore>>,
) -> Result<chain::repo_move::RepoMoveReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::ChainService::new(&store).repo_moves())
        .await?
}

/// The narrated live repair (issue #32): runs the deterministic pipeline in a
/// blocking task, streaming step events over the `chain-repair-live` channel
/// (payloads carry `run_id`, so concurrent runs don't cross). The invoke
/// resolves with the terminal outcome; `chain_repair_live_control` steers a
/// run in flight.
#[tauri::command]
pub async fn chain_repair_live(
    app: tauri::AppHandle,
    store: State<'_, Arc<SkillStore>>,
    fingerprints: Vec<String>,
    run_id: String,
    prefer_root: Option<String>,
) -> Result<chain::live::LiveOutcome, AppError> {
    use tauri::Emitter;
    let store = store.inner().clone();
    let control = chain::live::register(&run_id);
    let cleanup_id = run_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let service = chain::ChainService::new(&store);
        let mut emit = |event: chain::live::LiveEvent| {
            if let Err(err) = app.emit("chain-repair-live", &event) {
                log::error!("chain-repair-live emit failed: {err}");
            }
        };
        service.repair_live(
            &fingerprints,
            &run_id,
            prefer_root.as_deref(),
            &control,
            &mut emit,
        )
    })
    .await;
    // The run is over either way — its control must not outlive it.
    chain::live::unregister(&cleanup_id);
    result?
}

/// Steer a live repair run: "pause" | "resume" | "takeover". A run that
/// already finished is gone from the registry — reported as not found so the
/// UI can settle on the terminal state it just received.
#[tauri::command]
pub async fn chain_repair_live_control(run_id: String, action: String) -> Result<(), AppError> {
    let control = chain::live::control_of(&run_id)
        .ok_or_else(|| AppError::not_found("no live repair run with this id"))?;
    match action.as_str() {
        "pause" => control.pause(),
        "resume" => control.resume(),
        "takeover" => control.takeover(),
        _ => return Err(AppError::invalid_input("unknown live control action")),
    }
    Ok(())
}

/// The repair journal, newest first (issue #31): durable records of every
/// chain-repair apply that wrote, each carrying its own undo material.
#[tauri::command]
pub async fn chain_repair_journal(
    store: State<'_, Arc<SkillStore>>,
    limit: Option<i64>,
) -> Result<Vec<chain::journal::JournalRecord>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).repair_journal(limit)
    })
    .await?
}

/// One-click undo of a journaled repair (issue #31): replays the record's
/// inverses under per-item guards, audits each, then rescans and verifies the
/// original findings reappeared before reporting success.
#[tauri::command]
pub async fn chain_undo_repair(
    store: State<'_, Arc<SkillStore>>,
    id: i64,
) -> Result<chain::journal::UndoOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::ChainService::new(&store).undo_repair(id))
        .await?
}

/// Hide a repair record's workbench card without deleting the history.
#[tauri::command]
pub async fn chain_dismiss_repair_record(
    store: State<'_, Arc<SkillStore>>,
    id: i64,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).dismiss_repair_record(id)
    })
    .await?
}

/// Preview fast-forward-only pulls for the given Original Repositories without
/// fetching or mutating anything: returns each repository's eligible/skip
/// classification with a precise reason. Delegates to the Chain Service.
#[tauri::command]
pub async fn chain_plan_pull(
    store: State<'_, Arc<SkillStore>>,
    repo_paths: Vec<String>,
) -> Result<chain::pull::PullPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).plan_pull(&repo_paths)
    })
    .await?
}

/// Apply a previewed pull plan: fast-forward the eligible repositories only,
/// audit each attempt, and rescan to stamp a fresh timestamp. Patchbay never
/// resets, stashes, force-updates, merges, or auto-resolves conflicts.
#[tauri::command]
pub async fn chain_apply_pull(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::pull::PullPlan,
) -> Result<chain::pull::PullOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::ChainService::new(&store).apply_pull(&plan))
        .await?
}

/// Preview fast-forward-only fork synchronizations (`upstream` → `origin`) for
/// the given Original Repositories without fetching or pushing anything: returns
/// each repository's eligible/skip classification, naming the source, target,
/// branch, and lag. Delegates to the Chain Service.
#[tauri::command]
pub async fn chain_plan_fork_sync(
    store: State<'_, Arc<SkillStore>>,
    repo_paths: Vec<String>,
) -> Result<chain::fork_sync::ForkSyncPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).plan_fork_sync(&repo_paths)
    })
    .await?
}

/// Apply a previewed fork-sync plan: advance the eligible forks' `origin` branch
/// to `upstream` by fast-forward push only, audit each attempt, and rescan to
/// stamp a fresh timestamp. Patchbay never force-pushes, rebases, merges, or
/// rewrites history.
#[tauri::command]
pub async fn chain_apply_fork_sync(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::fork_sync::ForkSyncPlan,
) -> Result<chain::fork_sync::ForkSyncOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).apply_fork_sync(&plan)
    })
    .await?
}

/// Hide a Doctor finding by persisting a decision keyed on its rule and evidence
/// fingerprint. `kind` is "ignored" (generic accept) or "project_private"
/// (classify a legitimate physical Skill). Delegates to the Chain Service, which
/// validates the kind; it reads and writes only the settings table and never
/// touches Skill contents. Idempotent on (rule, fingerprint).
#[tauri::command]
pub async fn chain_ignore_finding(
    store: State<'_, Arc<SkillStore>>,
    rule: String,
    fingerprint: String,
    kind: String,
    note: Option<String>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).ignore_finding(&rule, &fingerprint, &kind, note)
    })
    .await?
}

/// Restore a previously hidden Doctor finding, removing its persisted decision so
/// the finding reappears on the next diagnose while its evidence still matches.
#[tauri::command]
pub async fn chain_restore_finding(
    store: State<'_, Arc<SkillStore>>,
    rule: String,
    fingerprint: String,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).restore_finding(&rule, &fingerprint)
    })
    .await?
}

/// Preview remediating a Global Guard violation into a selected registered
/// project without writing anything: returns the project link plan (or manual
/// guidance for a physical global entry), whether the global entry would be
/// removed, and the on-disk baseline apply re-checks. Delegates to the Chain
/// Service, which locates the violation and enforces the registration gate.
#[tauri::command]
pub async fn chain_plan_remediate(
    store: State<'_, Arc<SkillStore>>,
    global_path: String,
    project_path: String,
    agents: Vec<String>,
) -> Result<chain::remediate::RemediationPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project = PathBuf::from(&project_path);
        chain::ChainService::new(&store).plan_remediate(&global_path, &project, &agents)
    })
    .await?
}

/// Apply a previewed remediation. Establishes and verifies the project-local
/// chain before retiring the global symlink; a failed or conflicting link
/// leaves the global entry untouched; a physical global directory is never
/// deleted (manual guidance is returned instead). Audits each phase and rescans
/// the Global Guard before returning.
#[tauri::command]
pub async fn chain_apply_remediate(
    store: State<'_, Arc<SkillStore>>,
    plan: chain::remediate::RemediationPlan,
) -> Result<chain::remediate::RemediationOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::ChainService::new(&store).apply_remediate(&plan)
    })
    .await?
}

#[tauri::command]
pub async fn chain_get_warehouse_roots(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<chain::roots::RootConfigEntry>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || chain::roots::warehouse_roots_config(&store))
        .await?
}

#[tauri::command]
pub async fn chain_set_warehouse_roots(
    store: State<'_, Arc<SkillStore>>,
    roots: Vec<String>,
) -> Result<Vec<chain::roots::RootConfigEntry>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        chain::roots::set_warehouse_roots(&store, &roots)?;
        chain::roots::warehouse_roots_config(&store)
    })
    .await?
}
