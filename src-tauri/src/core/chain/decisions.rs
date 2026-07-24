//! Persisted user decisions that hide individual Doctor findings.
//!
//! A decision is keyed by a finding's rule id plus its evidence fingerprint —
//! the SHA-256 Doctor already computes over `rule + [entry_path, final_target,
//! topology_status]`. Keying on that fingerprint is what keeps a stale decision
//! from hiding materially changed evidence: re-pointing a link (or otherwise
//! changing the deviation's material evidence) yields a new fingerprint the old
//! decision no longer matches, so the finding is reconsidered and reappears.
//!
//! Decisions live entirely in the `settings` table as a JSON array under
//! `chain_finding_decisions`, mirroring `roots.rs`'s load/save shape. Recording
//! or removing a decision only touches that key — it never reads, moves, or
//! rewrites any Skill contents.

use serde::{Deserialize, Serialize};

use crate::core::{error::AppError, skill_store::SkillStore};

use super::doctor::Finding;

/// Settings key holding the JSON array of persisted decisions.
const DECISIONS_KEY: &str = "chain_finding_decisions";

/// Generic accept: hide an accepted Doctor finding.
pub const KIND_IGNORED: &str = "ignored";
/// Classify a legitimate physical project Skill as project-private.
pub const KIND_PROJECT_PRIVATE: &str = "project_private";

/// A persisted user decision to hide a Doctor finding, keyed by rule + the
/// finding's evidence fingerprint so a materially changed chain is reconsidered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FindingDecision {
    pub rule: String,
    pub fingerprint: String,
    /// "ignored" (generic accept) | "project_private" (legitimate physical Skill).
    pub kind: String,
    pub note: Option<String>,
    pub created_at: i64,
}

/// Processing state for one requested Doctor decision. The serialized values
/// are part of the CLI contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Persist,
    Noop,
    Applied,
    Error,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Persist => "persist",
            Self::Noop => "noop",
            Self::Applied => "applied",
            Self::Error => "error",
        }
    }

    pub fn is_error(self) -> bool {
        self == Self::Error
    }
}

/// One requested Doctor decision in a preview or apply result. `status` is
/// serialized as `action` for compatibility with the other Chain mutation
/// results. Errors carry a stable machine-readable code for CLI automation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionItem {
    pub fingerprint: String,
    pub rule: Option<String>,
    pub kind: String,
    #[serde(rename = "action")]
    pub status: DecisionStatus,
    pub error_code: Option<String>,
    pub message: Option<String>,
}

/// Read-only preview for one `chain decide` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPlan {
    pub ok: bool,
    pub kind: String,
    pub fingerprints: Vec<String>,
    pub items: Vec<DecisionItem>,
    pub scanned_at: i64,
}

/// Result of applying a decision preview after re-resolving every fingerprint
/// against current Doctor evidence.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionOutcome {
    pub ok: bool,
    pub kind: String,
    pub items: Vec<DecisionItem>,
    pub scanned_at: i64,
}

/// Every persisted decision, in stored order. A missing key means "none yet";
/// an unparseable value is treated the same, so a corrupt record can never crash
/// Doctor (the next `add`/`remove` rewrites the array cleanly).
pub fn load(store: &SkillStore) -> Result<Vec<FindingDecision>, AppError> {
    match store.get_setting(DECISIONS_KEY).map_err(AppError::db)? {
        Some(raw) => Ok(serde_json::from_str(&raw).unwrap_or_default()),
        None => Ok(Vec::new()),
    }
}

/// Persist the full decision list, replacing whatever was stored.
fn save(store: &SkillStore, decisions: &[FindingDecision]) -> Result<(), AppError> {
    let json = serde_json::to_string(decisions)
        .map_err(|e| AppError::internal(format!("serialize finding decisions: {e}")))?;
    store
        .set_setting(DECISIONS_KEY, &json)
        .map_err(AppError::db)
}

/// Record a decision. Idempotent on `(rule, fingerprint)`: an existing decision
/// for the same finding is replaced so re-ignoring, re-noting, or switching kind
/// never duplicates a record.
pub fn add(store: &SkillStore, decision: FindingDecision) -> Result<(), AppError> {
    let mut decisions = load(store)?;
    decisions.retain(|d| !(d.rule == decision.rule && d.fingerprint == decision.fingerprint));
    decisions.push(decision);
    save(store, &decisions)
}

/// Remove the decision for `(rule, fingerprint)` — the restore path. A no-op when
/// no such decision exists.
pub fn remove(store: &SkillStore, rule: &str, fingerprint: &str) -> Result<(), AppError> {
    let mut decisions = load(store)?;
    decisions.retain(|d| !(d.rule == rule && d.fingerprint == fingerprint));
    save(store, &decisions)
}

/// Whether a finding identified by `(rule, fingerprint)` is currently hidden.
pub fn is_ignored(decisions: &[FindingDecision], rule: &str, fingerprint: &str) -> bool {
    decisions
        .iter()
        .any(|d| d.rule == rule && d.fingerprint == fingerprint)
}

/// Partition findings into `(visible, ignored)` by matching each finding's
/// `(rule, fingerprint)` against the decisions. Pure and order-preserving: a
/// decision whose fingerprint does not match a finding leaves that finding
/// visible, so materially changed evidence is never hidden by a stale record.
pub fn apply_decisions(
    findings: Vec<Finding>,
    decisions: &[FindingDecision],
) -> (Vec<Finding>, Vec<Finding>) {
    let mut visible = Vec::new();
    let mut ignored = Vec::new();
    for finding in findings {
        if is_ignored(decisions, &finding.rule, &finding.fingerprint) {
            ignored.push(finding);
        } else {
            visible.push(finding);
        }
    }
    (visible, ignored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::doctor::{Deviation, Evidence, Finding, Severity};
    use std::path::Path;
    use tempfile::tempdir;

    fn store_in(dir: &Path) -> SkillStore {
        SkillStore::new(&dir.join("patchbay.db")).unwrap()
    }

    fn decision(rule: &str, fingerprint: &str, kind: &str) -> FindingDecision {
        FindingDecision {
            rule: rule.to_string(),
            fingerprint: fingerprint.to_string(),
            kind: kind.to_string(),
            note: None,
            // Explicit, never the wall clock: tests must not depend on `now`.
            created_at: 42,
        }
    }

    /// A minimal finding with the given rule and fingerprint; the other fields
    /// are inert because partitioning only keys on rule + fingerprint.
    fn finding(rule: &str, fingerprint: &str) -> Finding {
        Finding {
            rule: rule.to_string(),
            deviation: Deviation::Copy,
            severity: Severity::Warning,
            evidence: Evidence {
                entry_path: "/p/.claude/skills/x".to_string(),
                hops: Vec::new(),
                final_target: "/p/.claude/skills/x".to_string(),
                topology_status: "copy".to_string(),
            },
            affected: Vec::new(),
            actions: Vec::new(),
            fingerprint: fingerprint.to_string(),
        }
    }

    #[test]
    fn load_is_empty_when_unset() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        assert!(load(&store).unwrap().is_empty());
    }

    #[test]
    fn add_is_idempotent_on_rule_and_fingerprint() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());

        add(
            &store,
            decision("chain.unmanaged_copy", "fp1", KIND_IGNORED),
        )
        .unwrap();
        // Same (rule, fingerprint), different kind: replaces, does not duplicate.
        add(
            &store,
            decision("chain.unmanaged_copy", "fp1", KIND_PROJECT_PRIVATE),
        )
        .unwrap();
        // A different fingerprint is a distinct record.
        add(
            &store,
            decision("chain.unmanaged_copy", "fp2", KIND_IGNORED),
        )
        .unwrap();

        let stored = load(&store).unwrap();
        assert_eq!(stored.len(), 2);
        let fp1 = stored.iter().find(|d| d.fingerprint == "fp1").unwrap();
        assert_eq!(fp1.kind, KIND_PROJECT_PRIVATE, "latest classification wins");
    }

    #[test]
    fn remove_deletes_only_the_matching_decision() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        add(&store, decision("chain.broken_link", "fp1", KIND_IGNORED)).unwrap();
        add(&store, decision("chain.direct_link", "fp2", KIND_IGNORED)).unwrap();

        remove(&store, "chain.broken_link", "fp1").unwrap();

        let stored = load(&store).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].fingerprint, "fp2");

        // Removing a non-existent decision is a harmless no-op.
        remove(&store, "chain.broken_link", "fp1").unwrap();
        assert_eq!(load(&store).unwrap().len(), 1);
    }

    #[test]
    fn apply_decisions_partitions_by_rule_and_fingerprint() {
        let findings = vec![
            finding("chain.unmanaged_copy", "fp-a"),
            finding("chain.direct_link", "fp-b"),
            finding("chain.broken_link", "fp-c"),
        ];
        let decisions = vec![decision("chain.direct_link", "fp-b", KIND_IGNORED)];

        let (visible, ignored) = apply_decisions(findings, &decisions);
        let visible_fps: Vec<&str> = visible.iter().map(|f| f.fingerprint.as_str()).collect();
        assert_eq!(visible_fps, vec!["fp-a", "fp-c"]);
        assert_eq!(ignored.len(), 1);
        assert_eq!(ignored[0].fingerprint, "fp-b");
    }

    /// AC4: a decision keyed on a fingerprint that no longer matches (materially
    /// changed evidence) must NOT hide the finding — it stays visible.
    #[test]
    fn stale_fingerprint_does_not_hide_a_changed_finding() {
        let findings = vec![finding("chain.broken_link", "new-fingerprint")];
        // The decision remembers the OLD fingerprint from before the change.
        let decisions = vec![decision(
            "chain.broken_link",
            "old-fingerprint",
            KIND_IGNORED,
        )];

        let (visible, ignored) = apply_decisions(findings, &decisions);
        assert_eq!(visible.len(), 1, "changed evidence must reappear");
        assert!(ignored.is_empty());
        // A matching rule with the wrong fingerprint is not "ignored".
        assert!(!is_ignored(
            &decisions,
            "chain.broken_link",
            "new-fingerprint"
        ));
        assert!(is_ignored(
            &decisions,
            "chain.broken_link",
            "old-fingerprint"
        ));
    }
}
