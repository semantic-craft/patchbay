use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::core::{
    audit_log::AuditDraft,
    central_repo,
    error::AppError,
    project_registry,
    skill_store::{ProjectRecord, SkillStore},
    tool_adapters,
};

use super::candidates;
use super::decisions::{
    self, DecisionItem, DecisionOutcome, DecisionPlan, DecisionStatus, FindingDecision,
};
use super::doctor::{self, DoctorFilter, DoctorReport};
use super::duplicates;
use super::fork_sync;
use super::journal;
use super::live;
use super::ops::{self, LinkPlan};
use super::project_links::TracedEntry;
use super::pull;
use super::remediate;
use super::repair;
use super::repo_move;
use super::resolve::{self, RepositoryStatus, SkillResolution};
use super::ChainTopology;

/// The result of applying a link plan: the per-item write report plus proof,
/// from a fresh rescan, that the requested chain is actually on disk. A caller
/// must treat `verified == false` as a failure to report success — the chain
/// was not observed even if individual writes returned `created`.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyOutcome {
    pub report: ops::LinkReport,
    /// True only when the apply was clean and a rescan observed every requested
    /// chain resolving into a warehouse repo.
    pub verified: bool,
    /// Skill names the rescan confirmed resolving into an Original Repository.
    pub observed: Vec<String>,
    /// Applied skills the rescan did NOT observe as a repo-resolving chain.
    pub missing: Vec<String>,
}

/// The result of applying an unlink plan: the per-item write report plus proof,
/// from a fresh rescan, that access was removed where intended and preserved
/// everywhere else. As with [`ApplyOutcome`], `verified == false` means the
/// rescan did not confirm the intended shape and success must not be reported.
#[derive(Debug, Clone, Serialize)]
pub struct UnlinkOutcome {
    pub report: Vec<ops::OpResult>,
    /// True only when the apply was clean (no error/skipped item) and the rescan
    /// confirms every affected Agent no longer exposes the Skill.
    pub verified: bool,
    /// Agents that still expose the Skill after the operation (retained access).
    pub still_linked: Vec<String>,
    /// Agents the Skill was confirmed removed from by the rescan.
    pub removed_from: Vec<String>,
}

/// Application-level contract for scanning and changing three-tier chains.
///
/// Tauri commands and future entry points call this service so settings,
/// filesystem behavior, structured results, and audit records stay aligned.
pub struct ChainService<'a> {
    store: &'a SkillStore,
    managed_root: PathBuf,
}

impl<'a> ChainService<'a> {
    pub fn new(store: &'a SkillStore) -> Self {
        Self {
            store,
            managed_root: central_repo::skills_dir(),
        }
    }

    #[cfg(test)]
    fn with_managed_root(store: &'a SkillStore, managed_root: PathBuf) -> Self {
        Self {
            store,
            managed_root,
        }
    }

    /// Every allowed tier-1 source: the Patchbay-managed central library plus
    /// optional developer Git checkouts configured as warehouse roots.
    fn source_roots(&self) -> Result<Vec<PathBuf>, AppError> {
        let mut roots = super::roots::warehouse_roots(self.store)?;
        roots.push(self.managed_root.clone());
        Ok(super::roots::dedupe(&roots))
    }

    pub fn scan(&self) -> Result<ChainTopology, AppError> {
        let warehouse_roots = super::roots::warehouse_roots(self.store)?;
        let projects_root = self
            .store
            .get_setting("chain_projects_root")
            .map_err(AppError::db)?
            .map(PathBuf::from)
            .unwrap_or_else(super::roots::default_projects_root);

        let project_paths = self.registered_project_paths()?;
        // Global surface paths come from the Agent adapter catalogue, so the
        // guard follows each Agent's real global skills path (including
        // OpenCode's `.config/opencode/skills`) instead of a second table.
        let adapters = tool_adapters::enabled_installed_adapters(self.store);
        Ok(super::build_topology(
            &warehouse_roots,
            &self.managed_root,
            &projects_root,
            &project_paths,
            &adapters,
        ))
    }

    /// Diagnose the current topology into filtered Doctor findings. Read-only:
    /// it reuses `scan` (one scanner, one classifier) and never mutates the
    /// filesystem or Git. The filter narrows by severity and deviation type; a
    /// default filter returns every visible finding.
    ///
    /// Persisted ignore/project-private decisions split the raw findings into a
    /// visible set and an `ignored` set (each keyed by rule + evidence
    /// fingerprint, so a materially changed chain is reconsidered). The
    /// severity/deviation filter narrows only the visible set; `ignored` is
    /// returned unfiltered so the UI's restore panel is complete. `total` keeps
    /// its established meaning — the number of *visible* findings before the
    /// filter — so the UI can still show "N of M".
    pub fn doctor(&self, filter: &DoctorFilter) -> Result<DoctorReport, AppError> {
        let topology = self.scan()?;
        let all = doctor::diagnose(&topology);
        let decisions = decisions::load(self.store)?;
        let (visible, ignored) = decisions::apply_decisions(all, &decisions);
        let total = visible.len();
        Ok(DoctorReport {
            findings: filter.apply(visible),
            ignored,
            total,
            scanned_at: topology.scanned_at,
        })
    }

    /// Preview persisted decisions for current Doctor findings. Fingerprints
    /// are resolved from a fresh scan; missing or unsupported items are reported
    /// individually so a caller can present partial failure without guessing.
    /// This phase is read-only: it writes neither decisions nor audit records.
    pub fn plan_decisions(
        &self,
        fingerprints: &[String],
        kind: &str,
    ) -> Result<DecisionPlan, AppError> {
        validate_decision_kind(kind)?;
        let report = self.doctor(&DoctorFilter::default())?;
        let stored = decisions::load(self.store)?;
        let items = fingerprints
            .iter()
            .map(|fingerprint| {
                let finding = report
                    .findings
                    .iter()
                    .chain(report.ignored.iter())
                    .find(|finding| finding.fingerprint == *fingerprint);
                let Some(finding) = finding else {
                    return DecisionItem {
                        fingerprint: fingerprint.clone(),
                        rule: None,
                        kind: kind.to_string(),
                        status: DecisionStatus::Error,
                        error_code: Some("finding_not_found".to_string()),
                        message: Some(
                            "fingerprint does not match a current Doctor finding".to_string(),
                        ),
                    };
                };
                if kind == decisions::KIND_PROJECT_PRIVATE
                    && !finding
                        .actions
                        .iter()
                        .any(|action| action == "mark_private")
                {
                    return DecisionItem {
                        fingerprint: fingerprint.clone(),
                        rule: Some(finding.rule.clone()),
                        kind: kind.to_string(),
                        status: DecisionStatus::Error,
                        error_code: Some("action_not_supported".to_string()),
                        message: Some("finding cannot be marked project-private".to_string()),
                    };
                }
                let exists = stored.iter().any(|decision| {
                    decision.rule == finding.rule
                        && decision.fingerprint == *fingerprint
                        && decision.kind == kind
                });
                DecisionItem {
                    fingerprint: fingerprint.clone(),
                    rule: Some(finding.rule.clone()),
                    kind: kind.to_string(),
                    status: if exists {
                        DecisionStatus::Noop
                    } else {
                        DecisionStatus::Persist
                    },
                    error_code: None,
                    message: None,
                }
            })
            .collect::<Vec<_>>();
        Ok(DecisionPlan {
            ok: items.iter().all(|item| !item.status.is_error()),
            kind: kind.to_string(),
            fingerprints: fingerprints.to_vec(),
            items,
            scanned_at: report.scanned_at,
        })
    }

    /// Apply a decision preview after resolving its fingerprints from a fresh
    /// Doctor scan. Valid items continue when a sibling fails, and every item
    /// records one audit row containing the target, decision, and result.
    pub fn apply_decisions(&self, plan: &DecisionPlan) -> Result<DecisionOutcome, AppError> {
        let current = self.plan_decisions(&plan.fingerprints, &plan.kind)?;
        let mut items = Vec::with_capacity(current.items.len());
        for mut item in current.items {
            if item.status == DecisionStatus::Persist {
                let rule = item.rule.as_deref().unwrap_or_default();
                match self.ignore_finding(rule, &item.fingerprint, &item.kind, None) {
                    Ok(()) => item.status = DecisionStatus::Applied,
                    Err(error) => {
                        item.status = DecisionStatus::Error;
                        item.error_code = Some("persistence_failed".to_string());
                        item.message = Some(error.message);
                    }
                }
            }

            let detail = format!(
                "kind={} rule={} fingerprint={} result={}{}",
                item.kind,
                item.rule.as_deref().unwrap_or("unknown"),
                item.fingerprint,
                item.status.as_str(),
                item.error_code
                    .as_deref()
                    .map(|code| format!(" error_code={code}"))
                    .unwrap_or_default()
            );
            let draft = AuditDraft::new("chain_decide")
                .skill(
                    item.fingerprint.clone(),
                    item.rule.clone().unwrap_or_default(),
                )
                .detail(detail);
            self.store.log_audit(if item.status.is_error() {
                draft
            } else {
                draft.ok()
            });
            items.push(item);
        }
        Ok(DecisionOutcome {
            ok: items.iter().all(|item| !item.status.is_error()),
            kind: current.kind,
            items,
            scanned_at: current.scanned_at,
        })
    }

    /// Persist a decision to hide a Doctor finding, keyed by its rule and
    /// evidence fingerprint. `kind` is `"ignored"` (a generic accept) or
    /// `"project_private"` (classify a legitimate physical Skill); any other
    /// value is rejected. Idempotent on `(rule, fingerprint)`.
    ///
    /// Touches only the settings table — it never reads, moves, or rewrites any
    /// Skill contents, so classifying or ignoring can never alter a Skill.
    pub fn ignore_finding(
        &self,
        rule: &str,
        fingerprint: &str,
        kind: &str,
        note: Option<String>,
    ) -> Result<(), AppError> {
        validate_decision_kind(kind)?;
        let decision = FindingDecision {
            rule: rule.to_string(),
            fingerprint: fingerprint.to_string(),
            kind: kind.to_string(),
            note,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        decisions::add(self.store, decision)
    }

    /// Restore a previously hidden finding by removing its decision record — the
    /// finding reappears on the next diagnose while its evidence still matches.
    /// Touches only the settings table; never any Skill contents.
    pub fn restore_finding(&self, rule: &str, fingerprint: &str) -> Result<(), AppError> {
        decisions::remove(self.store, rule, fingerprint)
    }

    /// Detect Original Repository checkouts that resolve to the same remote
    /// identity. Read-only: reuses one `scan` (one scanner) and reads each
    /// checkout's HEAD best-effort; it never deletes, merges, or mutates
    /// anything. The report groups duplicates by normalized `origin` identity
    /// with advisory-only guidance codes.
    pub fn duplicate_checkouts(&self) -> Result<duplicates::DuplicatesReport, AppError> {
        Ok(duplicates::detect(&self.scan()?))
    }

    /// Resolve a single Skill name across every tier of the current topology.
    /// Read-only: reuses `scan` (one scanner) and returns the same evidence Link
    /// Topology shows. With `project` set, only that registered project's tier-2/3
    /// references are returned; unset returns every Original and project reference.
    pub fn resolve(&self, skill: &str, project: Option<&str>) -> Result<SkillResolution, AppError> {
        Ok(resolve::resolve(&self.scan()?, skill, project))
    }

    /// The Original Repository inventory with health, projected straight from a
    /// single `scan`. Read-only; the CLI's `repository-status` contract.
    pub fn repository_status(&self) -> Result<RepositoryStatus, AppError> {
        Ok(resolve::repository_status(&self.scan()?))
    }

    /// The registered projects that make up the chain inventory: every enrolled
    /// project workspace, regardless of where it lives, de-duplicated by
    /// canonical path so aliases collapse to one while same-named projects at
    /// different paths stay distinct. Paths are returned as stored — never
    /// canonicalized — so the symlink tracer and classifier see them exactly as
    /// the link operations wrote them. Linked-agent workspaces are not project
    /// roots and are excluded.
    fn registered_project_paths(&self) -> Result<Vec<PathBuf>, AppError> {
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for record in self.store.get_all_projects().map_err(AppError::db)? {
            if record.workspace_type != "project" {
                continue;
            }
            let stored = PathBuf::from(&record.path);
            if seen.insert(project_registry::canonical_key(&stored)) {
                paths.push(stored);
            }
        }
        Ok(paths)
    }

    /// Registered projects whose aggregate entry resolves to this exact tier-1
    /// Skill. Used by central-library deletion so removing a managed original
    /// cannot silently leave broken project links behind.
    pub fn projects_referencing_source(&self, source: &Path) -> Result<Vec<PathBuf>, AppError> {
        let Some(name) = source.file_name() else {
            return Ok(Vec::new());
        };
        let source = super::link_tracer::normalize(source);
        Ok(self
            .registered_project_paths()?
            .into_iter()
            .filter(|project| {
                let entry = project.join(".agents").join("skills").join(name);
                let trace = super::link_tracer::trace(&entry);
                trace.exists
                    && super::link_tracer::normalize(Path::new(&trace.final_target)) == source
            })
            .collect())
    }

    /// Persist a directory as a registered project so it participates in the
    /// topology and survives rescans and restarts. Idempotent: enrolling an
    /// already-registered directory (by canonical path) returns the existing
    /// record without creating a duplicate.
    pub fn enrol_project(&self, path: &Path) -> Result<ProjectRecord, AppError> {
        project_registry::register_project(self.store, path, false)
    }

    /// Is `project` (by canonical identity) a registered chain project? Write
    /// operations require this so an arbitrary directory can never be written to
    /// without an explicit enrolment step.
    fn is_registered(&self, project: &Path) -> Result<bool, AppError> {
        let key = project_registry::canonical_key(project);
        Ok(self
            .registered_project_paths()?
            .iter()
            .any(|path| project_registry::canonical_key(path) == key))
    }

    /// Preview linking `originals` into `project` for the given `agents`. The
    /// project must already be a registered chain project; planning is entirely
    /// read-only. The returned plan carries every intended action, conflict, and
    /// the on-disk evidence `apply_link` re-checks.
    pub fn plan_link(
        &self,
        project: &Path,
        originals: &[PathBuf],
        agents: &[String],
    ) -> Result<LinkPlan, AppError> {
        if !self.is_registered(project)? {
            return Err(AppError::not_found(
                "project is not registered for chain management",
            ));
        }
        let source_roots = self.source_roots()?;
        ops::plan_link(project, originals, agents, &source_roots).map_err(AppError::invalid_input)
    }

    /// Apply a previewed plan. Re-validates the write boundary from scratch,
    /// refuses any target whose evidence changed since the preview, records a
    /// structured result and audit entry per item, then rescans to confirm the
    /// requested chain is actually on disk before reporting success.
    pub fn apply_link(&self, plan: &LinkPlan) -> Result<ApplyOutcome, AppError> {
        let project = PathBuf::from(&plan.project);
        if !self.is_registered(&project)? {
            return Err(AppError::not_found(
                "project is not registered for chain management",
            ));
        }
        let source_roots = self.source_roots()?;
        let report = ops::apply_link(plan, &source_roots).map_err(AppError::invalid_input)?;

        // Structured audit record per applied or refused item (skills + entries).
        let agents = plan.agents.join(",");
        for item in report.skills.iter().chain(report.entries.iter()) {
            let draft = AuditDraft::new("chain_link")
                .skill(item.path.clone(), item.name.clone())
                .tool(agents.clone())
                .detail(format!("{} -> {}", project.display(), item.action));
            self.store.log_audit(if is_success_action(&item.action) {
                draft.ok()
            } else {
                draft
            });
        }

        // Success is observed, not assumed: a fresh rescan must show every
        // applied skill resolving into an Original Repository through the chain.
        let topology = self.scan()?;
        let (verified, observed, missing) = verify_chain(&topology, plan, &report);
        Ok(ApplyOutcome {
            report,
            verified,
            observed,
            missing,
        })
    }

    /// One-shot link: enrol the folder as a registered project (the explicit
    /// approval for enrolment), then plan and apply in one call. Convenience for
    /// callers that do not need to show the plan between the two phases.
    pub fn link(
        &self,
        project: &Path,
        originals: &[PathBuf],
        agents: &[String],
    ) -> Result<ApplyOutcome, AppError> {
        self.enrol_project(project)?;
        let plan = self.plan_link(project, originals, agents)?;
        self.apply_link(&plan)
    }

    /// Preview removing a Skill from a project for the given Agents, preserving
    /// every access that must survive. Read-only: it classifies each Agent
    /// surface (per-Agent entry vs shared directory link), decides which links
    /// can be removed and whether the shared aggregate is still required, and
    /// snapshots the on-disk evidence `apply_unlink` re-checks. An empty `agents`
    /// list previews unlinking from every Agent that currently exposes the Skill.
    pub fn plan_unlink(
        &self,
        project: &Path,
        skill_name: &str,
        agents: &[String],
    ) -> Result<ops::UnlinkPlan, AppError> {
        ops::plan_unlink(project, skill_name, agents).map_err(AppError::invalid_input)
    }

    /// Apply a previewed unlink plan. Removes only validated symlinks (never a
    /// physical directory or Original), writes an audit record, then rescans and
    /// verifies that every affected Agent no longer exposes the Skill while
    /// retained access is preserved before reporting success.
    pub fn apply_unlink(&self, plan: &ops::UnlinkPlan) -> Result<UnlinkOutcome, AppError> {
        let report = ops::apply_unlink(plan).map_err(AppError::invalid_input)?;
        let all_ok = report.iter().all(|result| result.action != "error");
        let draft = AuditDraft::new("chain_unlink")
            .skill(plan.project.as_str(), plan.skill.as_str())
            .detail(
                report
                    .iter()
                    .map(|result| format!("{}:{}", result.name, result.action))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        self.store
            .log_audit(if all_ok { draft.ok() } else { draft });

        let topology = self.scan()?;
        let (verified, still_linked, removed_from) = verify_unlink(&topology, plan, &report);
        Ok(UnlinkOutcome {
            report,
            verified,
            still_linked,
            removed_from,
        })
    }

    /// One-shot unlink from every Agent (the historic contract). Plans an
    /// all-Agents removal then applies it, returning the per-item report so the
    /// existing `chain_unlink_skill` command keeps its shape.
    pub fn unlink(&self, project: &Path, skill_name: &str) -> Result<Vec<ops::OpResult>, AppError> {
        let plan = self.plan_unlink(project, skill_name, &[])?;
        Ok(self.apply_unlink(&plan)?.report)
    }

    /// Preview fast-forward-only pulls for the given Original Repositories.
    /// Entirely read-only: it neither fetches nor mutates, classifying each
    /// repository from its current refs (clean & behind ⇒ eligible; every other
    /// state ⇒ a skip with a precise reason). The returned plan is what
    /// [`apply_pull`](Self::apply_pull) consumes so the update acts only on what
    /// the user previewed.
    pub fn plan_pull(&self, repo_paths: &[String]) -> Result<pull::PullPlan, AppError> {
        let paths: Vec<PathBuf> = repo_paths.iter().map(PathBuf::from).collect();
        Ok(pull::preview(&paths))
    }

    /// Apply a previewed pull plan: fast-forward the eligible repositories and
    /// reflect every skip verbatim. Each attempted repository is audited (AC6),
    /// then a fresh rescan stamps the outcome's `scanned_at` so the UI reflects
    /// the post-pull topology (AC3). The pull itself never resets, stashes,
    /// force-updates, merges, or resolves conflicts — that guarantee lives in
    /// [`pull::apply`].
    pub fn apply_pull(&self, plan: &pull::PullPlan) -> Result<pull::PullOutcome, AppError> {
        let results = pull::apply(plan);

        // Structured audit record per attempted repository (AC6). A skip is a
        // deliberate protected refusal, not a failure, so only `error` results
        // are logged as unsuccessful.
        for result in &results {
            let draft = AuditDraft::new("chain_pull")
                .skill(result.path.clone(), result.name.clone())
                .detail(pull_audit_detail(result));
            self.store.log_audit(if result.action == "error" {
                draft
            } else {
                draft.ok()
            });
        }

        // Rescan so the outcome carries a fresh timestamp reflecting the
        // post-pull state (rescanned afterward, AC3).
        let topology = self.scan()?;
        Ok(pull::PullOutcome {
            results,
            scanned_at: topology.scanned_at,
        })
    }

    /// Preview fast-forward-only fork synchronizations for the given Original
    /// Repositories (`upstream` → `origin`). Entirely read-only: it neither
    /// fetches nor pushes, classifying each repository from its current local
    /// refs so scanning an outdated fork never triggers a push (AC3/AC5). The
    /// returned plan is what [`apply_fork_sync`](Self::apply_fork_sync) consumes
    /// so the sync acts only on what the user previewed.
    pub fn plan_fork_sync(
        &self,
        repo_paths: &[String],
    ) -> Result<fork_sync::ForkSyncPlan, AppError> {
        let paths: Vec<PathBuf> = repo_paths.iter().map(PathBuf::from).collect();
        Ok(fork_sync::preview(&paths))
    }

    /// Apply a previewed fork-sync plan: advance the eligible forks' `origin`
    /// branch to `upstream` by fast-forward push only, and reflect every skip
    /// verbatim. Each attempted repository is audited (AC6), then a fresh rescan
    /// stamps the outcome's `scanned_at` so the UI reflects the post-sync
    /// topology. The sync itself never force-pushes, rebases, merges, or rewrites
    /// history — that guarantee lives in [`fork_sync::apply`].
    pub fn apply_fork_sync(
        &self,
        plan: &fork_sync::ForkSyncPlan,
    ) -> Result<fork_sync::ForkSyncOutcome, AppError> {
        let results = fork_sync::apply(plan);

        // Structured audit record per attempted repository (AC6). A skip is a
        // deliberate protected refusal, not a failure, so only `error` results
        // are logged as unsuccessful.
        for result in &results {
            let draft = AuditDraft::new("chain_fork_sync")
                .skill(result.path.clone(), result.name.clone())
                .detail(fork_sync_audit_detail(result));
            self.store.log_audit(if result.action == "error" {
                draft
            } else {
                draft.ok()
            });
        }

        // Rescan so the outcome carries a fresh timestamp reflecting the
        // post-sync state.
        let topology = self.scan()?;
        Ok(fork_sync::ForkSyncOutcome {
            results,
            scanned_at: topology.scanned_at,
        })
    }

    /// Candidate new targets for the given broken findings (issue #30) — the
    /// read-only evidence the workbench card prints: same-name and near-name
    /// Skills across the scanned topology plus a bounded Git rename probe.
    /// A fingerprint that does not resolve to a current broken finding, or one
    /// with nowhere plausible to point, is simply absent from the map.
    pub fn locate_candidates(
        &self,
        fingerprints: &[String],
    ) -> Result<candidates::CandidatesReport, AppError> {
        let topo = self.scan()?;
        let findings = doctor::diagnose(&topo);
        let requested: HashSet<&str> = fingerprints.iter().map(String::as_str).collect();
        let mut located = std::collections::BTreeMap::new();
        for finding in &findings {
            if !requested.contains(finding.fingerprint.as_str()) {
                continue;
            }
            let found = candidates::locate(&topo, finding);
            if !found.is_empty() {
                located.insert(finding.fingerprint.clone(), found);
            }
        }
        Ok(candidates::CandidatesReport {
            candidates: located,
            scanned_at: topo.scanned_at,
        })
    }

    /// The uncommitted tracked changes of one scanned repository (issue #34's
    /// feedback card evidence). The path must name a repository the current
    /// topology scan knows — the read stays inside the managed surface.
    pub fn repo_dirty_diff(
        &self,
        repo_path: &str,
    ) -> Result<super::repo_health::DirtyDiff, AppError> {
        let topo = self.scan()?;
        if !topo.repos.iter().any(|repo| repo.path == repo_path) {
            return Err(AppError::not_found("no scanned repository at this path"));
        }
        Ok(super::repo_health::dirty_diff(Path::new(repo_path)))
    }

    /// Common-cause analysis over the current findings (issue #33): detect
    /// whole-repository moves behind broken-link storms. Read-only.
    pub fn repo_moves(&self) -> Result<repo_move::RepoMoveReport, AppError> {
        let topo = self.scan()?;
        let findings = doctor::diagnose(&topo);
        Ok(repo_move::RepoMoveReport {
            groups: repo_move::detect(&topo, &findings),
            scanned_at: topo.scanned_at,
        })
    }

    /// Preview repairs for the given Doctor findings, identified by fingerprint.
    /// Re-scans and re-diagnoses so the plan is built from CURRENT evidence — a
    /// fingerprint that no longer maps to a supported (broken/direct/legacy)
    /// finding is reported unsupported, not repaired. Entirely read-only.
    pub fn plan_repair(&self, fingerprints: &[String]) -> Result<repair::RepairPlan, AppError> {
        let topo = self.scan()?;
        let findings = doctor::diagnose(&topo);
        Ok(repair::plan(&topo, &findings, fingerprints, None))
    }

    /// Apply a previewed repair plan. Re-validates the write boundary and TOCTOU
    /// evidence per item (in `repair::apply`), records a `chain_repair` audit
    /// entry per item, then rescans and VERIFIES the normalized chain before
    /// reporting success (AC6): each repaired non-broken finding must resolve
    /// through the aggregate to its preserved Original, and each broken removal
    /// must be gone. `verified` is set only when the apply was clean (no
    /// conflict/skip/error item) and the rescan confirms every repaired item.
    pub fn apply_repair(
        &self,
        plan: &repair::RepairPlan,
    ) -> Result<repair::RepairOutcome, AppError> {
        let results = self.gate_and_apply(plan)?;
        self.verify_and_journal(results)
    }

    /// The write half of a repair apply: registration gate, guarded apply,
    /// per-item audit. Shared by [`apply_repair`] and the live runner so both
    /// paths enforce identical guards.
    fn gate_and_apply(
        &self,
        plan: &repair::RepairPlan,
    ) -> Result<Vec<repair::RepairItem>, AppError> {
        // Registration gate (parity with `apply_link`): every project a writing
        // item would edit must be a registered chain project, so a stale or
        // forged plan can never write into an arbitrary directory.
        let mut checked: HashSet<String> = HashSet::new();
        for item in &plan.items {
            // Only writing items are gated; `checked` de-duplicates per project so
            // each distinct project's registration is looked up at most once.
            if is_repair_write(&item.action)
                && checked.insert(item.project.clone())
                && !self.is_registered(Path::new(&item.project))?
            {
                return Err(AppError::not_found(
                    "repair targets a project that is not registered for chain management",
                ));
            }
        }

        let results = repair::apply(plan);

        // One structured audit record per item (applied or refused).
        for item in &results {
            let skill = Path::new(&item.path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| item.path.clone());
            let draft = AuditDraft::new("chain_repair")
                .skill(item.path.clone(), skill)
                .detail(format!(
                    "{} {} -> {}",
                    item.kind, item.deviation, item.action
                ));
            self.store.log_audit(if is_repair_success(&item.action) {
                draft.ok()
            } else {
                draft
            });
        }
        Ok(results)
    }

    /// The observation half: rescan-verify (AC6) then journal (issue #31).
    /// Always runs after any write so no edit is ever left without its undo
    /// record.
    fn verify_and_journal(
        &self,
        results: Vec<repair::RepairItem>,
    ) -> Result<repair::RepairOutcome, AppError> {
        // Success is observed, not assumed: a fresh rescan must show every
        // repaired chain normalized (AC6).
        let topology = self.scan()?;
        let verified = verify_repair(&topology, &results);

        // Journal (issue #31): an apply that wrote anything gets a durable
        // record — the applied items ARE the undo material. Persisting is
        // best-effort: a journal failure must not un-report a repair that
        // already happened on disk.
        let journal_id = self.journal_repair(&results, verified);

        Ok(repair::RepairOutcome {
            results,
            verified,
            scanned_at: topology.scanned_at,
            journal_id,
        })
    }

    /// The narrated live repair (issue #32): the same deterministic pipeline
    /// as plan+apply, told as four steps over `emit` — check (rescan and
    /// re-diagnose), locate (candidate evidence), rebuild (guarded apply),
    /// verify (rescan + journal). `control` is consulted only at step
    /// boundaries BEFORE rebuild: a takeover there aborts with zero writes;
    /// once rebuild starts the run always finishes verify + journal, so a
    /// write is never left without its undo record.
    pub fn repair_live(
        &self,
        fingerprints: &[String],
        run_id: &str,
        prefer_root: Option<&str>,
        control: &live::LiveControl,
        emit: &mut dyn FnMut(live::LiveEvent),
    ) -> Result<live::LiveOutcome, AppError> {
        let mut seq: u32 = 0;
        let mut send = |step: &str, status: &str, detail: Option<String>| {
            seq += 1;
            emit(live::LiveEvent {
                run_id: run_id.to_string(),
                seq,
                step: step.to_string(),
                status: status.to_string(),
                detail,
            });
        };
        const ABORTED: live::LiveOutcome = live::LiveOutcome {
            aborted: true,
            outcome: None,
        };

        // ── check: the findings must still be present in a fresh diagnosis ──
        if control.checkpoint() == live::Decision::Takeover {
            return Ok(ABORTED);
        }
        send("check", "start", None);
        let topo = self.scan()?;
        let findings = doctor::diagnose(&topo);
        let requested: Vec<&doctor::Finding> = findings
            .iter()
            .filter(|finding| fingerprints.contains(&finding.fingerprint))
            .collect();
        if requested.is_empty() {
            send(
                "check",
                "failed",
                Some("finding no longer present — rescan and retry".to_string()),
            );
            return Err(AppError::not_found(
                "the requested findings are no longer present",
            ));
        }
        let check_detail = requested
            .iter()
            .map(|finding| {
                format!(
                    "{} → {} · {}",
                    finding.evidence.entry_path,
                    finding.evidence.final_target,
                    finding.evidence.topology_status
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        send("check", "done", Some(check_detail));

        // ── locate: candidate evidence for broken; normalization otherwise ──
        if control.checkpoint() == live::Decision::Takeover {
            return Ok(ABORTED);
        }
        send("locate", "start", None);
        let locate_detail = requested
            .iter()
            .map(|finding| {
                if finding.deviation == doctor::Deviation::Broken {
                    match candidates::best_relink_target(&topo, finding, prefer_root) {
                        Some(candidate) => {
                            format!("{} · {}%", candidate.path, candidate.score)
                        }
                        None => "no candidate — dangling link will be removed".to_string(),
                    }
                } else {
                    format!(
                        "{} · normalize through the aggregate",
                        finding.evidence.final_target
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        send("locate", "done", Some(locate_detail));

        // ── rebuild: last takeover window, then the guarded apply ──
        if control.checkpoint() == live::Decision::Takeover {
            return Ok(ABORTED);
        }
        send("rebuild", "start", None);
        let plan = repair::plan(&topo, &findings, fingerprints, prefer_root);
        if plan.items.is_empty() {
            send("rebuild", "failed", Some("nothing to repair".to_string()));
            return Err(AppError::not_found("the plan produced no repair items"));
        }
        let results = match self.gate_and_apply(&plan) {
            Ok(results) => results,
            Err(e) => {
                send("rebuild", "failed", Some(e.to_string()));
                return Err(e);
            }
        };
        let rebuild_detail = results
            .iter()
            .map(|item| match &item.new_target {
                Some(target) => format!("{} {} {} → {}", item.kind, item.action, item.path, target),
                None => format!("{} {} {}", item.kind, item.action, item.path),
            })
            .collect::<Vec<_>>()
            .join("\n");
        send("rebuild", "done", Some(rebuild_detail));

        // ── verify: rescan + journal; always runs once rebuild wrote ──
        send("verify", "start", None);
        let outcome = match self.verify_and_journal(results) {
            Ok(outcome) => outcome,
            Err(e) => {
                send("verify", "failed", Some(e.to_string()));
                return Err(e);
            }
        };
        send(
            "verify",
            "done",
            Some(if outcome.verified {
                match outcome.journal_id {
                    Some(id) => format!("verified · journal #{id}"),
                    None => "verified".to_string(),
                }
            } else {
                "unverified — see the report".to_string()
            }),
        );
        Ok(live::LiveOutcome {
            aborted: false,
            outcome: Some(outcome),
        })
    }

    /// Persist one repair-journal record for an apply's realized writes.
    /// Returns `None` (and logs) when nothing was written or the insert fails.
    fn journal_repair(&self, results: &[repair::RepairItem], verified: bool) -> Option<i64> {
        let writing: Vec<&repair::RepairItem> = results
            .iter()
            .filter(|item| journal::is_write(&item.action))
            .collect();
        if writing.is_empty() {
            return None;
        }
        let projects: Vec<&str> = dedup_sorted(writing.iter().map(|item| item.project.as_str()));
        let fingerprints: Vec<&str> =
            dedup_sorted(writing.iter().map(|item| item.fingerprint.as_str()));
        let row = (
            serde_json::to_string(&projects),
            serde_json::to_string(&fingerprints),
            serde_json::to_string(results),
        );
        let (Ok(projects_json), Ok(fingerprints_json), Ok(items_json)) = row else {
            log::error!("repair journal: failed to serialize record");
            return None;
        };
        match self.store.insert_repair_journal(
            &projects_json,
            &fingerprints_json,
            &items_json,
            verified,
        ) {
            Ok(id) => Some(id),
            Err(e) => {
                log::error!("repair journal: failed to persist record: {e}");
                None
            }
        }
    }

    /// The repair journal, newest first (issue #31). A row whose JSON no
    /// longer parses is skipped rather than failing the whole list.
    pub fn repair_journal(
        &self,
        limit: Option<i64>,
    ) -> Result<Vec<journal::JournalRecord>, AppError> {
        let rows = self
            .store
            .list_repair_journal(limit)
            .map_err(AppError::db)?;
        Ok(rows
            .iter()
            .filter_map(|row| journal::parse_record(row).ok())
            .collect())
    }

    /// One-click undo of a journaled repair (issue #31): replay the record's
    /// inverses newest-edit-first under per-item guards, then VERIFY the
    /// rollback from a fresh rescan — the undo restores the original
    /// deviation, so every repaired fingerprint must reappear among the
    /// diagnosed findings (their material evidence, hence their hash, is
    /// restored byte-identically).
    ///
    /// Status transition: the record flips to `undone` as soon as ANY inverse
    /// write landed (the record is spent — the repair is no longer intact).
    /// When every item was refused (disk changed everywhere), the record stays
    /// `applied` and the outcome reports why, so the card remains visible with
    /// its reasons instead of silently vanishing without a rollback.
    pub fn undo_repair(&self, id: i64) -> Result<journal::UndoOutcome, AppError> {
        let row = self
            .store
            .get_repair_journal(id)
            .map_err(AppError::db)?
            .ok_or_else(|| AppError::not_found("no such repair record"))?;
        let record = journal::parse_record(&row)
            .map_err(|e| AppError::invalid_input(format!("repair record is not readable: {e}")))?;
        if record.status == journal::STATUS_UNDONE {
            return Err(AppError::invalid_input("repair record is already undone"));
        }

        // Registration gate (parity with `apply_repair`): every project the
        // undo would edit must still be registered for chain management.
        for project in &record.projects {
            if !self.is_registered(Path::new(project))? {
                return Err(AppError::not_found(
                    "undo targets a project that is not registered for chain management",
                ));
            }
        }

        let results = journal::undo(&record.items);

        // One structured audit record per inverse (applied or refused).
        for item in &results {
            let skill = Path::new(&item.path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| item.path.clone());
            let draft = AuditDraft::new("chain_repair_undo")
                .skill(item.path.clone(), skill)
                .detail(format!(
                    "{} {} -> {}",
                    item.kind, item.deviation, item.action
                ));
            self.store.log_audit(if journal::is_write(&item.action) {
                draft.ok()
            } else {
                draft
            });
        }

        let any_write = results.iter().any(|item| journal::is_write(&item.action));
        if any_write {
            self.store
                .set_repair_journal_status(id, journal::STATUS_UNDONE)
                .map_err(AppError::db)?;
        }

        // Rollback is observed, not assumed: clean inverses AND the original
        // findings back in a fresh diagnosis.
        let topology = self.scan()?;
        let clean =
            !results.is_empty() && results.iter().all(|item| journal::is_write(&item.action));
        let findings = doctor::diagnose(&topology);
        let restored = record.fingerprints.iter().all(|fingerprint| {
            findings
                .iter()
                .any(|finding| &finding.fingerprint == fingerprint)
        });
        Ok(journal::UndoOutcome {
            results,
            verified: clean && restored,
            scanned_at: topology.scanned_at,
        })
    }

    /// Hide a record's card without deleting the history (issue #31).
    pub fn dismiss_repair_record(&self, id: i64) -> Result<(), AppError> {
        self.store
            .set_repair_journal_dismissed(id)
            .map_err(AppError::db)
    }

    /// Preview remediating one Global Guard violation into a selected registered
    /// project (AC1). Entirely read-only.
    ///
    /// The violation is located by a fresh scan of the guard, matched on its
    /// global entry `path` (a not-found path means nothing to remediate, so a
    /// stale plan cannot invent a target). The project must be a registered
    /// chain project — the same enrolment gate the link flow enforces.
    ///
    /// For a global SYMLINK the plan carries a project link plan whose Original
    /// is the entry's resolved target, plus the on-disk baseline apply re-checks
    /// before removing the global entry. For a PHYSICAL global directory nothing
    /// is linked or removed: the plan carries manual guidance only (AC4).
    pub fn plan_remediate(
        &self,
        global_path: &str,
        project: &Path,
        agents: &[String],
    ) -> Result<remediate::RemediationPlan, AppError> {
        // Locate the violation from a fresh guard scan (one scanner). Carry the
        // surface too so the plan can name the offending Agent.
        let topology = self.scan()?;
        let located = topology.guard.iter().find_map(|surface| {
            surface
                .violations
                .iter()
                .find(|violation| violation.path == global_path)
                .map(|violation| (surface, violation))
        });
        let Some((surface, violation)) = located else {
            return Err(AppError::not_found(
                "no Global Guard violation at that path",
            ));
        };

        // Enrolment gate (parity with the link flow): a Skill can only be
        // remediated into a project the user has explicitly registered.
        if !self.is_registered(project)? {
            return Err(AppError::not_found(
                "select a registered project for chain management",
            ));
        }

        let global_evidence = ops::observe(Path::new(global_path));

        // A physical global directory is manual-only — never linked or removed.
        if !violation.is_link {
            return Ok(remediate::RemediationPlan {
                global_path: violation.path.clone(),
                skill: violation.skill.clone(),
                agent: surface.agent.clone(),
                final_target: violation.final_target.clone(),
                is_link: false,
                project: project.to_string_lossy().to_string(),
                agents: agents.to_vec(),
                link_plan: None,
                remove_global: false,
                global_evidence,
                guidance: Some(remediate::PHYSICAL_GUIDANCE.to_string()),
            });
        }

        // The Original to link is the entry's resolved real Skill. plan_link
        // re-validates it against the warehouse boundary and the registration.
        let original = PathBuf::from(&violation.final_target);
        let link_plan = self.plan_link(project, &[original], agents)?;
        Ok(remediate::RemediationPlan {
            global_path: violation.path.clone(),
            skill: violation.skill.clone(),
            agent: surface.agent.clone(),
            final_target: violation.final_target.clone(),
            is_link: true,
            project: project.to_string_lossy().to_string(),
            agents: agents.to_vec(),
            link_plan: Some(link_plan),
            remove_global: true,
            global_evidence,
            guidance: None,
        })
    }

    /// Apply a previewed remediation. Establishes and VERIFIES the project-local
    /// chain BEFORE retiring the global entry (AC2); a failed or conflicting
    /// link leaves the global entry untouched (AC3); a physical global directory
    /// is never deleted (AC4); and a fresh Global Guard rescan plus a per-phase
    /// audit accompany every outcome (AC5).
    ///
    /// Each phase is audited under `chain_remediate` (the link, then the global
    /// removal), success-flagged per outcome. The end-to-end `verified` flag is
    /// set only when the link verified AND the global entry was removed.
    pub fn apply_remediate(
        &self,
        plan: &remediate::RemediationPlan,
    ) -> Result<remediate::RemediationOutcome, AppError> {
        // Physical / not linkable: never touch the global entry (AC4). Record a
        // guidance audit and rescan so the UI still refreshes.
        let Some(link_plan) = plan.link_plan.as_ref() else {
            self.store.log_audit(
                AuditDraft::new("chain_remediate")
                    .skill(plan.global_path.clone(), plan.skill.clone())
                    .tool(plan.agent.clone())
                    .detail(format!(
                        "guidance (physical global entry): {}",
                        plan.guidance
                            .as_deref()
                            .unwrap_or(remediate::PHYSICAL_GUIDANCE)
                    )),
            );
            let topology = self.scan()?;
            return Ok(remediate::RemediationOutcome {
                link: None,
                global_removed: false,
                verified: false,
                guidance: plan
                    .guidance
                    .clone()
                    .or_else(|| Some(remediate::PHYSICAL_GUIDANCE.to_string())),
                scanned_at: topology.scanned_at,
                guard: topology.guard,
            });
        };

        // Establish + verify the project-local chain FIRST (AC2). The project is
        // registered (checked at plan time and re-checked inside apply_link).
        let link = self.apply_link(link_plan)?;
        self.store.log_audit({
            let draft = AuditDraft::new("chain_remediate")
                .skill(plan.final_target.clone(), plan.skill.clone())
                .tool(plan.agents.join(","))
                .detail(format!(
                    "link {} into {} (verified={})",
                    plan.skill, plan.project, link.verified
                ));
            if link.verified {
                draft.ok()
            } else {
                draft
            }
        });

        // The global entry is retired only after a clean, verified link (AC3).
        // `verified` already implies a clean report, but re-check the report
        // explicitly so the gate is legible on its own.
        let link_clean = link
            .report
            .skills
            .iter()
            .chain(link.report.entries.iter())
            .all(|item| links_present(&item.action));

        let mut global_removed = false;
        let mut guidance: Option<String> = None;
        if link.verified && link_clean {
            match ops::remove_global_symlink(Path::new(&plan.global_path), &plan.global_evidence)
                .map_err(|e| AppError::invalid_input(e.to_string()))?
            {
                ops::GlobalRemoval::Removed => global_removed = true,
                ops::GlobalRemoval::Changed => {
                    guidance = Some(remediate::CHANGED_GUIDANCE.to_string());
                }
                ops::GlobalRemoval::Physical => {
                    guidance = Some(remediate::BECAME_PHYSICAL_GUIDANCE.to_string());
                }
            }
        }

        // Audit the global-removal phase, success-flagged by whether it was
        // retired.
        self.store.log_audit({
            let detail = if global_removed {
                format!("removed global entry {}", plan.global_path)
            } else if !link.verified {
                format!(
                    "link unverified; global entry left untouched: {}",
                    plan.global_path
                )
            } else {
                format!(
                    "global entry left untouched ({}): {}",
                    guidance.as_deref().unwrap_or("no change required"),
                    plan.global_path
                )
            };
            let draft = AuditDraft::new("chain_remediate")
                .skill(plan.global_path.clone(), plan.skill.clone())
                .tool(plan.agent.clone())
                .detail(detail);
            if global_removed {
                draft.ok()
            } else {
                draft
            }
        });

        // Rescan the Global Guard after the apply (AC5).
        let topology = self.scan()?;
        let verified = link.verified && global_removed;
        Ok(remediate::RemediationOutcome {
            link: Some(link),
            global_removed,
            verified,
            guidance,
            scanned_at: topology.scanned_at,
            guard: topology.guard,
        })
    }
}

fn validate_decision_kind(kind: &str) -> Result<(), AppError> {
    if kind == decisions::KIND_IGNORED || kind == decisions::KIND_PROJECT_PRIVATE {
        Ok(())
    } else {
        Err(AppError::invalid_input(format!(
            "unknown finding decision kind: {kind}"
        )))
    }
}

/// Human-readable audit detail for one pull attempt.
fn pull_audit_detail(result: &pull::PullResult) -> String {
    match result.action.as_str() {
        "updated" => format!(
            "updated {} -> {}",
            result.from.as_deref().unwrap_or("?"),
            result.to.as_deref().unwrap_or("?")
        ),
        "up_to_date" => "up_to_date".to_string(),
        "skipped" => format!("skipped: {}", result.reason.as_deref().unwrap_or("skip")),
        "error" => format!(
            "error: {}{}",
            result.reason.as_deref().unwrap_or("error"),
            result
                .message
                .as_deref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        ),
        other => other.to_string(),
    }
}

/// Human-readable audit detail for one fork-sync attempt.
fn fork_sync_audit_detail(result: &fork_sync::ForkSyncResult) -> String {
    match result.action.as_str() {
        "synced" => format!(
            "synced origin -> upstream {} -> {}",
            result.from.as_deref().unwrap_or("?"),
            result.to.as_deref().unwrap_or("?")
        ),
        "up_to_date" => "up_to_date".to_string(),
        "skipped" => format!("skipped: {}", result.reason.as_deref().unwrap_or("skip")),
        "error" => format!(
            "error: {}{}",
            result.reason.as_deref().unwrap_or("error"),
            result
                .message
                .as_deref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        ),
        other => other.to_string(),
    }
}

/// Actions that count as a successful write for the audit log's success flag.
fn is_success_action(action: &str) -> bool {
    matches!(action, "created" | "exists" | "removed" | "absent")
}

/// Actions that mean a link is present in the chain after apply (freshly made or
/// already there). The single source of truth for "applied" across verification.
fn links_present(action: &str) -> bool {
    matches!(action, "created" | "exists")
}

/// Repair actions that write to disk (the ones the registration gate protects).
fn is_repair_write(action: &str) -> bool {
    matches!(action, "create" | "repoint" | "remove")
}

/// Distinct values in sorted order — the stable identity lists a journal
/// record carries (projects, fingerprints).
fn dedup_sorted<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let set: std::collections::BTreeSet<&str> = values.collect();
    set.into_iter().collect()
}

/// Repair actions that count as a successful outcome for the audit success flag
/// and the clean check (a link created/re-pointed/removed, or already correct).
fn is_repair_success(action: &str) -> bool {
    matches!(action, "create" | "repoint" | "remove" | "exists")
}

/// Cross-check a repair report against a fresh topology scan (AC6). Verified only
/// when the apply was clean (no conflict/skip/error item), something was
/// repaired, and every item's intended terminal shape is observed:
///
/// * `remove_broken` — the dead link no longer appears anywhere in the topology.
/// * `ensure_aggregate` — the aggregate link resolves to the intended Original.
/// * `repoint_entry` on the aggregate — the aggregate now resolves straight to
///   the Original (retired hop collapsed).
/// * `repoint_entry` on a surface — the entry's hop chain runs through an
///   aggregate entry that resolves to the SAME Original, i.e. Agent access now
///   routes through the project aggregate surface (AC2).
/// * `relink_broken` — a rebuilt broken chain (issue #30); verified exactly
///   like `repoint_entry`/`ensure_aggregate`, with the candidate as the
///   intended Original.
///
/// The surface case verifies routing via the hop chain, not the topology status
/// token: the classifier keys `direct` off the fully-resolved final target, so a
/// correctly normalized per-entry surface link is still labelled `direct` even
/// though it now goes through `.agents/skills`. The Original preservation the
/// aggregate item confirms plus the routing the surface item confirms together
/// prove the normalized shape.
fn verify_repair(topology: &ChainTopology, results: &[repair::RepairItem]) -> bool {
    let clean = results.iter().all(|item| is_repair_success(&item.action));
    if !clean || results.is_empty() {
        return false;
    }

    // Index every traced entry across the rescanned topology, plus the aggregate
    // entries' resolved targets, so a surface entry can be confirmed to route
    // through the project aggregate to the same Original.
    let mut entries: HashMap<&str, &TracedEntry> = HashMap::new();
    let mut aggregate_final: HashMap<&str, &str> = HashMap::new();
    for project in &topology.projects {
        if let Some(agg) = &project.agents_dir {
            for entry in &agg.entries {
                entries.insert(entry.entry_path.as_str(), entry);
                aggregate_final.insert(entry.entry_path.as_str(), entry.final_target.as_str());
            }
        }
        for surface in &project.surfaces {
            for entry in &surface.entries {
                entries.insert(entry.entry_path.as_str(), entry);
            }
        }
    }

    results
        .iter()
        .all(|item| verify_repair_item(item, &entries, &aggregate_final))
}

fn verify_repair_item(
    item: &repair::RepairItem,
    entries: &HashMap<&str, &TracedEntry>,
    aggregate_final: &HashMap<&str, &str>,
) -> bool {
    match item.kind.as_str() {
        "remove_broken" => !entries.contains_key(item.path.as_str()),
        "ensure_aggregate" => entries
            .get(item.path.as_str())
            .is_some_and(|entry| item.new_target.as_deref() == Some(entry.final_target.as_str())),
        // A relinked broken chain must land in the same terminal shape a
        // repoint produces: the aggregate resolving to the candidate Original,
        // the surface routing through the aggregate.
        "repoint_entry" | "relink_broken" => {
            let Some(entry) = entries.get(item.path.as_str()) else {
                return false;
            };
            if is_aggregate_entry_path(&item.path) {
                item.new_target.as_deref() == Some(entry.final_target.as_str())
            } else {
                entry.hops.iter().any(|hop| {
                    aggregate_final
                        .get(hop.as_str())
                        .is_some_and(|agg_final| *agg_final == entry.final_target.as_str())
                })
            }
        }
        _ => true,
    }
}

/// Whether a path is a `.agents/skills/<skill>` aggregate entry (vs. a surface
/// entry), used to pick the right verification for a `repoint_entry` item.
fn is_aggregate_entry_path(path: &str) -> bool {
    Path::new(path)
        .parent()
        .is_some_and(|parent| parent.ends_with(".agents/skills"))
}

/// Cross-check an unlink report against a fresh topology scan. Returns
/// `(verified, still_linked, removed_from)`:
///
/// * `still_linked` — Agents whose surface still exposes the Skill (retained
///   access, e.g. Agents not targeted or reached through a still-required
///   shared aggregate).
/// * `removed_from`  — affected Agents the scan confirms no longer expose it.
/// * `verified` — true only when the apply was clean (no error/skipped item) and
///   every affected Agent no longer exposes the Skill.
///
/// An Agent "exposes" the Skill when its dir-link surface is healthy and the
/// aggregate still carries the entry, or its per-entry surface holds an entry
/// for the Skill. Matching is by canonical project identity, like `verify_chain`.
fn verify_unlink(
    topology: &ChainTopology,
    plan: &ops::UnlinkPlan,
    report: &[ops::OpResult],
) -> (bool, Vec<String>, Vec<String>) {
    let plan_key = project_registry::canonical_key(Path::new(&plan.project));
    let project = topology
        .projects
        .iter()
        .find(|candidate| project_registry::canonical_key(Path::new(&candidate.path)) == plan_key);

    let exposes = |agent: &str| -> bool {
        let Some(proj) = project else { return false };
        let Some(surface) = proj.surfaces.iter().find(|s| s.agent == agent) else {
            return false;
        };
        match surface.kind.as_str() {
            "dir_link" => {
                surface.dir_link_ok
                    && proj
                        .agents_dir
                        .as_ref()
                        .is_some_and(|dir| dir.entries.iter().any(|e| e.name == plan.skill))
            }
            "per_entry" => surface.entries.iter().any(|e| e.name == plan.skill),
            _ => false,
        }
    };

    let still_linked: Vec<String> = project
        .map(|proj| {
            proj.surfaces
                .iter()
                .filter(|s| exposes(&s.agent))
                .map(|s| s.agent.clone())
                .collect()
        })
        .unwrap_or_default();

    let removed_from: Vec<String> = plan
        .affected_agents
        .iter()
        .filter(|agent| !exposes(agent))
        .cloned()
        .collect();

    let clean = report
        .iter()
        .all(|r| r.action != "error" && r.action != "skipped");
    let all_affected_removed = plan.affected_agents.iter().all(|agent| !exposes(agent));

    (clean && all_affected_removed, still_linked, removed_from)
}

/// Cross-check an apply report against a fresh topology scan. Returns
/// `(verified, observed, missing)`:
///
/// * `observed` — applied skills the scan shows resolving into an Original
///   Repository (aggregate status `link_repo`).
/// * `missing`  — applied skills the scan did NOT confirm that way.
/// * `verified` — true only when the apply was clean (no conflict / error /
///   skipped item), something was applied, nothing is missing, and every
///   requested agent surface actually reaches the aggregate.
fn verify_chain(
    topology: &ChainTopology,
    plan: &LinkPlan,
    report: &ops::LinkReport,
) -> (bool, Vec<String>, Vec<String>) {
    let applied: Vec<String> = report
        .skills
        .iter()
        .filter(|item| links_present(&item.action))
        .map(|item| item.name.clone())
        .collect();

    // Match the scanned project by canonical identity, not raw string equality,
    // so a path alias (trailing slash, symlink, `.`/`..`) still resolves to the
    // project the links were written into — matching how the write path and the
    // registry identify projects. Otherwise a real chain would read as missing.
    let plan_key = project_registry::canonical_key(Path::new(&plan.project));
    let project = topology
        .projects
        .iter()
        .find(|candidate| project_registry::canonical_key(Path::new(&candidate.path)) == plan_key);

    let repo_linked = |name: &str| -> bool {
        project
            .and_then(|proj| proj.agents_dir.as_ref())
            .is_some_and(|dir| {
                dir.entries
                    .iter()
                    .any(|entry| entry.name == name && entry.status == "link_repo")
            })
    };

    let mut observed = Vec::new();
    let mut missing = Vec::new();
    for name in &applied {
        if repo_linked(name) {
            observed.push(name.clone());
        } else {
            missing.push(name.clone());
        }
    }

    // Every requested agent surface must reach the aggregate: either the surface
    // is a healthy dir link, or (physical surface) each observed skill has a
    // per-entry link resolving through `.agents/skills`.
    let agents_reach = plan.agents.iter().all(|agent| {
        let Some(proj) = project else { return false };
        let Some(surface) = proj.surfaces.iter().find(|s| &s.agent == agent) else {
            return false;
        };
        match surface.kind.as_str() {
            "dir_link" => surface.dir_link_ok,
            "per_entry" => observed.iter().all(|name| {
                surface
                    .entries
                    .iter()
                    .any(|entry| entry.name == *name && entry.status == "via_agents")
            }),
            _ => false,
        }
    });

    let clean = report
        .skills
        .iter()
        .chain(report.entries.iter())
        .all(|item| links_present(&item.action));

    let verified = clean && missing.is_empty() && !observed.is_empty() && agents_reach;
    (verified, observed, missing)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::{tempdir, TempDir};

    use super::ChainService;
    use crate::core::chain::journal;
    use crate::core::chain::ops;
    use crate::core::error::ErrorKind;
    use crate::core::skill_store::{ProjectRecord, SkillStore};

    /// Portable stand-in for `std::os::unix::fs::symlink`. These fixtures link
    /// directories, and gating this module on unix meant none of it ran on
    /// Windows — including the repair, undo and remediate suites.
    fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        crate::core::test_support::symlink_dir(target.as_ref(), link.as_ref())
    }

    /// A warehouse repo with one Skill plus an unlinked project directory.
    /// Returns (temp, warehouse_root, original, project); the caller builds the
    /// store so it controls settings and registration.
    fn guarded_fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempdir().unwrap();
        let warehouse_root = temp.path().join("xw-skills");
        let repo = warehouse_root.join("source-repo");
        let original = repo.join("skills").join("demo-skill");
        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&repo).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Guard fixture\n---\n",
        )
        .unwrap();
        let project = temp.path().join("Projects").join("demo-project");
        fs::create_dir_all(&project).unwrap();
        (temp, warehouse_root, original, project)
    }

    fn guarded_store(temp: &TempDir, warehouse_root: &std::path::Path) -> SkillStore {
        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
    }

    fn project_record(name: &str, path: String) -> ProjectRecord {
        ProjectRecord {
            id: format!("id-{path}"),
            name: name.to_string(),
            path,
            workspace_type: "project".to_string(),
            linked_agent_key: None,
            linked_agent_name: None,
            disabled_path: None,
            sort_order: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn scan_link_rescan_unlink_rescan_through_public_service() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let repo_path = warehouse_root.join("source-repo");
        let original = repo_path.join("skills").join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&project).unwrap();
        git2::Repository::init(&repo_path).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Contract fixture\n---\n",
        )
        .unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);

        let initial = service.scan().unwrap();
        let initial_repo = initial
            .repos
            .iter()
            .find(|repo| repo.name == "source-repo")
            .unwrap();
        assert!(initial_repo
            .skills
            .iter()
            .any(|skill| skill.name == "demo-skill" && skill.path == original.to_string_lossy()));
        assert!(initial.projects.is_empty());

        let link = service
            .link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();
        // Success is only reported after a rescan observed the chain.
        assert!(link.verified);
        assert_eq!(link.observed, ["demo-skill"]);
        assert!(link.missing.is_empty());
        let linked_skill = link
            .report
            .skills
            .iter()
            .find(|item| item.name == "demo-skill")
            .unwrap();
        assert_eq!(linked_skill.action, "created");
        assert_eq!(
            linked_skill.path,
            project
                .join(".agents")
                .join("skills")
                .join("demo-skill")
                .to_string_lossy()
        );
        let linked_surface = link
            .report
            .entries
            .iter()
            .find(|item| item.name == "claude")
            .unwrap();
        assert_eq!(linked_surface.action, "created");
        assert_eq!(
            linked_surface.path,
            project.join(".claude").join("skills").to_string_lossy()
        );

        let linked = service.scan().unwrap();
        let linked_repo = linked
            .repos
            .iter()
            .find(|repo| repo.name == "source-repo")
            .unwrap();
        // Reverse reference carries the project's canonical identity, not just
        // its display name.
        assert_eq!(linked_repo.referenced_by.len(), 1);
        assert_eq!(linked_repo.referenced_by[0].name, "demo-project");
        assert_eq!(linked_repo.referenced_by[0].path, project.to_string_lossy());
        let linked_project = linked
            .projects
            .iter()
            .find(|candidate| candidate.name == "demo-project")
            .unwrap();
        let aggregate_entry = linked_project
            .agents_dir
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.name == "demo-skill")
            .unwrap();
        assert_eq!(aggregate_entry.status, "link_repo");
        assert_eq!(aggregate_entry.repo.as_deref(), Some("source-repo"));
        assert_eq!(aggregate_entry.final_target, original.to_string_lossy());
        let claude_surface = linked_project
            .surfaces
            .iter()
            .find(|surface| surface.agent == "claude")
            .unwrap();
        assert_eq!(claude_surface.kind, "dir_link");
        assert!(claude_surface.dir_link_ok);
        assert_eq!(
            claude_surface.dir_link_target.as_deref(),
            Some(
                project
                    .join(".agents")
                    .join("skills")
                    .to_string_lossy()
                    .as_ref()
            )
        );

        let unlink = service.unlink(&project, "demo-skill").unwrap();
        assert!(unlink
            .iter()
            .any(|item| item.name == ".agents" && item.action == "removed"));
        assert!(original.join("SKILL.md").is_file());

        let unlinked = service.scan().unwrap();
        let unlinked_repo = unlinked
            .repos
            .iter()
            .find(|repo| repo.name == "source-repo")
            .unwrap();
        assert!(unlinked_repo.referenced_by.is_empty());
        let unlinked_project = unlinked
            .projects
            .iter()
            .find(|candidate| candidate.name == "demo-project")
            .unwrap();
        assert!(unlinked_project
            .agents_dir
            .as_ref()
            .unwrap()
            .entries
            .is_empty());
        let remaining_surface = unlinked_project
            .surfaces
            .iter()
            .find(|surface| surface.agent == "claude")
            .unwrap();
        assert_eq!(remaining_surface.kind, "dir_link");
        assert!(remaining_surface.dir_link_ok);
        assert!(!project
            .join(".agents")
            .join("skills")
            .join("demo-skill")
            .exists());
        assert!(!project.join(".claude/skills/demo-skill").exists());

        // Every applied item has its own audit record: one per skill link and
        // one per agent surface, plus the later unlink (newest first).
        let audit = store.list_audit(None).unwrap();
        let actions: Vec<&str> = audit.iter().map(|entry| entry.action.as_str()).collect();
        assert_eq!(actions, ["chain_unlink", "chain_link", "chain_link"]);
        assert!(audit
            .iter()
            .filter(|entry| entry.action == "chain_link")
            .all(|entry| entry.success));
        assert!(audit.iter().any(|entry| entry.action == "chain_link"
            && entry.skill_name.as_deref() == Some("demo-skill")));
        assert!(audit
            .iter()
            .any(|entry| entry.action == "chain_link"
                && entry.skill_name.as_deref() == Some("claude")));
    }

    #[test]
    fn agent_aware_unlink_preserves_other_agents_and_verifies() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let repo_path = warehouse_root.join("source-repo");
        let original = repo_path.join("skills").join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        // Pre-create the agent surfaces as physical dirs so linking makes
        // per-entry links (removable per Agent) rather than a shared dir link.
        fs::create_dir_all(project.join(".claude").join("skills")).unwrap();
        fs::create_dir_all(project.join(".codex/skills")).unwrap();
        git2::Repository::init(&repo_path).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Contract fixture\n---\n",
        )
        .unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);

        service
            .link(
                &project,
                &[original.clone()],
                &["claude".to_string(), "codex".to_string()],
            )
            .unwrap();

        // Unlink from claude only: a per-Agent removal, not a shared-surface op.
        let plan = service
            .plan_unlink(&project, "demo-skill", &["claude".to_string()])
            .unwrap();
        assert!(!plan.shared_surface);

        let outcome = service.apply_unlink(&plan).unwrap();
        assert!(outcome.verified, "rescan must confirm the intended removal");
        assert_eq!(outcome.removed_from, ["claude"]);
        assert!(outcome.still_linked.contains(&"codex".to_string()));

        // claude lost access; codex kept it; aggregate and Original preserved.
        assert!(!project.join(".claude/skills/demo-skill").exists());
        assert!(project.join(".codex/skills/demo-skill").exists());
        assert!(project
            .join(".agents")
            .join("skills")
            .join("demo-skill")
            .exists());
        assert!(original.join("SKILL.md").is_file());
    }

    #[test]
    fn repair_normalizes_a_direct_finding_and_verifies() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let repo_path = warehouse_root.join("source-repo");
        let original = repo_path.join("skills").join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&repo_path).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Repair fixture\n---\n",
        )
        .unwrap();

        // A direct surface entry straight to the Original, with no aggregate yet.
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        symlink(&original, surface.join("demo-skill")).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        // Doctor sees exactly one direct finding for the surface entry.
        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        let direct = report
            .findings
            .iter()
            .find(|finding| finding.rule == "chain.direct_link")
            .expect("a direct finding");

        let plan = service.plan_repair(&[direct.fingerprint.clone()]).unwrap();
        assert!(plan.unsupported.is_empty());
        // The re-pointed surface entry is snapshotted for recovery (AC3).
        assert!(plan
            .snapshot
            .iter()
            .any(|snap| snap.path == surface.join("demo-skill").to_string_lossy()));

        let outcome = service.apply_repair(&plan).unwrap();
        assert!(outcome.verified, "rescan must confirm the normalized chain");

        // A rescan shows the aggregate resolving into the Original Repository
        // (link_repo), and the surface entry now routing through the aggregate to
        // the SAME Original (AC2).
        let scanned = service.scan().unwrap();
        let proj = scanned
            .projects
            .iter()
            .find(|candidate| candidate.name == "demo-project")
            .unwrap();
        let agg_entry = proj
            .agents_dir
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.name == "demo-skill")
            .unwrap();
        assert_eq!(agg_entry.status, "link_repo");
        assert_eq!(agg_entry.final_target, original.to_string_lossy());
        let claude = proj
            .surfaces
            .iter()
            .find(|surface| surface.agent == "claude")
            .unwrap();
        let surf_entry = claude
            .entries
            .iter()
            .find(|entry| entry.name == "demo-skill")
            .unwrap();
        assert_eq!(surf_entry.final_target, original.to_string_lossy());
        assert!(surf_entry.hops.iter().any(|hop| hop
            == &project
                .join(".agents")
                .join("skills")
                .join("demo-skill")
                .to_string_lossy()));
        // The Original Skill itself is never moved or rewritten.
        assert!(original.join("SKILL.md").is_file());

        // At least one chain_repair audit record was written.
        let audit = store.list_audit(None).unwrap();
        assert!(audit.iter().any(|entry| entry.action == "chain_repair"));
    }

    /// The full issue-#30 loop at the service boundary: a broken chain whose
    /// Original moved to a sibling repo is located (`locate_candidates`),
    /// relinked by the planner, and VERIFIED against a fresh rescan.
    #[test]
    fn repair_relinks_a_broken_finding_to_the_located_candidate_and_verifies() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        // The Skill lives in a NEW repo; the surface entry still points at the
        // dead path in the old (deleted) one.
        let new_repo = warehouse_root.join("moved-repo");
        let original = new_repo.join("skills").join("demo-skill");
        let dead = warehouse_root
            .join("source-repo")
            .join("skills")
            .join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&new_repo).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Relink fixture\n---\n",
        )
        .unwrap();
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        symlink(&dead, surface.join("demo-skill")).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        let broken = report
            .findings
            .iter()
            .find(|finding| finding.rule == "chain.broken_link")
            .expect("a broken finding");

        // The evidence card's read-only lookup names the moved Original.
        let located = service
            .locate_candidates(&[broken.fingerprint.clone()])
            .unwrap();
        let evidence = located
            .candidates
            .get(&broken.fingerprint)
            .expect("candidates for the broken finding");
        assert_eq!(evidence[0].reason, "same_name");
        assert_eq!(evidence[0].path, original.to_string_lossy());

        // The planner rebuilds the chain to that candidate instead of removing.
        let plan = service.plan_repair(&[broken.fingerprint.clone()]).unwrap();
        assert!(plan.items.iter().all(|item| item.kind == "relink_broken"));

        let outcome = service.apply_repair(&plan).unwrap();
        assert!(outcome.verified, "rescan must confirm the relinked chain");
        assert_eq!(
            final_target_of(&service, &project, "demo-skill"),
            original.to_string_lossy()
        );
    }

    /// The issue-#31 loop: a journaled repair is undone item-by-item, the
    /// filesystem returns to its pre-repair shape, and the rescan proves it by
    /// reproducing the ORIGINAL finding fingerprint.
    #[test]
    fn undo_replays_the_journal_and_restores_the_original_finding() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let new_repo = warehouse_root.join("moved-repo");
        let original = new_repo.join("skills").join("demo-skill");
        let dead = warehouse_root
            .join("source-repo")
            .join("skills")
            .join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&new_repo).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Journal fixture\n---\n",
        )
        .unwrap();
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&dead, &entry).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        let broken = report
            .findings
            .iter()
            .find(|finding| finding.rule == "chain.broken_link")
            .expect("a broken finding");
        let original_fingerprint = broken.fingerprint.clone();

        // Repair (relink to the moved Original) journals the applied items.
        let plan = service.plan_repair(&[broken.fingerprint.clone()]).unwrap();
        let outcome = service.apply_repair(&plan).unwrap();
        assert!(outcome.verified);
        let journal_id = outcome.journal_id.expect("a journaled apply");
        let records = service.repair_journal(None).unwrap();
        let record = records.iter().find(|r| r.id == journal_id).unwrap();
        assert_eq!(record.status, journal::STATUS_APPLIED);
        assert_eq!(record.fingerprints, vec![original_fingerprint.clone()]);
        assert!(record
            .projects
            .contains(&project.to_string_lossy().to_string()));

        // Undo: every inverse lands, the record flips to undone, and the
        // filesystem is byte-identically back — the surface entry dangles at
        // the ORIGINAL dead target and the created aggregate is gone.
        let undo = service.undo_repair(journal_id).unwrap();
        assert!(undo.verified, "rollback must be observed by the rescan");
        assert_eq!(std::fs::read_link(&entry).unwrap(), dead);
        assert!(std::fs::symlink_metadata(
            project.join(".agents").join("skills").join("demo-skill")
        )
        .is_err());

        // The rescan reproduces the ORIGINAL fingerprint (AC: 状态回退正确).
        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.fingerprint == original_fingerprint));

        // The record is spent: it lists as undone and cannot be undone twice.
        let records = service.repair_journal(None).unwrap();
        assert_eq!(
            records.iter().find(|r| r.id == journal_id).unwrap().status,
            journal::STATUS_UNDONE
        );
        assert!(service.undo_repair(journal_id).is_err());

        // Undo left its own audit trail.
        let audit = store.list_audit(None).unwrap();
        assert!(audit
            .iter()
            .any(|entry| entry.action == "chain_repair_undo"));
    }

    /// Per-item guard semantics: an item whose link changed after the repair
    /// is skipped, the rest roll back, and the outcome is NOT verified.
    #[test]
    fn undo_skips_items_changed_since_the_repair_and_reports_unverified() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let new_repo = warehouse_root.join("moved-repo");
        let original = new_repo.join("skills").join("demo-skill");
        let dead = warehouse_root
            .join("source-repo")
            .join("skills")
            .join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&new_repo).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Journal fixture\n---\n",
        )
        .unwrap();
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&dead, &entry).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        let broken = report
            .findings
            .iter()
            .find(|finding| finding.rule == "chain.broken_link")
            .expect("a broken finding");
        let plan = service.plan_repair(&[broken.fingerprint.clone()]).unwrap();
        let outcome = service.apply_repair(&plan).unwrap();
        let journal_id = outcome.journal_id.unwrap();

        // The surface entry is re-pointed by hand between repair and undo.
        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        ops::remove_symlink(&entry).unwrap();
        symlink(&elsewhere, &entry).unwrap();

        let undo = service.undo_repair(journal_id).unwrap();
        assert!(!undo.verified);
        let skipped = undo
            .results
            .iter()
            .find(|item| item.path == entry.to_string_lossy())
            .unwrap();
        assert_eq!(skipped.action, "skip");
        assert_eq!(skipped.message.as_deref(), Some("changed since repair"));
        // The hand-made link is untouched; the aggregate (unchanged since the
        // repair) still rolled back.
        assert_eq!(std::fs::read_link(&entry).unwrap(), elsewhere);
        assert!(std::fs::symlink_metadata(
            project.join(".agents").join("skills").join("demo-skill")
        )
        .is_err());
    }

    /// One reusable broken-chain fixture for the live-run tests: a dead
    /// surface entry whose Original moved to a scanned sibling repo.
    struct LiveFixture {
        _temp: TempDir,
        store: SkillStore,
        entry: PathBuf,
        dead: PathBuf,
        original: PathBuf,
        fingerprint: String,
    }

    fn live_fixture() -> LiveFixture {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let new_repo = warehouse_root.join("moved-repo");
        let original = new_repo.join("skills").join("demo-skill");
        let dead = warehouse_root
            .join("source-repo")
            .join("skills")
            .join("demo-skill");
        let project = projects_root.join("demo-project");

        fs::create_dir_all(&original).unwrap();
        git2::Repository::init(&new_repo).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: Live fixture\n---\n",
        )
        .unwrap();
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&dead, &entry).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();
        let report = service.doctor(&super::DoctorFilter::default()).unwrap();
        let fingerprint = report
            .findings
            .iter()
            .find(|finding| finding.rule == "chain.broken_link")
            .expect("a broken finding")
            .fingerprint
            .clone();
        LiveFixture {
            _temp: temp,
            store,
            entry,
            dead,
            original,
            fingerprint,
        }
    }

    /// The narrated pipeline (issue #32): the four steps stream in order,
    /// the repair lands, and the outcome carries the journaled apply.
    #[test]
    fn live_run_streams_the_four_steps_and_journals_the_repair() {
        let f = live_fixture();
        let service = ChainService::new(&f.store);
        let control = crate::core::chain::live::LiveControl::default();
        let mut events = Vec::new();

        let outcome = service
            .repair_live(
                &[f.fingerprint.clone()],
                "run-1",
                None,
                &control,
                &mut |event| events.push(event),
            )
            .unwrap();

        let transitions: Vec<(String, String)> = events
            .iter()
            .map(|event| (event.step.clone(), event.status.clone()))
            .collect();
        let expect = |step: &str, status: &str| (step.to_string(), status.to_string());
        assert_eq!(
            transitions,
            vec![
                expect("check", "start"),
                expect("check", "done"),
                expect("locate", "start"),
                expect("locate", "done"),
                expect("rebuild", "start"),
                expect("rebuild", "done"),
                expect("verify", "start"),
                expect("verify", "done"),
            ]
        );
        // seq is strictly monotonic; every event names the run.
        assert!(events.windows(2).all(|w| w[0].seq < w[1].seq));
        assert!(events.iter().all(|event| event.run_id == "run-1"));
        // The locate step carried the candidate evidence.
        let locate_done = events
            .iter()
            .find(|e| e.step == "locate" && e.status == "done")
            .unwrap();
        assert!(locate_done
            .detail
            .as_deref()
            .unwrap()
            .contains(f.original.to_string_lossy().as_ref()));

        // The repair really landed and was journaled.
        assert!(!outcome.aborted);
        let repair = outcome.outcome.unwrap();
        assert!(repair.verified);
        assert!(repair.journal_id.is_some());
        assert_eq!(
            std::fs::read_link(&f.entry).unwrap(),
            PathBuf::from("../../.agents/skills/demo-skill")
        );
    }

    /// A takeover before rebuild aborts with ZERO writes and no journal.
    #[test]
    fn live_run_takeover_before_rebuild_writes_nothing() {
        let f = live_fixture();
        let service = ChainService::new(&f.store);
        let control = crate::core::chain::live::LiveControl::default();
        control.takeover();

        let outcome = service
            .repair_live(
                &[f.fingerprint.clone()],
                "run-2",
                None,
                &control,
                &mut |_| {},
            )
            .unwrap();

        assert!(outcome.aborted);
        assert!(outcome.outcome.is_none());
        // The dangling link is untouched and nothing was journaled.
        assert_eq!(std::fs::read_link(&f.entry).unwrap(), f.dead);
        assert!(service.repair_journal(None).unwrap().is_empty());
    }

    /// Pause blocks the runner at a step boundary until resumed.
    #[test]
    fn live_run_pause_blocks_until_resumed() {
        let f = live_fixture();
        let service = ChainService::new(&f.store);
        let control = crate::core::chain::live::LiveControl::default();
        control.pause();
        let resumer = control.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            resumer.resume();
        });

        let started = std::time::Instant::now();
        let outcome = service
            .repair_live(
                &[f.fingerprint.clone()],
                "run-3",
                None,
                &control,
                &mut |_| {},
            )
            .unwrap();
        handle.join().unwrap();

        assert!(started.elapsed() >= std::time::Duration::from_millis(120));
        assert!(!outcome.aborted);
        assert!(outcome.outcome.unwrap().verified);
    }

    /// A stale fingerprint fails the check step with a failed event.
    #[test]
    fn live_run_fails_the_check_step_for_a_vanished_finding() {
        let f = live_fixture();
        let service = ChainService::new(&f.store);
        let control = crate::core::chain::live::LiveControl::default();
        let mut events = Vec::new();

        let result = service.repair_live(
            &["fp-vanished".to_string()],
            "run-4",
            None,
            &control,
            &mut |event| events.push(event),
        );

        assert!(result.is_err());
        assert!(events
            .iter()
            .any(|event| event.step == "check" && event.status == "failed"));
        // Nothing was written.
        assert_eq!(std::fs::read_link(&f.entry).unwrap(), f.dead);
    }

    /// The issue-#33 storm loop at the service boundary: two dead links whose
    /// repo moved are detected as ONE root cause, batch-repaired in ONE apply
    /// (one journal record), and undone as a whole.
    #[test]
    fn repo_move_storm_batch_repairs_and_undoes_as_one_record() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        let new_repo = warehouse_root.join("xw-writing-v2");
        let project = projects_root.join("demo-project");
        git2::Repository::init(&new_repo).unwrap();

        // The moved repo holds BOTH skills; the old root is gone.
        let old_root = warehouse_root.join("xw-writing");
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        for skill in ["storm-alpha", "storm-beta"] {
            let original = new_repo.join("skills").join(skill);
            fs::create_dir_all(&original).unwrap();
            fs::write(
                original.join("SKILL.md"),
                format!("---\nname: {skill}\ndescription: Storm fixture\n---\n"),
            )
            .unwrap();
            symlink(old_root.join("skills").join(skill), surface.join(skill)).unwrap();
        }

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        // Common-cause analysis names the move and both members.
        let report = service.repo_moves().unwrap();
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.old_root, old_root.to_string_lossy());
        assert_eq!(group.new_root, new_repo.to_string_lossy());
        assert_eq!(group.skills, vec!["storm-alpha", "storm-beta"]);
        assert_eq!(group.fingerprints.len(), 2);

        // The batch plan relinks EVERY member; one apply → one journal record.
        let plan = service.plan_repair(&group.fingerprints).unwrap();
        assert!(plan.items.iter().all(|item| item.kind == "relink_broken"));
        let outcome = service.apply_repair(&plan).unwrap();
        assert!(outcome.verified);
        let journal_id = outcome.journal_id.expect("one journaled batch");
        for skill in ["storm-alpha", "storm-beta"] {
            assert_eq!(
                final_target_of(&service, &project, skill),
                new_repo.join("skills").join(skill).to_string_lossy()
            );
        }

        // One-click whole-storm rollback: both links dangle at the old root
        // again and both original fingerprints reappear.
        let undo = service.undo_repair(journal_id).unwrap();
        assert!(undo.verified);
        for skill in ["storm-alpha", "storm-beta"] {
            assert_eq!(
                std::fs::read_link(surface.join(skill)).unwrap(),
                old_root.join("skills").join(skill)
            );
        }
        let report = service.repo_moves().unwrap();
        assert_eq!(report.groups.len(), 1, "the storm is back after the undo");
    }

    fn final_target_of(service: &ChainService, project: &Path, skill: &str) -> String {
        let scanned = service.scan().unwrap();
        let proj = scanned
            .projects
            .iter()
            .find(|candidate| candidate.path == project.to_string_lossy())
            .unwrap();
        let claude = proj
            .surfaces
            .iter()
            .find(|surface| surface.agent == "claude")
            .unwrap();
        claude
            .entries
            .iter()
            .find(|entry| entry.name == skill)
            .unwrap()
            .final_target
            .clone()
    }

    /// A registered project outside the default discovery root is part of the
    /// topology and remains visible after a rescan and a restart.
    #[test]
    fn registered_project_outside_default_root_persists_across_restart() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let warehouse_root = projects_root.join("xw-skills");
        fs::create_dir_all(&warehouse_root).unwrap();
        // A project the discovery root would never reach (e.g. under Dropbox).
        let outside = temp.path().join("Dropbox").join("beta");
        fs::create_dir_all(&outside).unwrap();

        let db = temp.path().join("patchbay.db");
        {
            let store = SkillStore::new(&db).unwrap();
            store
                .set_setting(
                    "chain_warehouse_root",
                    warehouse_root.to_string_lossy().as_ref(),
                )
                .unwrap();
            store
                .set_setting(
                    "chain_projects_root",
                    projects_root.to_string_lossy().as_ref(),
                )
                .unwrap();
            let service = ChainService::new(&store);

            // Nothing is registered yet, so the topology is empty even though the
            // folder exists on disk — filesystem presence is no longer enough.
            assert!(service.scan().unwrap().projects.is_empty());

            // Enrolling it (the "select a folder for management" flow) persists it.
            service.enrol_project(&outside).unwrap();
            let scanned = service.scan().unwrap();
            assert_eq!(scanned.projects.len(), 1);
            assert_eq!(scanned.projects[0].name, "beta");
            assert_eq!(scanned.projects[0].path, outside.to_string_lossy());
        }

        // Restart: a fresh store over the same database still sees the project.
        {
            let store = SkillStore::new(&db).unwrap();
            let scanned = ChainService::new(&store).scan().unwrap();
            assert_eq!(scanned.projects.len(), 1);
            assert_eq!(scanned.projects[0].path, outside.to_string_lossy());
        }
    }

    /// Enrolment is keyed on canonical paths: aliases of one directory collapse
    /// to a single project, while same-named directories at different paths stay
    /// distinct.
    #[test]
    fn enrolment_dedupes_aliases_but_keeps_same_names_distinct() {
        let temp = tempdir().unwrap();
        let warehouse_root = temp.path().join("xw-skills");
        fs::create_dir_all(&warehouse_root).unwrap();
        let web_a = temp.path().join("a").join("web");
        let web_b = temp.path().join("b").join("web");
        fs::create_dir_all(web_a.join("nested")).unwrap();
        fs::create_dir_all(&web_b).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);

        service.enrol_project(&web_a).unwrap();
        service.enrol_project(&web_b).unwrap();
        // An alias of web_a must not add a third project.
        let alias = web_a.join("nested").join("..");
        service.enrol_project(&alias).unwrap();

        let scanned = service.scan().unwrap();
        assert_eq!(scanned.projects.len(), 2);
        assert!(scanned.projects.iter().all(|p| p.name == "web"));
        let mut paths: Vec<&str> = scanned.projects.iter().map(|p| p.path.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![web_a.to_str().unwrap(), web_b.to_str().unwrap()]
        );
    }

    /// Alias rows that predate canonical de-duplication (written directly by an
    /// older build or the file watcher) still collapse to one project at scan.
    #[test]
    fn topology_dedupes_pre_existing_alias_rows() {
        let temp = tempdir().unwrap();
        let warehouse_root = temp.path().join("xw-skills");
        fs::create_dir_all(&warehouse_root).unwrap();
        let proj = temp.path().join("alpha");
        fs::create_dir_all(proj.join("nested")).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .insert_project(&project_record("alpha", proj.to_string_lossy().to_string()))
            .unwrap();
        store
            .insert_project(&project_record(
                "alpha",
                proj.join("nested").join("..").to_string_lossy().to_string(),
            ))
            .unwrap();

        let scanned = ChainService::new(&store).scan().unwrap();
        assert_eq!(scanned.projects.len(), 1);
        assert_eq!(scanned.projects[0].name, "alpha");
    }

    /// Linked-agent workspaces are not project roots and never appear as chain
    /// projects.
    #[test]
    fn linked_workspaces_are_excluded_from_the_inventory() {
        let temp = tempdir().unwrap();
        let warehouse_root = temp.path().join("xw-skills");
        fs::create_dir_all(&warehouse_root).unwrap();
        let proj = temp.path().join("alpha");
        let linked = temp.path().join("global-skills");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&linked).unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        store
            .set_setting(
                "chain_warehouse_root",
                warehouse_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
            .insert_project(&project_record("alpha", proj.to_string_lossy().to_string()))
            .unwrap();
        let mut linked_record = project_record("global", linked.to_string_lossy().to_string());
        linked_record.workspace_type = "linked".to_string();
        store.insert_project(&linked_record).unwrap();

        let scanned = ChainService::new(&store).scan().unwrap();
        assert_eq!(scanned.projects.len(), 1);
        assert_eq!(scanned.projects[0].name, "alpha");
    }

    #[test]
    fn scan_covers_multiple_roots_tags_sources_and_reports_bad_root() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let root_a = temp.path().join("warehouse-a");
        let root_b = temp.path().join("warehouse-b");
        let missing = temp.path().join("warehouse-missing");
        let repo_a = root_a.join("repo-a");
        let repo_b = root_b.join("repo-b");
        let skill_a = repo_a.join("skills/skill-a");
        let skill_b = repo_b.join("skills/skill-b");

        fs::create_dir_all(&skill_a).unwrap();
        fs::create_dir_all(&skill_b).unwrap();
        fs::create_dir_all(&projects_root).unwrap();
        git2::Repository::init(&repo_a).unwrap();
        git2::Repository::init(&repo_b).unwrap();
        fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: skill-a\ndescription: A\n---\n",
        )
        .unwrap();
        fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: skill-b\ndescription: B\n---\n",
        )
        .unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        let roots_json = serde_json::to_string(&[
            root_a.to_string_lossy().to_string(),
            root_b.to_string_lossy().to_string(),
            missing.to_string_lossy().to_string(),
        ])
        .unwrap();
        store
            .set_setting("chain_warehouse_roots", &roots_json)
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();

        let topo = ChainService::new(&store).scan().unwrap();

        // Both valid roots contribute their repo, each tagged with its source root.
        let repo_a_info = topo.repos.iter().find(|r| r.name == "repo-a").unwrap();
        assert_eq!(repo_a_info.root, root_a.to_string_lossy());
        let repo_b_info = topo.repos.iter().find(|r| r.name == "repo-b").unwrap();
        assert_eq!(repo_b_info.root, root_b.to_string_lossy());

        // Per-root status is explicit: two ok, the missing root flagged (not empty).
        assert_eq!(topo.warehouse_roots.len(), 3);
        let missing_status = topo
            .warehouse_roots
            .iter()
            .find(|s| s.root == missing.to_string_lossy())
            .unwrap();
        assert_eq!(missing_status.status, "missing");
        assert_eq!(missing_status.repo_count, 0);
        assert!(missing_status.error.is_some());
        let ok_count = topo
            .warehouse_roots
            .iter()
            .filter(|s| s.status == "ok")
            .count();
        assert_eq!(ok_count, 2);
    }

    /// The guarded seam refuses to plan or apply against a project that was
    /// never registered, and a valid ops-level plan cannot be smuggled past the
    /// apply guard. No protected files are created either way.
    #[test]
    fn plan_and_apply_reject_unregistered_projects() {
        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);

        let plan_err = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap_err();
        assert!(matches!(plan_err.kind, ErrorKind::NotFound));

        // A plan built directly from the (unguarded) ops layer still cannot be
        // applied — the service re-checks registration before any write.
        let roots = vec![warehouse_root.clone()];
        let raw_plan = ops::plan_link(
            &project,
            &[original.clone()],
            &["claude".to_string()],
            &roots,
        )
        .unwrap();
        let apply_err = service.apply_link(&raw_plan).unwrap_err();
        assert!(matches!(apply_err.kind, ErrorKind::NotFound));

        assert!(!project.join(".agents").exists());
        assert!(!project.join(".claude").exists());
    }

    #[test]
    fn managed_central_skill_links_through_qoderwork_project_surface() {
        let temp = tempdir().unwrap();
        let managed_root = temp.path().join("central");
        let original = managed_root.join("ppt-master");
        let project = temp.path().join("consumer");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            original.join("SKILL.md"),
            "---\nname: ppt-master\ndescription: Managed fixture\n---\n",
        )
        .unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        let service = ChainService::with_managed_root(&store, managed_root.clone());
        let outcome = service
            .link(&project, &[original.clone()], &["qoderwork".to_string()])
            .unwrap();

        assert!(outcome.verified);
        assert_eq!(outcome.observed, ["ppt-master"]);
        let aggregate = project.join(".agents/skills/ppt-master");
        let qoderwork = project.join(".qoder/skills");
        assert_eq!(
            crate::core::chain::link_tracer::normalize(std::path::Path::new(
                &crate::core::chain::link_tracer::trace(&aggregate).final_target,
            )),
            crate::core::chain::link_tracer::normalize(&original)
        );
        assert_eq!(
            crate::core::chain::link_tracer::normalize(std::path::Path::new(
                &crate::core::chain::link_tracer::trace(&qoderwork).final_target,
            )),
            crate::core::chain::link_tracer::normalize(&project.join(".agents").join("skills"))
        );

        let topology = service.scan().unwrap();
        let central = topology
            .repos
            .iter()
            .find(|source| source.source_kind == "managed")
            .expect("managed source should be visible");
        assert_eq!(central.name, "Patchbay Central");
        assert!(central
            .skills
            .iter()
            .any(|skill| skill.name == "ppt-master"));
        assert_eq!(central.referenced_by.len(), 1);
        let project_chain = topology
            .projects
            .iter()
            .find(|candidate| candidate.path == project.to_string_lossy())
            .unwrap();
        let entry = project_chain
            .agents_dir
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.name == "ppt-master")
            .unwrap();
        assert_eq!(entry.status, "link_repo");
        assert_eq!(entry.repo.as_deref(), Some("Patchbay Central"));
        let surface = project_chain
            .surfaces
            .iter()
            .find(|surface| surface.agent == "qoderwork")
            .unwrap();
        assert_eq!(surface.kind, "dir_link");
        assert!(surface.dir_link_ok);
    }

    /// Two-phase flow: previewing writes nothing; applying the preview creates
    /// the chain and reports success only because a rescan observed it.
    #[test]
    fn plan_previews_then_apply_creates_and_is_verified_by_rescan() {
        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let plan = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();
        assert_eq!(plan.skills[0].action, "created");
        assert_eq!(plan.entries[0].action, "created");
        // Preview is read-only.
        assert!(!project.join(".agents").exists());
        assert!(!project.join(".claude").exists());

        let outcome = service.apply_link(&plan).unwrap();
        assert!(outcome.verified);
        assert_eq!(outcome.observed, ["demo-skill"]);
        assert!(outcome.missing.is_empty());
        // The chain is really on disk: .claude/skills/demo-skill -> original.
        let via = project.join(".claude/skills/demo-skill");
        let tr = crate::core::chain::link_tracer::trace(&via);
        assert!(tr.exists);
        assert_eq!(
            crate::core::chain::link_tracer::normalize(std::path::Path::new(&tr.final_target)),
            crate::core::chain::link_tracer::normalize(&original)
        );
    }

    /// AC5 shared-fixture equivalence proof. Drives the EXACT sequence the CLI's
    /// `chain link --apply` path runs — `enrol_project` → `plan_link` →
    /// `apply_link` — through the service and asserts both the resulting on-disk
    /// chain AND the audit records. Because the CLI is a thin delegator to these
    /// same methods (it constructs no plan or write of its own), this call
    /// sequence IS the CLI/GUI equivalence; the binary itself is not spawned
    /// (that would need a dev-dependency), matching #16's service-level approach.
    #[test]
    fn cli_link_flow_produces_equivalent_state_and_audit() {
        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);

        // The three service calls the CLI apply path makes, in order.
        service.enrol_project(&project).unwrap();
        let plan = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();
        let outcome = service.apply_link(&plan).unwrap();

        // Final state: a rescan verified the chain, the aggregate resolves into
        // the Original Repository, and the Original itself is never moved.
        assert!(outcome.verified);
        assert_eq!(outcome.observed, ["demo-skill"]);
        assert!(outcome.missing.is_empty());
        assert!(project
            .join(".agents")
            .join("skills")
            .join("demo-skill")
            .exists());
        assert!(project.join(".claude").join("skills").exists());
        assert!(original.join("SKILL.md").is_file());

        // Audit: one successful `chain_link` record per applied skill link and per
        // agent surface — the same records the GUI writes through apply_link.
        let audit = store.list_audit(None).unwrap();
        let links: Vec<_> = audit
            .iter()
            .filter(|entry| entry.action == "chain_link")
            .collect();
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|entry| entry.success));
        assert!(links
            .iter()
            .any(|entry| entry.skill_name.as_deref() == Some("demo-skill")));
        assert!(links
            .iter()
            .any(|entry| entry.skill_name.as_deref() == Some("claude")));
    }

    /// If the filesystem changes between preview and apply, the affected item is
    /// refused, the protected file is preserved, and success is NOT reported.
    #[test]
    fn apply_refuses_changed_evidence_and_is_not_verified() {
        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let plan = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();

        // A protected physical file appears where the plan intended to link.
        fs::create_dir_all(project.join(".agents").join("skills")).unwrap();
        let target = project.join(".agents").join("skills").join("demo-skill");
        fs::write(&target, "protected").unwrap();

        let outcome = service.apply_link(&plan).unwrap();
        assert!(!outcome.verified);
        assert!(outcome.missing.contains(&"demo-skill".to_string()) || outcome.observed.is_empty());
        assert!(outcome
            .report
            .skills
            .iter()
            .any(|item| item.action == "skipped"));
        // The protected file is intact.
        assert!(target.is_file());
        assert_eq!(fs::read_to_string(&target).unwrap(), "protected");

        // The refusal is recorded in the audit log.
        let audit = store.list_audit(None).unwrap();
        assert!(audit.iter().any(|entry| entry.action == "chain_link"));
    }

    /// The rescan verdict is keyed on canonical identity: linking through a path
    /// alias of the enrolled project still verifies, instead of falsely reporting
    /// the freshly written chain as missing.
    #[test]
    fn verify_matches_project_by_canonical_identity_not_raw_string() {
        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        // A trailing-slash alias: same directory, different string than the one
        // the registry and the scan report.
        let alias = PathBuf::from(format!("{}/", project.to_string_lossy()));
        let plan = service
            .plan_link(&alias, &[original.clone()], &["claude".to_string()])
            .unwrap();
        let outcome = service.apply_link(&plan).unwrap();
        assert!(
            outcome.verified,
            "an alias of the enrolled project should still verify"
        );
        assert_eq!(outcome.observed, ["demo-skill"]);
        assert!(outcome.missing.is_empty());
    }

    /// Reverse references key on the repository's canonical path, so two repos
    /// that share a name in different roots are never conflated: linking a skill
    /// from one leaves the same-named repo in the other root unreferenced.
    #[test]
    fn reverse_references_use_canonical_repo_identity_across_roots() {
        let temp = tempdir().unwrap();
        let projects_root = temp.path().join("Projects");
        let root_a = temp.path().join("warehouse-a");
        let root_b = temp.path().join("warehouse-b");
        // The conflation trap: same repo NAME ("shared") under two roots.
        let repo_a = root_a.join("shared");
        let repo_b = root_b.join("shared");
        let skill_a = repo_a.join("skills/skill-a");
        let skill_b = repo_b.join("skills/skill-b");
        let project = projects_root.join("consumer");

        fs::create_dir_all(&skill_a).unwrap();
        fs::create_dir_all(&skill_b).unwrap();
        fs::create_dir_all(&project).unwrap();
        git2::Repository::init(&repo_a).unwrap();
        git2::Repository::init(&repo_b).unwrap();
        fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: skill-a\ndescription: A\n---\n",
        )
        .unwrap();
        fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: skill-b\ndescription: B\n---\n",
        )
        .unwrap();

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        let roots_json = serde_json::to_string(&[
            root_a.to_string_lossy().to_string(),
            root_b.to_string_lossy().to_string(),
        ])
        .unwrap();
        store
            .set_setting("chain_warehouse_roots", &roots_json)
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);

        // Link a skill from root A's "shared" repo only.
        service
            .link(&project, &[skill_a.clone()], &["claude".to_string()])
            .unwrap();

        let topo = service.scan().unwrap();
        let both: Vec<_> = topo.repos.iter().filter(|r| r.name == "shared").collect();
        assert_eq!(both.len(), 2, "both same-named repos are present");

        let a = topo
            .repos
            .iter()
            .find(|r| r.path == repo_a.to_string_lossy())
            .unwrap();
        let b = topo
            .repos
            .iter()
            .find(|r| r.path == repo_b.to_string_lossy())
            .unwrap();
        // Only root A's "shared" is referenced; root B's same-named repo is not.
        assert_eq!(a.referenced_by.len(), 1);
        assert_eq!(a.referenced_by[0].name, "consumer");
        assert_eq!(a.referenced_by[0].path, project.to_string_lossy());
        assert!(
            b.referenced_by.is_empty(),
            "a same-named repo in another root must not be conflated"
        );
    }

    /// A clean-behind Original Repository is fast-forwarded through the public
    /// service, the attempt is audited, and the outcome carries a fresh rescan
    /// timestamp (AC3/AC6).
    #[test]
    fn plan_and_apply_pull_updates_audits_and_rescans() {
        use std::process::Command;

        fn git(dir: &std::path::Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "user.email=t@e.com",
                    "-c",
                    "user.name=T",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let temp = tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let seed = temp.path().join("seed");
        let work = temp.path().join("repo");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-b", "main"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        fs::write(seed.join("file.txt"), "base").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-m", "base"]);
        git(&seed, &["push", "origin", "main"]);
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&work)
            .output()
            .unwrap()
            .status
            .success());

        // A second checkout advances the remote; `work` fetches so it becomes
        // clean & behind without moving HEAD.
        let other = temp.path().join("other");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&other)
            .output()
            .unwrap()
            .status
            .success());
        fs::write(other.join("file.txt"), "advanced").unwrap();
        git(&other, &["commit", "-am", "advance"]);
        git(&other, &["push", "origin", "main"]);
        git(&work, &["fetch", "origin"]);

        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        let service = ChainService::new(&store);

        let plan = service
            .plan_pull(&[work.to_string_lossy().to_string()])
            .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].action, "fast_forward");

        let outcome = service.apply_pull(&plan).unwrap();
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(
            outcome.results[0].action, "updated",
            "{:?}",
            outcome.results[0]
        );
        assert!(outcome.scanned_at > 0, "outcome stamps a fresh rescan time");
        // The working tree was fast-forwarded to the remote head.
        assert_eq!(
            fs::read_to_string(work.join("file.txt")).unwrap(),
            "advanced"
        );

        // Each attempted repository is audited (AC6).
        let audit = store.list_audit(None).unwrap();
        let entry = audit
            .iter()
            .find(|e| e.action == "chain_pull")
            .expect("a chain_pull audit entry was written");
        assert_eq!(entry.skill_name.as_deref(), Some("repo"));
        assert!(entry.success, "an updated pull is audited as successful");
        assert!(entry.detail.as_deref().unwrap_or("").starts_with("updated"));
    }

    #[test]
    fn remediate_links_into_project_then_removes_verified_global_symlink() {
        use crate::core::chain::remediate::RemediationPlan;

        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        store
            .set_setting(
                "chain_projects_root",
                project.parent().unwrap().to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();
        let link_plan = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();

        // A global-surface symlink exposing the same Original — the violation.
        let global = temp.path().join("global-claude/skills/demo-skill");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        symlink(&original, &global).unwrap();

        let plan = RemediationPlan {
            global_path: global.to_string_lossy().to_string(),
            skill: "demo-skill".to_string(),
            agent: "Claude Code".to_string(),
            final_target: original.to_string_lossy().to_string(),
            is_link: true,
            project: project.to_string_lossy().to_string(),
            agents: vec!["claude".to_string()],
            link_plan: Some(link_plan),
            remove_global: true,
            global_evidence: ops::observe(&global),
            guidance: None,
        };

        let outcome = service.apply_remediate(&plan).unwrap();
        assert!(
            outcome.link.as_ref().unwrap().verified,
            "the project link verifies"
        );
        assert!(outcome.global_removed);
        assert!(outcome.verified);
        assert!(
            !global.exists(),
            "a verified remediation retires the global symlink"
        );
        assert!(
            original.join("SKILL.md").is_file(),
            "the Original is untouched"
        );
        assert!(
            project
                .join(".agents")
                .join("skills")
                .join("demo-skill")
                .exists(),
            "the project-local chain is established"
        );
    }

    #[test]
    fn remediate_never_deletes_a_physical_global_entry() {
        use crate::core::chain::remediate::RemediationPlan;

        let (temp, warehouse_root, _original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let global = temp.path().join("global-claude/skills/demo-skill");
        fs::create_dir_all(&global).unwrap();
        fs::write(global.join("SKILL.md"), "---\nname: demo-skill\n---\n").unwrap();

        let plan = RemediationPlan {
            global_path: global.to_string_lossy().to_string(),
            skill: "demo-skill".to_string(),
            agent: "Claude Code".to_string(),
            final_target: global.to_string_lossy().to_string(),
            is_link: false,
            project: project.to_string_lossy().to_string(),
            agents: vec!["claude".to_string()],
            link_plan: None,
            remove_global: false,
            global_evidence: ops::observe(&global),
            guidance: None,
        };

        let outcome = service.apply_remediate(&plan).unwrap();
        assert!(outcome.link.is_none());
        assert!(!outcome.global_removed);
        assert!(
            outcome.guidance.is_some(),
            "a physical entry gets manual guidance"
        );
        assert!(
            global.join("SKILL.md").is_file(),
            "a physical global Skill directory is never deleted (AC4)"
        );
    }

    #[test]
    fn remediate_leaves_global_untouched_when_the_link_fails() {
        use crate::core::chain::remediate::RemediationPlan;

        let (temp, warehouse_root, original, project) = guarded_fixture();
        let store = guarded_store(&temp, &warehouse_root);
        store
            .set_setting(
                "chain_projects_root",
                project.parent().unwrap().to_string_lossy().as_ref(),
            )
            .unwrap();
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();
        let link_plan = service
            .plan_link(&project, &[original.clone()], &["claude".to_string()])
            .unwrap();

        let global = temp.path().join("global-claude/skills/demo-skill");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        symlink(&original, &global).unwrap();

        // Occupy the aggregate target with a physical dir after planning so the
        // link's TOCTOU guard refuses it and the link never verifies.
        fs::create_dir_all(project.join(".agents").join("skills").join("demo-skill")).unwrap();

        let plan = RemediationPlan {
            global_path: global.to_string_lossy().to_string(),
            skill: "demo-skill".to_string(),
            agent: "Claude Code".to_string(),
            final_target: original.to_string_lossy().to_string(),
            is_link: true,
            project: project.to_string_lossy().to_string(),
            agents: vec!["claude".to_string()],
            link_plan: Some(link_plan),
            remove_global: true,
            global_evidence: ops::observe(&global),
            guidance: None,
        };

        let outcome = service.apply_remediate(&plan).unwrap();
        assert!(
            !outcome.global_removed,
            "a failed link must leave the global entry in place"
        );
        assert!(
            global.exists(),
            "the global symlink is untouched when the link fails (AC3)"
        );
    }
}

/// Doctor is exercised through the same public contract as the rest of the
/// service: temporary warehouse repositories, a registered project, and real
/// agent surfaces, then `service.doctor(...)`. Assertions observe the returned
/// contract (rule ids, severity, evidence, actions, filtering) rather than any
/// private rule ordering.
#[cfg(test)]
mod doctor_tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::ChainService;
    use crate::core::chain::decisions::DecisionStatus;
    use crate::core::chain::doctor::{Deviation, DoctorFilter, Severity};
    use crate::core::skill_store::SkillStore;

    /// Portable stand-in for `std::os::unix::fs::symlink`. These fixtures link
    /// directories, and gating this module on unix meant the whole Doctor rule
    /// suite was skipped on Windows.
    fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        crate::core::test_support::symlink_dir(target.as_ref(), link.as_ref())
    }

    fn make_skill(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: x\ndescription: fixture\n---\n",
        )
        .unwrap();
    }

    fn store_at(db: &Path, warehouse: &Path, projects_root: &Path) -> SkillStore {
        let store = SkillStore::new(&db.to_path_buf()).unwrap();
        store
            .set_setting("chain_warehouse_root", warehouse.to_string_lossy().as_ref())
            .unwrap();
        store
            .set_setting(
                "chain_projects_root",
                projects_root.to_string_lossy().as_ref(),
            )
            .unwrap();
        store
    }

    /// The full fixture matrix: broken, cyclic, direct, unmanaged copy,
    /// project-private, legacy, and orphan all present at once.
    #[test]
    fn doctor_reports_every_deviation_with_stable_contract() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let legacy_shared = temp.path().join("legacy-shared");

        // Tier 1: one referenced repo (good + directlink) and one orphan repo.
        let repo = warehouse.join("repo");
        make_skill(&repo.join("good"));
        make_skill(&repo.join("directlink"));
        git2::Repository::init(&repo).unwrap();
        let lonely = warehouse.join("lonely");
        make_skill(&lonely.join("s"));
        git2::Repository::init(&lonely).unwrap();

        // A retired shared-distribution layer (outside every warehouse root).
        make_skill(&legacy_shared.join("legacyagg"));

        // Tier 2/3: one project holding every deviation.
        let project = projects_root.join("demo");
        let agg = project.join(".agents").join("skills");
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&agg).unwrap();
        fs::create_dir_all(&surface).unwrap();

        // .agents/skills: healthy link_repo, project-private, broken, legacy.
        symlink(repo.join("good"), agg.join("good")).unwrap();
        make_skill(&agg.join("mine"));
        symlink(temp.path().join("nowhere"), agg.join("dead")).unwrap();
        symlink(legacy_shared.join("legacyagg"), agg.join("legacyagg")).unwrap();

        // .claude/skills (per-entry): direct, unmanaged copy, cyclic pair.
        symlink(repo.join("directlink"), surface.join("directlink")).unwrap();
        make_skill(&surface.join("copydir"));
        symlink(surface.join("cyclicB"), surface.join("cyclicA")).unwrap();
        symlink(surface.join("cyclicA"), surface.join("cyclicB")).unwrap();

        let store = store_at(&temp.path().join("patchbay.db"), &warehouse, &projects_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let report = service.doctor(&DoctorFilter::default()).unwrap();
        assert_eq!(report.total, report.findings.len());

        let rules: Vec<&str> = report.findings.iter().map(|f| f.rule.as_str()).collect();
        for expected in [
            "chain.broken_link",
            "chain.direct_link",
            "chain.unmanaged_copy",
            "chain.project_private",
            "chain.legacy_layer",
            "chain.orphan_original",
        ] {
            assert!(rules.contains(&expected), "missing {expected} in {rules:?}");
        }

        // Every finding carries the required fields, none empty where the
        // contract promises content.
        for finding in &report.findings {
            assert!(!finding.rule.is_empty());
            assert!(!finding.fingerprint.is_empty());
            assert!(!finding.affected.is_empty());
            assert!(!finding.actions.is_empty());
            assert!(!finding.evidence.entry_path.is_empty());
        }

        // Opening a broken finding exposes the same chain evidence Link Topology
        // would show: the traced final target, straight from link_tracer.
        let broken = report
            .findings
            .iter()
            // `Path::ends_with` matches whole components, so it is separator
            // agnostic; `str::ends_with("/dead")` would never match on Windows.
            .find(|f| {
                f.rule == "chain.broken_link" && Path::new(&f.evidence.entry_path).ends_with("dead")
            })
            .unwrap();
        assert_eq!(broken.severity, Severity::Violation);
        assert!(Path::new(&broken.evidence.final_target).ends_with("nowhere"));

        // The orphan points at the unreferenced repo, not the referenced one.
        let orphan = report
            .findings
            .iter()
            .find(|f| f.rule == "chain.orphan_original")
            .unwrap();
        assert_eq!(orphan.evidence.entry_path, lonely.to_string_lossy());
    }

    #[test]
    fn doctor_filters_by_severity_and_type() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let repo = warehouse.join("repo");
        make_skill(&repo.join("directlink"));
        git2::Repository::init(&repo).unwrap();

        let project = projects_root.join("demo");
        let surface = project.join(".claude").join("skills");
        fs::create_dir_all(&surface).unwrap();
        symlink(repo.join("directlink"), surface.join("directlink")).unwrap();
        symlink(temp.path().join("nowhere"), surface.join("dead")).unwrap();

        let store = store_at(&temp.path().join("patchbay.db"), &warehouse, &projects_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        // Severity filter keeps only the broken (violation) finding; `total`
        // still reflects the pre-filter count.
        let violations = service
            .doctor(&DoctorFilter {
                severities: vec![Severity::Violation],
                deviations: Vec::new(),
            })
            .unwrap();
        assert!(violations.total >= 2);
        assert!(violations
            .findings
            .iter()
            .all(|f| f.rule == "chain.broken_link"));
        assert!(!violations.findings.is_empty());

        // Type filter keeps only the direct-link finding.
        let direct = service
            .doctor(&DoctorFilter {
                severities: Vec::new(),
                deviations: vec![Deviation::Direct],
            })
            .unwrap();
        assert!(direct
            .findings
            .iter()
            .all(|f| f.rule == "chain.direct_link"));
        assert!(!direct.findings.is_empty());
    }

    /// A project in the canonical shape yields a clean report — Doctor must
    /// handle "nothing wrong" as a first-class outcome.
    #[test]
    fn doctor_clean_project_reports_nothing() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let repo = warehouse.join("repo");
        make_skill(&repo.join("demo-skill"));
        git2::Repository::init(&repo).unwrap();

        // Canonical chain: .agents/skills/<n> → repo, .claude/skills → .agents/skills.
        let project = projects_root.join("demo");
        let agg = project.join(".agents").join("skills");
        fs::create_dir_all(&agg).unwrap();
        fs::create_dir_all(project.join(".claude")).unwrap();
        symlink(repo.join("demo-skill"), agg.join("demo-skill")).unwrap();
        symlink(&agg, project.join(".claude").join("skills")).unwrap();

        let store = store_at(&temp.path().join("patchbay.db"), &warehouse, &projects_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();

        let report = service.doctor(&DoctorFilter::default()).unwrap();
        assert_eq!(
            report.total, 0,
            "unexpected findings: {:?}",
            report.findings
        );
        assert!(report.findings.is_empty());
    }

    #[test]
    fn decide_preview_is_read_only_then_apply_hides_and_audits_finding() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let repo = warehouse.join("repo");
        make_skill(&repo.join("original"));
        git2::Repository::init(&repo).unwrap();

        let project = projects_root.join("demo");
        let private = project.join(".agents/skills/private");
        make_skill(&private);

        let store = store_at(&temp.path().join("patchbay.db"), &warehouse, &projects_root);
        let service = ChainService::new(&store);
        service.enrol_project(&project).unwrap();
        let finding = service
            .doctor(&DoctorFilter::default())
            .unwrap()
            .findings
            .into_iter()
            .find(|finding| finding.rule == "chain.project_private")
            .expect("project-private finding");

        let plan = service
            .plan_decisions(
                std::slice::from_ref(&finding.fingerprint),
                "project_private",
            )
            .unwrap();
        assert!(plan.ok);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].status, DecisionStatus::Persist);
        assert!(
            store.list_audit(None).unwrap().is_empty(),
            "preview must not write audit or decision state"
        );
        assert!(service
            .doctor(&DoctorFilter::default())
            .unwrap()
            .findings
            .iter()
            .any(|candidate| candidate.fingerprint == finding.fingerprint));

        let outcome = service.apply_decisions(&plan).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.items[0].status, DecisionStatus::Applied);
        let after = service.doctor(&DoctorFilter::default()).unwrap();
        assert!(!after
            .findings
            .iter()
            .any(|candidate| candidate.fingerprint == finding.fingerprint));
        assert!(after
            .ignored
            .iter()
            .any(|candidate| candidate.fingerprint == finding.fingerprint));

        let audits = store.list_audit(None).unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "chain_decide");
        assert!(audits[0].success);
        assert!(audits[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("project_private"));
    }

    #[test]
    fn decide_applies_valid_items_reports_sibling_errors_and_rejects_private_orphan() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let repo = warehouse.join("repo");
        make_skill(&repo.join("orphan"));
        git2::Repository::init(&repo).unwrap();
        let store = store_at(&temp.path().join("patchbay.db"), &warehouse, &projects_root);
        let service = ChainService::new(&store);
        let orphan = service
            .doctor(&DoctorFilter::default())
            .unwrap()
            .findings
            .into_iter()
            .find(|finding| finding.rule == "chain.orphan_original")
            .expect("orphan finding");

        let unsupported = service
            .plan_decisions(std::slice::from_ref(&orphan.fingerprint), "project_private")
            .unwrap();
        assert!(!unsupported.ok);
        assert_eq!(
            unsupported.items[0].error_code.as_deref(),
            Some("action_not_supported")
        );

        let plan = service
            .plan_decisions(
                &[
                    orphan.fingerprint.clone(),
                    "missing-fingerprint".to_string(),
                ],
                "ignored",
            )
            .unwrap();
        assert!(!plan.ok);
        assert_eq!(plan.items[0].status, DecisionStatus::Persist);
        assert_eq!(plan.items[1].status, DecisionStatus::Error);
        assert_eq!(
            plan.items[1].error_code.as_deref(),
            Some("finding_not_found")
        );

        let outcome = service.apply_decisions(&plan).unwrap();
        assert!(!outcome.ok, "one failed sibling makes the command fail");
        assert_eq!(outcome.items[0].status, DecisionStatus::Applied);
        assert_eq!(outcome.items[1].status, DecisionStatus::Error);
        let after = service.doctor(&DoctorFilter::default()).unwrap();
        assert!(after
            .ignored
            .iter()
            .any(|finding| finding.fingerprint == orphan.fingerprint));

        let audits = store.list_audit(None).unwrap();
        assert_eq!(audits.len(), 2, "every requested item is audited");
        assert_eq!(audits.iter().filter(|entry| entry.success).count(), 1);
        assert_eq!(audits.iter().filter(|entry| !entry.success).count(), 1);
    }

    /// The full ignore lifecycle through the public service, over a real Git
    /// warehouse + SkillStore: classify/ignore hides a finding, a materially
    /// changed chain reappears, restore un-hides, the decision survives a
    /// reopened store, and no Skill is ever moved or rewritten.
    #[test]
    fn ignore_hides_survives_restart_and_reappears_on_changed_evidence() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects_root = temp.path().join("Projects");
        let repo = warehouse.join("repo");
        make_skill(&repo.join("good"));
        git2::Repository::init(&repo).unwrap();

        // One project with two aggregate deviations: a legitimate physical
        // project-private Skill, and a broken link we can re-point later.
        let project = projects_root.join("demo");
        let agg = project.join(".agents").join("skills");
        fs::create_dir_all(&agg).unwrap();
        make_skill(&agg.join("mine")); // project_private finding + AC5 witness
        let dead = agg.join("dead");
        symlink(temp.path().join("nowhere"), &dead).unwrap(); // broken finding

        let db = temp.path().join("patchbay.db");
        // The broken finding's fingerprint crosses the "restart" boundary; the
        // project-private identifiers stay scoped to the first store.
        let broken_fp = {
            let store = store_at(&db, &warehouse, &projects_root);
            let service = ChainService::new(&store);
            service.enrol_project(&project).unwrap();

            // Both deviations are visible and nothing is ignored yet.
            let before = service.doctor(&DoctorFilter::default()).unwrap();
            assert!(before.ignored.is_empty());
            let private = before
                .findings
                .iter()
                .find(|f| f.rule == "chain.project_private")
                .expect("project-private finding present");
            let broken = before
                .findings
                .iter()
                .find(|f| f.rule == "chain.broken_link")
                .expect("broken finding present");
            let private_rule = private.rule.clone();
            let private_fp = private.fingerprint.clone();
            let broken_rule = broken.rule.clone();
            let broken_fp = broken.fingerprint.clone();

            // AC1/AC2: classify the physical Skill as project-private.
            service
                .ignore_finding(&private_rule, &private_fp, "project_private", None)
                .unwrap();
            let after = service.doctor(&DoctorFilter::default()).unwrap();
            assert!(
                !after
                    .findings
                    .iter()
                    .any(|f| f.rule == "chain.project_private"),
                "classified finding is hidden from the visible set"
            );
            assert!(
                after.ignored.iter().any(|f| f.fingerprint == private_fp),
                "classified finding is listed under ignored for restore"
            );
            // `total` counts only visible findings; ignored is excluded.
            assert_eq!(after.total, after.findings.len());

            // AC5: classifying never moved or rewrote the physical Skill.
            assert!(agg.join("mine").join("SKILL.md").is_file());
            assert_eq!(
                fs::read_to_string(agg.join("mine").join("SKILL.md")).unwrap(),
                "---\nname: x\ndescription: fixture\n---\n"
            );

            // AC2: restore brings it back into the visible set.
            service.restore_finding(&private_rule, &private_fp).unwrap();
            let restored = service.doctor(&DoctorFilter::default()).unwrap();
            assert!(restored
                .findings
                .iter()
                .any(|f| f.rule == "chain.project_private"));
            assert!(restored.ignored.is_empty());

            // Now ignore the broken finding so persistence can be checked.
            service
                .ignore_finding(&broken_rule, &broken_fp, "ignored", None)
                .unwrap();

            broken_fp
        };

        // AC3: a fresh service over a reopened store still hides the ignored
        // finding — the decision persisted across "restart".
        {
            let store = store_at(&db, &warehouse, &projects_root);
            let service = ChainService::new(&store);
            let report = service.doctor(&DoctorFilter::default()).unwrap();
            assert!(
                !report
                    .findings
                    .iter()
                    .any(|f| f.rule == "chain.broken_link"),
                "ignore record must survive a reopened store"
            );
            assert!(report.ignored.iter().any(|f| f.fingerprint == broken_fp));

            // AC4: re-point the broken link so its final_target — and therefore
            // its fingerprint — changes. The stale decision no longer matches,
            // so the finding reappears with a different fingerprint.
            crate::core::chain::ops::remove_symlink(&dead).unwrap();
            symlink(temp.path().join("elsewhere"), &dead).unwrap();

            let changed = service.doctor(&DoctorFilter::default()).unwrap();
            let broken_now = changed
                .findings
                .iter()
                .find(|f| f.rule == "chain.broken_link")
                .expect("materially changed broken finding reappears");
            assert_ne!(
                broken_now.fingerprint, broken_fp,
                "changed evidence yields a new fingerprint"
            );
            assert!(
                changed.ignored.is_empty(),
                "the stale decision now matches nothing"
            );
        }
    }
}
