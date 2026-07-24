//! Application-level facade for the instructions governance surface.
//!
//! Mirrors `chain::ChainService`: the CLI, and future Tauri commands / GUI, call
//! this single service so registered-project identity, the installed-agent
//! catalogue, and structured results stay aligned across entry points. Every
//! method here is read-only (P0).

use std::path::{Path, PathBuf};

use crate::core::chain::decisions::{self, FindingDecision};
use crate::core::{error::AppError, project_registry, skill_store::SkillStore};

use super::doctor::{self, DoctorFilter, DoctorReport};
use super::init::{self, InitOutcome, InitPlan};
use super::normalize::{self, NormalizeOutcome, NormalizePlan};
use super::scanner::{self, AgentReadChain, ScanReport};
use super::surfaces::{self, Agent};
use super::{snapshot, write_guard};

/// Read-only contract for scanning and resolving instructions surfaces.
pub struct InstructionsService<'a> {
    store: &'a SkillStore,
}

impl<'a> InstructionsService<'a> {
    pub fn new(store: &'a SkillStore) -> Self {
        Self { store }
    }

    /// The user's home directory, or an error when it cannot be determined.
    fn home() -> Result<PathBuf, AppError> {
        dirs::home_dir().ok_or_else(|| AppError::invalid_input("Cannot determine home directory"))
    }

    /// Distinct registered project paths (workspace projects only), de-duplicated
    /// by canonical identity — the same rule the chain module scans by.
    fn registered_project_paths(&self) -> Result<Vec<PathBuf>, AppError> {
        let mut seen = std::collections::HashSet::new();
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

    /// Scan instructions surfaces. With `project`, scans exactly that path (an
    /// explicit read of any directory); without it, scans every registered
    /// project. Global surfaces and the installed-agent set are resolved live.
    pub fn scan(&self, project: Option<&Path>) -> Result<ScanReport, AppError> {
        let home = Self::home()?;
        let installed = surfaces::installed_agents_live(&home);
        let project_paths = match project {
            Some(p) => vec![p.to_path_buf()],
            None => self.registered_project_paths()?,
        };
        let scanned_at = chrono::Utc::now().timestamp_millis();
        Ok(scanner::scan_with(
            &project_paths,
            &home,
            &installed,
            scanned_at,
        ))
    }

    /// Per-agent read chain for one project. `agent` narrows to a single agent
    /// key (validated against the catalogue); omitted, every installed agent is
    /// reported. Read-only; the path need not be registered.
    pub fn where_chain(
        &self,
        project: &Path,
        agent: Option<&str>,
    ) -> Result<Vec<AgentReadChain>, AppError> {
        let home = Self::home()?;
        let agents = match agent {
            Some(key) => vec![Agent::from_key(key)
                .ok_or_else(|| AppError::invalid_input(format!("unknown agent key: {key}")))?],
            None => surfaces::installed_agents_live(&home),
        };
        Ok(scanner::where_with(project, &home, &agents))
    }

    /// Read-only Doctor over the same scan the cost view uses (design §3). With
    /// `project`, diagnoses exactly that path; without it, every registered
    /// project plus the machine's global surfaces. Never writes or evaluates
    /// content.
    ///
    /// Persisted ignore decisions — reused from chain's shared decision store,
    /// keyed by rule + evidence fingerprint — split the raw findings into a
    /// visible set and an `ignored` set. The severity/rule filter narrows only the
    /// visible set; `ignored` is returned unfiltered so the GUI's restore panel is
    /// complete. `total` is the visible count before the filter (for "N of M").
    pub fn doctor(
        &self,
        filter: &DoctorFilter,
        project: Option<&Path>,
    ) -> Result<DoctorReport, AppError> {
        let scan = self.scan(project)?;
        let home = Self::home()?;
        let all = doctor::diagnose(&scan, &home);
        let decisions = decisions::load(self.store)?;
        let (visible, ignored) = doctor::partition_by_decisions(all, &decisions);
        let total = visible.len();
        Ok(DoctorReport {
            findings: filter.apply(visible),
            ignored,
            total,
            scanned_at: scan.scanned_at,
        })
    }

    /// Hide a Doctor finding, keyed by its rule and evidence fingerprint. The
    /// instructions module only records `ignored` (design §3: `project_private`
    /// does not apply). Idempotent on `(rule, fingerprint)`; touches only the
    /// shared settings-table decision store, never any file.
    pub fn ignore_finding(
        &self,
        rule: &str,
        fingerprint: &str,
        note: Option<String>,
    ) -> Result<(), AppError> {
        let decision = FindingDecision {
            rule: rule.to_string(),
            fingerprint: fingerprint.to_string(),
            kind: decisions::KIND_IGNORED.to_string(),
            note,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        decisions::add(self.store, decision)
    }

    /// Restore a previously ignored finding by removing its decision record — it
    /// reappears on the next diagnose while its evidence still matches. Touches
    /// only the settings table.
    pub fn restore_finding(&self, rule: &str, fingerprint: &str) -> Result<(), AppError> {
        decisions::remove(self.store, rule, fingerprint)
    }

    /// Preview a `normalize` over `project` (design §4.1). Re-scans and
    /// re-diagnoses so the plan is built from CURRENT evidence, then plans the
    /// fixable findings (all, or only `fingerprints` when given). Entirely
    /// read-only — no enrolment, no write.
    pub fn plan_normalize(
        &self,
        project: &Path,
        fingerprints: &[String],
    ) -> Result<NormalizePlan, AppError> {
        let scan = self.scan(Some(project))?;
        let home = Self::home()?;
        let findings = doctor::diagnose(&scan, &home);
        Ok(normalize::plan(
            &findings,
            project,
            fingerprints,
            scan.scanned_at,
        ))
    }

    /// Apply a previewed normalize plan. Enrolling the project is the explicit
    /// "收编批准" of naming `--project` (same enrolment semantics as chain link
    /// apply). Every write goes through the §8 guard stack: originals are
    /// snapshotted first, each mutation re-verifies its TOCTOU baseline and the
    /// write-target whitelist, and the canonical is create-only. One audit record
    /// carries the snapshot id and the changed files, then a fresh rescan VERIFIES
    /// that every fixed finding is gone before success is reported.
    pub fn apply_normalize(
        &self,
        project: &Path,
        plan: &NormalizePlan,
    ) -> Result<NormalizeOutcome, AppError> {
        // Enrolment gate: naming the project approves adopting it (parity with
        // chain link apply). A relative/odd path is normalized by the registry.
        project_registry::register_project(self.store, project, false).map_err(AppError::db)?;

        let snapshot_root =
            snapshot::default_root().map_err(|e| AppError::internal(e.to_string()))?;
        let now = chrono::Utc::now().timestamp_millis();
        let (results, snapshot_id) = normalize::apply(plan, project, &snapshot_root, now);

        // One audit record for the apply: the snapshot id and the files it changed
        // (design §7). Logged only when at least one write actually landed.
        let changed: Vec<String> = results
            .iter()
            .filter(|i| normalize::is_success(&i.action) && i.action != "noop")
            .map(|i| i.path.clone())
            .collect();
        if !changed.is_empty() {
            let draft = write_guard::audit_draft(
                write_guard::ACTION_NORMALIZE,
                Some("claude"),
                snapshot_id.as_deref().unwrap_or("none"),
                &changed,
            )
            .ok();
            self.store.log_audit(draft);
        }

        // Success is observed, not assumed: a fresh diagnosis must show every fixed
        // finding gone (design §4.1 / AC6).
        let rescan = self.scan(Some(project))?;
        let home = Self::home()?;
        let rediagnosed = doctor::diagnose(&rescan, &home);
        let verified = normalize::verify(&results, &rediagnosed);
        Ok(NormalizeOutcome {
            items: results,
            snapshot_id,
            verified,
            scanned_at: rescan.scanned_at,
        })
    }

    /// Preview an `init` scaffold over `project` (design §4.2). The plan lists the
    /// canonical skeleton, the per-agent wrapper entries, and — with `docs_dir` —
    /// the docs directory, each as create-or-noop. Read-only.
    pub fn plan_init(&self, project: &Path, docs_dir: bool) -> Result<InitPlan, AppError> {
        let home = Self::home()?;
        let installed = surfaces::installed_agents_live(&home);
        let scanned_at = chrono::Utc::now().timestamp_millis();
        Ok(init::plan(project, &installed, docs_dir, scanned_at))
    }

    /// Apply a previewed init plan. Naming `--project` enrols it (same adoption
    /// approval as normalize / chain link). File creates go through the §8
    /// create-only whitelist guard; init never overwrites, so there is nothing to
    /// snapshot. One `instructions_init` audit record lists the created targets,
    /// then the outcome is verified (every intended target now exists).
    pub fn apply_init(&self, project: &Path, plan: &InitPlan) -> Result<InitOutcome, AppError> {
        project_registry::register_project(self.store, project, false).map_err(AppError::db)?;

        let results = init::apply(plan, project);
        let verified = init::verify(&results);

        let created: Vec<String> = results
            .iter()
            .filter(|i| i.action == "create")
            .map(|i| i.path.clone())
            .collect();
        if !created.is_empty() {
            // Init writes no rewrite/removal, so there is no snapshot id to record.
            let draft = write_guard::audit_draft(
                write_guard::ACTION_INIT,
                Some("claude"),
                "none",
                &created,
            )
            .ok();
            self.store.log_audit(draft);
        }

        let scanned_at = self.scan(Some(project))?.scanned_at;
        Ok(InitOutcome {
            items: results,
            verified,
            scanned_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ignore_and_restore_persist_and_clear_a_decision() {
        let dir = tempdir().unwrap();
        let store = SkillStore::new(&dir.path().join("patchbay.db")).unwrap();
        let service = InstructionsService::new(&store);

        service
            .ignore_finding(
                "instructions.dual_body",
                "fp-abc",
                Some("noise".to_string()),
            )
            .unwrap();
        let stored = decisions::load(&store).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].rule, "instructions.dual_body");
        assert_eq!(stored[0].fingerprint, "fp-abc");
        // The instructions module only ever records the generic `ignored` kind.
        assert_eq!(stored[0].kind, decisions::KIND_IGNORED);

        service
            .restore_finding("instructions.dual_body", "fp-abc")
            .unwrap();
        assert!(decisions::load(&store).unwrap().is_empty());
    }
}
