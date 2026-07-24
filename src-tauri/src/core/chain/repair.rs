//! Plan/apply repair engine for noncanonical chains (issue #10).
//!
//! Doctor classifies topology deviations; this module turns the *repairable*
//! ones — broken, direct, and legacy links — into a bounded, previewable set of
//! symlink edits that normalize the chain to the convention:
//!
//! ```text
//! <project>/<agent surface>/<skill>  ->  <project>/.agents/skills/<skill>  ->  <Original>
//! ```
//!
//! Tier-2 aggregate link `.agents/skills/<skill>` points at the Original with an
//! ABSOLUTE target; tier-3 per-entry surface link `<surface>/<skill>` points at
//! the aggregate with the RELATIVE `../../.agents/skills/<skill>` target (the
//! exact shape [`super::ops::apply_per_skill_entries`] writes). The repair never
//! moves, copies, or rewrites an Original Skill: it only re-points the links that
//! route Agent access to it, so the *final* Original a chain resolves to is
//! always preserved (AC2).
//!
//! Like the link/unlink engine this is a guarded two-phase operation:
//!
//! * [`plan`] is pure over the diagnosed findings plus one read-only [`ops::observe`]
//!   per touched path. It builds the smallest edit per finding, snapshots the
//!   pre-change target of every existing link it would change (the recoverable
//!   record, AC3), records the on-disk evidence apply re-checks (AC4), and lists
//!   any requested fingerprint that no longer maps to a supported finding.
//! * [`apply`] re-observes every target from scratch, refuses any whose evidence
//!   changed since the preview (time-of-check/time-of-use guard, AC4), never
//!   touches a physical directory or file, and only ever edits symlinks. It
//!   makes the smallest change per item and records a per-item outcome (AC5).
//!
//! Refusals (physical entries, conflicting/changed evidence) are reported as
//! item outcomes, never as silent replacements. Verification that the normalized
//! chain is actually on disk happens one level up in `ChainService`, from a fresh
//! rescan (AC6).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::candidates;
use super::doctor::{Deviation, Finding};
use super::ops::{self, EntryEvidence};
use super::ChainTopology;

/// One planned (or applied) repair edit. Each item repairs exactly one link of
/// one Doctor finding; a finding may need one or two items (see [`plan`]). The
/// same struct carries both the previewed intent and, after [`apply`], the
/// realized outcome — the two share the write verbs (`create`/`repoint`/`remove`)
/// so a result with those actions means the edit succeeded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairItem {
    /// Fingerprint of the Doctor finding this item repairs.
    pub fingerprint: String,
    /// Stable rule identifier copied from the finding (e.g. `chain.direct_link`).
    pub rule: String,
    /// The finding's deviation as a token: "broken" | "direct" | "legacy".
    pub deviation: String,
    /// Registered project root the edited link lives in. Carried so [`apply`] can
    /// re-validate the write boundary from scratch (a repoint/create can never
    /// escape the project) even though the plan crosses the wire and back.
    pub project: String,
    /// Absolute path of the link this item acts on.
    pub path: String,
    /// What kind of link is edited: "ensure_aggregate" | "repoint_entry" |
    /// "relink_broken" | "remove_broken".
    pub kind: String,
    /// Intended (preview) or realized (apply) outcome:
    /// "create" | "repoint" | "remove" | "exists" | "conflict" | "skip" | "error".
    /// `create`/`repoint`/`remove` are the only actions that write; the rest are
    /// no-ops reflected verbatim.
    pub action: String,
    /// The link's target before the change, for recovery. `None` when there was
    /// no prior link (a fresh `create`).
    pub old_target: Option<String>,
    /// The link's intended target after the change. Absolute for an aggregate
    /// link (the Original), relative (`../../.agents/skills/<skill>`) for a
    /// surface entry, `None` for a removal.
    pub new_target: Option<String>,
    pub message: Option<String>,
}

/// One recoverable pre-change record: a link the apply would change, with its
/// current target captured BEFORE any write (AC3). Only links that already exist
/// are snapshotted (a fresh `create` has nothing to restore — its recovery is a
/// plain removal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub path: String,
    pub target: String,
}

/// A previewed, guarded repair operation. Produced by [`plan`] and consumed
/// unchanged by [`apply`]. It crosses the wire back from the UI, so it carries
/// everything apply needs to re-validate independently: the per-target TOCTOU
/// `evidence` baseline, the recoverable `snapshot`, and per-item project roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairPlan {
    pub items: Vec<RepairItem>,
    /// Target path -> on-disk evidence observed at plan time (TOCTOU baseline).
    pub evidence: BTreeMap<String, EntryEvidence>,
    /// Pre-change target of every existing link the apply would change (AC3).
    pub snapshot: Vec<SnapshotEntry>,
    /// Requested fingerprints that map to no current *supported* finding — a
    /// finding that vanished, changed, or is not broken/direct/legacy. Reported,
    /// not repaired.
    pub unsupported: Vec<String>,
    /// Copied from the topology so the plan and Link Topology share one clock.
    pub scanned_at: i64,
}

/// The result of applying a repair plan: the per-item outcomes plus proof, from
/// a fresh rescan, that the normalized chain is really on disk. As with the
/// link/unlink outcomes, `verified == false` means success must not be reported.
#[derive(Debug, Clone, Serialize)]
pub struct RepairOutcome {
    pub results: Vec<RepairItem>,
    /// True only when the apply was clean (no conflict/skip/error item) and a
    /// rescan confirmed every repaired chain (AC6). Set by `ChainService`.
    pub verified: bool,
    /// Fresh scan clock stamped after the rescan.
    pub scanned_at: i64,
    /// Repair-journal record id when the apply wrote anything (issue #31);
    /// `None` for a no-op apply, or if persisting the record failed.
    pub journal_id: Option<i64>,
}

// ── Plan phase (read-only) ────────────────────────────────────────────────

/// Build a repair plan for the requested Doctor findings from CURRENT evidence.
///
/// `findings` is the freshly diagnosed set (the caller re-scans and re-diagnoses
/// so the plan is never stale). Only the requested `fingerprints` are considered;
/// for each, the matching finding drives the smallest normalizing edit:
///
/// * **Broken with a relink candidate** — the chain is rebuilt instead of
///   removed (issue #30): when [`candidates::best_relink_target`] pins down
///   where the dead Original went, `relink_broken` items re-point the dangling
///   links at it in the canonical shape (aggregate at the ABSOLUTE candidate,
///   surface entry at the relative aggregate).
/// * **Broken without a candidate** — one `remove_broken` item. A dangling
///   symlink is removed (there is no Original to preserve and nowhere
///   defensible to point); a physical entry refuses (`conflict`); an
///   already-absent one is a `skip`.
/// * **Direct** / **Legacy on a surface** — two items: `ensure_aggregate` creates
///   (or confirms) `.agents/skills/<skill>` pointing at the resolved Original,
///   then `repoint_entry` re-points the surface link at the relative aggregate.
/// * **Legacy on the aggregate** — one `repoint_entry` item that collapses the
///   retired hop, re-pointing `.agents/skills/<skill>` straight at the resolved
///   Original.
///
/// A requested fingerprint that matches no supported finding is pushed to
/// `unsupported`. Iterating `findings` (already deterministically ordered by
/// `diagnose`) keeps the item order stable regardless of the caller's fingerprint
/// order, and keeps each finding's `ensure_aggregate` ahead of its
/// `repoint_entry` so apply can rely on that order.
pub fn plan(
    topo: &ChainTopology,
    findings: &[Finding],
    fingerprints: &[String],
    prefer_root: Option<&str>,
) -> RepairPlan {
    let requested: HashSet<&str> = fingerprints.iter().map(String::as_str).collect();
    let mut builder = PlanBuilder::default();
    let mut matched: HashSet<String> = HashSet::new();

    for finding in findings {
        if !requested.contains(finding.fingerprint.as_str()) {
            continue;
        }
        let built = match finding.deviation {
            // A broken chain is relinked when the candidate evidence pins down
            // where the dead Original went; only a candidate-less break falls
            // back to removing the dangling link.
            Deviation::Broken => match candidates::best_relink_target(topo, finding, prefer_root) {
                Some(candidate) => builder.plan_relink(finding, &candidate.path),
                None => builder.plan_broken(finding),
            },
            Deviation::Direct | Deviation::Legacy => builder.plan_normalize(finding),
            // Copy / ProjectPrivate / Orphan are not repairable here.
            _ => false,
        };
        if built {
            matched.insert(finding.fingerprint.clone());
        }
    }

    // Requested fingerprints that matched no supported finding, de-duplicated and
    // in the order the caller listed them.
    let mut unsupported = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for fp in fingerprints {
        if !matched.contains(fp) && seen.insert(fp.as_str()) {
            unsupported.push(fp.clone());
        }
    }

    RepairPlan {
        items: builder.items,
        evidence: builder.evidence,
        snapshot: builder.snapshot,
        unsupported,
        scanned_at: topo.scanned_at,
    }
}

/// Accumulator for the plan phase: the items, the TOCTOU evidence baseline, and
/// the recoverable snapshot, threaded through the per-finding planners.
#[derive(Default)]
struct PlanBuilder {
    items: Vec<RepairItem>,
    evidence: BTreeMap<String, EntryEvidence>,
    snapshot: Vec<SnapshotEntry>,
}

impl PlanBuilder {
    /// Record the on-disk evidence of a touched path for the TOCTOU baseline.
    fn observe(&mut self, path: &Path) -> EntryEvidence {
        let ev = ops::observe(path);
        self.evidence
            .insert(path.to_string_lossy().to_string(), ev.clone());
        ev
    }

    /// Note that `path` currently holds a symlink to `target` that the apply will
    /// change, so it can be restored from the plan (AC3).
    fn snapshot(&mut self, path: &Path, target: &str) {
        self.snapshot.push(SnapshotEntry {
            path: path.to_string_lossy().to_string(),
            target: target.to_string(),
        });
    }

    /// Plan the repair of a Broken finding: remove the dangling symlink. Returns
    /// false when the finding has no derivable project (never for a real scan).
    fn plan_broken(&mut self, finding: &Finding) -> bool {
        let Some(project) = project_of(finding) else {
            return false;
        };
        let path = PathBuf::from(&finding.evidence.entry_path);
        let ev = self.observe(&path);
        let (action, old_target, message) = match &ev {
            EntryEvidence::Symlink(target) => {
                self.snapshot(&path, target);
                ("remove", Some(target.clone()), None)
            }
            // A physical directory/file is never removed — refuse (AC4).
            EntryEvidence::Dir | EntryEvidence::File => (
                "conflict",
                None,
                Some("physical entry, not removing".to_string()),
            ),
            // Nothing to remove: the dead link is already gone.
            EntryEvidence::Absent => ("skip", None, None),
        };
        self.items.push(item(
            finding,
            &project,
            "remove_broken",
            action,
            &finding.evidence.entry_path,
            old_target,
            None,
            message,
        ));
        true
    }

    /// Plan the relink of a Broken finding to `new_original`, the candidate the
    /// evidence pinned down (issue #30): the dangling chain is rebuilt to the
    /// canonical shape instead of removed. A broken aggregate entry is one item
    /// re-pointed straight at the candidate; a broken surface entry gets its
    /// aggregate ensured (created, or re-pointed when itself dangling) and is
    /// then routed through it. Every item carries kind `relink_broken` so the
    /// UI and audit trail can tell a rebuild from a normalization. Returns
    /// false when the project or skill cannot be derived from the finding.
    fn plan_relink(&mut self, finding: &Finding, new_original: &str) -> bool {
        let Some(project) = project_of(finding) else {
            return false;
        };
        let skill = candidates::skill_of(finding);
        if skill.is_empty() {
            return false;
        }
        let project_path = PathBuf::from(&project);
        let agg_dir = project_path.join(".agents").join("skills");
        let agg_skill = agg_dir.join(&skill);
        let entry_path = PathBuf::from(&finding.evidence.entry_path);

        // The broken aggregate entry itself: re-point it at the candidate.
        if entry_path.parent() == Some(agg_dir.as_path()) {
            let ev = self.observe(&entry_path);
            let (action, old_target, message) = match &ev {
                EntryEvidence::Symlink(target) => {
                    self.snapshot(&entry_path, target);
                    ("repoint", Some(target.clone()), None)
                }
                EntryEvidence::Dir | EntryEvidence::File => (
                    "conflict",
                    None,
                    Some("physical entry, not repointing".to_string()),
                ),
                EntryEvidence::Absent => ("skip", None, None),
            };
            self.items.push(item(
                finding,
                &project,
                "relink_broken",
                action,
                &finding.evidence.entry_path,
                old_target,
                Some(new_original.to_string()),
                message,
            ));
            return true;
        }

        // Broken surface entry: point the aggregate at the candidate, then
        // route the entry through it. Aggregate first — apply relies on order.
        self.plan_relink_aggregate(finding, &project, &agg_skill, new_original);
        self.plan_repoint_entry(finding, &project, "relink_broken", &entry_path, &skill);
        true
    }

    /// The aggregate link a relink routes through must resolve to the candidate.
    /// Unlike [`plan_ensure_aggregate`], a DANGLING aggregate is re-pointed, not
    /// refused — a dead link holds nothing to preserve and re-pointing it is
    /// the repair itself. A live aggregate linking elsewhere, or a physical
    /// entry, still refuses (AC4).
    fn plan_relink_aggregate(
        &mut self,
        finding: &Finding,
        project: &str,
        agg_skill: &Path,
        original: &str,
    ) {
        let ev = self.observe(agg_skill);
        let (action, old_target, message) = match &ev {
            EntryEvidence::Absent => ("create", None, None),
            EntryEvidence::Symlink(target) => {
                if ops::resolves_to(agg_skill, Path::new(original)) {
                    ("exists", Some(target.clone()), None)
                } else if !agg_skill.exists() {
                    // `exists()` follows the link: false means it dangles.
                    self.snapshot(agg_skill, target);
                    ("repoint", Some(target.clone()), None)
                } else {
                    (
                        "conflict",
                        Some(target.clone()),
                        Some("aggregate links elsewhere, not replacing".to_string()),
                    )
                }
            }
            EntryEvidence::Dir | EntryEvidence::File => (
                "conflict",
                None,
                Some("physical aggregate entry, not replacing".to_string()),
            ),
        };
        self.items.push(item(
            finding,
            project,
            "relink_broken",
            action,
            &agg_skill.to_string_lossy(),
            old_target,
            Some(original.to_string()),
            message,
        ));
    }

    /// Plan the normalization of a Direct or Legacy finding. A surface entry gets
    /// two items (ensure the aggregate, re-point the entry); the aggregate entry
    /// of a Legacy finding gets one item (collapse the retired hop). Returns false
    /// when the project or skill cannot be derived from the finding.
    fn plan_normalize(&mut self, finding: &Finding) -> bool {
        let Some(project) = project_of(finding) else {
            return false;
        };
        let skill = candidates::skill_of(finding);
        if skill.is_empty() {
            return false;
        }
        let project_path = PathBuf::from(&project);
        let agg_dir = project_path.join(".agents").join("skills");
        let agg_skill = agg_dir.join(&skill);
        let entry_path = PathBuf::from(&finding.evidence.entry_path);
        // The Original the chain currently resolves to — what we must preserve.
        let original = finding.evidence.final_target.clone();

        // Legacy on the aggregate itself: the finding's link IS the aggregate
        // entry. Collapse the retired layer by re-pointing it straight at the
        // resolved Original.
        if entry_path.parent() == Some(agg_dir.as_path()) {
            let ev = self.observe(&entry_path);
            let (action, old_target, message) = match &ev {
                EntryEvidence::Symlink(target) if normalized_eq(target, &original) => {
                    // Already collapsed to the Original — nothing to do.
                    ("exists", Some(target.clone()), None)
                }
                EntryEvidence::Symlink(target) => {
                    self.snapshot(&entry_path, target);
                    ("repoint", Some(target.clone()), None)
                }
                EntryEvidence::Dir | EntryEvidence::File => (
                    "conflict",
                    None,
                    Some("physical aggregate entry, not replacing".to_string()),
                ),
                EntryEvidence::Absent => ("skip", None, None),
            };
            self.items.push(item(
                finding,
                &project,
                "repoint_entry",
                action,
                &finding.evidence.entry_path,
                old_target,
                Some(original),
                message,
            ));
            return true;
        }

        // Direct / Legacy on a surface entry: ensure the aggregate, then re-point
        // the surface entry at it. IMPORTANT: this pushes ensure_aggregate first
        // so apply creates the aggregate before the entry that must reach it.
        self.plan_ensure_aggregate(finding, &project, &agg_skill, &original);
        self.plan_repoint_entry(finding, &project, "repoint_entry", &entry_path, &skill);
        true
    }

    /// The aggregate link `.agents/skills/<skill>` must exist and resolve to the
    /// Original. Absent ⇒ create it pointing at the ABSOLUTE Original; already
    /// resolving there ⇒ `exists`; anything else (a symlink elsewhere, or a
    /// physical entry) ⇒ `conflict` — the aggregate is never clobbered (AC4).
    fn plan_ensure_aggregate(
        &mut self,
        finding: &Finding,
        project: &str,
        agg_skill: &Path,
        original: &str,
    ) {
        let ev = self.observe(agg_skill);
        let (action, old_target, message) = match &ev {
            EntryEvidence::Absent => ("create", None, None),
            EntryEvidence::Symlink(target) => {
                if ops::resolves_to(agg_skill, Path::new(original)) {
                    ("exists", Some(target.clone()), None)
                } else {
                    (
                        "conflict",
                        Some(target.clone()),
                        Some("aggregate links elsewhere, not replacing".to_string()),
                    )
                }
            }
            EntryEvidence::Dir | EntryEvidence::File => (
                "conflict",
                None,
                Some("physical aggregate entry, not replacing".to_string()),
            ),
        };
        self.items.push(item(
            finding,
            project,
            "ensure_aggregate",
            action,
            &agg_skill.to_string_lossy(),
            old_target,
            Some(original.to_string()),
            message,
        ));
    }

    /// The surface entry must route through the aggregate. Re-point it at the
    /// RELATIVE `../../.agents/skills/<skill>`; a symlink already pointing there
    /// is `exists`; a physical entry refuses; an absent one is a `skip`.
    /// `kind` labels the item (`repoint_entry` for a normalization,
    /// `relink_broken` for a broken-chain rebuild) — the edit is identical.
    fn plan_repoint_entry(
        &mut self,
        finding: &Finding,
        project: &str,
        kind: &str,
        entry_path: &Path,
        skill: &str,
    ) {
        let relative = relative_aggregate_target(skill);
        let ev = self.observe(entry_path);
        let (action, old_target, message) = match &ev {
            EntryEvidence::Symlink(target) if *target == relative => {
                ("exists", Some(target.clone()), None)
            }
            EntryEvidence::Symlink(target) => {
                self.snapshot(entry_path, target);
                ("repoint", Some(target.clone()), None)
            }
            EntryEvidence::Dir | EntryEvidence::File => (
                "conflict",
                None,
                Some("physical entry, not repointing".to_string()),
            ),
            EntryEvidence::Absent => ("skip", None, None),
        };
        self.items.push(item(
            finding,
            project,
            kind,
            action,
            &entry_path.to_string_lossy(),
            old_target,
            Some(relative),
            message,
        ));
    }
}

// ── Apply phase (writes) ──────────────────────────────────────────────────

/// Apply a previewed [`RepairPlan`]. Each writing item (`create`/`repoint`/
/// `remove`) is re-observed and refused as `skip` if its on-disk evidence changed
/// since the preview (TOCTOU, AC4). The project boundary is re-validated from
/// scratch, so a repoint/create can never escape the project; a physical
/// directory or file is never touched (`conflict`). Non-writing items
/// (`exists`/`conflict`/`skip`) are reflected verbatim so the report is complete.
///
/// Items are applied in plan order, which places each finding's `ensure_aggregate`
/// ahead of its `repoint_entry` — the entry's new target therefore exists before
/// the entry is pointed at it.
pub fn apply(plan: &RepairPlan) -> Vec<RepairItem> {
    plan.items
        .iter()
        .map(|item| {
            if is_writing(&item.action) {
                apply_one(plan, item)
            } else {
                // exists / conflict / skip / error: reflected without any write.
                item.clone()
            }
        })
        .collect()
}

fn is_writing(action: &str) -> bool {
    matches!(action, "create" | "repoint" | "remove")
}

/// Apply one writing item under the full guard stack.
fn apply_one(plan: &RepairPlan, item: &RepairItem) -> RepairItem {
    let path = PathBuf::from(&item.path);
    let project = PathBuf::from(&item.project);

    // Boundary: the project must be a real, safe directory and the edited link's
    // parent must resolve inside it (catches a symlinked `.agents`/surface parent
    // trying to redirect the write out of the project).
    if ops::validate_project(&project).is_err() || !ops::parent_within_project(&project, &path) {
        return refuse(item, "conflict", "escapes the project boundary");
    }

    // TOCTOU: refuse anything whose on-disk state no longer matches the preview.
    let current = ops::observe(&path);
    if plan.evidence.get(&item.path) != Some(&current) {
        return refuse(item, "skip", "changed since preview");
    }

    match item.action.as_str() {
        "remove" => match current {
            EntryEvidence::Symlink(_) => match ops::remove_symlink(&path) {
                Ok(()) => item.clone(),
                Err(e) => refuse(item, "error", &e.to_string()),
            },
            // Never remove a physical directory/file (AC4).
            _ => refuse(item, "conflict", "not a symlink, not removing"),
        },
        "create" => match current {
            EntryEvidence::Absent => create_link(item, &project, &path),
            _ => refuse(item, "conflict", "entry already exists"),
        },
        "repoint" => match current {
            EntryEvidence::Symlink(_) => repoint_link(item, &path),
            // Never overwrite a physical directory/file (AC4).
            _ => refuse(item, "conflict", "not a symlink, not repointing"),
        },
        _ => item.clone(),
    }
}

/// Create a fresh symlink for an `ensure_aggregate` item, first creating the
/// aggregate directory if needed — guarded so `.agents/skills` cannot be
/// redirected outside the project.
fn create_link(item: &RepairItem, project: &Path, path: &Path) -> RepairItem {
    let Some(target) = &item.new_target else {
        return refuse(item, "error", "missing target");
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return refuse(item, "error", &e.to_string());
        }
        // Re-validate after creating: a symlinked parent that appeared must not
        // redirect the write out of the project.
        if !ops::parent_within_project(project, path) {
            return refuse(item, "conflict", "escapes the project boundary");
        }
    }
    match ops::make_symlink(Path::new(target), path) {
        Ok(()) => item.clone(),
        Err(e) => refuse(item, "error", &e.to_string()),
    }
}

/// Re-point an existing symlink to the item's new target: remove the old symlink
/// (only a symlink reaches here) then create the replacement. The plan's snapshot
/// records the old target so the change is recoverable if creation fails.
fn repoint_link(item: &RepairItem, path: &Path) -> RepairItem {
    let Some(target) = &item.new_target else {
        return refuse(item, "error", "missing target");
    };
    if let Err(e) = ops::remove_symlink(path) {
        return refuse(item, "error", &e.to_string());
    }
    match ops::make_symlink(Path::new(target), path) {
        Ok(()) => item.clone(),
        Err(e) => refuse(item, "error", &e.to_string()),
    }
}

/// Reflect an item with a refusal/failure action and message, leaving the rest
/// of the record intact for the report and recovery.
fn refuse(item: &RepairItem, action: &str, message: &str) -> RepairItem {
    RepairItem {
        action: action.to_string(),
        message: Some(message.to_string()),
        ..item.clone()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Assemble one repair item from a finding and the decided edit.
#[allow(clippy::too_many_arguments)]
fn item(
    finding: &Finding,
    project: &str,
    kind: &str,
    action: &str,
    path: &str,
    old_target: Option<String>,
    new_target: Option<String>,
    message: Option<String>,
) -> RepairItem {
    RepairItem {
        fingerprint: finding.fingerprint.clone(),
        rule: finding.rule.clone(),
        deviation: deviation_tag(finding.deviation).to_string(),
        project: project.to_string(),
        path: path.to_string(),
        kind: kind.to_string(),
        action: action.to_string(),
        old_target,
        new_target,
        message,
    }
}

/// Token for a repairable deviation, matching the Doctor wire tokens.
fn deviation_tag(deviation: Deviation) -> &'static str {
    match deviation {
        Deviation::Broken => "broken",
        Deviation::Direct => "direct",
        Deviation::Legacy => "legacy",
        // Unreachable: only broken/direct/legacy findings reach `item`.
        _ => "unsupported",
    }
}

/// The registered project root a finding is about, from its "project" affected
/// object (the canonical identity the write path is derived from).
fn project_of(finding: &Finding) -> Option<String> {
    finding
        .affected
        .iter()
        .find(|obj| obj.kind == "project")
        .map(|obj| obj.path.clone())
}

/// The tier-3 relative target a surface entry points at: `../../.agents/skills/<skill>`.
/// Built the same way [`super::ops::apply_per_skill_entries`] builds it so the two
/// engines produce byte-identical links.
fn relative_aggregate_target(skill: &str) -> String {
    Path::new("..")
        .join("..")
        .join(".agents")
        .join("skills")
        .join(skill)
        .to_string_lossy()
        .to_string()
}

/// Lexical-normalized equality of two symlink targets, so an already-collapsed
/// legacy aggregate (`target` equals the resolved Original) is recognized without
/// a filesystem round-trip.
fn normalized_eq(a: &str, b: &str) -> bool {
    super::link_tracer::normalize(Path::new(a)) == super::link_tracer::normalize(Path::new(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::warehouse::RepoInfo;
    use crate::core::chain::{doctor, project_links};
    /// Portable stand-in for `std::os::unix::fs::symlink`. These fixtures link
    /// directories, and gating the module on unix meant they never ran on
    /// Windows — the platform whose symlink semantics differ most.
    fn symlink(
        target: impl AsRef<std::path::Path>,
        link: impl AsRef<std::path::Path>,
    ) -> std::io::Result<()> {
        crate::core::test_support::symlink_dir(target.as_ref(), link.as_ref())
    }
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    /// A warehouse root, a real Original Skill, a "retired" distribution layer,
    /// and an empty project — the raw material every repair scenario shapes.
    struct Fixture {
        temp: TempDir,
        warehouse: PathBuf,
        original: PathBuf,
        project: PathBuf,
    }

    fn make_skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: repair fixture\n---\n"),
        )
        .unwrap();
    }

    fn setup() -> Fixture {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let original = warehouse.join("repo").join("skills").join("demo-skill");
        make_skill(&original, "demo-skill");
        let project = temp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        Fixture {
            temp,
            warehouse,
            original,
            project,
        }
    }

    /// Scan the fixture into a topology (no Git needed: classification keys off
    /// the warehouse root, and the repo list only labels the resolved target).
    fn topo(f: &Fixture) -> ChainTopology {
        topo_with_repos(f, Vec::new())
    }

    /// Like [`topo`], with an explicit repo list so candidate location (which
    /// searches `topo.repos[].skills`) has something to find.
    fn topo_with_repos(f: &Fixture, repos: Vec<RepoInfo>) -> ChainTopology {
        let projects =
            project_links::discover(&[f.project.clone()], &[f.warehouse.clone()], &repos);
        ChainTopology {
            warehouse_roots: Vec::new(),
            projects_root: f.temp.path().to_string_lossy().to_string(),
            repos,
            projects,
            guard: Vec::new(),
            scanned_at: 7,
        }
    }

    /// A minimal checkout-shaped repo record exposing one scanned skill, enough
    /// for candidate location. Git health is irrelevant to the planner.
    fn repo_with_skill(repo_path: &Path, skill_name: &str, skill_path: &Path) -> RepoInfo {
        RepoInfo {
            name: repo_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: repo_path.to_string_lossy().to_string(),
            source_kind: "checkout".to_string(),
            root: repo_path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            health: crate::core::chain::repo_health::RepoHealth {
                dirty: false,
                state: "no_upstream".to_string(),
                ahead: 0,
                behind: 0,
                branch: None,
                error: None,
            },
            origin: None,
            upstream: None,
            skills: vec![crate::core::chain::warehouse::RepoSkill {
                name: skill_name.to_string(),
                path: skill_path.to_string_lossy().to_string(),
            }],
            referenced_by: Vec::new(),
        }
    }

    fn find(findings: &[Finding], deviation: Deviation) -> &Finding {
        findings
            .iter()
            .find(|finding| finding.deviation == deviation)
            .expect("expected a finding of this deviation")
    }

    fn item_of<'a>(items: &'a [RepairItem], kind: &str) -> &'a RepairItem {
        items
            .iter()
            .find(|item| item.kind == kind)
            .unwrap_or_else(|| panic!("expected a {kind} item"))
    }

    fn final_target(entry: &Path) -> String {
        super::super::link_tracer::trace(entry).final_target
    }

    #[test]
    fn broken_link_is_removed_and_the_chain_is_clean() {
        let f = setup();
        // A dead surface entry: `.claude/skills/x` points nowhere.
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(f.temp.path().join("nowhere"), &entry).unwrap();

        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let broken = find(&findings, Deviation::Broken);

        let plan = plan(&topology, &findings, &[broken.fingerprint.clone()], None);
        let item = item_of(&plan.items, "remove_broken");
        assert_eq!(item.action, "remove");
        assert_eq!(item.path, entry.to_string_lossy());

        let results = apply(&plan);
        assert_eq!(results[0].action, "remove");
        // The dead link is gone; the surface directory itself is untouched.
        assert!(!entry.exists() && std::fs::symlink_metadata(&entry).is_err());
        assert!(surface.is_dir());
    }

    #[test]
    fn broken_surface_link_is_relinked_to_a_same_name_candidate() {
        let f = setup();
        // A dead surface entry whose skill still exists — under a NEW repo the
        // topology has scanned (the whole-repo-moved case).
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(f.temp.path().join("gone/skills/demo-skill"), &entry).unwrap();
        let repo = f.warehouse.join("repo");

        let topology = topo_with_repos(&f, vec![repo_with_skill(&repo, "demo-skill", &f.original)]);
        let findings = doctor::diagnose(&topology);
        let broken = find(&findings, Deviation::Broken);

        // The chain is rebuilt, not removed: the aggregate is created at the
        // candidate and the dead entry re-pointed through it.
        let plan = plan(&topology, &findings, &[broken.fingerprint.clone()], None);
        assert!(plan.items.iter().all(|item| item.kind == "relink_broken"));
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].action, "create");
        assert_eq!(
            plan.items[0].new_target.as_deref(),
            Some(f.original.to_string_lossy().as_ref())
        );
        assert_eq!(plan.items[1].action, "repoint");
        // Relative targets are built with `Path::join`, so they carry the OS
        // separator; normalize before comparing, as `repo_move` does for roots.
        assert_eq!(
            plan.items[1]
                .new_target
                .as_deref()
                .map(|t| t.replace('\\', "/")),
            Some("../../.agents/skills/demo-skill".to_string())
        );

        let results = apply(&plan);
        assert!(results
            .iter()
            .all(|r| r.action == "create" || r.action == "repoint"));
        let agg_skill = f.project.join(".agents/skills/demo-skill");
        assert_eq!(std::fs::read_link(&agg_skill).unwrap(), f.original);
        assert_eq!(final_target(&entry), f.original.to_string_lossy());
    }

    #[test]
    fn broken_aggregate_link_is_repointed_to_the_candidate() {
        let f = setup();
        // The aggregate entry itself dangles; the Original moved within reach.
        let agg = f.project.join(".agents").join("skills");
        std::fs::create_dir_all(&agg).unwrap();
        let agg_skill = agg.join("demo-skill");
        let dead = f.temp.path().join("gone").join("skills").join("demo-skill");
        symlink(&dead, &agg_skill).unwrap();
        let repo = f.warehouse.join("repo");

        let topology = topo_with_repos(&f, vec![repo_with_skill(&repo, "demo-skill", &f.original)]);
        let findings = doctor::diagnose(&topology);
        let broken = find(&findings, Deviation::Broken);

        let plan = plan(&topology, &findings, &[broken.fingerprint.clone()], None);
        assert_eq!(plan.items.len(), 1);
        let relink = item_of(&plan.items, "relink_broken");
        assert_eq!(relink.action, "repoint");
        assert_eq!(
            relink.new_target.as_deref(),
            Some(f.original.to_string_lossy().as_ref())
        );
        // The dangling pre-change target is snapshotted for recovery (AC3).
        assert_eq!(plan.snapshot.len(), 1);
        assert_eq!(plan.snapshot[0].target, dead.to_string_lossy());

        let results = apply(&plan);
        assert_eq!(results[0].action, "repoint");
        assert_eq!(std::fs::read_link(&agg_skill).unwrap(), f.original);
    }

    #[test]
    fn broken_entry_through_a_dangling_aggregate_repoints_the_aggregate() {
        let f = setup();
        // Canonical-shaped chain whose aggregate dangles: the surface entry
        // already points at the relative aggregate, so its own item is a no-op
        // and the repair happens on the aggregate.
        let agg = f.project.join(".agents").join("skills");
        std::fs::create_dir_all(&agg).unwrap();
        let agg_skill = agg.join("demo-skill");
        symlink(f.temp.path().join("gone/skills/demo-skill"), &agg_skill).unwrap();
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        // Built with `join`, matching what a repair actually writes on this OS:
        // a literal "../../..." would be a POSIX-shaped link that production
        // never produces on Windows, so the canonicality check would classify
        // it differently than any real entry.
        let canonical_rel = Path::new("..")
            .join("..")
            .join(".agents")
            .join("skills")
            .join("demo-skill");
        symlink(&canonical_rel, &entry).unwrap();
        let repo = f.warehouse.join("repo");

        let topology = topo_with_repos(&f, vec![repo_with_skill(&repo, "demo-skill", &f.original)]);
        let findings = doctor::diagnose(&topology);
        // The dangling chain surfaces on the SURFACE entry finding here.
        let broken = findings
            .iter()
            .find(|finding| {
                finding.deviation == Deviation::Broken
                    && finding.evidence.entry_path == entry.to_string_lossy()
            })
            .expect("a broken surface finding");

        let plan = plan(&topology, &findings, &[broken.fingerprint.clone()], None);
        assert_eq!(plan.items.len(), 2);
        // The dangling aggregate is re-pointed (not refused as a conflict)...
        assert_eq!(plan.items[0].action, "repoint");
        assert_eq!(
            plan.items[0].path,
            agg_skill.to_string_lossy(),
            "the aggregate carries the write"
        );
        // ...and the already-canonical surface entry is a no-op.
        assert_eq!(plan.items[1].action, "exists");

        let results = apply(&plan);
        assert_eq!(results[0].action, "repoint");
        assert_eq!(std::fs::read_link(&agg_skill).unwrap(), f.original);
        assert_eq!(final_target(&entry), f.original.to_string_lossy());
    }

    #[test]
    fn direct_link_normalizes_through_the_aggregate_preserving_the_original() {
        let f = setup();
        // A direct surface entry straight to the Original, no aggregate.
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&f.original, &entry).unwrap();

        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let direct = find(&findings, Deviation::Direct);

        let plan = plan(&topology, &findings, &[direct.fingerprint.clone()], None);
        let ensure = item_of(&plan.items, "ensure_aggregate");
        assert_eq!(ensure.action, "create");
        assert_eq!(
            ensure.new_target.as_deref(),
            Some(f.original.to_string_lossy().as_ref())
        );
        let repoint = item_of(&plan.items, "repoint_entry");
        assert_eq!(repoint.action, "repoint");
        assert_eq!(
            repoint.new_target.as_deref().map(|t| t.replace('\\', "/")),
            Some("../../.agents/skills/demo-skill".to_string())
        );

        let results = apply(&plan);
        assert!(results
            .iter()
            .all(|r| r.action == "create" || r.action == "repoint"));

        // The aggregate resolves to the Original (ABSOLUTE), and the surface
        // entry now routes through it to the SAME Original (AC2).
        let agg_skill = f.project.join(".agents/skills/demo-skill");
        assert_eq!(std::fs::read_link(&agg_skill).unwrap(), f.original);
        assert_eq!(
            std::fs::read_link(&entry).unwrap(),
            PathBuf::from("../../.agents/skills/demo-skill")
        );
        assert_eq!(final_target(&entry), f.original.to_string_lossy());
        assert_eq!(final_target(&agg_skill), f.original.to_string_lossy());
        // The Original Skill itself is never moved or rewritten.
        assert!(f.original.join("SKILL.md").is_file());
    }

    #[test]
    fn legacy_aggregate_collapses_the_retired_hop_preserving_the_original() {
        let f = setup();
        // The Original this legacy chain resolves to lives OUTSIDE every warehouse
        // root (the retired layer's skills were never adopted into a managed
        // repo), so the aggregate entry is classified `external` (legacy).
        let external = f.temp.path().join("external-original").join("demo-skill");
        make_skill(&external, "demo-skill");
        // A retired distribution layer: `.agents/skills/x -> <retired>/x -> external`.
        let retired = f.temp.path().join("local-skills").join("shared");
        std::fs::create_dir_all(&retired).unwrap();
        symlink(&external, retired.join("demo-skill")).unwrap();
        let agg = f.project.join(".agents").join("skills");
        std::fs::create_dir_all(&agg).unwrap();
        let agg_skill = agg.join("demo-skill");
        symlink(retired.join("demo-skill"), &agg_skill).unwrap();

        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let legacy = find(&findings, Deviation::Legacy);

        let plan = plan(&topology, &findings, &[legacy.fingerprint.clone()], None);
        assert_eq!(plan.items.len(), 1);
        let repoint = item_of(&plan.items, "repoint_entry");
        assert_eq!(repoint.action, "repoint");
        assert_eq!(
            repoint.new_target.as_deref(),
            Some(external.to_string_lossy().as_ref())
        );
        // The pre-change (retired) target is snapshotted for recovery (AC3).
        assert_eq!(plan.snapshot.len(), 1);
        assert_eq!(
            plan.snapshot[0].target,
            retired.join("demo-skill").to_string_lossy()
        );

        let results = apply(&plan);
        assert_eq!(results[0].action, "repoint");
        // The aggregate now points straight at the resolved Original (retired hop
        // collapsed), and that Original is preserved.
        assert_eq!(std::fs::read_link(&agg_skill).unwrap(), external);
        assert_eq!(final_target(&agg_skill), external.to_string_lossy());
        assert!(external.join("SKILL.md").is_file());
    }

    #[test]
    fn physical_entry_refuses_rather_than_replacing() {
        let f = setup();
        // A hand-crafted Broken finding whose entry is actually a physical dir:
        // the repair must refuse, never remove it (AC4).
        let entry = f.project.join(".claude").join("skills").join("hand-made");
        make_skill(&entry, "hand-made");
        let finding = Finding {
            rule: "chain.broken_link".to_string(),
            deviation: Deviation::Broken,
            severity: doctor::Severity::Violation,
            evidence: doctor::Evidence {
                entry_path: entry.to_string_lossy().to_string(),
                hops: Vec::new(),
                final_target: entry.to_string_lossy().to_string(),
                topology_status: "broken".to_string(),
            },
            affected: vec![doctor::AffectedObject {
                kind: "project".to_string(),
                name: "proj".to_string(),
                path: f.project.to_string_lossy().to_string(),
            }],
            actions: Vec::new(),
            fingerprint: "fp-physical".to_string(),
        };

        let plan = plan(&topo(&f), &[finding], &["fp-physical".to_string()], None);
        let item = item_of(&plan.items, "remove_broken");
        assert_eq!(item.action, "conflict");
        assert!(plan.snapshot.is_empty());

        let results = apply(&plan);
        assert_eq!(results[0].action, "conflict");
        // The physical directory is untouched — nothing removed or rewritten.
        assert!(entry.join("SKILL.md").is_file());
    }

    #[test]
    fn target_changed_since_preview_is_skipped() {
        let f = setup();
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&f.original, &entry).unwrap();

        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let direct = find(&findings, Deviation::Direct);
        let plan = plan(&topology, &findings, &[direct.fingerprint.clone()], None);

        // TOCTOU: the surface entry is re-pointed elsewhere between plan and apply.
        let elsewhere = f.temp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        ops::remove_symlink(&entry).unwrap();
        symlink(&elsewhere, &entry).unwrap();

        let results = apply(&plan);
        let repoint = item_of(&results, "repoint_entry");
        assert_eq!(repoint.action, "skip");
        assert_eq!(repoint.message.as_deref(), Some("changed since preview"));
        // The attacker's link is untouched — not overwritten.
        assert_eq!(std::fs::read_link(&entry).unwrap(), elsewhere);
    }

    #[test]
    fn snapshot_records_each_changed_links_pre_change_target() {
        let f = setup();
        let surface = f.project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        let entry = surface.join("demo-skill");
        symlink(&f.original, &entry).unwrap();

        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let direct = find(&findings, Deviation::Direct);
        let plan = plan(&topology, &findings, &[direct.fingerprint.clone()], None);

        // The re-pointed surface entry is snapshotted with its pre-change target
        // (the Original it pointed straight at); the freshly created aggregate is
        // not snapshotted (nothing to restore) (AC3).
        assert_eq!(plan.snapshot.len(), 1);
        assert_eq!(plan.snapshot[0].path, entry.to_string_lossy());
        assert_eq!(plan.snapshot[0].target, f.original.to_string_lossy());
    }

    #[test]
    fn unmatched_fingerprint_is_reported_unsupported() {
        let f = setup();
        let topology = topo(&f);
        let findings = doctor::diagnose(&topology);
        let plan = plan(&topology, &findings, &["does-not-exist".to_string()], None);
        assert!(plan.items.is_empty());
        assert_eq!(plan.unsupported, vec!["does-not-exist".to_string()]);
    }
}
