use std::sync::Arc;

use tauri::State;

use crate::core::{error::AppError, fleet, skill_store::SkillStore};

/// Read-only status matrix for the `/fleet` view (design §6). The same
/// `FleetService` payload the CLI `fleet status` prints — no second scanner.
#[tauri::command]
pub async fn fleet_status(
    store: State<'_, Arc<SkillStore>>,
) -> Result<fleet::service::FleetStatus, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).status()).await?
}

/// Unmanaged git directories under the projects root (design §2 discover).
#[tauri::command]
pub async fn fleet_discover(
    store: State<'_, Arc<SkillStore>>,
) -> Result<fleet::service::FleetDiscovery, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).discover())
        .await?
}

/// Persisted global scheduler/backoff state; reading it never touches Git or
/// the network.
#[tauri::command]
pub async fn fleet_auto_status(
    store: State<'_, Arc<SkillStore>>,
) -> Result<fleet::auto_round::AutoRoundStatus, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        fleet::auto_round::auto_round_status_at(&store, chrono::Utc::now().timestamp_millis())
    })
    .await?
}

/// Toggle the one P2-owned manifest field for an existing repository.
#[tauri::command]
pub async fn fleet_set_repo_auto_sync(
    store: State<'_, Arc<SkillStore>>,
    repo: String,
    enabled: bool,
) -> Result<fleet::meta_repo::AutoSyncUpdateOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        fleet::FleetService::new(&store).set_repo_auto_sync(&repo, enabled)
    })
    .await?
}

/// Fresh editable manifest snapshot for the `/fleet` management form.
#[tauri::command]
pub async fn fleet_manifest_get(
    store: State<'_, Arc<SkillStore>>,
) -> Result<fleet::service::FleetManifestSnapshot, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).manifest_get())
        .await?
}

/// Preview or apply a manifest edit through one exact-plan endpoint. The
/// apply request must return the previewed plan unchanged after confirmation.
#[tauri::command]
pub async fn fleet_manifest_update(
    store: State<'_, Arc<SkillStore>>,
    request: fleet::service::FleetManifestUpdateRequest,
) -> Result<fleet::service::FleetManifestUpdateResponse, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let service = fleet::FleetService::new(&store);
        match request {
            fleet::service::FleetManifestUpdateRequest::Preview { base, repos } => service
                .plan_manifest_update(&base, repos)
                .map(|plan| fleet::service::FleetManifestUpdateResponse::Preview { plan }),
            fleet::service::FleetManifestUpdateRequest::Apply { plan } => service
                .apply_manifest_update(&plan)
                .map(|outcome| fleet::service::FleetManifestUpdateResponse::Apply { outcome }),
        }
    })
    .await?
}

/// Read-only push preview. The exact evidence-bearing plan returned here must
/// be sent back to `fleet_apply_push` after user confirmation.
#[tauri::command]
pub async fn fleet_plan_push(
    store: State<'_, Arc<SkillStore>>,
    repos: Vec<String>,
) -> Result<fleet::service::FleetPushPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).plan_push(&repos))
        .await?
}

/// Apply an approved push plan. Service code re-resolves the manifest and
/// re-checks every plan evidence field under `fleet.lock` before writing.
#[tauri::command]
pub async fn fleet_apply_push(
    store: State<'_, Arc<SkillStore>>,
    plan: fleet::service::FleetPushPlan,
) -> Result<fleet::service::FleetPushOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).apply_push(&plan))
        .await?
}

/// Read-only pull preview. The exact evidence-bearing plan returned here must
/// be sent back unchanged after confirmation.
#[tauri::command]
pub async fn fleet_plan_pull(
    store: State<'_, Arc<SkillStore>>,
    repos: Vec<String>,
) -> Result<fleet::service::FleetPullPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).plan_pull(&repos))
        .await?
}

/// Apply an approved pull plan under fleet.lock with fresh-manifest and full
/// evidence revalidation before SAFE fast-forward checkout.
#[tauri::command]
pub async fn fleet_apply_pull(
    store: State<'_, Arc<SkillStore>>,
    plan: fleet::service::FleetPullPlan,
) -> Result<fleet::service::FleetPullOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).apply_pull(&plan))
        .await?
}

/// Read-only missing-repository bootstrap preview.
#[tauri::command]
pub async fn fleet_plan_bootstrap(
    store: State<'_, Arc<SkillStore>>,
    repos: Vec<String>,
) -> Result<fleet::service::FleetBootstrapPlan, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        fleet::FleetService::new(&store).plan_bootstrap(&repos)
    })
    .await?
}

/// Apply an approved exact bootstrap plan under fleet.lock.
#[tauri::command]
pub async fn fleet_apply_bootstrap(
    store: State<'_, Arc<SkillStore>>,
    plan: fleet::service::FleetBootstrapPlan,
) -> Result<fleet::service::FleetBootstrapOutcome, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        fleet::FleetService::new(&store).apply_bootstrap(&plan)
    })
    .await?
}

/// Push this machine's status report to the meta repo — fleet P0's only write
/// surface, and it only ever touches the meta repo cache and the hub.
#[tauri::command]
pub async fn fleet_report(
    store: State<'_, Arc<SkillStore>>,
) -> Result<fleet::service::FleetReportResult, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || fleet::FleetService::new(&store).apply_report())
        .await?
}
