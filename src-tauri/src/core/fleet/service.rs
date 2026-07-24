//! FleetService: the single entry point CLI and GUI share (payloads are these
//! service structs, both shells are pass-throughs).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::audit_log::AuditDraft;
use crate::core::error::AppError;
use crate::core::fleet::manifest::{self, Manifest};
use crate::core::fleet::meta_repo::{
    AutoSyncUpdateOutcome, MachineReport, MetaRepo, MetaSyncState, ReportOutcome, ReportedRepo,
};
use crate::core::fleet::repo_ops;
use crate::core::skill_store::SkillStore;
use crate::core::{central_repo, git_backup, path_guard};

pub const MACHINE_ID_KEY: &str = "fleet_machine_id";
pub const META_URL_KEY: &str = "fleet_meta_url";
pub const PROJECTS_ROOT_KEY: &str = "fleet_projects_root";

/// This machine's fleet settings, resolved the way the read paths resolve them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FleetConfig {
    pub machine_id: String,
    /// `None` when unset — the only fleet setting with no working default.
    pub meta_url: Option<String>,
    pub projects_root: String,
}

/// Stable machine slug: lowercase alphanumerics and dashes only. Distinct from
/// `backup_device_name`, which is a human display name (design §1 terms).
pub fn sanitize_machine_id(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "machine".into()
    } else {
        out
    }
}

/// The full status matrix (design §6): rows = manifest repos, columns =
/// machines. The self column is measured live; other columns replay each
/// machine's last pushed report with its `reported_at` shown honestly.
#[derive(Debug, Clone, Serialize)]
pub struct FleetStatus {
    pub machine: String,
    pub meta_url: String,
    /// "fresh" | "stale"
    pub meta_state: String,
    pub meta_warning: Option<String>,
    pub projects_root: String,
    pub scanned_at: i64,
    pub machines: Vec<MachineColumn>,
    pub repos: Vec<FleetRepoRow>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineColumn {
    pub id: String,
    pub display_name: Option<String>,
    pub is_self: bool,
    /// RFC3339; `None` for the live-measured self column.
    pub reported_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetRepoRow {
    pub name: String,
    pub hub: String,
    pub authority: String,
    pub branch: String,
    pub auto_sync: bool,
    /// Hub branch tip (short) as seen from this machine via ls-remote.
    pub hub_head: Option<String>,
    pub hub_note: Option<String>,
    /// Cells keyed by machine id; a machine that never reported the repo has
    /// no entry (rendered as unknown, not as absent-from-disk).
    pub cells: BTreeMap<String, ReportedRepo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetDiscovery {
    pub machine: String,
    pub projects_root: String,
    pub scanned_at: i64,
    pub unlisted: Vec<repo_ops::DiscoveredRepo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetReportResult {
    pub ok: bool,
    pub machine: String,
    pub meta_url: String,
    pub action: String,
    pub pushed: bool,
    pub commit: Option<String>,
    pub report: MachineReport,
}

/// Fresh editable manifest snapshot. The GUI sends these identity fields back
/// for preview so a stale editor cannot silently replace newer fleet state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetManifestSnapshot {
    pub machine: String,
    pub meta_head: String,
    pub manifest_digest: String,
    pub manifest: Manifest,
    pub known_machines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetManifestChange {
    /// `add`, `update`, or `remove`.
    pub action: String,
    pub repo: String,
    pub before: Option<manifest::RepoEntry>,
    pub after: Option<manifest::RepoEntry>,
}

/// Exact validated plan rendered by the GUI and returned unchanged on confirm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetManifestUpdatePlan {
    pub machine: String,
    pub meta_head: String,
    pub manifest_digest: String,
    pub planned_at: i64,
    pub manifest: Manifest,
    pub changes: Vec<FleetManifestChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetManifestUpdateOutcome {
    pub ok: bool,
    /// `updated`, `unchanged`, or `conflict`.
    pub action: String,
    pub pushed: bool,
    pub commit: Option<String>,
    pub manifest_digest: String,
    pub changes: Vec<FleetManifestChange>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum FleetManifestUpdateRequest {
    Preview {
        base: FleetManifestSnapshot,
        repos: Vec<manifest::RepoEntry>,
    },
    Apply {
        plan: FleetManifestUpdatePlan,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum FleetManifestUpdateResponse {
    Preview { plan: FleetManifestUpdatePlan },
    Apply { outcome: FleetManifestUpdateOutcome },
}

/// Immutable project-repo evidence captured by push preview and re-checked by
/// apply. Full OIDs are used here even though the status matrix shows shorts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetPushEvidence {
    pub head_oid: String,
    pub dirty_count: u32,
    pub branch: String,
    pub remote_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPushPlanItem {
    pub repo: String,
    /// `ready` or `refused`.
    pub status: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub evidence: Option<FleetPushEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPushPlan {
    pub ok: bool,
    pub machine: String,
    pub planned_at: i64,
    pub items: Vec<FleetPushPlanItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPushResult {
    pub repo: String,
    /// `pushed`, `up_to_date`, `refused`, `conflict`, or `error`.
    pub action: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub before_head: Option<String>,
    pub after_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPushOutcome {
    pub ok: bool,
    pub machine: String,
    pub items: Vec<FleetPushResult>,
}

/// Immutable pull evidence. `hub_url` is the manifest-resolved transport;
/// `remote_url` is the checkout's configured hub remote and is evidence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetPullEvidence {
    pub head_oid: String,
    pub target_oid: String,
    pub dirty_count: u32,
    pub branch: String,
    pub remote_url: Option<String>,
    pub hub_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPullPlanItem {
    pub repo: String,
    /// `ready` or `refused`.
    pub status: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub evidence: Option<FleetPullEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPullPlan {
    pub ok: bool,
    pub machine: String,
    pub manifest_digest: String,
    pub planned_at: i64,
    pub items: Vec<FleetPullPlanItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPullResult {
    pub repo: String,
    /// `pulled`, `refused`, `conflict`, or `error`.
    pub action: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub before_head: Option<String>,
    pub after_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPullOutcome {
    pub ok: bool,
    pub machine: String,
    pub items: Vec<FleetPullResult>,
}

/// Immutable evidence for one `fleet init` item. Mirror existence is `None`
/// away from the hub host because preview never probes a remote machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetInitEvidence {
    pub host_machine: String,
    pub repo_path: String,
    pub mirror_path: Option<String>,
    pub mirror_exists: Option<bool>,
    pub local_repo_exists: bool,
    pub remote_exists: bool,
    pub current_remote_url: Option<String>,
    pub target_remote_url: String,
    pub origin_url: Option<String>,
    pub head_oid: Option<String>,
    pub dirty_count: Option<u32>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetInitPlanItem {
    pub repo: String,
    /// `ready`, `no_op`, or `refused`.
    pub status: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    /// `create`, `no_op`, or `guide`.
    pub mirror_action: String,
    /// `add`, `set_url`, `no_op`, or `unavailable`.
    pub remote_action: String,
    pub evidence: Option<FleetInitEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetInitPlan {
    pub ok: bool,
    pub machine: String,
    pub manifest_digest: String,
    pub meta_repo: FleetMetaInitPlan,
    pub planned_at: i64,
    pub items: Vec<FleetInitPlanItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetMetaInitPlan {
    pub url: String,
    pub host_machine: Option<String>,
    pub exists: Option<bool>,
    /// `create`, `no_op`, `guide`, or `refused`.
    pub action: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetInitResult {
    pub repo: String,
    /// `applied`, `no_op`, `refused`, `conflict`, `partial`, or `error`.
    pub action: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub mirror_action: String,
    pub remote_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetInitOutcome {
    pub ok: bool,
    pub machine: String,
    pub meta_repo_action: String,
    pub items: Vec<FleetInitResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetBootstrapEvidence {
    pub target_path: String,
    pub hub_name: String,
    pub hub_url: String,
    pub branch: String,
    pub target_oid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetBootstrapPlanItem {
    pub repo: String,
    pub status: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub evidence: Option<FleetBootstrapEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetBootstrapPlan {
    pub ok: bool,
    pub machine: String,
    pub manifest_digest: String,
    pub planned_at: i64,
    pub items: Vec<FleetBootstrapPlanItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetBootstrapResult {
    pub repo: String,
    pub action: String,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub after_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetBootstrapOutcome {
    pub ok: bool,
    pub machine: String,
    pub items: Vec<FleetBootstrapResult>,
}

pub struct FleetService<'a> {
    store: &'a SkillStore,
}

impl<'a> FleetService<'a> {
    pub fn new(store: &'a SkillStore) -> Self {
        Self { store }
    }

    /// This machine's stable id; lazily seeded from the sanitized hostname the
    /// first time it is read (same lazy-seed pattern as `backup_device_name`).
    pub fn machine_id(&self) -> Result<String, AppError> {
        if let Some(existing) = self
            .store
            .get_setting(MACHINE_ID_KEY)
            .map_err(AppError::db)?
        {
            let trimmed = existing.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
        let seeded = sanitize_machine_id(&gethostname::gethostname().to_string_lossy());
        self.store
            .set_setting(MACHINE_ID_KEY, &seeded)
            .map_err(AppError::db)?;
        Ok(seeded)
    }

    /// Read-only form used by preview paths: derive the same default slug but
    /// do not lazily persist it until an apply/status operation is allowed to
    /// write settings.
    fn preview_machine_id(&self) -> Result<String, AppError> {
        if let Some(existing) = self
            .store
            .get_setting(MACHINE_ID_KEY)
            .map_err(AppError::db)?
        {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        Ok(sanitize_machine_id(
            &gethostname::gethostname().to_string_lossy(),
        ))
    }

    fn meta_url(&self) -> Result<String, AppError> {
        match self.store.get_setting(META_URL_KEY).map_err(AppError::db)? {
            Some(url) if !url.trim().is_empty() => Ok(url.trim().to_string()),
            _ => Err(AppError::invalid_input(
                "fleet_meta_url is not configured; set it to the fleet meta repo URL \
                 (e.g. alpha:git-mirrors/projects/_patchbay-fleet.git, or a local \
                 path on the hub host)",
            )),
        }
    }

    /// This machine's fleet settings, as `status` and friends resolve them.
    /// `meta_url` is `None` when unset — the one value with no working default,
    /// and so the only thing standing between a fresh machine and the fleet.
    pub fn config(&self) -> Result<FleetConfig, AppError> {
        let meta_url = self
            .store
            .get_setting(META_URL_KEY)
            .map_err(AppError::db)?
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        let projects_root = self
            .store
            .get_setting(PROJECTS_ROOT_KEY)
            .map_err(AppError::db)?
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .map(|p| expand_tilde(&p))
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Projects"));
        Ok(FleetConfig {
            machine_id: self.preview_machine_id()?,
            meta_url,
            projects_root: projects_root.to_string_lossy().into_owned(),
        })
    }

    /// Set the fleet meta repo URL. The only fleet setting with no working
    /// default, so without this a machine cannot join the fleet from the CLI.
    /// Validated through the same transport allowlist the manifest uses (#54).
    pub fn set_meta_url(&self, url: &str) -> Result<FleetConfig, AppError> {
        let trimmed = url.trim();
        manifest::check_remote_url(trimmed)?;
        self.store
            .set_setting(META_URL_KEY, trimmed)
            .map_err(AppError::db)?;
        self.config()
    }

    fn meta_repo(&self) -> Result<MetaRepo, AppError> {
        let url = self.meta_url()?;
        let cache = central_repo::base_dir().join("fleet").join("meta");
        Ok(MetaRepo::at(url, cache))
    }

    /// Projects root priority: local setting > manifest default > `~/Projects`.
    fn projects_root(&self, manifest: &Manifest) -> Result<PathBuf, AppError> {
        if let Some(local) = self
            .store
            .get_setting(PROJECTS_ROOT_KEY)
            .map_err(AppError::db)?
        {
            let trimmed = local.trim();
            if !trimmed.is_empty() {
                return Ok(expand_tilde(trimmed));
            }
        }
        if let Some(from_manifest) = manifest
            .fleet
            .projects_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(expand_tilde(from_manifest));
        }
        Ok(dirs::home_dir().unwrap_or_default().join("Projects"))
    }

    fn display_name(&self) -> Option<String> {
        match self.store.get_setting("backup_device_name") {
            Ok(Some(name)) if !name.trim().is_empty() => Some(name.trim().to_string()),
            Ok(_) => Some(git_backup::default_device_name()),
            Err(_) => None,
        }
    }

    /// Measure every manifest repo locally, including ahead/behind vs the hub
    /// (ls-remote only — the status path never writes to project repos). One
    /// ls-remote per repo; the hub comparison rides along for row-level use.
    fn measure_local(
        &self,
        manifest: &Manifest,
        machine: &str,
        root: &std::path::Path,
    ) -> Vec<(ReportedRepo, repo_ops::HubComparison)> {
        manifest
            .repos
            .iter()
            .map(|entry| {
                let path = root.join(&entry.name);
                let local = repo_ops::local_state(&path);
                let mut hub = manifest
                    .hubs
                    .get(&entry.hub)
                    .map(|hub| {
                        let base = manifest::resolve_hub_base(hub, machine);
                        repo_ops::compare_with_hub(
                            &path,
                            &manifest::repo_url(&base, &entry.name),
                            &entry.branch,
                        )
                    })
                    .unwrap_or_default();
                // The comparison counts the checkout's HEAD against the hub's
                // *manifest* branch. When the checkout sits on some other
                // branch those are two unrelated tips, and the difference
                // between them says nothing about sync state — it reads as
                // "you are N ahead, push" when the managed branch may be
                // perfectly in sync. Refuse to produce that number.
                if local.present && local.branch.as_deref() != Some(entry.branch.as_str()) {
                    hub.ahead = None;
                    hub.behind = None;
                    // Keep notes that explain the hub side; this one explains
                    // the local side and is more specific than "not fetched".
                    if hub.note.is_none() || hub.note.as_deref() == Some("hub_head_not_local") {
                        hub.note = Some("branch_off_manifest".into());
                    }
                }
                let cell = ReportedRepo {
                    name: entry.name.clone(),
                    present: local.present,
                    branch: local.branch,
                    head: local.head,
                    dirty: local.dirty,
                    detached: local.detached,
                    ahead: hub.ahead,
                    behind: hub.behind,
                    note: hub.note.clone(),
                };
                (cell, hub)
            })
            .collect()
    }

    pub fn status(&self) -> Result<FleetStatus, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        let sync_state = meta.ensure_fresh()?;
        let manifest = meta.read_manifest()?;
        let root = self.projects_root(&manifest)?;
        let (reports, mut warnings) = meta.read_reports();

        let (meta_state, meta_warning) = match sync_state {
            MetaSyncState::Fresh => ("fresh".to_string(), None),
            MetaSyncState::Stale { error } => ("stale".to_string(), Some(error)),
        };

        let mut machines = vec![MachineColumn {
            id: machine.clone(),
            display_name: self.display_name(),
            is_self: true,
            reported_at: None,
        }];
        for report in &reports {
            if report.machine == machine {
                continue; // self column is live, not replayed
            }
            machines.push(MachineColumn {
                id: report.machine.clone(),
                display_name: report.display_name.clone(),
                is_self: false,
                reported_at: Some(report.reported_at.clone()),
            });
        }

        let local_cells = self.measure_local(&manifest, &machine, &root);
        let mut repos = Vec::new();
        for (entry, (local, hub_cmp)) in manifest.repos.iter().zip(local_cells) {
            let (hub_head, hub_note) = (hub_cmp.hub_head, hub_cmp.note);
            let mut cells = BTreeMap::new();
            cells.insert(machine.clone(), local);
            for report in &reports {
                if report.machine == machine {
                    continue;
                }
                if let Some(cell) = report.repos.iter().find(|r| r.name == entry.name) {
                    cells.insert(report.machine.clone(), cell.clone());
                }
            }
            repos.push(FleetRepoRow {
                name: entry.name.clone(),
                hub: entry.hub.clone(),
                authority: entry.authority.clone(),
                branch: entry.branch.clone(),
                auto_sync: entry.auto_sync,
                hub_head,
                hub_note,
                cells,
            });
        }
        warnings.sort();
        Ok(FleetStatus {
            machine,
            meta_url: self.meta_url()?,
            meta_state,
            meta_warning,
            projects_root: root.to_string_lossy().into_owned(),
            scanned_at: chrono::Utc::now().timestamp_millis(),
            machines,
            repos,
            warnings,
        })
    }

    pub fn discover(&self) -> Result<FleetDiscovery, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        meta.ensure_fresh()?;
        let manifest = meta.read_manifest()?;
        let root = self.projects_root(&manifest)?;
        let known: HashSet<String> = manifest.repos.iter().map(|r| r.name.clone()).collect();
        Ok(FleetDiscovery {
            machine,
            projects_root: root.to_string_lossy().into_owned(),
            scanned_at: chrono::Utc::now().timestamp_millis(),
            unlisted: repo_ops::discover(&root, &known),
        })
    }

    /// Narrow P2 manifest mutation: toggle one existing repo's automatic-round
    /// opt-in. General manifest editing remains in #43.
    pub fn set_repo_auto_sync(
        &self,
        repo: &str,
        enabled: bool,
    ) -> Result<AutoSyncUpdateOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        self.meta_repo()?.set_repo_auto_sync(repo, enabled)
    }

    /// Load the only editable fleet manifest after proving the meta cache is
    /// fresh. The snapshot binds GUI edits to machine, remote HEAD, and content.
    pub fn manifest_get(&self) -> Result<FleetManifestSnapshot, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet manifest editing requires fresh metadata: {error}"
            )));
        }
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let meta_head = meta.head_oid()?;
        let known_machines = known_machine_ids(&meta, &manifest, &machine);
        Ok(FleetManifestSnapshot {
            machine,
            meta_head,
            manifest_digest,
            manifest,
            known_machines,
        })
    }

    /// Validate repo edits and produce the exact add/update/remove plan. This
    /// refreshes metadata but writes neither the manifest nor any project repo.
    pub fn plan_manifest_update(
        &self,
        base: &FleetManifestSnapshot,
        repos: Vec<manifest::RepoEntry>,
    ) -> Result<FleetManifestUpdatePlan, AppError> {
        let machine = self.preview_machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet manifest preview requires fresh metadata: {error}"
            )));
        }
        let (current, manifest_digest) = meta.read_manifest_snapshot()?;
        let meta_head = meta.head_oid()?;
        if machine != base.machine
            || manifest_digest != base.manifest_digest
            || meta_head != base.meta_head
        {
            return Err(AppError::invalid_input(
                "fleet manifest conflict: machine or remote metadata changed before preview",
            ));
        }
        let mut next = current.clone();
        next.repos = repos;
        let known = known_machine_ids(&meta, &current, &machine);
        validate_manifest_for_machines(&next, &known)?;
        let changes = manifest_changes(&current, &next);
        Ok(FleetManifestUpdatePlan {
            machine,
            meta_head,
            manifest_digest,
            planned_at: chrono::Utc::now().timestamp_millis(),
            manifest: next,
            changes,
        })
    }

    /// Apply only the exact confirmed plan. Drift in the machine, manifest
    /// digest, or meta-repo remote HEAD is returned as a conflict with no write.
    pub fn apply_manifest_update(
        &self,
        plan: &FleetManifestUpdatePlan,
    ) -> Result<FleetManifestUpdateOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet manifest update requires fresh metadata: {error}"
            )));
        }
        let (current, manifest_digest) = meta.read_manifest_snapshot()?;
        let meta_head = meta.head_oid()?;
        let conflict = |message: &str| FleetManifestUpdateOutcome {
            ok: false,
            action: "conflict".into(),
            pushed: false,
            commit: None,
            manifest_digest: manifest_digest.clone(),
            changes: plan.changes.clone(),
            message: Some(message.into()),
        };
        if machine != plan.machine
            || manifest_digest != plan.manifest_digest
            || meta_head != plan.meta_head
        {
            return Ok(conflict(
                "machine, manifest, or remote metadata changed after preview",
            ));
        }
        if plan.manifest.fleet != current.fleet || plan.manifest.hubs != current.hubs {
            return Ok(conflict(
                "the confirmed plan changed non-editable manifest fields",
            ));
        }
        let known = known_machine_ids(&meta, &current, &machine);
        validate_manifest_for_machines(&plan.manifest, &known)?;
        let exact_changes = manifest_changes(&current, &plan.manifest);
        if exact_changes != plan.changes {
            return Ok(conflict(
                "the confirmed manifest diff does not match its exact plan",
            ));
        }
        if exact_changes.is_empty() {
            return Ok(FleetManifestUpdateOutcome {
                ok: true,
                action: "unchanged".into(),
                pushed: false,
                commit: None,
                manifest_digest,
                changes: exact_changes,
                message: None,
            });
        }
        let written = meta.write_manifest(&plan.manifest_digest, &plan.manifest)?;
        Ok(FleetManifestUpdateOutcome {
            ok: written.action != "conflict",
            action: written.action,
            pushed: written.pushed,
            commit: written.commit,
            manifest_digest: written.manifest_digest,
            changes: exact_changes,
            message: written.message,
        })
    }

    /// Read-only preview for local hub mirror creation and local worktree
    /// remote convergence. It consumes the cached manifest and never probes a
    /// remote host or refreshes the cache.
    pub fn plan_init(&self, selected: &[String]) -> Result<FleetInitPlan, AppError> {
        let machine = self.preview_machine_id()?;
        let meta = self.meta_repo()?;
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;
        let meta_repo = self.plan_meta_init(&manifest, &machine, &meta);
        let items = selected_repo_names(&manifest, selected)
            .iter()
            .map(|name| self.plan_init_item(&manifest, &root, &machine, name))
            .collect::<Vec<_>>();
        Ok(FleetInitPlan {
            ok: meta_repo.action != "refused"
                && items
                    .iter()
                    .all(|item| matches!(item.status.as_str(), "ready" | "no_op")),
            machine,
            manifest_digest,
            meta_repo,
            planned_at: chrono::Utc::now().timestamp_millis(),
            items,
        })
    }

    /// Apply the exact init preview under `fleet.lock`. A fresh manifest and
    /// every machine/path/mirror/remote evidence field are re-checked before
    /// the two allowed mutations: local bare init and hub remote add/set-url.
    pub fn apply_init(&self, plan: &FleetInitPlan) -> Result<FleetInitOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        let (cached_manifest, cached_digest) = meta.read_manifest_snapshot()?;
        let current_meta = self.plan_meta_init(&cached_manifest, &machine, &meta);
        if machine != plan.machine
            || cached_digest != plan.manifest_digest
            || current_meta != plan.meta_repo
        {
            let items: Vec<_> = plan
                .items
                .iter()
                .map(|item| init_conflict(&item.repo))
                .collect();
            for result in &items {
                self.log_init_audit(result);
            }
            return Ok(FleetInitOutcome {
                ok: false,
                machine,
                meta_repo_action: "conflict".into(),
                items,
            });
        }
        let meta_repo_action = match plan.meta_repo.action.as_str() {
            "create" => {
                if let Err(error) = meta.init_missing_local_from_cache() {
                    self.store.log_audit(AuditDraft::new("fleet_init").detail(
                        "repo=_patchbay-fleet mirror=error remote=no_op result=error before_head=missing after_head=missing",
                    ));
                    return Err(error);
                }
                let after_head = repo_ops::head_oid(std::path::Path::new(meta.url()))
                    .unwrap_or_else(|_| "unreadable".into());
                self.store.log_audit(
                    AuditDraft::new("fleet_init")
                        .detail(format!(
                            "repo=_patchbay-fleet mirror=created remote=no_op result=applied before_head=missing after_head={after_head}"
                        ))
                        .ok(),
                );
                "created"
            }
            "no_op" => "no_op",
            "guide" => "guided",
            "refused" => {
                let items: Vec<_> = plan
                    .items
                    .iter()
                    .map(|item| FleetInitResult {
                        repo: item.repo.clone(),
                        action: "refused".into(),
                        reason_code: plan.meta_repo.reason_code.clone(),
                        message: plan.meta_repo.message.clone(),
                        mirror_action: "skipped".into(),
                        remote_action: "skipped".into(),
                    })
                    .collect();
                for result in &items {
                    self.log_init_audit(result);
                }
                return Ok(FleetInitOutcome {
                    ok: false,
                    machine,
                    meta_repo_action: "refused".into(),
                    items,
                });
            }
            _ => {
                return Err(AppError::invalid_input(
                    "unsupported fleet meta init action",
                ))
            }
        }
        .to_string();
        if plan.items.iter().all(|item| item.status == "refused") {
            let items = plan
                .items
                .iter()
                .map(|item| FleetInitResult {
                    repo: item.repo.clone(),
                    action: "refused".into(),
                    reason_code: item.reason_code.clone(),
                    message: item.message.clone(),
                    mirror_action: item.mirror_action.clone(),
                    remote_action: item.remote_action.clone(),
                })
                .collect::<Vec<_>>();
            for result in &items {
                self.log_init_audit(result);
            }
            return Ok(FleetInitOutcome {
                ok: false,
                machine,
                meta_repo_action,
                items,
            });
        }
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet init requires fresh fleet metadata; refresh failed: {error}"
            )));
        }
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;

        let mut items = Vec::with_capacity(plan.items.len());
        for planned in &plan.items {
            if planned.status == "refused" {
                let result = FleetInitResult {
                    repo: planned.repo.clone(),
                    action: "refused".into(),
                    reason_code: planned.reason_code.clone(),
                    message: planned.message.clone(),
                    mirror_action: planned.mirror_action.clone(),
                    remote_action: planned.remote_action.clone(),
                };
                self.log_init_audit_with_evidence(&result, planned.evidence.as_ref());
                items.push(result);
                continue;
            }

            let current = self.plan_init_item(&manifest, &root, &machine, &planned.repo);
            if machine != plan.machine
                || manifest_digest != plan.manifest_digest
                || current.status != planned.status
                || current.mirror_action != planned.mirror_action
                || current.remote_action != planned.remote_action
                || current.evidence != planned.evidence
            {
                let result = init_conflict(&planned.repo);
                self.log_init_audit_with_evidence(&result, planned.evidence.as_ref());
                items.push(result);
                continue;
            }

            if planned.status == "no_op" {
                let result = FleetInitResult {
                    repo: planned.repo.clone(),
                    action: "no_op".into(),
                    reason_code: None,
                    message: None,
                    mirror_action: "no_op".into(),
                    remote_action: "no_op".into(),
                };
                self.log_init_audit_with_evidence(&result, planned.evidence.as_ref());
                items.push(result);
                continue;
            }

            let evidence = current.evidence.expect("ready init item has evidence");
            let mut mirror_result = planned.mirror_action.clone();
            if planned.mirror_action == "create" {
                let mirror = PathBuf::from(
                    evidence
                        .mirror_path
                        .as_deref()
                        .expect("local mirror creation has a path"),
                );
                if let Err(error) = repo_ops::init_bare_mirror(&mirror) {
                    let result = match error {
                        repo_ops::MirrorInitError::TargetChanged => init_conflict(&planned.repo),
                        repo_ops::MirrorInitError::InitFailed(message) => init_error(
                            &planned.repo,
                            "mirror_init_failed",
                            message,
                            "error",
                            "skipped",
                        ),
                    };
                    self.log_init_audit_with_evidence(&result, Some(&evidence));
                    items.push(result);
                    continue;
                }
                if repo_ops::mirror_state(&mirror) != repo_ops::MirrorState::Bare {
                    let result = init_error(
                        &planned.repo,
                        "mirror_verify_failed",
                        "created mirror is not a bare repository",
                        "error",
                        "skipped",
                    );
                    self.log_init_audit_with_evidence(&result, Some(&evidence));
                    items.push(result);
                    continue;
                }
                mirror_result = "created".into();
            }

            let path = root.join(&planned.repo);
            let remote_write = match planned.remote_action.as_str() {
                "add" => repo_ops::add_remote(
                    &path,
                    manifest
                        .repos
                        .iter()
                        .find(|entry| entry.name == planned.repo)
                        .map(|entry| entry.hub.as_str())
                        .expect("planned repo remains in fresh manifest"),
                    &evidence.target_remote_url,
                ),
                "set_url" => repo_ops::set_remote_url(
                    &path,
                    manifest
                        .repos
                        .iter()
                        .find(|entry| entry.name == planned.repo)
                        .map(|entry| entry.hub.as_str())
                        .expect("planned repo remains in fresh manifest"),
                    &evidence.target_remote_url,
                ),
                "no_op" | "unavailable" => Ok(()),
                other => Err(format!("unsupported remote action {other}")),
            };
            if let Err(message) = remote_write {
                let action = if mirror_result == "created" {
                    "partial"
                } else {
                    "error"
                };
                let result = init_error(
                    &planned.repo,
                    "remote_update_failed",
                    message,
                    &mirror_result,
                    "error",
                );
                let result = FleetInitResult {
                    action: action.into(),
                    ..result
                };
                self.log_init_audit_with_evidence(&result, Some(&evidence));
                items.push(result);
                continue;
            }

            let entry = manifest
                .repos
                .iter()
                .find(|entry| entry.name == planned.repo)
                .expect("planned repo remains in fresh manifest");
            if evidence.local_repo_exists {
                let remote_after = repo_ops::configured_remote_url(&path, &entry.hub);
                let origin_after = repo_ops::configured_remote_url(&path, "origin");
                if remote_after.as_ref().ok().and_then(|url| url.as_deref())
                    != Some(evidence.target_remote_url.as_str())
                    || origin_after.ok() != Some(evidence.origin_url.clone())
                {
                    let result = init_error(
                        &planned.repo,
                        "remote_verify_failed",
                        "hub remote did not reach the target URL or origin changed",
                        &mirror_result,
                        "error",
                    );
                    self.log_init_audit_with_evidence(&result, Some(&evidence));
                    items.push(result);
                    continue;
                }
            }

            let result = FleetInitResult {
                repo: planned.repo.clone(),
                action: "applied".into(),
                reason_code: None,
                message: None,
                mirror_action: mirror_result,
                remote_action: planned.remote_action.clone(),
            };
            self.log_init_audit_with_evidence(&result, Some(&evidence));
            items.push(result);
        }

        Ok(FleetInitOutcome {
            ok: items
                .iter()
                .all(|item| matches!(item.action.as_str(), "applied" | "no_op")),
            machine,
            meta_repo_action,
            items,
        })
    }

    fn plan_meta_init(
        &self,
        manifest: &Manifest,
        machine: &str,
        meta: &MetaRepo,
    ) -> FleetMetaInitPlan {
        let mut hosts = manifest
            .hubs
            .values()
            .filter_map(|hub| hub.host_machine.clone())
            .collect::<Vec<_>>();
        hosts.sort();
        hosts.dedup();
        let host_machine = (hosts.len() == 1).then(|| hosts[0].clone());
        let target = PathBuf::from(meta.url());
        if host_machine.as_deref() != Some(machine) || !target.is_absolute() {
            return FleetMetaInitPlan {
                url: meta.url().to_string(),
                host_machine,
                exists: None,
                action: "guide".into(),
                reason_code: None,
                message: Some(
                    "meta repo initialization is host-only; run fleet init on the hub host".into(),
                ),
            };
        }
        let target_is_guarded = manifest.hubs.values().any(|hub| {
            if hub.host_machine.as_deref() != Some(machine) {
                return false;
            }
            let base = PathBuf::from(manifest::resolve_hub_base(hub, machine));
            base.is_absolute() && target != base && path_guard::is_path_safe(&base, &target)
        });
        if !target_is_guarded {
            return FleetMetaInitPlan {
                url: meta.url().to_string(),
                host_machine,
                exists: None,
                action: "refused".into(),
                reason_code: Some("meta_repo_path_outside_hub".into()),
                message: Some("fleet meta repo path must stay inside a declared local hub".into()),
            };
        }
        match repo_ops::mirror_state(&target) {
            repo_ops::MirrorState::Missing => FleetMetaInitPlan {
                url: meta.url().to_string(),
                host_machine,
                exists: Some(false),
                action: "create".into(),
                reason_code: None,
                message: None,
            },
            repo_ops::MirrorState::Bare => FleetMetaInitPlan {
                url: meta.url().to_string(),
                host_machine,
                exists: Some(true),
                action: "no_op".into(),
                reason_code: None,
                message: None,
            },
            repo_ops::MirrorState::NotBare => FleetMetaInitPlan {
                url: meta.url().to_string(),
                host_machine,
                exists: Some(true),
                action: "refused".into(),
                reason_code: Some("meta_repo_not_bare".into()),
                message: Some("existing fleet meta repo path is not bare".into()),
            },
        }
    }

    fn plan_init_item(
        &self,
        manifest: &Manifest,
        root: &std::path::Path,
        machine: &str,
        name: &str,
    ) -> FleetInitPlanItem {
        let Some(entry) = manifest.repos.iter().find(|entry| entry.name == name) else {
            return refused_init(name, "repo_not_in_manifest", "repo is not managed by fleet");
        };
        if entry.hub == "origin" {
            return refused_init(
                name,
                "origin_reserved",
                "origin is reserved and never managed by fleet init",
            );
        }
        let Some(hub) = manifest.hubs.get(&entry.hub) else {
            return refused_init(name, "hub_missing", "manifest hub is missing");
        };
        let Some(host_machine) = hub.host_machine.as_deref() else {
            return refused_init(
                name,
                "hub_host_missing",
                "manifest hub needs host_machine for local initialization",
            );
        };
        let path = root.join(&entry.name);
        if !path_guard::is_path_safe(root, &path) {
            return refused_init(
                name,
                "path_outside_projects_root",
                "repo path escapes the configured projects root",
            );
        }
        let local_repo_exists = path.join(".git").exists();
        if path.exists() && !local_repo_exists {
            return refused_init(
                name,
                "repo_unreadable",
                "local path is not a Git working repository",
            );
        }
        let (head_oid, dirty_count, branch) = if local_repo_exists {
            let state = repo_ops::local_state(&path);
            if !state.present || state.dirty.is_none() {
                return refused_init(
                    name,
                    "repo_unreadable",
                    "local path is not a readable Git working repository",
                );
            }
            if state.detached || state.branch.is_none() {
                return refused_init(name, "detached_head", "local repo has a detached HEAD");
            }
            let dirty_count = state.dirty.expect("checked above");
            if dirty_count != 0 {
                return refused_init(name, "dirty_worktree", "local repo is dirty");
            }
            let head_oid = match repo_ops::head_oid(&path) {
                Ok(head) => head,
                Err(message) => return refused_init(name, "repo_unreadable", message),
            };
            (Some(head_oid), Some(dirty_count), state.branch)
        } else {
            (None, None, None)
        };
        let target_remote_url =
            manifest::repo_url(&manifest::resolve_hub_base(hub, machine), &entry.name);
        let current_remote_url = if local_repo_exists {
            match repo_ops::configured_remote_url(&path, &entry.hub) {
                Ok(url) => url,
                Err(message) => return refused_init(name, "repo_unreadable", message),
            }
        } else {
            None
        };
        let origin_url = if local_repo_exists {
            match repo_ops::configured_remote_url(&path, "origin") {
                Ok(url) => url,
                Err(message) => return refused_init(name, "repo_unreadable", message),
            }
        } else {
            None
        };
        let remote_action = if !local_repo_exists {
            "unavailable"
        } else if current_remote_url.is_none() {
            "add"
        } else if current_remote_url.as_deref() != Some(target_remote_url.as_str()) {
            "set_url"
        } else {
            "no_op"
        };

        let (mirror_path, mirror_exists, mirror_action) = if host_machine == machine {
            let base = PathBuf::from(manifest::resolve_hub_base(hub, machine));
            if !base.is_absolute() {
                return refused_init(
                    name,
                    "hub_path_not_local",
                    "hub host must resolve its mirror base to an absolute local path",
                );
            }
            let mirror = base.join(format!("{}.git", entry.name));
            if !path_guard::is_path_safe(&base, &mirror) {
                return refused_init(
                    name,
                    "mirror_path_outside_hub",
                    "mirror path escapes the manifest hub base",
                );
            }
            match repo_ops::mirror_state(&mirror) {
                repo_ops::MirrorState::Missing => (
                    Some(mirror.to_string_lossy().into_owned()),
                    Some(false),
                    "create",
                ),
                repo_ops::MirrorState::Bare => (
                    Some(mirror.to_string_lossy().into_owned()),
                    Some(true),
                    "no_op",
                ),
                repo_ops::MirrorState::NotBare => {
                    return refused_init(
                        name,
                        "mirror_not_bare",
                        "existing mirror path is not a bare Git repository",
                    )
                }
            }
        } else {
            (None, None, "guide")
        };

        let guidance = (mirror_action == "guide").then(|| {
            format!("mirror initialization is host-only; run fleet init on {host_machine}")
        });
        let (status, reason_code, message) =
            if matches!(remote_action, "add" | "set_url") || mirror_action == "create" {
                ("ready", None, guidance)
            } else if remote_action == "unavailable" {
                (
                    "refused",
                    Some("repo_missing".to_string()),
                    Some("local repo is missing; remote convergence is unavailable".to_string()),
                )
            } else {
                ("no_op", None, guidance)
            };
        FleetInitPlanItem {
            repo: name.to_string(),
            status: status.into(),
            reason_code,
            message,
            mirror_action: mirror_action.into(),
            remote_action: remote_action.into(),
            evidence: Some(FleetInitEvidence {
                host_machine: host_machine.to_string(),
                repo_path: path.to_string_lossy().into_owned(),
                mirror_path,
                mirror_exists,
                local_repo_exists,
                remote_exists: current_remote_url.is_some(),
                current_remote_url,
                target_remote_url,
                origin_url,
                head_oid,
                dirty_count,
                branch,
            }),
        }
    }

    fn log_init_audit(&self, result: &FleetInitResult) {
        self.log_init_audit_with_evidence(result, None);
    }

    fn log_init_audit_with_evidence(
        &self,
        result: &FleetInitResult,
        evidence: Option<&FleetInitEvidence>,
    ) {
        let before_head = evidence
            .and_then(|item| item.head_oid.as_deref())
            .unwrap_or("none");
        let before_remote = evidence
            .and_then(|item| item.current_remote_url.as_deref())
            .unwrap_or("none");
        let after_remote = match (
            result.action.as_str(),
            result.remote_action.as_str(),
            evidence,
        ) {
            ("applied", "add" | "set_url", Some(item)) => item.target_remote_url.as_str(),
            _ => before_remote,
        };
        let draft = AuditDraft::new("fleet_init").detail(format!(
            "repo={} mirror={} remote={} result={} before_head={} after_head={} before_remote={} after_remote={}",
            result.repo,
            result.mirror_action,
            result.remote_action,
            result.action,
            before_head,
            before_head,
            before_remote,
            after_remote,
        ));
        self.store
            .log_audit(if matches!(result.action.as_str(), "applied" | "no_op") {
                draft.ok()
            } else {
                draft
            });
    }

    /// Read-only push preview. An empty selector means every manifest repo;
    /// repeated `--repo` selectors preserve their first occurrence only.
    pub fn plan_push(&self, selected: &[String]) -> Result<FleetPushPlan, AppError> {
        let machine = self.preview_machine_id()?;
        let meta = self.meta_repo()?;
        // Preview is strictly read-only: consume the cache prepared by status
        // or report, but never clone/fetch/pull it here.
        let manifest = meta.read_manifest()?;
        let root = self.projects_root(&manifest)?;
        let names = selected_repo_names(&manifest, selected);
        let items = names
            .iter()
            .map(|name| self.plan_push_item(&manifest, &root, &machine, name))
            .collect::<Vec<_>>();
        Ok(FleetPushPlan {
            ok: items.iter().all(|item| item.status == "ready"),
            machine,
            planned_at: chrono::Utc::now().timestamp_millis(),
            items,
        })
    }

    /// Apply the exact previewed plan under the fleet-wide lock. Every repo is
    /// resolved again from the current manifest and all plan evidence is
    /// re-measured before a push is attempted.
    pub fn apply_push(&self, plan: &FleetPushPlan) -> Result<FleetPushOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        self.apply_push_locked(plan)
    }

    pub(super) fn apply_push_locked(
        &self,
        plan: &FleetPushPlan,
    ) -> Result<FleetPushOutcome, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet push requires fresh fleet metadata; refresh failed: {error}"
            )));
        }
        let manifest = meta.read_manifest()?;
        let root = self.projects_root(&manifest)?;

        let mut items = Vec::with_capacity(plan.items.len());
        for planned in &plan.items {
            if planned.status != "ready" {
                let result = FleetPushResult {
                    repo: planned.repo.clone(),
                    action: "refused".into(),
                    reason_code: planned.reason_code.clone(),
                    message: planned.message.clone(),
                    before_head: None,
                    after_head: None,
                };
                self.log_push_audit(&result);
                items.push(result);
                continue;
            }

            let current = self.plan_push_item(&manifest, &root, &machine, &planned.repo);
            if machine != plan.machine
                || current.status != "ready"
                || current.evidence != planned.evidence
            {
                let result = FleetPushResult {
                    repo: planned.repo.clone(),
                    action: "conflict".into(),
                    reason_code: Some("plan_conflict".into()),
                    message: Some("machine or repository evidence changed after preview".into()),
                    before_head: None,
                    after_head: None,
                };
                self.log_push_audit(&result);
                items.push(result);
                continue;
            }

            let evidence = current.evidence.expect("ready push item has evidence");
            let before = match repo_ops::hub_head(&evidence.remote_url, &evidence.branch) {
                Ok(head) => head,
                Err(error) => {
                    let result = push_error(&planned.repo, "hub_unreachable", error, None);
                    self.log_push_audit(&result);
                    items.push(result);
                    continue;
                }
            };
            let path = root.join(&planned.repo);
            if let Err(error) = repo_ops::push_branch(
                &path,
                &evidence.remote_url,
                &evidence.branch,
                &evidence.head_oid,
            ) {
                let code = if error.to_ascii_lowercase().contains("non-fast-forward")
                    || error.to_ascii_lowercase().contains("fetch first")
                {
                    "non_fast_forward"
                } else {
                    "push_failed"
                };
                let result = push_error(&planned.repo, code, error, before.clone());
                self.log_push_audit(&result);
                items.push(result);
                continue;
            }
            let after = match repo_ops::hub_head(&evidence.remote_url, &evidence.branch) {
                Ok(head) => head,
                Err(error) => {
                    let result = push_error(&planned.repo, "verify_failed", error, before.clone());
                    self.log_push_audit(&result);
                    items.push(result);
                    continue;
                }
            };
            if after.as_deref() != Some(evidence.head_oid.as_str()) {
                let result = FleetPushResult {
                    repo: planned.repo.clone(),
                    action: "error".into(),
                    reason_code: Some("verify_failed".into()),
                    message: Some("hub branch did not reach the planned local head".into()),
                    before_head: before,
                    after_head: after,
                };
                self.log_push_audit(&result);
                items.push(result);
                continue;
            }
            let action = if before == after {
                "up_to_date"
            } else {
                "pushed"
            };
            let result = FleetPushResult {
                repo: planned.repo.clone(),
                action: action.into(),
                reason_code: None,
                message: None,
                before_head: before,
                after_head: after,
            };
            self.log_push_audit(&result);
            items.push(result);
        }

        Ok(FleetPushOutcome {
            ok: items
                .iter()
                .all(|item| matches!(item.action.as_str(), "pushed" | "up_to_date")),
            machine,
            items,
        })
    }

    fn plan_push_item(
        &self,
        manifest: &Manifest,
        root: &std::path::Path,
        machine: &str,
        name: &str,
    ) -> FleetPushPlanItem {
        let Some(entry) = manifest.repos.iter().find(|entry| entry.name == name) else {
            return refused_push(name, "repo_not_in_manifest", "repo is not managed by fleet");
        };
        if entry.authority != machine && entry.authority != manifest::AUTHORITY_SHARED {
            return refused_push(
                name,
                "not_authority",
                "this machine is not the repo authority",
            );
        }
        let path = root.join(&entry.name);
        if !path_guard::is_path_safe(root, &path) {
            return refused_push(
                name,
                "path_outside_projects_root",
                "repo path escapes the configured projects root",
            );
        }
        let state = repo_ops::local_state(&path);
        if !state.present {
            return refused_push(name, "repo_missing", "local repo is missing");
        }
        if state.detached || state.branch.is_none() {
            return refused_push(name, "detached_head", "local repo has a detached HEAD");
        }
        let dirty_count = match state.dirty {
            Some(count) => count,
            None => return refused_push(name, "repo_unreadable", "local repo state is unreadable"),
        };
        if dirty_count != 0 {
            return refused_push(name, "repo_dirty", "local repo has uncommitted changes");
        }
        // Push moves the checked-out branch. On a branch the manifest does not
        // manage that would publish an unrelated ref to the hub — silently
        // creating a hub branch nobody asked for. Pull already refuses the
        // same mismatch; push must too.
        if state.branch.as_deref() != Some(entry.branch.as_str()) {
            return refused_push(
                name,
                "branch_mismatch",
                "local branch does not match the manifest branch",
            );
        }
        let head_oid = match repo_ops::head_oid(&path) {
            Ok(oid) => oid,
            Err(message) => return refused_push(name, "repo_unreadable", message),
        };
        let Some(hub) = manifest.hubs.get(&entry.hub) else {
            return refused_push(name, "hub_missing", "manifest hub is missing");
        };
        let remote_url = manifest::repo_url(&manifest::resolve_hub_base(hub, machine), &entry.name);
        FleetPushPlanItem {
            repo: name.to_string(),
            status: "ready".into(),
            reason_code: None,
            message: None,
            evidence: Some(FleetPushEvidence {
                head_oid,
                dirty_count,
                branch: state.branch.unwrap(),
                remote_url,
            }),
        }
    }

    fn log_push_audit(&self, result: &FleetPushResult) {
        let before = result.before_head.as_deref().unwrap_or("missing");
        let after = result.after_head.as_deref().unwrap_or("missing");
        let draft = AuditDraft::new("fleet_push").detail(format!(
            "repo={} before={} after={} result={}",
            result.repo, before, after, result.action
        ));
        self.store.log_audit(
            if matches!(result.action.as_str(), "pushed" | "up_to_date") {
                draft.ok()
            } else {
                draft
            },
        );
    }

    /// Read-only pull preview. It reads the cached manifest and the hub branch
    /// tip via ls-remote, but never fetches, updates refs, writes settings, or
    /// appends audit records.
    pub fn plan_pull(&self, selected: &[String]) -> Result<FleetPullPlan, AppError> {
        let machine = self.preview_machine_id()?;
        let meta = self.meta_repo()?;
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;
        let items = selected_repo_names(&manifest, selected)
            .iter()
            .map(|name| self.plan_pull_item(&manifest, &root, &machine, name))
            .collect::<Vec<_>>();
        Ok(FleetPullPlan {
            ok: items.iter().all(|item| item.status == "ready"),
            machine,
            manifest_digest,
            planned_at: chrono::Utc::now().timestamp_millis(),
            items,
        })
    }

    /// Apply the exact pull plan under fleet.lock. Manifest identity, machine,
    /// authority, path containment, and every repository evidence field are
    /// re-checked before and after the network fetch; checkout is the shared
    /// libgit2 SAFE fast-forward primitive.
    pub fn apply_pull(&self, plan: &FleetPullPlan) -> Result<FleetPullOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        self.apply_pull_locked(plan)
    }

    pub(super) fn apply_pull_locked(
        &self,
        plan: &FleetPullPlan,
    ) -> Result<FleetPullOutcome, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet pull requires fresh fleet metadata; refresh failed: {error}"
            )));
        }
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;

        let mut items = Vec::with_capacity(plan.items.len());
        for planned in &plan.items {
            if planned.status != "ready" {
                let result = FleetPullResult {
                    repo: planned.repo.clone(),
                    action: "refused".into(),
                    reason_code: planned.reason_code.clone(),
                    message: planned.message.clone(),
                    before_head: None,
                    after_head: None,
                };
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            }

            let conflict = |message: &str| FleetPullResult {
                repo: planned.repo.clone(),
                action: "conflict".into(),
                reason_code: Some("plan_conflict".into()),
                message: Some(message.into()),
                before_head: planned.evidence.as_ref().map(|e| e.head_oid.clone()),
                after_head: planned.evidence.as_ref().map(|e| e.head_oid.clone()),
            };
            if machine != plan.machine || manifest_digest != plan.manifest_digest {
                let result = conflict("machine or manifest changed after preview");
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            }
            let Some(planned_evidence) = planned.evidence.as_ref() else {
                let result = conflict("ready pull item is missing plan evidence");
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            };
            let current =
                match self.measure_pull_evidence(&manifest, &root, &machine, &planned.repo) {
                    Ok(evidence) => evidence,
                    Err(_) => {
                        let result = conflict("repository eligibility changed after preview");
                        self.log_pull_audit(&result);
                        items.push(result);
                        continue;
                    }
                };
            if &current != planned_evidence {
                let result = conflict("repository evidence changed after preview");
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            }

            let path = root.join(&planned.repo);
            if let Err(error) =
                repo_ops::fetch_hub(&path, &planned_evidence.hub_url, &planned_evidence.branch)
            {
                let result = pull_failure(
                    &planned.repo,
                    "fetch_failed",
                    error,
                    &planned_evidence.head_oid,
                );
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            }

            // The network wait is another TOCTOU window. Re-measure all local
            // fields and the hub tip before touching the worktree.
            let after_fetch =
                match self.measure_pull_evidence(&manifest, &root, &machine, &planned.repo) {
                    Ok(evidence) => evidence,
                    Err(_) => {
                        let result = conflict("repository eligibility changed during fetch");
                        self.log_pull_audit(&result);
                        items.push(result);
                        continue;
                    }
                };
            if &after_fetch != planned_evidence {
                let result = conflict("repository or hub changed during fetch");
                self.log_pull_audit(&result);
                items.push(result);
                continue;
            }

            match repo_ops::pull_relation(
                &path,
                &planned_evidence.head_oid,
                &planned_evidence.target_oid,
            ) {
                Ok(Some(repo_ops::PullRelation::Behind)) => {}
                Ok(Some(repo_ops::PullRelation::Diverged)) => {
                    let result = pull_refusal(
                        &planned.repo,
                        "diverged",
                        "hub and local history have diverged",
                        &planned_evidence.head_oid,
                    );
                    self.log_pull_audit(&result);
                    items.push(result);
                    continue;
                }
                Ok(Some(repo_ops::PullRelation::Ahead)) => {
                    let result = pull_refusal(
                        &planned.repo,
                        "local_ahead",
                        "local history is ahead of the hub",
                        &planned_evidence.head_oid,
                    );
                    self.log_pull_audit(&result);
                    items.push(result);
                    continue;
                }
                Ok(Some(repo_ops::PullRelation::Same)) => {
                    let result = pull_refusal(
                        &planned.repo,
                        "up_to_date",
                        "local branch already equals the hub tip",
                        &planned_evidence.head_oid,
                    );
                    self.log_pull_audit(&result);
                    items.push(result);
                    continue;
                }
                Ok(None) => {
                    let result = pull_failure(
                        &planned.repo,
                        "fetch_failed",
                        "fetch did not make the previewed hub target available locally",
                        &planned_evidence.head_oid,
                    );
                    self.log_pull_audit(&result);
                    items.push(result);
                    continue;
                }
                Err(error) => {
                    let result = pull_failure(
                        &planned.repo,
                        "repo_unreadable",
                        error,
                        &planned_evidence.head_oid,
                    );
                    self.log_pull_audit(&result);
                    items.push(result);
                    continue;
                }
            }

            let result = match repo_ops::fast_forward_checkout(
                &path,
                &planned_evidence.branch,
                &planned_evidence.head_oid,
                &planned_evidence.target_oid,
            ) {
                Ok(()) => {
                    let state = repo_ops::local_state(&path);
                    let after = repo_ops::head_oid(&path).ok();
                    if after.as_deref() == Some(planned_evidence.target_oid.as_str())
                        && state.branch.as_deref() == Some(planned_evidence.branch.as_str())
                        && state.dirty == Some(0)
                    {
                        FleetPullResult {
                            repo: planned.repo.clone(),
                            action: "pulled".into(),
                            reason_code: None,
                            message: None,
                            before_head: Some(planned_evidence.head_oid.clone()),
                            after_head: after,
                        }
                    } else {
                        FleetPullResult {
                            repo: planned.repo.clone(),
                            action: "error".into(),
                            reason_code: Some("verify_failed".into()),
                            message: Some(
                                "branch or working tree did not reach the previewed hub target"
                                    .into(),
                            ),
                            before_head: Some(planned_evidence.head_oid.clone()),
                            after_head: after,
                        }
                    }
                }
                Err(repo_ops::PullCheckoutError::Collision) => pull_refusal(
                    &planned.repo,
                    "untracked_collision",
                    "SAFE checkout refused to overwrite an untracked file",
                    &planned_evidence.head_oid,
                ),
                Err(repo_ops::PullCheckoutError::Other(error)) => pull_failure(
                    &planned.repo,
                    "checkout_failed",
                    error,
                    &planned_evidence.head_oid,
                ),
            };
            self.log_pull_audit(&result);
            items.push(result);
        }

        Ok(FleetPullOutcome {
            ok: items.iter().all(|item| item.action == "pulled"),
            machine,
            items,
        })
    }

    fn plan_pull_item(
        &self,
        manifest: &Manifest,
        root: &std::path::Path,
        machine: &str,
        name: &str,
    ) -> FleetPullPlanItem {
        let evidence = match self.measure_pull_evidence(manifest, root, machine, name) {
            Ok(evidence) => evidence,
            Err(item) => return item,
        };
        match repo_ops::pull_relation(
            root.join(name).as_path(),
            &evidence.head_oid,
            &evidence.target_oid,
        ) {
            Ok(Some(repo_ops::PullRelation::Same)) => refused_pull(
                name,
                "up_to_date",
                "local branch already equals the hub tip",
            ),
            Ok(Some(repo_ops::PullRelation::Ahead)) => {
                refused_pull(name, "local_ahead", "local history is ahead of the hub")
            }
            Ok(Some(repo_ops::PullRelation::Diverged)) => {
                refused_pull(name, "diverged", "hub and local history have diverged")
            }
            Err(message) => refused_pull(name, "repo_unreadable", message),
            Ok(Some(repo_ops::PullRelation::Behind)) | Ok(None) => FleetPullPlanItem {
                repo: name.to_string(),
                status: "ready".into(),
                reason_code: None,
                message: None,
                evidence: Some(evidence),
            },
        }
    }

    fn measure_pull_evidence(
        &self,
        manifest: &Manifest,
        root: &std::path::Path,
        machine: &str,
        name: &str,
    ) -> Result<FleetPullEvidence, FleetPullPlanItem> {
        let Some(entry) = manifest.repos.iter().find(|entry| entry.name == name) else {
            return Err(refused_pull(
                name,
                "repo_not_in_manifest",
                "repo is not managed by fleet",
            ));
        };
        if entry.authority == machine && entry.authority != manifest::AUTHORITY_SHARED {
            return Err(refused_pull(
                name,
                "authority_self_pull",
                "the authority machine must not pull its own repository",
            ));
        }
        let path = root.join(&entry.name);
        if !path_guard::is_path_safe(root, &path) {
            return Err(refused_pull(
                name,
                "path_outside_projects_root",
                "repo path escapes the configured projects root",
            ));
        }
        if !path.exists() {
            return Err(refused_pull(name, "repo_missing", "local repo is missing"));
        }
        if !path.join(".git").exists() {
            return Err(refused_pull(
                name,
                "repo_unreadable",
                "local path is not a readable Git working repository",
            ));
        }
        let state = repo_ops::local_state(&path);
        if !state.present {
            return Err(refused_pull(
                name,
                "repo_unreadable",
                "local repo state is unreadable",
            ));
        }
        if state.detached || state.branch.is_none() {
            return Err(refused_pull(
                name,
                "detached_head",
                "local repo has a detached HEAD",
            ));
        }
        let dirty_count = state.dirty.ok_or_else(|| {
            refused_pull(name, "repo_unreadable", "local repo state is unreadable")
        })?;
        if dirty_count != 0 {
            return Err(refused_pull(
                name,
                "repo_dirty",
                "local repo has uncommitted changes",
            ));
        }
        let branch = state.branch.unwrap();
        if branch != entry.branch {
            return Err(refused_pull(
                name,
                "branch_mismatch",
                "local branch does not match the manifest branch",
            ));
        }
        let head_oid = repo_ops::head_oid(&path)
            .map_err(|message| refused_pull(name, "repo_unreadable", message))?;
        let hub = manifest
            .hubs
            .get(&entry.hub)
            .ok_or_else(|| refused_pull(name, "hub_missing", "manifest hub is missing"))?;
        let hub_url = manifest::repo_url(&manifest::resolve_hub_base(hub, machine), &entry.name);
        let target_oid = match repo_ops::hub_head(&hub_url, &entry.branch) {
            Ok(Some(oid)) => oid,
            Ok(None) => {
                return Err(refused_pull(
                    name,
                    "hub_branch_missing",
                    "manifest branch is missing on the hub",
                ))
            }
            Err(message) => return Err(refused_pull(name, "hub_unreachable", message)),
        };
        let remote_url = repo_ops::configured_remote_url(&path, &entry.hub)
            .map_err(|message| refused_pull(name, "repo_unreadable", message))?;
        Ok(FleetPullEvidence {
            head_oid,
            target_oid,
            dirty_count,
            branch,
            remote_url,
            hub_url,
        })
    }

    fn log_pull_audit(&self, result: &FleetPullResult) {
        let before = result.before_head.as_deref().unwrap_or("missing");
        let after = result.after_head.as_deref().unwrap_or("missing");
        let draft = AuditDraft::new("fleet_pull").detail(format!(
            "repo={} before={} after={} result={}",
            result.repo, before, after, result.action
        ));
        self.store.log_audit(if result.action == "pulled" {
            draft.ok()
        } else {
            draft
        });
    }

    /// Read-only bootstrap preview for manifest repos missing on this machine.
    pub fn plan_bootstrap(&self, selected: &[String]) -> Result<FleetBootstrapPlan, AppError> {
        let machine = self.preview_machine_id()?;
        let meta = self.meta_repo()?;
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;
        let items = selected_repo_names(&manifest, selected)
            .iter()
            .map(|name| self.plan_bootstrap_item(&manifest, &root, &machine, name))
            .collect::<Vec<_>>();
        Ok(FleetBootstrapPlan {
            ok: items.iter().all(|item| item.status == "ready"),
            machine,
            manifest_digest,
            planned_at: chrono::Utc::now().timestamp_millis(),
            items,
        })
    }

    /// Apply the exact missing-repo plan under the fleet-wide lock.
    pub fn apply_bootstrap(
        &self,
        plan: &FleetBootstrapPlan,
    ) -> Result<FleetBootstrapOutcome, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        if let MetaSyncState::Stale { error } = meta.ensure_fresh()? {
            return Err(AppError::git(format!(
                "fleet bootstrap requires fresh fleet metadata; refresh failed: {error}"
            )));
        }
        let (manifest, manifest_digest) = meta.read_manifest_snapshot()?;
        let root = self.projects_root(&manifest)?;
        let mut items = Vec::with_capacity(plan.items.len());

        for planned in &plan.items {
            if planned.status != "ready" {
                let result = FleetBootstrapResult {
                    repo: planned.repo.clone(),
                    action: "refused".into(),
                    reason_code: planned.reason_code.clone(),
                    message: planned.message.clone(),
                    after_head: None,
                };
                self.log_bootstrap_audit(&result);
                items.push(result);
                continue;
            }
            let current = self.plan_bootstrap_item(&manifest, &root, &machine, &planned.repo);
            // Name the specific drift, the way apply_pull does: "something
            // changed" sends the operator hunting, "the hub branch moved"
            // does not.
            let drift = if machine != plan.machine {
                Some("this machine's id changed after preview")
            } else if manifest_digest != plan.manifest_digest {
                Some("the fleet manifest changed after preview")
            } else if current.status != "ready" {
                Some("the repository stopped being eligible after preview")
            } else if current.evidence != planned.evidence {
                Some("the target path or hub branch changed after preview")
            } else {
                None
            };
            if let Some(reason) = drift {
                let result =
                    bootstrap_result(&planned.repo, "conflict", "plan_conflict", reason, None);
                self.log_bootstrap_audit(&result);
                items.push(result);
                continue;
            }
            let evidence = current.evidence.expect("ready bootstrap item has evidence");
            let target = std::path::Path::new(&evidence.target_path);
            let result = match repo_ops::clone_branch(
                &evidence.hub_url,
                &evidence.hub_name,
                &evidence.branch,
                target,
            ) {
                Ok(()) => {
                    let state = repo_ops::local_state(target);
                    let after = repo_ops::head_oid(target).ok();
                    let remotes = repo_ops::remote_names(target).unwrap_or_default();
                    let remote_url = repo_ops::configured_remote_url(target, &evidence.hub_name)
                        .ok()
                        .flatten();
                    if state.branch.as_deref() == Some(evidence.branch.as_str())
                        && state.dirty == Some(0)
                        && after.as_deref() == Some(evidence.target_oid.as_str())
                        && remotes == vec![evidence.hub_name.clone()]
                        && remote_url.as_deref() == Some(evidence.hub_url.as_str())
                    {
                        FleetBootstrapResult {
                            repo: planned.repo.clone(),
                            action: "bootstrapped".into(),
                            reason_code: None,
                            message: None,
                            after_head: after,
                        }
                    } else {
                        bootstrap_result(
                            &planned.repo,
                            "error",
                            "verify_failed",
                            "clone did not match the previewed branch, head, and hub remote",
                            after,
                        )
                    }
                }
                Err(repo_ops::CloneBranchError::TargetChanged(error)) => {
                    // Whatever raced into the path is not ours; reporting its
                    // HEAD as this operation's `after_head` would be a lie.
                    bootstrap_result(
                        &planned.repo,
                        "conflict",
                        "plan_conflict",
                        format!("bootstrap target appeared after preview: {error}"),
                        None,
                    )
                }
                Err(repo_ops::CloneBranchError::CloneFailed { message, debris }) => {
                    let message = if debris {
                        format!(
                            "{message} — a partial clone was left at {}; remove it before retrying",
                            evidence.target_path
                        )
                    } else {
                        message
                    };
                    bootstrap_result(&planned.repo, "error", "clone_failed", message, None)
                }
            };
            self.log_bootstrap_audit(&result);
            items.push(result);
        }

        Ok(FleetBootstrapOutcome {
            ok: items.iter().all(|item| item.action == "bootstrapped"),
            machine,
            items,
        })
    }

    fn plan_bootstrap_item(
        &self,
        manifest: &Manifest,
        root: &std::path::Path,
        machine: &str,
        name: &str,
    ) -> FleetBootstrapPlanItem {
        let Some(entry) = manifest.repos.iter().find(|entry| entry.name == name) else {
            return refused_bootstrap(name, "repo_not_in_manifest", "repo is not managed by fleet");
        };
        // The authority machine may bootstrap its own repository. Unlike pull —
        // which moves an existing checkout and could let the hub overwrite the
        // source of truth — bootstrap only ever writes a path that does not
        // exist, so there is nothing local to protect. The hub can hold only
        // what this machine pushed, so a clone back can never introduce foreign
        // history; at worst it lacks unpushed commits, which were already lost
        // with the checkout. Refusing here would fail exactly the
        // disaster-recovery case bootstrap exists for.
        if entry.hub == "origin" {
            return refused_bootstrap(
                name,
                "hub_name_reserved",
                "origin is reserved for a human-configured upstream remote",
            );
        }
        let target = root.join(&entry.name);
        if !path_guard::is_path_safe(root, &target) {
            return refused_bootstrap(
                name,
                "path_outside_projects_root",
                "repo path escapes the configured projects root",
            );
        }
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => {
                // An empty directory here is almost always our own debris: a
                // clone that died after reserving the path. Saying so turns a
                // dead end into a one-command recovery. `symlink_metadata`
                // does not follow links, so a symlink is never seen as a dir.
                let empty_dir = meta.is_dir()
                    && std::fs::read_dir(&target)
                        .map(|mut entries| entries.next().is_none())
                        .unwrap_or(false);
                return if empty_dir {
                    refused_bootstrap(
                        name,
                        "target_exists_empty",
                        format!(
                            "bootstrap target {} is an empty directory, most likely left by a \
                             failed bootstrap; remove it to retry",
                            target.display()
                        ),
                    )
                } else {
                    refused_bootstrap(name, "target_exists", "bootstrap target already exists")
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return refused_bootstrap(
                    name,
                    "target_unreadable",
                    format!("cannot prove the bootstrap target is absent: {error}"),
                )
            }
        }
        let Some(hub) = manifest.hubs.get(&entry.hub) else {
            return refused_bootstrap(name, "hub_missing", "manifest hub is missing");
        };
        let hub_url = manifest::repo_url(&manifest::resolve_hub_base(hub, machine), &entry.name);
        let target_oid = match repo_ops::hub_head(&hub_url, &entry.branch) {
            Ok(Some(oid)) => oid,
            Ok(None) => {
                return refused_bootstrap(
                    name,
                    "hub_branch_missing",
                    "manifest branch is missing on the hub",
                )
            }
            Err(message) => return refused_bootstrap(name, "hub_unreachable", message),
        };
        FleetBootstrapPlanItem {
            repo: name.to_string(),
            status: "ready".into(),
            reason_code: None,
            message: None,
            evidence: Some(FleetBootstrapEvidence {
                target_path: target.to_string_lossy().into_owned(),
                hub_name: entry.hub.clone(),
                hub_url,
                branch: entry.branch.clone(),
                target_oid,
            }),
        }
    }

    fn log_bootstrap_audit(&self, result: &FleetBootstrapResult) {
        let after = result.after_head.as_deref().unwrap_or("missing");
        let draft = AuditDraft::new("fleet_bootstrap").detail(format!(
            "repo={} before=missing after={} result={}",
            result.repo, after, result.action
        ));
        self.store.log_audit(if result.action == "bootstrapped" {
            draft.ok()
        } else {
            draft
        });
    }

    /// Preview of the report that `apply_report` would push (CLI default).
    pub fn plan_report(&self) -> Result<MachineReport, AppError> {
        let machine = self.machine_id()?;
        let meta = self.meta_repo()?;
        meta.ensure_fresh()?;
        let manifest = meta.read_manifest()?;
        let root = self.projects_root(&manifest)?;
        Ok(MachineReport {
            machine: machine.clone(),
            display_name: self.display_name(),
            reported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            repos: self
                .measure_local(&manifest, &machine, &root)
                .into_iter()
                .map(|(cell, _)| cell)
                .collect(),
        })
    }

    /// Push this machine's report to the meta repo — the only write in fleet
    /// P0, serialized across GUI/CLI/background via `fleet.lock`.
    pub fn apply_report(&self) -> Result<FleetReportResult, AppError> {
        let _lock = FleetLock::acquire(Duration::from_secs(20))?;
        let report = self.plan_report()?;
        let meta = self.meta_repo()?;
        let ReportOutcome {
            action,
            pushed,
            commit,
        } = meta.write_report(&report)?;
        Ok(FleetReportResult {
            ok: true,
            machine: report.machine.clone(),
            meta_url: self.meta_url()?,
            action,
            pushed,
            commit,
            report,
        })
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    // Both separators: `projects_root` is a per-machine setting, and on Beta
    // it is typed as `~\Projects`. Matches `central_repo::normalize_path`.
    if path.starts_with("~/") || path.starts_with("~\\") {
        return dirs::home_dir().unwrap_or_default().join(&path[2..]);
    }
    if path == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    PathBuf::from(path)
}

fn known_machine_ids(meta: &MetaRepo, manifest: &Manifest, machine: &str) -> Vec<String> {
    let (reports, _) = meta.read_reports();
    let mut ids = BTreeSet::from([machine.to_string()]);
    ids.extend(reports.into_iter().map(|report| report.machine));
    ids.extend(
        manifest
            .hubs
            .values()
            .filter_map(|hub| hub.host_machine.clone()),
    );
    ids.into_iter().collect()
}

fn validate_manifest_for_machines(
    manifest: &Manifest,
    known_machines: &[String],
) -> Result<(), AppError> {
    let text = manifest::to_toml(manifest)?;
    manifest::parse(&text)?;
    let known: HashSet<&str> = known_machines.iter().map(String::as_str).collect();
    let mut names = HashSet::new();
    for repo in &manifest.repos {
        if !names.insert(repo.name.as_str()) {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: duplicate repo name {:?}",
                repo.name
            )));
        }
        if repo.authority != manifest::AUTHORITY_SHARED && !known.contains(repo.authority.as_str())
        {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: repo {:?} references unknown authority {:?}",
                repo.name, repo.authority
            )));
        }
    }
    Ok(())
}

fn manifest_changes(before: &Manifest, after: &Manifest) -> Vec<FleetManifestChange> {
    let before_by_name: BTreeMap<_, _> = before
        .repos
        .iter()
        .map(|repo| (repo.name.as_str(), repo))
        .collect();
    let after_by_name: BTreeMap<_, _> = after
        .repos
        .iter()
        .map(|repo| (repo.name.as_str(), repo))
        .collect();
    let names: BTreeSet<_> = before_by_name
        .keys()
        .chain(after_by_name.keys())
        .copied()
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let before = before_by_name.get(name).copied();
            let after = after_by_name.get(name).copied();
            if before == after {
                return None;
            }
            Some(FleetManifestChange {
                action: match (before, after) {
                    (None, Some(_)) => "add",
                    (Some(_), None) => "remove",
                    (Some(_), Some(_)) => "update",
                    (None, None) => return None,
                }
                .into(),
                repo: name.to_string(),
                before: before.cloned(),
                after: after.cloned(),
            })
        })
        .collect()
}

fn selected_repo_names(manifest: &Manifest, selected: &[String]) -> Vec<String> {
    if selected.is_empty() {
        return manifest
            .repos
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
    }
    let mut seen = HashSet::new();
    selected
        .iter()
        .filter(|name| seen.insert((*name).clone()))
        .cloned()
        .collect()
}

fn refused_init(repo: &str, code: &str, message: impl Into<String>) -> FleetInitPlanItem {
    FleetInitPlanItem {
        repo: repo.to_string(),
        status: "refused".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        mirror_action: "unavailable".into(),
        remote_action: "unavailable".into(),
        evidence: None,
    }
}

fn init_conflict(repo: &str) -> FleetInitResult {
    FleetInitResult {
        repo: repo.to_string(),
        action: "conflict".into(),
        reason_code: Some("plan_conflict".into()),
        message: Some(
            "machine, manifest, path, mirror, or remote evidence changed after preview".into(),
        ),
        mirror_action: "skipped".into(),
        remote_action: "skipped".into(),
    }
}

fn init_error(
    repo: &str,
    code: &str,
    message: impl Into<String>,
    mirror_action: &str,
    remote_action: &str,
) -> FleetInitResult {
    FleetInitResult {
        repo: repo.to_string(),
        action: "error".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        mirror_action: mirror_action.into(),
        remote_action: remote_action.into(),
    }
}

fn refused_push(repo: &str, code: &str, message: impl Into<String>) -> FleetPushPlanItem {
    FleetPushPlanItem {
        repo: repo.to_string(),
        status: "refused".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        evidence: None,
    }
}

fn push_error(
    repo: &str,
    code: &str,
    message: impl Into<String>,
    before_head: Option<String>,
) -> FleetPushResult {
    FleetPushResult {
        repo: repo.to_string(),
        action: "error".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        before_head,
        after_head: None,
    }
}

fn refused_pull(repo: &str, code: &str, message: impl Into<String>) -> FleetPullPlanItem {
    FleetPullPlanItem {
        repo: repo.to_string(),
        status: "refused".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        evidence: None,
    }
}

fn refused_bootstrap(repo: &str, code: &str, message: impl Into<String>) -> FleetBootstrapPlanItem {
    FleetBootstrapPlanItem {
        repo: repo.to_string(),
        status: "refused".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        evidence: None,
    }
}

fn bootstrap_result(
    repo: &str,
    action: &str,
    code: &str,
    message: impl Into<String>,
    after_head: Option<String>,
) -> FleetBootstrapResult {
    FleetBootstrapResult {
        repo: repo.to_string(),
        action: action.into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        after_head,
    }
}

fn pull_refusal(repo: &str, code: &str, message: impl Into<String>, head: &str) -> FleetPullResult {
    FleetPullResult {
        repo: repo.to_string(),
        action: "refused".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        before_head: Some(head.to_string()),
        after_head: Some(head.to_string()),
    }
}

fn pull_failure(repo: &str, code: &str, message: impl Into<String>, head: &str) -> FleetPullResult {
    FleetPullResult {
        repo: repo.to_string(),
        action: "error".into(),
        reason_code: Some(code.into()),
        message: Some(message.into()),
        before_head: Some(head.to_string()),
        after_head: Some(head.to_string()),
    }
}

/// Advisory lock at `<base_dir>/fleet.lock` (design §4). Same fail-slow
/// semantics as `repo_lock` but a distinct file: fleet serializes only fleet.
pub(super) struct FleetLock {
    file: std::fs::File,
}

impl FleetLock {
    fn open_file() -> Result<std::fs::File, AppError> {
        let path = central_repo::base_dir().join("fleet.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::io)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(AppError::io)
    }

    pub(super) fn try_acquire() -> Result<Option<Self>, AppError> {
        use fs2::FileExt;
        let file = Self::open_file()?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(_) => Ok(None),
        }
    }

    fn acquire(timeout: Duration) -> Result<Self, AppError> {
        use fs2::FileExt;
        let file = Self::open_file()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    return Err(AppError::invalid_input(
                        "another fleet operation is in progress (fleet.lock busy)",
                    ))
                }
            }
        }
    }
}

impl Drop for FleetLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
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
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn seed_work_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-b", "main"]);
        std::fs::write(dir.join("file.txt"), "base").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-m", "base"]);
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn file_stamp(path: &Path) -> Option<(u64, u128, Vec<u8>)> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some((metadata.len(), modified, std::fs::read(path).ok()?))
    }

    fn seed_meta_cache(fx: &Fixture) {
        let cache = central_repo::base_dir().join("fleet/meta");
        MetaRepo::at(fx.meta_bare.to_str().unwrap(), cache)
            .ensure_fresh()
            .unwrap();
    }

    fn update_meta_manifest(fx: &Fixture, replace: impl FnOnce(String) -> String) {
        let checkout = fx
            .projects
            .parent()
            .unwrap()
            .join(format!("meta-update-{}", uuid::Uuid::new_v4()));
        let out = Command::new("git")
            .args(["clone"])
            .arg(&fx.meta_bare)
            .arg(&checkout)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "clone meta updater failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let manifest_path = checkout.join("manifest.toml");
        let current = std::fs::read_to_string(&manifest_path).unwrap();
        std::fs::write(&manifest_path, replace(current)).unwrap();
        git(&checkout, &["add", "manifest.toml"]);
        git(&checkout, &["commit", "-m", "update manifest"]);
        git(&checkout, &["push", "origin", "main"]);
    }

    fn advance_hub(fx: &Fixture, content: &str) -> String {
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let publisher = fx
            .projects
            .parent()
            .unwrap()
            .join(format!("pull-publisher-{}", uuid::Uuid::new_v4()));
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&hub)
            .arg(&publisher)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(publisher.join("file.txt"), content).unwrap();
        git(&publisher, &["add", "-A"]);
        git(&publisher, &["commit", "-m", "hub update"]);
        git(&publisher, &["push", "origin", "main"]);
        git_stdout(&hub, &["rev-parse", "refs/heads/main"])
    }

    fn assert_pull_conflict(mutate: impl FnOnce(&Fixture)) {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "ready target");
        let alpha = fx.projects.join("alpha");
        let service = FleetService::new(&fx.store);
        let plan = service.plan_pull(&["alpha".to_string()]).unwrap();
        assert!(plan.ok, "plan: {:?}", plan.items);

        mutate(&fx);
        let protected_head = git_stdout(&alpha, &["rev-parse", "HEAD"]);

        let outcome = service.apply_pull(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), protected_head);
    }

    fn assert_push_conflict(mutate: impl FnOnce(&Fixture)) {
        let fx = fixture();
        seed_meta_cache(&fx);
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);
        mutate(&fx);
        let outcome = service.apply_push(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );
    }

    fn assert_init_conflict(mutate: impl FnOnce(&Fixture)) {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        let service = FleetService::new(&fx.store);
        let plan = service.plan_init(&["alpha".to_string()]).unwrap();
        assert!(plan.ok, "plan: {:?}", plan.items);

        mutate(&fx);
        let protected_config = std::fs::read(alpha.join(".git/config")).unwrap();
        let outcome = service.apply_init(&plan).unwrap();

        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
        assert_eq!(
            std::fs::read(alpha.join(".git/config")).unwrap(),
            protected_config,
            "conflicting apply must not perform another config write"
        );
        let audit = fx.store.list_audit(None).unwrap();
        assert!(
            audit
                .iter()
                .any(|entry| entry.action == "fleet_init" && !entry.success),
            "every refused init apply must be audited"
        );
    }

    fn assert_bootstrap_conflict(mutate: impl FnOnce(&Fixture)) {
        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target).unwrap();
        let service = FleetService::new(&fx.store);
        let plan = service.plan_bootstrap(&["alpha".to_string()]).unwrap();
        assert!(plan.ok, "plan: {:?}", plan.items);
        mutate(&fx);
        let outcome = service.apply_bootstrap(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
    }

    fn bootstrap_fixture() -> Fixture {
        let fx = fixture();
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        fx
    }

    /// Full fixture: projects root with repo `alpha`, a bare hub mirror of it,
    /// and a seeded meta repo whose manifest lists `alpha`. Returns the store.
    struct Fixture {
        _temp: tempfile::TempDir,
        _guard: std::sync::MutexGuard<'static, ()>,
        store: SkillStore,
        meta_bare: PathBuf,
        projects: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            central_repo::set_test_base_dir_override(None);
        }
    }

    fn fixture() -> Fixture {
        let guard = central_repo::test_base_dir_lock();
        let temp = tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(temp.path().join("appdata")));

        let projects = temp.path().join("projects");
        let alpha = projects.join("alpha");
        seed_work_repo(&alpha);
        let mirrors = temp.path().join("mirrors");
        std::fs::create_dir_all(&mirrors).unwrap();
        let out = Command::new("git")
            .args(["clone", "--bare"])
            .arg(&alpha)
            .arg(mirrors.join("alpha.git"))
            .output()
            .unwrap();
        assert!(out.status.success());

        let meta_bare = mirrors.join("_patchbay-fleet.git");
        let out = Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&meta_bare)
            .output()
            .unwrap();
        assert!(out.status.success());
        let seed = temp.path().join("meta-seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-b", "main"]);
        git(
            &seed,
            &["remote", "add", "origin", meta_bare.to_str().unwrap()],
        );
        std::fs::write(
            seed.join("manifest.toml"),
            format!(
                r#"
[hub.test]
url = '{}'

[[repo]]
name = "alpha"
hub = "test"
authority = "selfie"
branch = "main"
"#,
                mirrors.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(seed.join("machines")).unwrap();
        std::fs::write(
            seed.join("machines/other.json"),
            r#"{
  "machine": "other",
  "display_name": "Other Mac",
  "reported_at": "2026-07-18T00:00:00Z",
  "repos": [
    { "name": "alpha", "present": true, "branch": "main",
      "head": "abc1234", "dirty": 3, "ahead": 0, "behind": 2 }
  ]
}"#,
        )
        .unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-m", "seed"]);
        git(&seed, &["push", "origin", "main"]);

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store.set_setting(MACHINE_ID_KEY, "selfie").unwrap();
        store
            .set_setting(META_URL_KEY, meta_bare.to_str().unwrap())
            .unwrap();
        store
            .set_setting(PROJECTS_ROOT_KEY, projects.to_str().unwrap())
            .unwrap();
        Fixture {
            _temp: temp,
            _guard: guard,
            store,
            meta_bare,
            projects,
        }
    }

    /// A checkout parked on a feature branch must not be reported as "N ahead"
    /// of the hub: that number compares two unrelated tips and reads as "push
    /// me" while the managed branch may be perfectly in sync. Push refuses the
    /// same mismatch, so it cannot publish an unmanaged ref to the hub.
    #[test]
    fn a_checkout_off_the_manifest_branch_reports_no_divergence_and_refuses_push() {
        let fx = fixture();
        let alpha = fx.projects.join("alpha");
        git(&alpha, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(alpha.join("file.txt"), "feature work").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "feature work"]);

        let service = FleetService::new(&fx.store);
        let status = service.status().unwrap();
        let cell = &status.repos[0].cells["selfie"];

        assert_eq!(
            cell.branch.as_deref(),
            Some("feature"),
            "branch reported honestly"
        );
        assert_eq!(cell.ahead, None, "must not count across branches");
        assert_eq!(cell.behind, None);
        assert_eq!(cell.note.as_deref(), Some("branch_off_manifest"));

        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(!plan.ok);
        assert_eq!(
            plan.items[0].reason_code.as_deref(),
            Some("branch_mismatch")
        );
    }

    #[test]
    fn status_composes_live_self_column_with_reported_columns() {
        let fx = fixture();
        let service = FleetService::new(&fx.store);
        let status = service.status().unwrap();

        assert_eq!(status.machine, "selfie");
        assert_eq!(status.meta_state, "fresh");
        let ids: Vec<_> = status.machines.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["selfie", "other"]);
        assert!(status.machines[0].is_self);
        assert_eq!(
            status.machines[1].reported_at.as_deref(),
            Some("2026-07-18T00:00:00Z")
        );

        assert_eq!(status.repos.len(), 1);
        let row = &status.repos[0];
        assert_eq!(row.name, "alpha");
        assert_eq!(row.authority, "selfie");
        assert!(row.hub_head.is_some());

        let own = &row.cells["selfie"];
        assert!(own.present);
        assert_eq!(own.branch.as_deref(), Some("main"));
        assert_eq!(own.dirty, Some(0));
        assert_eq!(own.ahead, Some(0));
        assert_eq!(own.behind, Some(0));

        let other = &row.cells["other"];
        assert_eq!(other.dirty, Some(3));
        assert_eq!(other.behind, Some(2));
    }

    #[test]
    fn report_round_trip_is_visible_to_other_machines() {
        let fx = fixture();
        let service = FleetService::new(&fx.store);
        let result = service.apply_report().unwrap();
        assert!(result.ok && result.pushed);
        assert_eq!(result.action, "reported");
        assert_eq!(result.report.repos.len(), 1);
        assert!(result.report.repos[0].present);

        // A different machine's cache sees the pushed report.
        let other_cache = fx.projects.parent().unwrap().join("other-cache/meta");
        let other = MetaRepo::at(fx.meta_bare.to_str().unwrap(), other_cache);
        other.ensure_fresh().unwrap();
        let (reports, _) = other.read_reports();
        assert!(reports.iter().any(|r| r.machine == "selfie"));
    }

    #[test]
    fn bootstrap_preview_json_round_trip_then_apply_clones_manifest_branch_and_audits() {
        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        let target_path = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target_path).unwrap();
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        git(&hub, &["symbolic-ref", "HEAD", "refs/heads/not-main"]);
        let target_oid = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);

        let service = FleetService::new(&fx.store);
        let plan = service.plan_bootstrap(&["alpha".to_string()]).unwrap();
        assert!(plan.ok, "plan: {:?}", plan.items);
        let evidence = plan.items[0].evidence.as_ref().unwrap();
        assert_eq!(evidence.target_path, target_path.to_string_lossy());
        assert_eq!(evidence.hub_name, "test");
        assert_eq!(evidence.branch, "main");
        assert_eq!(evidence.target_oid, target_oid);
        assert!(!target_path.exists(), "preview must not create the target");
        assert!(fx.store.list_audit(None).unwrap().is_empty());

        let exact_plan: FleetBootstrapPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        let outcome = service.apply_bootstrap(&exact_plan).unwrap();
        assert!(outcome.ok, "outcome: {:?}", outcome.items);
        assert_eq!(outcome.items[0].action, "bootstrapped");
        assert_eq!(
            git_stdout(&target_path, &["branch", "--show-current"]),
            "main"
        );
        assert_eq!(git_stdout(&target_path, &["rev-parse", "HEAD"]), target_oid);
        assert_eq!(git_stdout(&target_path, &["remote"]), "test");
        assert_eq!(
            git_stdout(&target_path, &["remote", "get-url", "test"]),
            hub.to_string_lossy()
        );

        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "fleet_bootstrap");
        assert!(audit[0].success);
        let detail = audit[0].detail.as_deref().unwrap();
        assert!(detail.contains("repo=alpha"));
        assert!(detail.contains(&format!("after={target_oid}")));
        assert!(detail.contains("result=bootstrapped"));
    }

    /// The authority machine may restore its own repository. Bootstrap only
    /// writes a path that does not exist, so the rule that stops the hub from
    /// overwriting the source of truth (`authority_self_pull`) has nothing to
    /// protect here — and refusing would fail the disaster-recovery case that
    /// bootstrap exists for.
    #[test]
    fn bootstrap_allows_the_authority_machine_to_restore_its_own_repo() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target).unwrap();

        let service = FleetService::new(&fx.store);
        let plan = service.plan_bootstrap(&["alpha".to_string()]).unwrap();

        assert!(
            plan.ok,
            "authority self-bootstrap must plan: {:?}",
            plan.items
        );
        assert_eq!(plan.items[0].status, "ready");
        assert!(!target.exists(), "preview must still write nothing");

        let outcome = service.apply_bootstrap(&plan).unwrap();
        assert!(outcome.ok, "{:?}", outcome.items);
        assert_eq!(outcome.items[0].action, "bootstrapped");
        assert!(target.join(".git").exists());
    }

    #[test]
    fn bootstrap_preview_refuses_every_kind_of_existing_target() {
        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");

        // Every occupied form refuses before any write; an empty directory is
        // called out separately because it is almost always our own debris and
        // the operator can clear it in one command.
        let assert_refused_with = |expected: &str| {
            let plan = FleetService::new(&fx.store)
                .plan_bootstrap(&["alpha".to_string()])
                .unwrap();
            assert!(!plan.ok);
            assert_eq!(plan.items[0].reason_code.as_deref(), Some(expected));
            assert!(fx.store.list_audit(None).unwrap().is_empty());
        };

        // Existing Git working repository.
        assert_refused_with("target_exists");
        std::fs::remove_dir_all(&target).unwrap();
        // Empty directory.
        std::fs::create_dir_all(&target).unwrap();
        assert_refused_with("target_exists_empty");
        std::fs::remove_dir_all(&target).unwrap();
        // Ordinary file.
        std::fs::write(&target, "occupied").unwrap();
        assert_refused_with("target_exists");
        std::fs::remove_file(&target).unwrap();
        // Non-Git directory.
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "occupied").unwrap();
        assert_refused_with("target_exists");
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_preview_refuses_when_target_absence_cannot_be_proven() {
        use std::os::unix::fs::PermissionsExt;

        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target).unwrap();
        let original_permissions = std::fs::metadata(&fx.projects).unwrap().permissions();
        std::fs::set_permissions(&fx.projects, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = FleetService::new(&fx.store).plan_bootstrap(&["alpha".to_string()]);

        std::fs::set_permissions(&fx.projects, original_permissions).unwrap();
        let plan = result.unwrap();
        assert!(!plan.ok);
        assert_eq!(
            plan.items[0].reason_code.as_deref(),
            Some("target_unreadable")
        );
        assert!(!target.exists());
    }

    #[test]
    fn bootstrap_refuses_unlisted_symlink_escape_and_missing_hub_branch() {
        {
            let fx = bootstrap_fixture();
            seed_meta_cache(&fx);
            let unknown = FleetService::new(&fx.store)
                .plan_bootstrap(&["rogue".to_string()])
                .unwrap();
            assert_eq!(
                unknown.items[0].reason_code.as_deref(),
                Some("repo_not_in_manifest")
            );

            {
                let target = fx.projects.join("alpha");
                std::fs::remove_dir_all(&target).unwrap();
                let outside = fx.projects.parent().unwrap().join("outside-alpha");
                std::fs::create_dir_all(&outside).unwrap();
                crate::core::test_support::symlink_dir(&outside, &target).unwrap();
                let escaped = FleetService::new(&fx.store)
                    .plan_bootstrap(&["alpha".to_string()])
                    .unwrap();
                assert_eq!(
                    escaped.items[0].reason_code.as_deref(),
                    Some("path_outside_projects_root")
                );
            }
        }

        {
            let fx = bootstrap_fixture();
            update_meta_manifest(&fx, |manifest| {
                manifest.replace("branch = \"main\"", "branch = \"missing\"")
            });
            seed_meta_cache(&fx);
            std::fs::remove_dir_all(fx.projects.join("alpha")).unwrap();
            let missing = FleetService::new(&fx.store)
                .plan_bootstrap(&["alpha".to_string()])
                .unwrap();
            assert_eq!(
                missing.items[0].reason_code.as_deref(),
                Some("hub_branch_missing")
            );
        }
    }

    #[test]
    fn bootstrap_never_creates_the_reserved_origin_remote() {
        let fx = bootstrap_fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest
                .replace("[hub.test]", "[hub.origin]")
                .replace("hub = \"test\"", "hub = \"origin\"")
        });
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target).unwrap();

        let plan = FleetService::new(&fx.store)
            .plan_bootstrap(&["alpha".to_string()])
            .unwrap();

        assert!(!plan.ok);
        assert_eq!(
            plan.items[0].reason_code.as_deref(),
            Some("hub_name_reserved")
        );
        assert!(!target.exists());
    }

    #[test]
    fn bootstrap_apply_conflicts_on_machine_manifest_path_target_or_hub_drift() {
        assert_bootstrap_conflict(|fx| {
            fx.store.set_setting(MACHINE_ID_KEY, "third").unwrap();
        });
        assert_bootstrap_conflict(|fx| {
            update_meta_manifest(fx, |manifest| {
                manifest.replace("authority = \"selfie\"", "authority = \"shared\"")
            });
        });
        assert_bootstrap_conflict(|fx| {
            std::fs::create_dir_all(fx.projects.join("alpha")).unwrap();
        });
        assert_bootstrap_conflict(|fx| {
            advance_hub(fx, "hub moved after preview");
        });

        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        std::fs::remove_dir_all(fx.projects.join("alpha")).unwrap();
        let service = FleetService::new(&fx.store);
        let mut plan = service.plan_bootstrap(&["alpha".to_string()]).unwrap();
        plan.items[0].evidence.as_mut().unwrap().target_path = fx
            .projects
            .parent()
            .unwrap()
            .join("escape")
            .to_string_lossy()
            .into_owned();
        let outcome = service.apply_bootstrap(&plan).unwrap();
        assert_eq!(outcome.items[0].action, "conflict");
        assert!(!fx.projects.join("alpha").exists());
    }

    #[test]
    fn bootstrap_apply_requires_fresh_manifest_and_uses_fleet_lock() {
        let fx = bootstrap_fixture();
        seed_meta_cache(&fx);
        let target = fx.projects.join("alpha");
        std::fs::remove_dir_all(&target).unwrap();
        let service = FleetService::new(&fx.store);
        let plan = service.plan_bootstrap(&["alpha".to_string()]).unwrap();
        let unavailable = fx.projects.parent().unwrap().join("meta-unavailable.git");
        std::fs::rename(&fx.meta_bare, &unavailable).unwrap();
        let error = service.apply_bootstrap(&plan).unwrap_err();
        assert!(error.message.contains("fresh fleet metadata"));
        assert!(!target.exists());
        assert!(fx.store.list_audit(None).unwrap().is_empty());

        let _held = FleetLock::acquire(Duration::ZERO).unwrap();
        let error = FleetLock::acquire(Duration::ZERO)
            .err()
            .expect("a concurrent bootstrap apply must not acquire fleet.lock");
        assert!(error.message.contains("fleet.lock busy"));
    }

    #[test]
    fn manifest_get_preview_update_round_trip_adds_discovered_repo_to_status() {
        let fx = fixture();
        seed_work_repo(&fx.projects.join("stray"));
        let service = FleetService::new(&fx.store);
        let discovered = service.discover().unwrap();
        assert_eq!(discovered.unlisted[0].name, "stray");

        let snapshot = service.manifest_get().unwrap();
        assert!(snapshot.known_machines.contains(&"selfie".to_string()));
        assert!(snapshot.known_machines.contains(&"other".to_string()));
        let mut repos = snapshot.manifest.repos.clone();
        repos.push(manifest::RepoEntry {
            name: discovered.unlisted[0].name.clone(),
            hub: "test".into(),
            authority: "selfie".into(),
            branch: "main".into(),
            auto_sync: false,
        });

        let plan = service.plan_manifest_update(&snapshot, repos).unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].action, "add");
        assert_eq!(plan.changes[0].repo, "stray");
        let exact_plan: FleetManifestUpdatePlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();

        let outcome = service.apply_manifest_update(&exact_plan).unwrap();

        assert!(outcome.ok);
        assert_eq!(outcome.action, "updated");
        assert!(outcome.pushed);
        let status = service.status().unwrap();
        let names: Vec<_> = status.repos.iter().map(|repo| repo.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "stray"]);
    }

    #[test]
    fn manifest_remove_only_changes_the_manifest_and_keeps_the_checkout() {
        let fx = fixture();
        let repo_path = fx.projects.join("alpha");
        let before_head = git_stdout(&repo_path, &["rev-parse", "HEAD"]);
        let before_file = std::fs::read(repo_path.join("file.txt")).unwrap();
        let service = FleetService::new(&fx.store);
        let snapshot = service.manifest_get().unwrap();

        let plan = service.plan_manifest_update(&snapshot, vec![]).unwrap();
        assert_eq!(plan.changes[0].action, "remove");
        let outcome = service.apply_manifest_update(&plan).unwrap();

        assert!(outcome.ok);
        assert!(repo_path.join(".git").is_dir());
        assert_eq!(git_stdout(&repo_path, &["rev-parse", "HEAD"]), before_head);
        assert_eq!(
            std::fs::read(repo_path.join("file.txt")).unwrap(),
            before_file
        );
        assert!(service.status().unwrap().repos.is_empty());
    }

    #[test]
    fn manifest_preview_rejects_unsafe_or_incomplete_repo_entries() {
        let fx = fixture();
        let service = FleetService::new(&fx.store);
        let snapshot = service.manifest_get().unwrap();
        let valid = manifest::RepoEntry {
            name: "beta".into(),
            hub: "test".into(),
            authority: "selfie".into(),
            branch: "main".into(),
            auto_sync: false,
        };
        let cases = [
            (
                manifest::RepoEntry {
                    name: "../evil".into(),
                    ..valid.clone()
                },
                "unsafe repo name",
            ),
            (
                manifest::RepoEntry {
                    hub: "missing".into(),
                    ..valid.clone()
                },
                "undefined hub",
            ),
            (
                manifest::RepoEntry {
                    hub: "   ".into(),
                    ..valid.clone()
                },
                "non-empty hub",
            ),
            (
                manifest::RepoEntry {
                    authority: "   ".into(),
                    ..valid.clone()
                },
                "non-empty hub",
            ),
            (
                manifest::RepoEntry {
                    branch: "   ".into(),
                    ..valid.clone()
                },
                "non-empty hub",
            ),
            (
                manifest::RepoEntry {
                    authority: "unknown-machine".into(),
                    ..valid
                },
                "unknown authority",
            ),
        ];

        for (repo, expected) in cases {
            let error = service
                .plan_manifest_update(&snapshot, vec![repo])
                .unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn manifest_apply_conflicts_on_machine_manifest_or_remote_head_drift() {
        let planned_add = |service: &FleetService<'_>| {
            let snapshot = service.manifest_get().unwrap();
            let mut repos = snapshot.manifest.repos.clone();
            repos.push(manifest::RepoEntry {
                name: "beta".into(),
                hub: "test".into(),
                authority: "selfie".into(),
                branch: "main".into(),
                auto_sync: false,
            });
            service.plan_manifest_update(&snapshot, repos).unwrap()
        };

        {
            let fx = fixture();
            let service = FleetService::new(&fx.store);
            let plan = planned_add(&service);
            fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
            let outcome = service.apply_manifest_update(&plan).unwrap();
            assert!(!outcome.ok);
            assert_eq!(outcome.action, "conflict");
        }

        {
            let fx = fixture();
            let service = FleetService::new(&fx.store);
            let plan = planned_add(&service);
            update_meta_manifest(&fx, |text| format!("{text}\n# concurrent manifest edit\n"));
            let outcome = service.apply_manifest_update(&plan).unwrap();
            assert!(!outcome.ok);
            assert_eq!(outcome.action, "conflict");
        }

        {
            let fx = fixture();
            let service = FleetService::new(&fx.store);
            let plan = planned_add(&service);
            let racer = MetaRepo::at(
                fx.meta_bare.to_str().unwrap(),
                fx.projects.parent().unwrap().join("report-racer/meta"),
            );
            racer
                .write_report(&MachineReport {
                    machine: "racer".into(),
                    display_name: None,
                    reported_at: "2026-07-18T05:00:00Z".into(),
                    repos: vec![],
                })
                .unwrap();
            let outcome = service.apply_manifest_update(&plan).unwrap();
            assert!(!outcome.ok);
            assert_eq!(outcome.action, "conflict");
            assert!(outcome.message.unwrap().contains("remote metadata"));
        }
    }

    #[test]
    fn manifest_and_report_commits_are_distinguishable() {
        let fx = fixture();
        let service = FleetService::new(&fx.store);
        service.apply_report().unwrap();
        let snapshot = service.manifest_get().unwrap();
        let mut repos = snapshot.manifest.repos.clone();
        repos.push(manifest::RepoEntry {
            name: "beta".into(),
            hub: "test".into(),
            authority: "selfie".into(),
            branch: "main".into(),
            auto_sync: false,
        });
        let plan = service.plan_manifest_update(&snapshot, repos).unwrap();
        service.apply_manifest_update(&plan).unwrap();

        let subjects = Command::new("git")
            .arg("--git-dir")
            .arg(&fx.meta_bare)
            .args(["log", "-2", "--format=%s"])
            .output()
            .unwrap();
        let subjects = String::from_utf8(subjects.stdout).unwrap();
        let mut lines = subjects.lines();
        assert_eq!(
            lines.next(),
            Some("fleet manifest: update managed repositories")
        );
        assert!(lines.next().unwrap().starts_with("fleet report: selfie "));
    }

    #[test]
    fn pull_preview_json_round_trip_then_apply_fast_forwards_and_audits() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        let alpha = fx.projects.join("alpha");
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let before = git_stdout(&alpha, &["rev-parse", "HEAD"]);

        let publisher = fx.projects.parent().unwrap().join("publisher");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&hub)
            .arg(&publisher)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(publisher.join("file.txt"), "from hub").unwrap();
        git(&publisher, &["add", "-A"]);
        git(&publisher, &["commit", "-m", "hub update"]);
        git(&publisher, &["push", "origin", "main"]);
        let target = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);

        let service = FleetService::new(&fx.store);
        let plan = service.plan_pull(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);
        assert_eq!(plan.items[0].status, "ready");
        assert_eq!(plan.items[0].evidence.as_ref().unwrap().target_oid, target);
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            std::fs::read_to_string(alpha.join("file.txt")).unwrap(),
            "base"
        );
        assert!(fx.store.list_audit(None).unwrap().is_empty());

        let exact_plan: FleetPullPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        let outcome = service.apply_pull(&exact_plan).unwrap();
        assert!(outcome.ok, "outcome: {:?}", outcome.items);
        assert_eq!(outcome.items[0].action, "pulled");
        assert_eq!(
            outcome.items[0].before_head.as_deref(),
            Some(before.as_str())
        );
        assert_eq!(
            outcome.items[0].after_head.as_deref(),
            Some(target.as_str())
        );
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), target);
        assert_eq!(
            std::fs::read_to_string(alpha.join("file.txt")).unwrap(),
            "from hub"
        );

        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "fleet_pull");
        assert!(audit[0].success);
        let detail = audit[0].detail.as_deref().unwrap();
        assert!(detail.contains("repo=alpha"));
        assert!(detail.contains(&format!("before={before}")));
        assert!(detail.contains(&format!("after={target}")));
    }

    #[test]
    fn pull_preview_does_not_write_repo_settings_meta_cache_or_audit() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "preview target");
        let alpha = fx.projects.join("alpha");
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(alpha.join("file.txt"), "base").unwrap();
        let index_before = file_stamp(&alpha.join(".git/index")).unwrap();
        let project_fetch_before = file_stamp(&alpha.join(".git/FETCH_HEAD"));
        let meta_fetch = central_repo::base_dir().join("fleet/meta/.git/FETCH_HEAD");
        let meta_fetch_before = file_stamp(&meta_fetch);
        let setting_before = fx.store.get_setting(MACHINE_ID_KEY).unwrap();

        let plan = FleetService::new(&fx.store)
            .plan_pull(&["alpha".to_string()])
            .unwrap();

        assert!(plan.ok);
        assert_eq!(file_stamp(&alpha.join(".git/index")).unwrap(), index_before);
        assert_eq!(
            file_stamp(&alpha.join(".git/FETCH_HEAD")),
            project_fetch_before
        );
        assert_eq!(file_stamp(&meta_fetch), meta_fetch_before);
        assert_eq!(
            fx.store.get_setting(MACHINE_ID_KEY).unwrap(),
            setting_before
        );
        assert!(fx.store.list_audit(None).unwrap().is_empty());
    }

    #[test]
    fn pull_preview_refuses_dirty_repo_with_stable_code() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "target");
        std::fs::write(fx.projects.join("alpha/untracked.txt"), "dirty").unwrap();

        let plan = FleetService::new(&fx.store)
            .plan_pull(&["alpha".to_string()])
            .unwrap();

        assert!(!plan.ok);
        assert_eq!(plan.items[0].reason_code.as_deref(), Some("repo_dirty"));
    }

    #[test]
    fn pull_preview_refuses_detached_repo_with_stable_code() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "target");
        git(
            &fx.projects.join("alpha"),
            &["checkout", "--detach", "HEAD"],
        );

        let plan = FleetService::new(&fx.store)
            .plan_pull(&["alpha".to_string()])
            .unwrap();

        assert!(!plan.ok);
        assert_eq!(plan.items[0].reason_code.as_deref(), Some("detached_head"));
    }

    #[test]
    fn pull_preview_refuses_authority_machine_with_stable_code() {
        let fx = fixture();
        seed_meta_cache(&fx);
        advance_hub(&fx, "target");

        let plan = FleetService::new(&fx.store)
            .plan_pull(&["alpha".to_string()])
            .unwrap();

        assert!(!plan.ok);
        assert_eq!(
            plan.items[0].reason_code.as_deref(),
            Some("authority_self_pull")
        );
    }

    #[test]
    fn pull_apply_refuses_diverged_history_without_moving_head() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "hub side");
        let alpha = fx.projects.join("alpha");
        std::fs::write(alpha.join("local.txt"), "local side").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "local side"]);
        let before = git_stdout(&alpha, &["rev-parse", "HEAD"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_pull(&["alpha".to_string()]).unwrap();
        assert!(plan.ok, "target is intentionally unknown until apply fetch");

        let outcome = service.apply_pull(&plan).unwrap();

        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].reason_code.as_deref(), Some("diverged"));
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn shared_repo_can_pull_from_the_manifest_hub() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("authority = \"selfie\"", "authority = \"shared\"")
        });
        seed_meta_cache(&fx);
        let target = advance_hub(&fx, "shared target");
        let service = FleetService::new(&fx.store);

        let plan = service.plan_pull(&["alpha".to_string()]).unwrap();
        let outcome = service.apply_pull(&plan).unwrap();

        assert!(outcome.ok);
        assert_eq!(
            git_stdout(&fx.projects.join("alpha"), &["rev-parse", "HEAD"]),
            target
        );
    }

    #[test]
    fn pull_apply_conflicts_on_head_branch_remote_machine_manifest_or_hub_drift() {
        assert_pull_conflict(|fx| {
            let alpha = fx.projects.join("alpha");
            std::fs::write(alpha.join("local.txt"), "new local head").unwrap();
            git(&alpha, &["add", "-A"]);
            git(&alpha, &["commit", "-m", "racing local commit"]);
        });
        assert_pull_conflict(|fx| {
            git(&fx.projects.join("alpha"), &["checkout", "-b", "feature"]);
        });
        assert_pull_conflict(|fx| {
            git(
                &fx.projects.join("alpha"),
                &["remote", "add", "test", "/tmp/changed-hub.git"],
            );
        });
        assert_pull_conflict(|fx| {
            fx.store.set_setting(MACHINE_ID_KEY, "third").unwrap();
        });
        assert_pull_conflict(|fx| {
            update_meta_manifest(fx, |manifest| {
                manifest.replace("authority = \"selfie\"", "authority = \"shared\"")
            });
        });
        assert_pull_conflict(|fx| {
            advance_hub(fx, "later hub target");
        });
    }

    #[test]
    fn pull_refuses_unlisted_and_symlink_escape_repositories() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        let service = FleetService::new(&fx.store);
        let unknown = service.plan_pull(&["rogue".to_string()]).unwrap();
        assert_eq!(
            unknown.items[0].reason_code.as_deref(),
            Some("repo_not_in_manifest")
        );

        {
            let alpha = fx.projects.join("alpha");
            let outside = fx.projects.parent().unwrap().join("outside-alpha-pull");
            std::fs::rename(&alpha, &outside).unwrap();
            crate::core::test_support::symlink_dir(&outside, &alpha).unwrap();
            let escaped = service.plan_pull(&["alpha".to_string()]).unwrap();
            assert_eq!(
                escaped.items[0].reason_code.as_deref(),
                Some("path_outside_projects_root")
            );
        }
    }

    #[test]
    fn pull_apply_requires_fresh_meta_manifest_before_fetch_or_checkout() {
        let fx = fixture();
        seed_meta_cache(&fx);
        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        advance_hub(&fx, "freshness target");
        let alpha = fx.projects.join("alpha");
        let before = git_stdout(&alpha, &["rev-parse", "HEAD"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_pull(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);
        let unavailable = fx
            .projects
            .parent()
            .unwrap()
            .join("meta-unavailable-pull.git");
        std::fs::rename(&fx.meta_bare, &unavailable).unwrap();

        let error = service
            .apply_pull(&plan)
            .err()
            .expect("pull apply must not trust a stale manifest cache");

        assert!(error.message.contains("fresh fleet metadata"));
        assert_eq!(git_stdout(&alpha, &["rev-parse", "HEAD"]), before);
        assert!(fx.store.list_audit(None).unwrap().is_empty());
    }

    #[test]
    fn push_preview_then_apply_advances_hub_and_records_audit() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);
        let bare_head_before = git_stdout(&hub, &["symbolic-ref", "HEAD"]);

        std::fs::write(alpha.join("file.txt"), "committed v2").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "v2"]);
        let local_head = git_stdout(&alpha, &["rev-parse", "HEAD"]);

        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);
        assert_eq!(plan.items[0].status, "ready");
        assert_eq!(
            plan.items[0].evidence.as_ref().unwrap().head_oid,
            local_head
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            hub_before,
            "preview must not advance the hub"
        );
        assert!(fx.store.list_audit(None).unwrap().is_empty());

        // The GUI crosses a JSON/Tauri boundary between preview and apply.
        let plan: FleetPushPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        let outcome = service.apply_push(&plan).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.items[0].action, "pushed");
        assert_eq!(
            outcome.items[0].before_head.as_deref(),
            Some(hub_before.as_str())
        );
        assert_eq!(
            outcome.items[0].after_head.as_deref(),
            Some(local_head.as_str())
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            local_head
        );
        assert_eq!(
            git_stdout(&hub, &["symbolic-ref", "HEAD"]),
            bare_head_before
        );

        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "fleet_push");
        assert!(audit[0].success);
        let detail = audit[0].detail.as_deref().unwrap();
        assert!(detail.contains("repo=alpha"));
        assert!(detail.contains(&format!("before={hub_before}")));
        assert!(detail.contains(&format!("after={local_head}")));
    }

    #[test]
    fn push_preview_does_not_refresh_cache_index_or_seed_machine_id() {
        let fx = fixture();
        let cache = central_repo::base_dir().join("fleet/meta");
        let meta = MetaRepo::at(fx.meta_bare.to_str().unwrap(), cache.clone());
        meta.ensure_fresh().unwrap();

        // Make the worktree file stat differ from the index without changing
        // content. Plain `git status` may refresh the index unless optional
        // locks are disabled for preview reads.
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(fx.projects.join("alpha/file.txt"), "base").unwrap();
        let index = fx.projects.join("alpha/.git/index");
        let index_before = file_stamp(&index).unwrap();
        let fetch_head = cache.join(".git/FETCH_HEAD");
        let fetch_before = file_stamp(&fetch_head);

        let plan = FleetService::new(&fx.store)
            .plan_push(&["alpha".to_string()])
            .unwrap();
        assert!(plan.ok);
        assert_eq!(file_stamp(&index).unwrap(), index_before);
        assert_eq!(file_stamp(&fetch_head), fetch_before);

        // An absent/empty id is derived for preview but must not be persisted.
        fx.store.set_setting(MACHINE_ID_KEY, "").unwrap();
        let before_setting = fx.store.get_setting(MACHINE_ID_KEY).unwrap();
        let _ = FleetService::new(&fx.store)
            .plan_push(&["alpha".to_string()])
            .unwrap();
        assert_eq!(
            fx.store.get_setting(MACHINE_ID_KEY).unwrap(),
            before_setting
        );
    }

    #[test]
    fn push_apply_conflicts_and_audits_when_head_changes_after_preview() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);

        std::fs::write(alpha.join("file.txt"), "changed after preview").unwrap();
        git(&alpha, &["add", "-A"]);
        git(&alpha, &["commit", "-m", "racing commit"]);

        let outcome = service.apply_push(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );

        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "fleet_push");
        assert!(!audit[0].success);
        assert!(audit[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("result=conflict"));
    }

    #[test]
    fn push_apply_conflicts_when_machine_changes_after_shared_preview() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("authority = \"selfie\"", "authority = \"shared\"")
        });
        seed_meta_cache(&fx);
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);

        fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        let outcome = service.apply_push(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "conflict");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("plan_conflict")
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );
    }

    #[test]
    fn push_preview_refuses_dirty_detached_and_non_authority_with_stable_codes() {
        {
            let fx = fixture();
            seed_meta_cache(&fx);
            std::fs::write(fx.projects.join("alpha/untracked.txt"), "dirty").unwrap();
            let plan = FleetService::new(&fx.store)
                .plan_push(&["alpha".to_string()])
                .unwrap();
            assert!(!plan.ok);
            assert_eq!(plan.items[0].status, "refused");
            assert_eq!(plan.items[0].reason_code.as_deref(), Some("repo_dirty"));
        }

        {
            let fx = fixture();
            seed_meta_cache(&fx);
            git(
                &fx.projects.join("alpha"),
                &["checkout", "--detach", "HEAD"],
            );
            let plan = FleetService::new(&fx.store)
                .plan_push(&["alpha".to_string()])
                .unwrap();
            assert!(!plan.ok);
            assert_eq!(plan.items[0].status, "refused");
            assert_eq!(plan.items[0].reason_code.as_deref(), Some("detached_head"));
        }

        {
            let fx = fixture();
            seed_meta_cache(&fx);
            fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
            let plan = FleetService::new(&fx.store)
                .plan_push(&["alpha".to_string()])
                .unwrap();
            assert!(!plan.ok);
            assert_eq!(plan.items[0].status, "refused");
            assert_eq!(plan.items[0].reason_code.as_deref(), Some("not_authority"));
        }
    }

    #[test]
    fn push_apply_conflicts_when_dirty_branch_or_remote_url_drifts() {
        assert_push_conflict(|fx| {
            std::fs::write(fx.projects.join("alpha/untracked.txt"), "dirty after plan").unwrap();
        });

        assert_push_conflict(|fx| {
            git(&fx.projects.join("alpha"), &["checkout", "-b", "feature"]);
        });

        assert_push_conflict(|fx| {
            let old_base = fx.projects.parent().unwrap().join("mirrors");
            let new_base = fx.projects.parent().unwrap().join("other-mirrors");
            update_meta_manifest(fx, |text| {
                text.replace(
                    &old_base.to_string_lossy().into_owned(),
                    &new_base.to_string_lossy(),
                )
            });
        });
    }

    #[test]
    fn push_refuses_unlisted_and_symlink_escape_paths_without_touching_hub() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);
        seed_work_repo(&fx.projects.join("rogue"));

        let unknown = FleetService::new(&fx.store)
            .plan_push(&["rogue".to_string()])
            .unwrap();
        assert!(!unknown.ok);
        assert_eq!(
            unknown.items[0].reason_code.as_deref(),
            Some("repo_not_in_manifest")
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );

        {
            let alpha = fx.projects.join("alpha");
            let outside = fx.projects.parent().unwrap().join("outside-alpha");
            std::fs::rename(&alpha, &outside).unwrap();
            crate::core::test_support::symlink_dir(&outside, &alpha).unwrap();
            let escaped = FleetService::new(&fx.store)
                .plan_push(&["alpha".to_string()])
                .unwrap();
            assert!(!escaped.ok);
            assert_eq!(
                escaped.items[0].reason_code.as_deref(),
                Some("path_outside_projects_root")
            );
            assert_eq!(
                git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
                hub_before
            );
        }
    }

    #[test]
    fn push_never_rewinds_a_hub_that_advanced_after_preview() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);

        let racer = fx.projects.parent().unwrap().join("racer");
        let out = Command::new("git")
            .args(["clone"])
            .arg(&hub)
            .arg(&racer)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(racer.join("file.txt"), "hub advanced elsewhere").unwrap();
        git(&racer, &["add", "-A"]);
        git(&racer, &["commit", "-m", "racing hub commit"]);
        git(&racer, &["push", "origin", "main"]);
        let raced_head = git_stdout(&hub, &["rev-parse", "refs/heads/main"]);

        let outcome = service.apply_push(&plan).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.items[0].action, "error");
        assert_eq!(
            outcome.items[0].reason_code.as_deref(),
            Some("non_fast_forward")
        );
        assert_eq!(
            git_stdout(&hub, &["rev-parse", "refs/heads/main"]),
            raced_head
        );
        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert!(!audit[0].success);
    }

    #[test]
    fn fleet_lock_rejects_a_concurrent_apply() {
        let _fx = fixture();
        let _first = FleetLock::acquire(Duration::ZERO).unwrap();
        let error = FleetLock::acquire(Duration::ZERO)
            .err()
            .expect("a second fleet apply must not acquire the same lock");
        assert!(error.message.contains("fleet.lock busy"));
    }

    #[test]
    fn push_apply_requires_a_fresh_meta_manifest() {
        let fx = fixture();
        seed_meta_cache(&fx);
        let project_hub = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let hub_before = git_stdout(&project_hub, &["rev-parse", "refs/heads/main"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_push(&["alpha".to_string()]).unwrap();
        assert!(plan.ok);

        let unavailable = fx.projects.parent().unwrap().join("meta-unavailable.git");
        std::fs::rename(&fx.meta_bare, &unavailable).unwrap();
        let error = service
            .apply_push(&plan)
            .err()
            .expect("apply must not trust a stale meta cache");
        assert!(error.message.contains("fresh fleet metadata"));
        assert_eq!(
            git_stdout(&project_hub, &["rev-parse", "refs/heads/main"]),
            hub_before
        );
        assert!(fx.store.list_audit(None).unwrap().is_empty());
    }

    #[test]
    fn discover_lists_repos_outside_the_manifest() {
        let fx = fixture();
        seed_work_repo(&fx.projects.join("stray"));
        let service = FleetService::new(&fx.store);
        let discovery = service.discover().unwrap();
        let names: Vec<_> = discovery.unlisted.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["stray"]);
    }

    #[test]
    fn init_preview_then_apply_creates_local_bare_and_converges_remote_without_origin_change() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        let mirror = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        std::fs::remove_dir_all(&mirror).unwrap();
        let alpha = fx.projects.join("alpha");
        git(
            &alpha,
            &[
                "remote",
                "add",
                "origin",
                "git@example.invalid:team/alpha.git",
            ],
        );
        git(&alpha, &["remote", "add", "test", "gamma:/stale/alpha.git"]);
        let origin_before = git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]);
        seed_meta_cache(&fx);

        let service = FleetService::new(&fx.store);
        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        assert!(plan.ok, "plan: {:?}", plan.items);
        assert_eq!(plan.items[0].status, "ready");
        assert_eq!(plan.items[0].mirror_action, "create");
        assert_eq!(plan.items[0].remote_action, "set_url");
        let evidence = plan.items[0].evidence.as_ref().unwrap();
        assert_eq!(evidence.host_machine, "selfie");
        assert_eq!(evidence.mirror_exists, Some(false));
        assert!(evidence.remote_exists);
        assert_eq!(
            evidence.current_remote_url.as_deref(),
            Some("gamma:/stale/alpha.git")
        );
        assert_eq!(evidence.target_remote_url, mirror.to_string_lossy());
        assert!(!mirror.exists(), "preview must not create the mirror");
        assert_eq!(
            git_stdout(&alpha, &["remote", "get-url", "test"]),
            "gamma:/stale/alpha.git",
            "preview must not rewrite the remote"
        );
        assert_eq!(
            git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]),
            origin_before
        );
        assert!(fx.store.list_audit(None).unwrap().is_empty());

        let exact_plan: FleetInitPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        let outcome = service.apply_init(&exact_plan).unwrap();

        assert!(outcome.ok, "outcome: {:?}", outcome.items);
        assert_eq!(outcome.items[0].action, "applied");
        assert_eq!(outcome.items[0].mirror_action, "created");
        assert_eq!(outcome.items[0].remote_action, "set_url");
        assert_eq!(
            git_stdout(&mirror, &["rev-parse", "--is-bare-repository"]),
            "true"
        );
        assert_eq!(
            git_stdout(&alpha, &["remote", "get-url", "test"]),
            mirror.to_string_lossy()
        );
        assert_eq!(
            git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]),
            origin_before,
            "fleet init must not change origin config bytes"
        );
        let audit = fx.store.list_audit(None).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "fleet_init");
        assert!(audit[0].success);
        let detail = audit[0].detail.as_deref().unwrap();
        assert!(detail.contains("before_head="));
        assert!(detail.contains("after_head="));
        assert!(detail.contains("before_remote=gamma:/stale/alpha.git"));
        assert!(detail.contains(&format!("after_remote={}", mirror.display())));
    }

    #[test]
    fn init_recreates_a_missing_local_meta_bare_from_the_previewed_cache() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        std::fs::remove_dir_all(&fx.meta_bare).unwrap();
        let service = FleetService::new(&fx.store);

        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        assert_eq!(plan.meta_repo.action, "create");
        assert_eq!(plan.meta_repo.exists, Some(false));
        assert!(!fx.meta_bare.exists(), "preview must not recreate meta");

        let outcome = service.apply_init(&plan).unwrap();

        assert!(outcome.ok, "outcome: {:?}", outcome.items);
        assert_eq!(outcome.meta_repo_action, "created");
        assert_eq!(
            git_stdout(&fx.meta_bare, &["rev-parse", "--is-bare-repository"]),
            "true"
        );
        let manifest = git_stdout(&fx.meta_bare, &["show", "main:manifest.toml"]);
        assert!(manifest.contains("host_machine = \"selfie\""));
        let meta_head = git_stdout(&fx.meta_bare, &["rev-parse", "HEAD"]);
        let audit = fx.store.list_audit(None).unwrap();
        assert!(audit.iter().any(|entry| {
            entry.action == "fleet_init"
                && entry.success
                && entry.detail.as_deref().is_some_and(|detail| {
                    detail.contains("repo=_patchbay-fleet")
                        && detail.contains("before_head=missing")
                        && detail.contains(&format!("after_head={meta_head}"))
                })
        }));
    }

    #[test]
    fn init_audits_a_meta_repo_initialization_failure() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        std::fs::remove_dir_all(&fx.meta_bare).unwrap();
        let cache = central_repo::base_dir().join("fleet/meta");
        git(&cache, &["checkout", "--detach"]);
        let service = FleetService::new(&fx.store);
        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        let error = service.apply_init(&plan).unwrap_err();

        assert!(error.message.contains("not a symbolic ref"), "{error:?}");
        assert!(
            !fx.meta_bare.exists(),
            "failed init must not publish a partial target"
        );
        let audit = fx.store.list_audit(None).unwrap();
        assert!(audit.iter().any(|entry| {
            entry.action == "fleet_init"
                && !entry.success
                && entry.detail.as_deref().is_some_and(|detail| {
                    detail.contains("repo=_patchbay-fleet")
                        && detail.contains("before_head=missing")
                        && detail.contains("after_head=missing")
                        && detail.contains("result=error")
                })
        }));
    }

    #[test]
    fn init_adds_a_missing_hub_remote_and_preserves_origin() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        git(
            &alpha,
            &[
                "remote",
                "add",
                "origin",
                "git@example.invalid:team/alpha.git",
            ],
        );
        let origin_before = git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]);
        let target = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        let service = FleetService::new(&fx.store);

        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        assert!(plan.ok);
        assert_eq!(plan.items[0].mirror_action, "no_op");
        assert_eq!(plan.items[0].remote_action, "add");
        assert!(!plan.items[0].evidence.as_ref().unwrap().remote_exists);

        let outcome = service.apply_init(&plan).unwrap();

        assert!(outcome.ok);
        assert_eq!(outcome.items[0].remote_action, "add");
        assert_eq!(
            git_stdout(&alpha, &["remote", "get-url", "test"]),
            target.to_string_lossy()
        );
        assert_eq!(
            git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]),
            origin_before
        );
    }

    #[test]
    fn init_is_a_no_op_when_mirror_and_remote_already_match() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        let target = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        git(&alpha, &["remote", "add", "test", target.to_str().unwrap()]);
        let config_before = std::fs::read(alpha.join(".git/config")).unwrap();
        let service = FleetService::new(&fx.store);

        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        assert!(plan.ok);
        assert_eq!(plan.items[0].status, "no_op");
        assert_eq!(plan.items[0].mirror_action, "no_op");
        assert_eq!(plan.items[0].remote_action, "no_op");
        let outcome = service.apply_init(&plan).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.items[0].action, "no_op");
        assert_eq!(
            std::fs::read(alpha.join(".git/config")).unwrap(),
            config_before,
            "no-op init must leave the entire remote config byte-identical"
        );
    }

    #[test]
    fn init_on_a_non_host_guides_mirror_creation_but_converges_the_local_remote() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"hub-machine\"")
        });
        seed_meta_cache(&fx);
        let alpha = fx.projects.join("alpha");
        let mirror = fx.projects.parent().unwrap().join("mirrors/alpha.git");
        git(&alpha, &["remote", "add", "test", "gamma:/stale/alpha.git"]);
        std::fs::remove_dir_all(&mirror).unwrap();
        let service = FleetService::new(&fx.store);

        let plan = service.plan_init(&["alpha".to_string()]).unwrap();

        assert!(plan.ok);
        assert_eq!(plan.items[0].status, "ready");
        assert_eq!(plan.items[0].reason_code, None);
        assert_eq!(plan.items[0].mirror_action, "guide");
        assert_eq!(plan.items[0].remote_action, "set_url");
        assert_eq!(plan.items[0].evidence.as_ref().unwrap().mirror_exists, None);
        assert_eq!(
            plan.items[0].message.as_deref(),
            Some("mirror initialization is host-only; run fleet init on hub-machine")
        );

        let outcome = service.apply_init(&plan).unwrap();

        assert!(outcome.ok);
        assert_eq!(outcome.items[0].action, "applied");
        assert_eq!(outcome.items[0].mirror_action, "guide");
        assert_eq!(outcome.items[0].remote_action, "set_url");
        assert!(!mirror.exists());
        assert_eq!(
            git_stdout(&alpha, &["remote", "get-url", "test"]),
            mirror.to_string_lossy()
        );
        assert!(!alpha.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn init_preview_refuses_dirty_and_detached_worktrees() {
        let dirty = fixture();
        update_meta_manifest(&dirty, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&dirty);
        std::fs::write(dirty.projects.join("alpha/dirty.txt"), "dirty").unwrap();
        let plan = FleetService::new(&dirty.store)
            .plan_init(&["alpha".to_string()])
            .unwrap();
        assert_eq!(plan.items[0].status, "refused");
        assert_eq!(plan.items[0].reason_code.as_deref(), Some("dirty_worktree"));
        drop(dirty);

        let detached = fixture();
        update_meta_manifest(&detached, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&detached);
        git(&detached.projects.join("alpha"), &["checkout", "--detach"]);
        let plan = FleetService::new(&detached.store)
            .plan_init(&["alpha".to_string()])
            .unwrap();
        assert_eq!(plan.items[0].status, "refused");
        assert_eq!(plan.items[0].reason_code.as_deref(), Some("detached_head"));
    }

    #[test]
    fn init_apply_conflicts_when_clean_worktree_evidence_drifts() {
        assert_init_conflict(|fx| {
            std::fs::write(fx.projects.join("alpha/dirty.txt"), "dirty").unwrap();
        });
        assert_init_conflict(|fx| {
            git(&fx.projects.join("alpha"), &["checkout", "--detach"]);
        });
    }

    #[test]
    fn init_refuses_a_meta_target_outside_the_declared_local_hub() {
        let fx = fixture();
        update_meta_manifest(&fx, |manifest| {
            manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
        });
        seed_meta_cache(&fx);
        let outside = fx
            .projects
            .parent()
            .unwrap()
            .join("outside/_patchbay-fleet.git");
        fx.store
            .set_setting(META_URL_KEY, outside.to_str().unwrap())
            .unwrap();

        let plan = FleetService::new(&fx.store)
            .plan_init(&["alpha".to_string()])
            .unwrap();

        assert_eq!(plan.meta_repo.action, "refused");
        assert_eq!(
            plan.meta_repo.reason_code.as_deref(),
            Some("meta_repo_path_outside_hub")
        );
        assert!(!outside.exists());
    }

    #[test]
    fn init_apply_conflicts_on_remote_manifest_machine_mirror_or_path_drift() {
        assert_init_conflict(|fx| {
            git(
                &fx.projects.join("alpha"),
                &["remote", "add", "test", "/tmp/racing-alpha.git"],
            );
        });
        assert_init_conflict(|fx| {
            update_meta_manifest(fx, |manifest| {
                manifest.replace("authority = \"selfie\"", "authority = \"shared\"")
            });
        });
        assert_init_conflict(|fx| {
            fx.store.set_setting(MACHINE_ID_KEY, "other").unwrap();
        });
        assert_init_conflict(|fx| {
            std::fs::remove_dir_all(fx.projects.parent().unwrap().join("mirrors/alpha.git"))
                .unwrap();
        });
        assert_init_conflict(|fx| {
            let other_projects = fx.projects.parent().unwrap().join("other-projects");
            seed_work_repo(&other_projects.join("alpha"));
            fx.store
                .set_setting(PROJECTS_ROOT_KEY, other_projects.to_str().unwrap())
                .unwrap();
        });
        assert_init_conflict(|fx| {
            let alpha = fx.projects.join("alpha");
            let outside = fx.projects.parent().unwrap().join("outside-alpha-init");
            std::fs::rename(&alpha, &outside).unwrap();
            crate::core::test_support::symlink_dir(&outside, &alpha).unwrap();
        });
    }

    #[test]
    fn init_refuses_unlisted_origin_named_and_path_escape_targets() {
        {
            let fx = fixture();
            update_meta_manifest(&fx, |manifest| {
                manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
            });
            seed_meta_cache(&fx);
            let unknown = FleetService::new(&fx.store)
                .plan_init(&["rogue".to_string()])
                .unwrap();
            assert_eq!(
                unknown.items[0].reason_code.as_deref(),
                Some("repo_not_in_manifest")
            );
        }

        {
            let fx = fixture();
            update_meta_manifest(&fx, |manifest| {
                manifest.replace("[hub.test]", "[hub.test]\nhost_machine = \"selfie\"")
            });
            seed_meta_cache(&fx);
            let alpha = fx.projects.join("alpha");
            let outside = fx.projects.parent().unwrap().join("outside-alpha-preview");
            std::fs::rename(&alpha, &outside).unwrap();
            crate::core::test_support::symlink_dir(&outside, &alpha).unwrap();
            let escaped = FleetService::new(&fx.store)
                .plan_init(&["alpha".to_string()])
                .unwrap();
            assert_eq!(
                escaped.items[0].reason_code.as_deref(),
                Some("path_outside_projects_root")
            );
        }

        {
            let fx = fixture();
            update_meta_manifest(&fx, |manifest| {
                manifest
                    .replace("[hub.test]", "[hub.origin]\nhost_machine = \"selfie\"")
                    .replace("hub = \"test\"", "hub = \"origin\"")
            });
            seed_meta_cache(&fx);
            let reserved = FleetService::new(&fx.store)
                .plan_init(&["alpha".to_string()])
                .unwrap();
            assert_eq!(
                reserved.items[0].reason_code.as_deref(),
                Some("origin_reserved")
            );
        }
    }

    #[test]
    fn machine_id_is_lazily_seeded_from_hostname() {
        let guard = central_repo::test_base_dir_lock();
        let temp = tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(temp.path().join("appdata")));
        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        let service = FleetService::new(&store);
        let id = service.machine_id().unwrap();
        assert!(!id.is_empty());
        assert_eq!(id, sanitize_machine_id(&id), "seeded id is already a slug");
        assert_eq!(
            store.get_setting(MACHINE_ID_KEY).unwrap().as_deref(),
            Some(id.as_str())
        );
        central_repo::set_test_base_dir_override(None);
        drop(guard);
    }

    #[test]
    fn sanitize_machine_id_produces_stable_slugs() {
        assert_eq!(sanitize_machine_id("Alpha"), "alpha");
        assert_eq!(sanitize_machine_id("Example Host (2)"), "example-host-2");
        assert_eq!(sanitize_machine_id("---"), "machine");
    }
}
