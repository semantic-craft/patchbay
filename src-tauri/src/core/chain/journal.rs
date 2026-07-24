//! Repair journal: durable record of every chain-repair apply, with one-click
//! undo (issue #31).
//!
//! Each [`RepairItem`] the apply realized already names the edit completely —
//! `path`, `old_target`, `new_target`, `action` — so the applied results ARE
//! the undo record: no separate inverse plan is stored. [`undo`] replays the
//! inverses in REVERSE order under the same guard stack the repair engine
//! uses:
//!
//! * inverse of `create`  — remove the link we created;
//! * inverse of `repoint` — point the link back at its recorded old target;
//! * inverse of `remove`  — recreate the removed link at its old target
//!   (restoring a dangling link restores the FAULT, which is the point).
//!
//! Guards are per-item, mirroring `repair::apply` (the user-chosen semantics):
//! an item whose on-disk state no longer matches what the repair left behind
//! is skipped (`changed since repair`), the rest roll back, and `verified`
//! upstream is true only when every inverse landed. Physical entries are never
//! touched and every write re-validates the project boundary from scratch.

use serde::Serialize;

use std::path::{Path, PathBuf};

use super::ops::{self, EntryEvidence};
use super::repair::RepairItem;

/// A persisted repair record, parsed from the store row. `items` are the
/// apply-time results verbatim (including non-writing `exists`/`skip` items,
/// kept so the record renders the complete report).
#[derive(Debug, Clone, Serialize)]
pub struct JournalRecord {
    pub id: i64,
    /// Unix seconds of the apply.
    pub created_at: i64,
    /// Distinct registered project roots the writing items edited.
    pub projects: Vec<String>,
    /// Distinct fingerprints of the findings the writing items repaired.
    pub fingerprints: Vec<String>,
    pub items: Vec<RepairItem>,
    /// Apply-time verification flag (rescan confirmed the repaired shape).
    pub verified: bool,
    /// "applied" | "undone"
    pub status: String,
    /// Record card hidden by the user; history retained.
    pub dismissed: bool,
}

/// The result of undoing a journaled repair: per-inverse outcomes plus proof,
/// from a fresh rescan, that the rollback really restored the original state.
/// As with the repair outcome, `verified == false` means success must not be
/// reported.
#[derive(Debug, Clone, Serialize)]
pub struct UndoOutcome {
    pub results: Vec<RepairItem>,
    /// True only when every inverse landed (no skip/conflict/error) AND the
    /// rescan shows every repaired fingerprint back among the findings.
    pub verified: bool,
    /// Fresh scan clock stamped after the rescan.
    pub scanned_at: i64,
}

pub const STATUS_APPLIED: &str = "applied";
pub const STATUS_UNDONE: &str = "undone";

/// Parse a raw store row into a typed record. A row whose JSON no longer
/// parses (schema drift) yields an error the caller can skip over — one
/// corrupt record must not blank the whole journal.
pub fn parse_record(
    row: &crate::core::skill_store::RepairJournalRow,
) -> Result<JournalRecord, serde_json::Error> {
    Ok(JournalRecord {
        id: row.id,
        created_at: row.created_at,
        projects: serde_json::from_str(&row.projects)?,
        fingerprints: serde_json::from_str(&row.fingerprints)?,
        items: serde_json::from_str(&row.items)?,
        verified: row.verified,
        status: row.status.clone(),
        dismissed: row.dismissed,
    })
}

/// Replay a record's inverses, newest edit first. Returns one result per
/// ORIGINALLY WRITING item (non-writing originals never changed disk and are
/// omitted). Result actions reuse the repair vocabulary: `remove`/`repoint`/
/// `create` are realized inverse writes; `skip`/`conflict`/`error` are
/// refusals with a message.
pub fn undo(items: &[RepairItem]) -> Vec<RepairItem> {
    items
        .iter()
        .rev()
        .filter(|item| matches!(item.action.as_str(), "create" | "repoint" | "remove"))
        .map(undo_one)
        .collect()
}

/// Whether an undo (or repair) result action realized a write.
pub fn is_write(action: &str) -> bool {
    matches!(action, "create" | "repoint" | "remove")
}

fn undo_one(item: &RepairItem) -> RepairItem {
    let path = PathBuf::from(&item.path);
    let project = PathBuf::from(&item.project);

    // Boundary: identical to repair::apply — the recorded project must be a
    // real directory and the edited link's parent must resolve inside it.
    if ops::validate_project(&project).is_err() || !ops::parent_within_project(&project, &path) {
        return result(item, "conflict", None, None, "escapes the project boundary");
    }

    let current = ops::observe(&path);
    match item.action.as_str() {
        // We created this link; it must still be exactly ours to remove.
        "create" => match &current {
            EntryEvidence::Symlink(target) if Some(target) == item.new_target.as_ref() => {
                match ops::remove_symlink(&path) {
                    Ok(()) => result_ok(item, "remove", Some(target.clone()), None),
                    Err(e) => result(item, "error", None, None, &e.to_string()),
                }
            }
            EntryEvidence::Dir | EntryEvidence::File => {
                result(item, "conflict", None, None, "physical entry, not removing")
            }
            _ => result(item, "skip", None, None, "changed since repair"),
        },
        // We re-pointed this link; point it back at the recorded old target.
        "repoint" => {
            let Some(old) = item.old_target.clone() else {
                return result(item, "error", None, None, "record carries no old target");
            };
            match &current {
                EntryEvidence::Symlink(target) if Some(target) == item.new_target.as_ref() => {
                    if let Err(e) = ops::remove_symlink(&path) {
                        return result(item, "error", None, None, &e.to_string());
                    }
                    match ops::make_symlink(Path::new(&old), &path) {
                        Ok(()) => result_ok(item, "repoint", Some(target.clone()), Some(old)),
                        Err(e) => result(item, "error", None, None, &e.to_string()),
                    }
                }
                EntryEvidence::Dir | EntryEvidence::File => result(
                    item,
                    "conflict",
                    None,
                    None,
                    "physical entry, not repointing",
                ),
                _ => result(item, "skip", None, None, "changed since repair"),
            }
        }
        // We removed this (dangling) link; recreate it at its old target.
        "remove" => {
            let Some(old) = item.old_target.clone() else {
                return result(item, "error", None, None, "record carries no old target");
            };
            match &current {
                EntryEvidence::Absent => {
                    if let Some(parent) = path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            return result(item, "error", None, None, &e.to_string());
                        }
                        // Re-validate after creating: a symlinked parent that
                        // appeared must not redirect the write out of the project.
                        if !ops::parent_within_project(&project, &path) {
                            return result(
                                item,
                                "conflict",
                                None,
                                None,
                                "escapes the project boundary",
                            );
                        }
                    }
                    match ops::make_symlink(Path::new(&old), &path) {
                        Ok(()) => result_ok(item, "create", None, Some(old)),
                        Err(e) => result(item, "error", None, None, &e.to_string()),
                    }
                }
                _ => result(item, "skip", None, None, "changed since repair"),
            }
        }
        // Filtered out by `undo`; kept total for safety.
        _ => result(item, "skip", None, None, "was not applied"),
    }
}

/// An undo result mirroring the original item's identity, with the inverse
/// edit's own action/targets.
fn result_ok(
    item: &RepairItem,
    action: &str,
    old_target: Option<String>,
    new_target: Option<String>,
) -> RepairItem {
    RepairItem {
        action: action.to_string(),
        old_target,
        new_target,
        message: None,
        ..item.clone()
    }
}

fn result(
    item: &RepairItem,
    action: &str,
    old_target: Option<String>,
    new_target: Option<String>,
    message: &str,
) -> RepairItem {
    RepairItem {
        action: action.to_string(),
        old_target,
        new_target,
        message: Some(message.to_string()),
        ..item.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Portable stand-in for `std::os::unix::fs::symlink`. These fixtures link
    /// directories, and gating the module on unix meant they never ran on
    /// Windows — the platform whose symlink semantics differ most.
    fn symlink(
        target: impl AsRef<std::path::Path>,
        link: impl AsRef<std::path::Path>,
    ) -> std::io::Result<()> {
        crate::core::test_support::symlink_dir(target.as_ref(), link.as_ref())
    }
    use tempfile::tempdir;

    fn item(
        project: &Path,
        path: &Path,
        kind: &str,
        action: &str,
        old_target: Option<&str>,
        new_target: Option<&str>,
    ) -> RepairItem {
        RepairItem {
            fingerprint: "fp".to_string(),
            rule: "chain.broken_link".to_string(),
            deviation: "broken".to_string(),
            project: project.to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            action: action.to_string(),
            old_target: old_target.map(str::to_string),
            new_target: new_target.map(str::to_string),
            message: None,
        }
    }

    #[test]
    fn undo_of_repoint_restores_the_old_target() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        let dir = project.join(".agents/skills");
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("demo");
        // The repair re-pointed old → new; disk currently shows new.
        symlink(temp.path().join("new-target"), &link).unwrap();
        let old = temp.path().join("old-target").to_string_lossy().to_string();
        let new = temp.path().join("new-target").to_string_lossy().to_string();

        let results = undo(&[item(
            &project,
            &link,
            "relink_broken",
            "repoint",
            Some(&old),
            Some(&new),
        )]);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action, "repoint");
        assert_eq!(std::fs::read_link(&link).unwrap().to_string_lossy(), old);
    }

    #[test]
    fn undo_of_create_removes_only_the_exact_link_we_made() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        let dir = project.join(".agents/skills");
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("ours");
        let theirs = dir.join("theirs");
        let target = temp.path().join("target").to_string_lossy().to_string();
        symlink(&target, &ours).unwrap();
        // A link whose target changed after the repair must survive the undo.
        symlink(temp.path().join("elsewhere"), &theirs).unwrap();

        let results = undo(&[
            item(
                &project,
                &ours,
                "ensure_aggregate",
                "create",
                None,
                Some(&target),
            ),
            item(
                &project,
                &theirs,
                "ensure_aggregate",
                "create",
                None,
                Some(&target),
            ),
        ]);

        let ours_result = results.iter().find(|r| r.path.ends_with("ours")).unwrap();
        let theirs_result = results.iter().find(|r| r.path.ends_with("theirs")).unwrap();
        assert_eq!(ours_result.action, "remove");
        assert!(std::fs::symlink_metadata(&ours).is_err(), "ours removed");
        assert_eq!(theirs_result.action, "skip");
        assert_eq!(
            theirs_result.message.as_deref(),
            Some("changed since repair")
        );
        assert!(std::fs::symlink_metadata(&theirs).is_ok(), "theirs kept");
    }

    #[test]
    fn undo_of_remove_recreates_the_dangling_link() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        let dir = project.join(".claude/skills");
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("demo");
        let dead = temp.path().join("nowhere").to_string_lossy().to_string();
        // The repair removed the dangling link; disk currently shows absence.

        let results = undo(&[item(
            &project,
            &link,
            "remove_broken",
            "remove",
            Some(&dead),
            None,
        )]);

        assert_eq!(results[0].action, "create");
        // The recreated link dangles exactly like the original — restoring
        // the fault is the correct rollback.
        assert_eq!(std::fs::read_link(&link).unwrap().to_string_lossy(), dead);
        assert!(!link.exists());
    }

    #[test]
    fn inverses_replay_in_reverse_order_and_skip_non_writing_items() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        let dir = project.join(".agents/skills");
        std::fs::create_dir_all(&dir).unwrap();
        let agg = dir.join("demo");
        let target = temp.path().join("target").to_string_lossy().to_string();
        symlink(&target, &agg).unwrap();

        let results = undo(&[
            item(
                &project,
                &agg,
                "ensure_aggregate",
                "create",
                None,
                Some(&target),
            ),
            item(
                &project,
                &agg,
                "relink_broken",
                "exists",
                None,
                Some(&target),
            ),
        ]);

        // The non-writing `exists` item produced no inverse; the created link
        // was removed.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action, "remove");
        assert!(std::fs::symlink_metadata(&agg).is_err());
    }

    #[test]
    fn a_physical_entry_is_never_touched() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        let dir = project.join(".agents/skills");
        let entry = dir.join("demo");
        std::fs::create_dir_all(&entry).unwrap();
        std::fs::write(entry.join("SKILL.md"), "content").unwrap();
        let target = temp.path().join("target").to_string_lossy().to_string();

        let results = undo(&[item(
            &project,
            &entry,
            "ensure_aggregate",
            "create",
            None,
            Some(&target),
        )]);

        assert_eq!(results[0].action, "conflict");
        assert!(entry.join("SKILL.md").is_file());
    }

    #[test]
    fn an_edit_outside_the_project_boundary_is_refused() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        // The recorded path lies outside the recorded project root.
        let outside = temp.path().join("elsewhere/link");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        let target = temp.path().join("target").to_string_lossy().to_string();
        symlink(&target, &outside).unwrap();

        let results = undo(&[item(
            &project,
            &outside,
            "ensure_aggregate",
            "create",
            None,
            Some(&target),
        )]);

        assert_eq!(results[0].action, "conflict");
        assert_eq!(
            results[0].message.as_deref(),
            Some("escapes the project boundary")
        );
        assert!(std::fs::symlink_metadata(&outside).is_ok(), "untouched");
    }
}
