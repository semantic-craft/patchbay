use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::core::{error::AppError, instructions, skill_store::SkillStore};

/// Read-only instructions scan for the GUI (design §6). A thin shell over the
/// same `InstructionsService` the CLI `instructions scan` uses — no second
/// scanner. With `project`, scans exactly that path (an explicit read); without
/// it, every registered project. The result is the stable scan schema the
/// project-page panel and overview cost bar render directly (no front-end math).
#[tauri::command]
pub async fn instructions_scan(
    store: State<'_, Arc<SkillStore>>,
    project: Option<String>,
) -> Result<instructions::scanner::ScanReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = project.map(PathBuf::from);
        instructions::InstructionsService::new(&store).scan(path.as_deref())
    })
    .await?
}

/// Read-only instructions diagnosis over the same service and scan used by the
/// CLI. An omitted filter returns every visible finding; `project` optionally
/// narrows diagnosis to one path. The command layer contains no rule logic.
#[tauri::command]
pub async fn instructions_doctor_report(
    store: State<'_, Arc<SkillStore>>,
    filter: Option<instructions::doctor::DoctorFilter>,
    project: Option<String>,
) -> Result<instructions::doctor::DoctorReport, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let filter = filter.unwrap_or_default();
        let path = project.map(PathBuf::from);
        instructions::InstructionsService::new(&store).doctor(&filter, path.as_deref())
    })
    .await?
}

/// Hide an instructions finding using the shared decision store. Instructions
/// decisions always have kind `ignored`; the service owns that invariant.
#[tauri::command]
pub async fn instructions_ignore_finding(
    store: State<'_, Arc<SkillStore>>,
    rule: String,
    fingerprint: String,
    note: Option<String>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store).ignore_finding(&rule, &fingerprint, note)
    })
    .await?
}

/// Restore a previously ignored instructions finding so it reappears on the
/// next diagnosis while its evidence still matches.
#[tauri::command]
pub async fn instructions_restore_finding(
    store: State<'_, Arc<SkillStore>>,
    rule: String,
    fingerprint: String,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store).restore_finding(&rule, &fingerprint)
    })
    .await?
}

/// Preview normalize edits for one registered project. The service owns all
/// diagnosis and planning; this command only moves the guarded plan over Tauri.
#[tauri::command]
pub async fn instructions_plan_normalize(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    fingerprints: Vec<String>,
) -> Result<instructions::normalize::NormalizePlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store)
            .plan_normalize(&PathBuf::from(project_path), &fingerprints)
    })
    .await?
}

/// Apply the exact normalize plan the user previewed. The service re-validates
/// its evidence, snapshots originals, writes, and rescans before returning.
#[tauri::command]
pub async fn instructions_apply_normalize(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    plan: instructions::normalize::NormalizePlan,
) -> Result<instructions::normalize::NormalizeOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store)
            .apply_normalize(&PathBuf::from(project_path), &plan)
    })
    .await?
}

/// Preview the create-only instructions scaffold for one project.
#[tauri::command]
pub async fn instructions_plan_init(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    docs_dir: bool,
) -> Result<instructions::init::InitPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store)
            .plan_init(&PathBuf::from(project_path), docs_dir)
    })
    .await?
}

/// Apply the exact create-only init plan the user previewed and return the
/// service's verification result unchanged.
#[tauri::command]
pub async fn instructions_apply_init(
    store: State<'_, Arc<SkillStore>>,
    project_path: String,
    plan: instructions::init::InitPlan,
) -> Result<instructions::init::InitOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        instructions::InstructionsService::new(&store)
            .apply_init(&PathBuf::from(project_path), &plan)
    })
    .await?
}
