//! Opt-in P2 fleet automatic rounds.

use crate::core::audit_log::AuditDraft;
use crate::core::central_repo;
use crate::core::error::AppError;
use crate::core::fleet::manifest;
use crate::core::fleet::service::{FleetLock, FleetService};
use crate::core::skill_store::SkillStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};

pub const AUTO_MODE_KEY: &str = "fleet_auto_mode";
pub const AUTO_STATE_KEY: &str = "fleet_auto_state";

const ROUND_INTERVAL_MS: i64 = 5 * 60 * 1_000;
const MAX_BACKOFF_SHIFT: u32 = 4;
const INITIAL_DELAY: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_secs(15);
pub const EVENT_COMPLETED: &str = "fleet-auto-round-completed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoRoundAttention {
    pub repo: String,
    pub reason: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoRoundResult {
    pub ok: bool,
    pub finished_at: i64,
    pub pulled: Vec<String>,
    pub pushed: Vec<String>,
    pub attention: Vec<AutoRoundAttention>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AutoRoundTick {
    Idle(&'static str),
    Completed(AutoRoundResult),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    next_round_at: Option<i64>,
    #[serde(default)]
    consecutive_failures: u32,
    last_round: Option<AutoRoundResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutoRoundStatus {
    pub enabled: bool,
    pub in_backoff: bool,
    pub next_round_at: Option<i64>,
    pub consecutive_failures: u32,
    pub last_round: Option<AutoRoundResult>,
}

fn is_enabled(store: &SkillStore) -> bool {
    matches!(
        store
            .get_setting(AUTO_MODE_KEY)
            .ok()
            .flatten()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("on" | "true" | "1" | "yes")
    )
}

pub fn auto_round_status_at(store: &SkillStore, now_ms: i64) -> Result<AutoRoundStatus, AppError> {
    let state = read_state(store);
    Ok(AutoRoundStatus {
        enabled: is_enabled(store),
        in_backoff: state.next_round_at.is_some_and(|next| now_ms < next)
            && state.consecutive_failures > 0,
        next_round_at: state.next_round_at,
        consecutive_failures: state.consecutive_failures,
        last_round: state.last_round,
    })
}

pub fn start<R: Runtime>(app: AppHandle<R>, store: Arc<SkillStore>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;
        loop {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let round_store = store.clone();
            let tick = tauri::async_runtime::spawn_blocking(move || {
                match run_due_round_at(&round_store, now_ms) {
                    Ok(tick) => Ok(tick),
                    Err(error) => record_failed_round(&round_store, now_ms, &error.message)
                        .map(AutoRoundTick::Completed),
                }
            })
            .await;
            match tick {
                Ok(Ok(AutoRoundTick::Completed(result))) => {
                    if let Err(error) = app.emit(EVENT_COMPLETED, &result) {
                        log::debug!("fleet auto round: emit failed: {error}");
                    }
                }
                Ok(Ok(AutoRoundTick::Idle(reason))) => {
                    log::debug!("fleet auto round: idle ({reason})");
                }
                Ok(Err(error)) => log::warn!(
                    "fleet auto round: failed to record round error: {}",
                    error.message
                ),
                Err(error) => log::warn!("fleet auto round: scheduler join failed: {error}"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// One controllable scheduler tick. Production passes wall-clock milliseconds;
/// tests pass literals, so no test waits on the real polling interval.
pub fn run_due_round_at(store: &SkillStore, now_ms: i64) -> Result<AutoRoundTick, AppError> {
    if !is_enabled(store) {
        return Ok(AutoRoundTick::Idle("disabled"));
    }
    let manifest_path = central_repo::base_dir().join("fleet/meta/manifest.toml");
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AutoRoundTick::Idle("manifest_unavailable"))
        }
        Err(error) => return Err(AppError::io(error)),
    };
    let cached_manifest = manifest::parse(&text)?;
    if !cached_manifest.repos.iter().any(|repo| repo.auto_sync) {
        return Ok(AutoRoundTick::Idle("no_opt_in"));
    }
    let previous = read_state(store);
    if previous.next_round_at.is_some_and(|next| now_ms < next) {
        return Ok(AutoRoundTick::Idle("backoff"));
    }
    let Some(_lock) = FleetLock::try_acquire()? else {
        return Ok(AutoRoundTick::Idle("busy"));
    };

    let service = FleetService::new(store);
    let status = service.status()?;
    let mut push = Vec::new();
    let mut pull = Vec::new();
    let mut attention = Vec::new();
    let opted_rows: Vec<_> = status.repos.iter().filter(|row| row.auto_sync).collect();
    if opted_rows.is_empty() {
        return Ok(AutoRoundTick::Idle("no_opt_in"));
    }
    if status.meta_state != "fresh" {
        for row in opted_rows {
            attention.push(attention_item(
                row.name.clone(),
                "metadata_stale",
                status
                    .meta_warning
                    .as_deref()
                    .unwrap_or("fleet metadata is stale"),
            ));
        }
    } else {
        for row in opted_rows {
            let Some(cell) = row.cells.get(&status.machine) else {
                attention.push(attention_item(
                    row.name.clone(),
                    "not_reported",
                    "local status is missing",
                ));
                continue;
            };
            if !cell.present {
                attention.push(attention_item(
                    row.name.clone(),
                    "repo_missing",
                    "local repo is missing",
                ));
                continue;
            }
            if cell.detached {
                attention.push(attention_item(
                    row.name.clone(),
                    "detached_head",
                    "local repo has a detached HEAD",
                ));
                continue;
            }
            if cell.branch.as_deref() != Some(row.branch.as_str()) {
                attention.push(attention_item(
                    row.name.clone(),
                    "branch_mismatch",
                    "local branch does not match the manifest branch",
                ));
                continue;
            }
            let Some(dirty) = cell.dirty else {
                attention.push(attention_item(
                    row.name.clone(),
                    "repo_unreadable",
                    "local dirty state is unavailable",
                ));
                continue;
            };
            if dirty > 0 {
                attention.push(attention_item(
                    row.name.clone(),
                    "repo_dirty",
                    "local repo has uncommitted changes",
                ));
                continue;
            }
            let can_push =
                row.authority == status.machine || row.authority == manifest::AUTHORITY_SHARED;
            let can_pull =
                row.authority != status.machine || row.authority == manifest::AUTHORITY_SHARED;
            let (Some(ahead), Some(behind)) = (cell.ahead, cell.behind) else {
                if row.hub_note.as_deref() == Some("hub_head_not_local") && can_pull {
                    // The status scan is intentionally ls-remote-only, so a newly
                    // advanced hub tip may not exist in the local object database
                    // yet. The existing pull plan/apply seam fetches and proves
                    // Behind/Ahead/Diverged under the lock before any checkout.
                    pull.push(row.name.clone());
                    continue;
                }
                attention.push(attention_item(
                    row.name.clone(),
                    row.hub_note.as_deref().unwrap_or("hub_unreachable"),
                    "hub relationship is unavailable",
                ));
                continue;
            };
            if ahead > 0 && behind > 0 {
                attention.push(attention_item(
                    row.name.clone(),
                    "diverged",
                    "local and hub history have diverged",
                ));
                continue;
            }
            if ahead > 0 {
                if can_push {
                    push.push(row.name.clone());
                } else {
                    attention.push(attention_item(
                        row.name.clone(),
                        "local_ahead",
                        "non-authority repo is ahead of the hub",
                    ));
                }
            } else if behind > 0 {
                if can_pull {
                    pull.push(row.name.clone());
                } else {
                    attention.push(attention_item(
                        row.name.clone(),
                        "authority_behind",
                        "authority repo is behind the hub",
                    ));
                }
            }
        }
    }

    let mut pushed = Vec::new();
    if !push.is_empty() {
        let plan = service.plan_push(&push)?;
        let outcome = service.apply_push_locked(&plan)?;
        for item in outcome.items {
            if matches!(item.action.as_str(), "pushed" | "up_to_date") {
                pushed.push(item.repo);
            } else {
                attention.push(attention_item(
                    item.repo,
                    item.reason_code.as_deref().unwrap_or("push_failed"),
                    item.message.as_deref().unwrap_or("automatic push failed"),
                ));
            }
        }
    }

    let mut pulled = Vec::new();
    if !pull.is_empty() {
        let plan = service.plan_pull(&pull)?;
        let outcome = service.apply_pull_locked(&plan)?;
        for item in outcome.items {
            if item.action == "pulled" {
                pulled.push(item.repo);
            } else {
                attention.push(attention_item(
                    item.repo,
                    item.reason_code.as_deref().unwrap_or("pull_failed"),
                    item.message.as_deref().unwrap_or("automatic pull failed"),
                ));
            }
        }
    }

    let ok = attention.is_empty();
    let result = AutoRoundResult {
        ok,
        finished_at: now_ms,
        pulled,
        pushed,
        attention,
    };
    let draft = AuditDraft::new("fleet_auto_round").detail(format!(
        "pulled={} pushed={} attention={}",
        result.pulled.len(),
        result.pushed.len(),
        result.attention.len()
    ));
    store.log_audit(if ok { draft.ok() } else { draft });

    let failures = if ok {
        0
    } else {
        previous.consecutive_failures.saturating_add(1)
    };
    let delay = ROUND_INTERVAL_MS << failures.min(MAX_BACKOFF_SHIFT);
    persist_state(
        store,
        &PersistedState {
            next_round_at: Some(now_ms.saturating_add(delay)),
            consecutive_failures: failures,
            last_round: Some(result.clone()),
        },
    )?;
    Ok(AutoRoundTick::Completed(result))
}

fn attention_item(
    repo: impl Into<String>,
    reason: impl Into<String>,
    message: impl Into<String>,
) -> AutoRoundAttention {
    AutoRoundAttention {
        repo: repo.into(),
        reason: reason.into(),
        message: message.into(),
    }
}

fn read_state(store: &SkillStore) -> PersistedState {
    store
        .get_setting(AUTO_STATE_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

fn persist_state(store: &SkillStore, state: &PersistedState) -> Result<(), AppError> {
    let json = serde_json::to_string(state)
        .map_err(|error| AppError::internal(format!("serialize fleet auto state: {error}")))?;
    store
        .set_setting(AUTO_STATE_KEY, &json)
        .map_err(AppError::db)
}

fn record_failed_round(
    store: &SkillStore,
    now_ms: i64,
    message: &str,
) -> Result<AutoRoundResult, AppError> {
    let previous = read_state(store);
    let failures = previous.consecutive_failures.saturating_add(1);
    let result = AutoRoundResult {
        ok: false,
        finished_at: now_ms,
        pulled: Vec::new(),
        pushed: Vec::new(),
        attention: vec![attention_item("", "round_failed", message)],
    };
    store.log_audit(AuditDraft::new("fleet_auto_round").detail(format!("round_failed: {message}")));
    persist_state(
        store,
        &PersistedState {
            next_round_at: Some(
                now_ms.saturating_add(ROUND_INTERVAL_MS << failures.min(MAX_BACKOFF_SHIFT)),
            ),
            consecutive_failures: failures,
            last_round: Some(result.clone()),
        },
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::fleet::meta_repo::MetaRepo;
    use crate::core::fleet::service::{MACHINE_ID_KEY, META_URL_KEY, PROJECTS_ROOT_KEY};
    use crate::core::{central_repo, skill_store::SkillStore};
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    struct Fixture {
        _guard: std::sync::MutexGuard<'static, ()>,
        _temp: tempfile::TempDir,
        store: SkillStore,
        db: std::path::PathBuf,
        projects: std::path::PathBuf,
        hub: std::path::PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            central_repo::set_test_base_dir_override(None);
        }
    }

    fn fixture(authority: &str) -> Fixture {
        let guard = central_repo::test_base_dir_lock();
        let temp = tempdir().unwrap();
        let base = temp.path().join("appdata");
        central_repo::set_test_base_dir_override(Some(base.clone()));

        let projects = temp.path().join("projects");
        let alpha = projects.join("alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        git(&alpha, &["init", "-b", "main"]);
        std::fs::write(alpha.join("file.txt"), "base").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "base"]);

        let mirrors = temp.path().join("mirrors");
        std::fs::create_dir_all(&mirrors).unwrap();
        let hub = mirrors.join("alpha.git");
        let output = Command::new("git")
            .args(["clone", "--bare"])
            .arg(&alpha)
            .arg(&hub)
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta_bare = temp.path().join("_patchbay-fleet.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&meta_bare)
            .output()
            .unwrap()
            .status
            .success());
        let meta_seed = temp.path().join("meta-seed");
        std::fs::create_dir_all(&meta_seed).unwrap();
        git(&meta_seed, &["init", "-b", "main"]);
        git(
            &meta_seed,
            &["remote", "add", "origin", meta_bare.to_str().unwrap()],
        );
        std::fs::write(
            meta_seed.join("manifest.toml"),
            format!(
                r#"
[hub.test]
url = '{}'

[[repo]]
name = "alpha"
hub = "test"
authority = "{authority}"
branch = "main"
auto_sync = true
"#,
                mirrors.display()
            ),
        )
        .unwrap();
        git(&meta_seed, &["add", "manifest.toml"]);
        git(&meta_seed, &["commit", "-m", "seed"]);
        git(&meta_seed, &["push", "origin", "main"]);

        let db = temp.path().join("patchbay.db");
        let store = SkillStore::new(&db).unwrap();
        store.set_setting(AUTO_MODE_KEY, "on").unwrap();
        store.set_setting(MACHINE_ID_KEY, "selfie").unwrap();
        store
            .set_setting(META_URL_KEY, meta_bare.to_str().unwrap())
            .unwrap();
        store
            .set_setting(PROJECTS_ROOT_KEY, projects.to_str().unwrap())
            .unwrap();
        MetaRepo::at(meta_bare.to_str().unwrap(), base.join("fleet/meta"))
            .ensure_fresh()
            .unwrap();

        Fixture {
            _guard: guard,
            _temp: temp,
            store,
            db,
            projects,
            hub,
        }
    }

    fn advance_hub(fixture: &Fixture, content: &str) -> String {
        let publisher = fixture.projects.parent().unwrap().join("publisher");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&fixture.hub)
            .arg(&publisher)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(publisher.join("file.txt"), content).unwrap();
        git(&publisher, &["add", "-A"]);
        git(&publisher, &["commit", "-m", "hub update"]);
        git(&publisher, &["push", "origin", "main"]);
        git_stdout(&fixture.hub, &["rev-parse", "refs/heads/main"])
    }

    #[test]
    fn global_default_off_has_zero_network_git_lock_state_or_audit() {
        let _guard = central_repo::test_base_dir_lock();
        let temp = tempdir().unwrap();
        let base = temp.path().join("appdata");
        central_repo::set_test_base_dir_override(Some(base.clone()));
        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();

        let tick = run_due_round_at(&store, 1_000_000).unwrap();

        assert_eq!(tick, AutoRoundTick::Idle("disabled"));
        assert!(store.list_audit(None).unwrap().is_empty());
        assert_eq!(store.get_setting(AUTO_STATE_KEY).unwrap(), None);
        assert!(!base.join("fleet.lock").exists());
        assert!(!base.join("fleet/meta").exists());
        central_repo::set_test_base_dir_override(None);
    }

    #[test]
    fn global_on_without_repo_opt_in_still_has_zero_actions() {
        let _guard = central_repo::test_base_dir_lock();
        let temp = tempdir().unwrap();
        let base = temp.path().join("appdata");
        central_repo::set_test_base_dir_override(Some(base.clone()));
        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store.set_setting(AUTO_MODE_KEY, "on").unwrap();
        let cache = base.join("fleet/meta");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("manifest.toml"),
            r#"
[hub.test]
url = "invalid.example:mirrors"

[[repo]]
name = "alpha"
hub = "test"
authority = "shared"
branch = "main"
"#,
        )
        .unwrap();

        let tick = run_due_round_at(&store, 1_000_000).unwrap();

        assert_eq!(tick, AutoRoundTick::Idle("no_opt_in"));
        assert!(store.list_audit(None).unwrap().is_empty());
        assert_eq!(store.get_setting(AUTO_STATE_KEY).unwrap(), None);
        assert!(!base.join("fleet.lock").exists());
        central_repo::set_test_base_dir_override(None);
    }

    #[test]
    fn opted_in_non_authority_repo_behind_fast_forwards_through_existing_apply() {
        let fixture = fixture("other");
        let target = advance_hub(&fixture, "remote update");

        let tick = run_due_round_at(&fixture.store, 1_000_000).unwrap();

        let AutoRoundTick::Completed(result) = tick else {
            panic!("expected completed automatic round");
        };
        assert_eq!(result.pulled, vec!["alpha"], "result: {result:?}");
        assert!(result.pushed.is_empty());
        assert!(result.attention.is_empty());
        assert_eq!(
            git_stdout(&fixture.projects.join("alpha"), &["rev-parse", "HEAD"]),
            target
        );
        let actions: Vec<_> = fixture
            .store
            .list_audit(None)
            .unwrap()
            .into_iter()
            .map(|entry| entry.action)
            .collect();
        assert!(actions.contains(&"fleet_pull".to_string()));
        assert!(actions.contains(&"fleet_auto_round".to_string()));
    }

    #[test]
    fn opted_in_authority_repo_ahead_pushes_through_existing_apply() {
        let fixture = fixture("selfie");
        let alpha = fixture.projects.join("alpha");
        std::fs::write(alpha.join("file.txt"), "local authority update").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "authority update"]);
        let target = git_stdout(&alpha, &["rev-parse", "HEAD"]);

        let tick = run_due_round_at(&fixture.store, 2_000_000).unwrap();

        let AutoRoundTick::Completed(result) = tick else {
            panic!("expected completed automatic round");
        };
        assert_eq!(result.pushed, vec!["alpha"], "result: {result:?}");
        assert!(result.pulled.is_empty());
        assert!(result.attention.is_empty());
        assert_eq!(
            git_stdout(&fixture.hub, &["rev-parse", "refs/heads/main"]),
            target
        );
        let actions: Vec<_> = fixture
            .store
            .list_audit(None)
            .unwrap()
            .into_iter()
            .map(|entry| entry.action)
            .collect();
        assert!(actions.contains(&"fleet_push".to_string()));
        assert!(actions.contains(&"fleet_auto_round".to_string()));
    }

    #[test]
    fn dirty_repo_reports_once_and_suppresses_ticks_during_persisted_backoff() {
        let fixture = fixture("other");
        let alpha = fixture.projects.join("alpha");
        let head_before = git_stdout(&alpha, &["rev-parse", "HEAD"]);
        let hub_before = git_stdout(&fixture.hub, &["rev-parse", "refs/heads/main"]);
        std::fs::write(alpha.join("dirty.txt"), "uncommitted").unwrap();

        let first = run_due_round_at(&fixture.store, 3_000_000).unwrap();

        let AutoRoundTick::Completed(result) = first else {
            panic!("expected completed automatic round");
        };
        assert!(!result.ok);
        assert_eq!(result.attention.len(), 1);
        assert_eq!(result.attention[0].reason, "repo_dirty");
        assert!(result.pulled.is_empty() && result.pushed.is_empty());
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(
            git_stdout(&fixture.hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );
        assert!(fixture.store.get_setting(AUTO_STATE_KEY).unwrap().is_some());
        let audit_count = fixture.store.list_audit(None).unwrap().len();
        assert_eq!(audit_count, 1);

        let retry = run_due_round_at(&fixture.store, 3_000_000 + ROUND_INTERVAL_MS).unwrap();
        assert_eq!(retry, AutoRoundTick::Idle("backoff"));
        assert_eq!(fixture.store.list_audit(None).unwrap().len(), audit_count);
    }

    #[test]
    fn diverged_repo_is_reported_without_moving_local_or_hub_head() {
        let fixture = fixture("shared");
        let alpha = fixture.projects.join("alpha");
        std::fs::write(alpha.join("local.txt"), "local").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "local branch"]);
        let local_before = git_stdout(&alpha, &["rev-parse", "HEAD"]);
        let hub_before = advance_hub(&fixture, "remote branch");

        let tick = run_due_round_at(&fixture.store, 4_000_000).unwrap();

        let AutoRoundTick::Completed(result) = tick else {
            panic!("expected completed automatic round");
        };
        assert!(!result.ok);
        assert_eq!(result.attention.len(), 1, "result: {result:?}");
        assert_eq!(result.attention[0].reason, "diverged");
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), local_before);
        assert_eq!(
            git_stdout(&fixture.hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );
        let actions: Vec<_> = fixture
            .store
            .list_audit(None)
            .unwrap()
            .into_iter()
            .map(|entry| entry.action)
            .collect();
        assert_eq!(actions, vec!["fleet_auto_round", "fleet_pull"]);
    }

    #[test]
    fn background_round_yields_immediately_while_manual_lock_is_held() {
        let fixture = fixture("other");
        let _manual = FleetLock::try_acquire().unwrap().unwrap();
        let started = std::time::Instant::now();

        let tick = run_due_round_at(&fixture.store, 5_000_000).unwrap();

        assert_eq!(tick, AutoRoundTick::Idle("busy"));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(fixture.store.list_audit(None).unwrap().is_empty());
        assert_eq!(fixture.store.get_setting(AUTO_STATE_KEY).unwrap(), None);
    }

    #[test]
    fn restart_restores_global_setting_last_result_and_backoff_deadline() {
        let fixture = fixture("other");
        std::fs::write(fixture.projects.join("alpha/dirty.txt"), "dirty").unwrap();
        let now = 6_000_000;
        assert!(matches!(
            run_due_round_at(&fixture.store, now).unwrap(),
            AutoRoundTick::Completed(_)
        ));

        let restarted = SkillStore::new(&fixture.db).unwrap();
        let status = auto_round_status_at(&restarted, now + ROUND_INTERVAL_MS).unwrap();

        assert!(status.enabled);
        assert!(status.in_backoff);
        assert_eq!(status.consecutive_failures, 1);
        assert!(status
            .next_round_at
            .is_some_and(|next| next > now + ROUND_INTERVAL_MS));
        assert_eq!(status.last_round.unwrap().attention[0].reason, "repo_dirty");
    }
}
