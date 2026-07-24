//! Plan/apply `normalize` — the first instructions *write* operation (design
//! §4.1). It turns the four repairable Doctor findings into a bounded, previewable
//! set of content edits that converge a project on the canonical shape
//! (`AGENTS.md` body + `@AGENTS.md` wrapper entries), **mechanically merging only,
//! never semantically**.
//!
//! Repairable rules and their edits:
//! * `missing_canonical` — promote the entry body verbatim to a new `AGENTS.md`,
//!   then rewrite the entry to a pure wrapper. (Full promotion is the only choice
//!   that makes no semantic judgement; the preview shows the whole body and a human
//!   reviews afterwards — design §4.1.)
//! * `dual_body` — block-merge the entry against the canonical: drop blocks whose
//!   normalized fingerprint already appears in the canonical, keep the remainder in
//!   original order behind an append marker. **The canonical is never rewritten.**
//! * `symlink_entry` — replace a symlink-to-canonical with a wrapper file (the link
//!   is removed, its target file untouched and recorded in the snapshot manifest).
//! * `missing_entry` — create the project-root `CLAUDE.md` wrapper.
//!
//! Like chain's repair engine this is a guarded two phase operation: [`plan`] is
//! read-only over freshly diagnosed findings and captures each target's on-disk
//! [`WriteEvidence`] (the TOCTOU baseline); [`apply`] snapshots the originals
//! first (§8 "快照先行"), then routes every mutation through
//! [`super::write_guard`], which re-verifies that baseline and refuses (never
//! overwrites) on any drift. The canonical `AGENTS.md` is create-only in the guard,
//! so no code path here can rewrite it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::blocks;
use super::doctor::Finding;
use super::snapshot;
use super::write_guard::{self, WriteEvidence, WriteOutcome};

/// A pure `@AGENTS.md` wrapper entry — the normalized shape of a Claude entry.
pub const WRAPPER: &str = "@AGENTS.md\n";
/// The append marker separating the wrapper from a Claude-specific append layer
/// (design §1; an HTML comment, invisible to the model).
pub const APPEND_MARKER: &str = "<!-- patchbay:append claude -->";

/// The Doctor rules `normalize` can fix (design §4.1). Any other finding is left
/// for report-only handling.
pub fn is_fixable_rule(rule: &str) -> bool {
    matches!(
        rule,
        "instructions.missing_canonical"
            | "instructions.dual_body"
            | "instructions.symlink_entry"
            | "instructions.missing_entry"
    )
}

// ── plan / outcome shapes (design §4.1 / §5) ────────────────────────────────

/// One planned (or realized) file edit. The same struct carries the previewed
/// intent and, after [`apply`], the outcome — a result whose `action` is still a
/// write verb (`create`/`rewrite`/`replace_link`) means the edit succeeded; a
/// `conflict` means it was refused and nothing was written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizeItem {
    /// Fingerprint of the Doctor finding this item fixes.
    pub fingerprint: String,
    /// Stable rule id copied from the finding (e.g. `instructions.dual_body`).
    pub rule: String,
    /// Registered project root the edited file lives in.
    pub project: String,
    /// Absolute path of the file this item writes.
    pub path: String,
    /// `create` | `rewrite` | `replace_link` | `noop` | `conflict`. The first
    /// three write; `noop` is already-compliant; `conflict` is a refusal.
    pub action: String,
    /// On-disk evidence at plan time — the TOCTOU baseline apply re-checks.
    pub before: WriteEvidence,
    /// The intended file content after the edit; `None` for `noop`/`conflict`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_content: Option<String>,
    /// Whether apply snapshots this target's original bytes/link before writing
    /// (true for `rewrite`/`replace_link`; false for a `create`, which has nothing
    /// to restore).
    pub snapshot: bool,
    /// A read-input this item's `after_content` was derived from at plan time —
    /// the canonical body for a `dual_body` merge, or the entry body for a
    /// `missing_canonical` promotion. Apply re-verifies its evidence before
    /// writing, so drift in the *source* (not only the write target) refuses the
    /// write: §4.1 reads both `B` and `E`, and "变了即拒" must cover both, else the
    /// entry could be rewritten from a stale merge while success is reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<SourceGuard>,
    /// Conflict reason (or other note); `None` on a clean write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A plan-time evidence baseline for a *read* input an item's content depends on,
/// re-verified before the write so source drift refuses (never silently rewrites
/// from a stale computation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceGuard {
    pub path: String,
    pub before: WriteEvidence,
}

/// A previewed, guarded normalize operation. Produced by [`plan`] and consumed by
/// [`apply`]; it carries everything apply needs to re-validate independently (the
/// per-item TOCTOU baseline in `before`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizePlan {
    pub items: Vec<NormalizeItem>,
    /// Requested fingerprints matching no current *fixable* finding — reported,
    /// not fixed.
    pub unsupported: Vec<String>,
    pub scanned_at: i64,
}

/// The result of applying a plan: per-item outcomes, the snapshot id covering the
/// originals, and — set by the service from a fresh rescan — whether every fix is
/// really on disk. As with chain, `verified == false` means success must not be
/// reported.
#[derive(Debug, Clone, Serialize)]
pub struct NormalizeOutcome {
    pub items: Vec<NormalizeItem>,
    /// Snapshot id covering the rewritten/removed originals; `None` when no item
    /// had an original to capture (only fresh creates).
    pub snapshot_id: Option<String>,
    pub verified: bool,
    pub scanned_at: i64,
}

// ── plan phase (read-only) ──────────────────────────────────────────────────

/// Build a normalize plan for `project_root` from freshly diagnosed `findings`.
/// With a non-empty `fingerprints`, only those findings are planned (a requested
/// fingerprint that maps to no fixable finding is reported `unsupported`); empty
/// means every fixable finding in the project. Read-only apart from
/// [`write_guard::observe_target`] per touched path.
pub fn plan(
    findings: &[Finding],
    project_root: &Path,
    fingerprints: &[String],
    scanned_at: i64,
) -> NormalizePlan {
    let requested: Option<HashSet<&str>> = if fingerprints.is_empty() {
        None
    } else {
        Some(fingerprints.iter().map(String::as_str).collect())
    };

    let mut items = Vec::new();
    let mut matched: HashSet<String> = HashSet::new();
    let project = project_root.to_string_lossy().to_string();

    for finding in findings {
        if !is_fixable_rule(&finding.rule) {
            continue;
        }
        if let Some(req) = &requested {
            if !req.contains(finding.fingerprint.as_str()) {
                continue;
            }
        }
        let before_len = items.len();
        match finding.rule.as_str() {
            "instructions.missing_canonical" => {
                plan_missing_canonical(finding, &project, &mut items)
            }
            "instructions.dual_body" => plan_dual_body(finding, &project, &mut items),
            "instructions.symlink_entry" => plan_symlink_entry(finding, &project, &mut items),
            "instructions.missing_entry" => plan_missing_entry(finding, &project, &mut items),
            _ => {}
        }
        if items.len() > before_len {
            matched.insert(finding.fingerprint.clone());
        }
    }

    // Requested fingerprints that matched no fixable finding, de-duplicated in the
    // order the caller listed them.
    let unsupported = match &requested {
        None => Vec::new(),
        Some(_) => {
            let mut seen = HashSet::new();
            fingerprints
                .iter()
                .filter(|fp| !matched.contains(fp.as_str()) && seen.insert(fp.as_str()))
                .cloned()
                .collect()
        }
    };

    NormalizePlan {
        items,
        unsupported,
        scanned_at,
    }
}

/// The canonical body path a finding is about (its `counterpart_path`).
fn canonical_of(finding: &Finding) -> Option<PathBuf> {
    finding
        .evidence
        .counterpart_path
        .as_ref()
        .map(PathBuf::from)
}

#[allow(clippy::too_many_arguments)]
fn item(
    finding: &Finding,
    project: &str,
    path: &Path,
    action: &str,
    before: WriteEvidence,
    after_content: Option<String>,
    snapshot: bool,
    message: Option<String>,
) -> NormalizeItem {
    NormalizeItem {
        fingerprint: finding.fingerprint.clone(),
        rule: finding.rule.clone(),
        project: project.to_string(),
        path: path.to_string_lossy().to_string(),
        action: action.to_string(),
        before,
        after_content,
        snapshot,
        depends_on: None,
        message,
    }
}

fn conflict_item(
    finding: &Finding,
    project: &str,
    path: &Path,
    before: WriteEvidence,
    message: &str,
) -> NormalizeItem {
    item(
        finding,
        project,
        path,
        "conflict",
        before,
        None,
        false,
        Some(message.to_string()),
    )
}

/// Read a target as UTF-8 text, or `Err` with a ready-to-report reason when it is
/// missing or not UTF-8 (design §4.1 step 1).
fn read_utf8(path: &Path) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// `missing_canonical` → promote the entry body verbatim to a new `AGENTS.md`,
/// then rewrite the entry to a wrapper. The create item is pushed first so apply
/// materializes the canonical before the wrapper points at it.
fn plan_missing_canonical(finding: &Finding, project: &str, items: &mut Vec<NormalizeItem>) {
    let entry = PathBuf::from(&finding.evidence.primary_path);
    let Some(canonical) = canonical_of(finding) else {
        return;
    };

    let entry_before = write_guard::observe_target(&entry);
    let body = match read_utf8(&entry) {
        Ok(text) => text,
        Err(reason) => {
            items.push(conflict_item(
                finding,
                project,
                &entry,
                entry_before,
                &reason,
            ));
            return;
        }
    };

    // Item 1: create AGENTS.md with the entry body verbatim.
    let canon_before = write_guard::observe_target(&canonical);
    match &canon_before {
        WriteEvidence::Absent => {
            // The promoted body IS the entry's content, so guard the entry against
            // drift: if it changes before apply, the create is refused rather than
            // materializing a stale canonical (Warning: entry is the read source).
            let mut create = item(
                finding,
                project,
                &canonical,
                "create",
                canon_before.clone(),
                Some(body),
                false,
                None,
            );
            create.depends_on = Some(SourceGuard {
                path: entry.to_string_lossy().to_string(),
                before: entry_before.clone(),
            });
            items.push(create);
        }
        WriteEvidence::Dir => {
            items.push(conflict_item(
                finding,
                project,
                &canonical,
                canon_before,
                "canonical path is occupied by a directory",
            ));
            return;
        }
        // A canonical that already exists means the finding is stale; refuse
        // rather than promote a second body.
        _ => {
            items.push(conflict_item(
                finding,
                project,
                &canonical,
                canon_before,
                "canonical already exists since preview",
            ));
            return;
        }
    }

    // Item 2: rewrite the entry to a pure wrapper.
    items.push(item(
        finding,
        project,
        &entry,
        "rewrite",
        entry_before,
        Some(WRAPPER.to_string()),
        true,
        None,
    ));
}

/// `dual_body` → block-merge the entry against the canonical, rewriting only the
/// entry. The canonical is read but never written.
fn plan_dual_body(finding: &Finding, project: &str, items: &mut Vec<NormalizeItem>) {
    let entry = PathBuf::from(&finding.evidence.primary_path);
    let Some(canonical) = canonical_of(finding) else {
        return;
    };
    let entry_before = write_guard::observe_target(&entry);

    let entry_text = match read_utf8(&entry) {
        Ok(t) => t,
        Err(reason) => {
            items.push(conflict_item(
                finding,
                project,
                &entry,
                entry_before,
                &reason,
            ));
            return;
        }
    };
    let canon_text = match read_utf8(&canonical) {
        Ok(t) => t,
        Err(reason) => {
            items.push(conflict_item(
                finding,
                project,
                &entry,
                entry_before,
                &reason,
            ));
            return;
        }
    };

    let merged = merge_dual_body(&entry_text, &canon_text);
    if merged == entry_text {
        // Already in normalized shape (nothing to change).
        items.push(item(
            finding,
            project,
            &entry,
            "noop",
            entry_before,
            None,
            false,
            None,
        ));
        return;
    }
    // The merge's block set depends on the canonical; guard it against drift so a
    // stale merge can never be written (Warning: canonical is a read input).
    let canon_before = write_guard::observe_target(&canonical);
    let mut rewrite = item(
        finding,
        project,
        &entry,
        "rewrite",
        entry_before,
        Some(merged),
        true,
        None,
    );
    rewrite.depends_on = Some(SourceGuard {
        path: canonical.to_string_lossy().to_string(),
        before: canon_before,
    });
    items.push(rewrite);
}

/// `symlink_entry` → replace the symlink with a wrapper file (apply removes the
/// link, then creates the wrapper). A target that is no longer a symlink (drift)
/// conflicts.
fn plan_symlink_entry(finding: &Finding, project: &str, items: &mut Vec<NormalizeItem>) {
    let entry = PathBuf::from(&finding.evidence.primary_path);
    let before = write_guard::observe_target(&entry);
    if !matches!(before, WriteEvidence::Symlink { .. }) {
        items.push(conflict_item(
            finding,
            project,
            &entry,
            before,
            "entry is no longer a symlink since preview",
        ));
        return;
    }
    items.push(item(
        finding,
        project,
        &entry,
        "replace_link",
        before,
        Some(WRAPPER.to_string()),
        true,
        None,
    ));
}

/// `missing_entry` → create the project-root `CLAUDE.md` wrapper.
fn plan_missing_entry(finding: &Finding, project: &str, items: &mut Vec<NormalizeItem>) {
    let entry = PathBuf::from(&finding.evidence.primary_path);
    let before = write_guard::observe_target(&entry);
    match &before {
        WriteEvidence::Absent => items.push(item(
            finding,
            project,
            &entry,
            "create",
            before.clone(),
            Some(WRAPPER.to_string()),
            false,
            None,
        )),
        WriteEvidence::Dir => items.push(conflict_item(
            finding,
            project,
            &entry,
            before,
            "entry path is occupied by a directory",
        )),
        // Something appeared at the entry since the finding — refuse.
        _ => items.push(conflict_item(
            finding,
            project,
            &entry,
            before,
            "entry already exists since preview",
        )),
    }
}

/// The mechanical block merge (design §4.1): keep the entry's blocks whose
/// normalized fingerprint is NOT already in the canonical, in original order,
/// behind an append marker. Empty remainder ⇒ a pure wrapper.
pub fn merge_dual_body(entry_text: &str, canonical_text: &str) -> String {
    let canon_fps = blocks::fingerprint_set(canonical_text);
    let remainder: Vec<String> = blocks::blockize(entry_text)
        .into_iter()
        .filter(|b| !canon_fps.contains(&b.fingerprint))
        .map(|b| b.text)
        .collect();
    if remainder.is_empty() {
        WRAPPER.to_string()
    } else {
        format!(
            "@AGENTS.md\n\n{APPEND_MARKER}\n\n{}\n",
            remainder.join("\n\n")
        )
    }
}

// ── apply phase (writes, guarded) ───────────────────────────────────────────

fn is_writing(action: &str) -> bool {
    matches!(action, "create" | "rewrite" | "replace_link")
}

/// Whether an applied item's action means the edit landed cleanly (a write verb
/// or a no-op); only `conflict` is a refusal.
pub fn is_success(action: &str) -> bool {
    is_writing(action) || action == "noop"
}

/// Apply a previewed plan, snapshotting originals first (§8). Returns the realized
/// items and the snapshot id (if any original was captured). Every mutation goes
/// through [`write_guard`], which re-verifies the TOCTOU baseline; a drifted
/// target is refused as `conflict` and nothing is written. `now_ms` and
/// `snapshot_root` are injected so the phase is deterministic and hermetic.
pub fn apply(
    plan: &NormalizePlan,
    project_root: &Path,
    snapshot_root: &Path,
    now_ms: i64,
) -> (Vec<NormalizeItem>, Option<String>) {
    // Snapshot-first: capture the originals of writing items that still match
    // their baseline (a drifted one will conflict at write time and must not be
    // captured — snapshot::capture requires a live file/symlink). The write's own
    // `verify_unchanged` still guards content, so at worst an adversarial writer
    // that flaps a file away-and-back to its exact baseline hash within this
    // window could have its backup skipped; the content written stays correct.
    let mut sources: Vec<PathBuf> = Vec::new();
    for it in &plan.items {
        if is_writing(&it.action) && it.snapshot {
            let path = PathBuf::from(&it.path);
            if write_guard::verify_unchanged(&it.before, &path) {
                sources.push(path);
            }
        }
    }
    let (snapshot_id, snapshot_failed) = if sources.is_empty() {
        (None, false)
    } else {
        match snapshot::capture(snapshot_root, now_ms, &sources) {
            Ok(manifest) => (Some(manifest.id), false),
            // No snapshot ⇒ no mutation (§8): items needing a snapshot conflict.
            Err(_) => (None, true),
        }
    };

    let results = plan
        .items
        .iter()
        .map(|it| {
            if !is_writing(&it.action) {
                return it.clone(); // noop / conflict reflected verbatim
            }
            // No snapshot ⇒ no mutation, for the WHOLE operation (§8): a capture
            // failure refuses every write, including a `create` that needs no
            // backup of its own, so an apply never lands half-done.
            if snapshot_failed {
                return refuse(it, "snapshot failed; nothing written");
            }
            apply_one(it, project_root)
        })
        .collect();

    (results, snapshot_id)
}

/// Apply one writing item through the guard stack, mapping the guard's outcome
/// back onto the item's action (kept as the write verb on success, `conflict` on
/// refusal).
fn apply_one(it: &NormalizeItem, project_root: &Path) -> NormalizeItem {
    // Source guard: refuse if a read-input this write was derived from drifted
    // (the canonical of a merge, the entry body of a promotion). Without this the
    // target could match its baseline while the merge is computed from stale
    // source content.
    if let Some(dep) = &it.depends_on {
        if !write_guard::verify_unchanged(&dep.before, Path::new(&dep.path)) {
            return refuse(it, "source changed since preview (content guard)");
        }
    }

    let path = PathBuf::from(&it.path);
    let content = it.after_content.as_deref().unwrap_or("");
    let outcome = match it.action.as_str() {
        "create" => write_guard::create_only(project_root, &path, content),
        "rewrite" => write_guard::rewrite(project_root, &path, content, &it.before),
        "replace_link" => apply_replace_link(project_root, &path, content, &it.before),
        _ => return it.clone(),
    };
    match outcome {
        Ok(WriteOutcome::Created | WriteOutcome::Rewritten | WriteOutcome::RemovedLink) => {
            it.clone()
        }
        Ok(WriteOutcome::Conflict(msg)) => refuse(it, &msg),
        Err(e) => refuse(it, &e.to_string()),
    }
}

/// The symlink→wrapper conversion: remove the link (guarded), then create the
/// wrapper in its place. Returns the *creation* outcome so a clean run reports
/// `Created`; if the link is removed but the wrapper cannot be created, the item
/// conflicts (the removed link is recorded in the snapshot for manual recovery).
fn apply_replace_link(
    project_root: &Path,
    path: &Path,
    content: &str,
    before: &WriteEvidence,
) -> std::io::Result<WriteOutcome> {
    match write_guard::remove_symlink(project_root, path, before)? {
        WriteOutcome::RemovedLink => write_guard::create_only(project_root, path, content),
        other => Ok(other), // Conflict from the guard — reflected as-is.
    }
}

fn refuse(it: &NormalizeItem, message: &str) -> NormalizeItem {
    NormalizeItem {
        action: "conflict".to_string(),
        message: Some(message.to_string()),
        ..it.clone()
    }
}

/// Verify a normalize apply against a fresh diagnosis: clean only when no item was
/// refused AND none of the applied items' findings still appear as fixable (each
/// fingerprint is identity-keyed, so a fixed finding simply disappears).
pub fn verify(results: &[NormalizeItem], rediagnosed: &[Finding]) -> bool {
    if results.iter().any(|i| i.action == "conflict") {
        return false;
    }
    let remaining: HashSet<&str> = rediagnosed
        .iter()
        .filter(|f| is_fixable_rule(&f.rule))
        .map(|f| f.fingerprint.as_str())
        .collect();
    !results
        .iter()
        .any(|i| remaining.contains(i.fingerprint.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::instructions::{scanner, surfaces::Agent};
    use std::fs;
    use std::path::Path;
    use tempfile::{tempdir, TempDir};

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Diagnose a project the way the service does (scan → diagnose).
    fn findings_for(root: &Path, home: &Path) -> Vec<Finding> {
        let scan = scanner::scan_with(&[root.to_path_buf()], home, &[Agent::Claude], 0);
        crate::core::instructions::doctor::diagnose(&scan, home)
    }

    fn plan_for(root: &Path, home: &Path, fps: &[String]) -> NormalizePlan {
        plan(&findings_for(root, home), root, fps, 0)
    }

    /// Apply a plan under a temp snapshot root; returns (results, snapshot_id).
    fn apply_in(
        plan: &NormalizePlan,
        root: &Path,
        snaps: &TempDir,
    ) -> (Vec<NormalizeItem>, Option<String>) {
        apply(plan, root, snaps.path(), 1_700_000_000_000)
    }

    fn item_for<'a>(items: &'a [NormalizeItem], suffix: &str) -> &'a NormalizeItem {
        items
            .iter()
            .find(|i| i.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no item for {suffix} in {:?}", paths(items)))
    }

    fn paths(items: &[NormalizeItem]) -> Vec<&str> {
        items.iter().map(|i| i.path.as_str()).collect()
    }

    // ── merge algorithm ──────────────────────────────────────────────────────

    #[test]
    fn merge_drops_shared_blocks_and_keeps_remainder_in_order() {
        let canonical = "# Shared\n\ncommon para\n";
        let entry = "# Shared\n\ncommon para\n\nclaude first\n\nclaude second\n";
        let merged = merge_dual_body(entry, canonical);
        assert!(merged.starts_with("@AGENTS.md\n\n<!-- patchbay:append claude -->\n\n"));
        // Remainder preserved in original order; shared blocks dropped.
        let tail = merged.split(APPEND_MARKER).nth(1).unwrap();
        let first = tail.find("claude first").unwrap();
        let second = tail.find("claude second").unwrap();
        assert!(first < second);
        assert!(!tail.contains("common para"));
    }

    #[test]
    fn merge_with_no_remainder_is_a_pure_wrapper() {
        let canonical = "# A\n\nbody\n";
        let entry = "# A\n\nbody\n"; // every block already in the canonical
        assert_eq!(merge_dual_body(entry, canonical), "@AGENTS.md\n");
    }

    // ── dual_body: canonical is never rewritten ─────────────────────────────

    #[test]
    fn dual_body_rewrites_entry_and_leaves_canonical_byte_identical() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        let canonical_bytes = "# Shared\n\ncommon\n";
        write(&root.path().join("AGENTS.md"), canonical_bytes);
        write(
            &root.path().join("CLAUDE.md"),
            "# Shared\n\ncommon\n\nclaude only note\n",
        );

        let plan = plan_for(root.path(), home.path(), &[]);
        let entry_item = item_for(&plan.items, "CLAUDE.md");
        assert_eq!(entry_item.rule, "instructions.dual_body");
        assert_eq!(entry_item.action, "rewrite");
        assert!(entry_item.snapshot);
        // No plan item ever touches the canonical.
        assert!(!plan.items.iter().any(|i| i.path.ends_with("AGENTS.md")));

        let (results, snap) = apply_in(&plan, root.path(), &snaps);
        assert_eq!(item_for(&results, "CLAUDE.md").action, "rewrite");
        assert!(snap.is_some(), "the rewritten entry is snapshotted");

        // AC: the canonical bytes are unchanged.
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            canonical_bytes
        );
        // The entry is now a wrapper-plus with the claude-only remainder.
        let entry = fs::read_to_string(root.path().join("CLAUDE.md")).unwrap();
        assert!(entry.starts_with("@AGENTS.md\n"));
        assert!(entry.contains(APPEND_MARKER));
        assert!(entry.contains("claude only note"));
        assert!(!entry.contains("common"));
    }

    // ── missing_canonical: promote body, wrapperize entry ───────────────────

    #[test]
    fn missing_canonical_promotes_body_verbatim_then_wrappers_entry() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        let body = "# Real Instructions\n\nlots of content\n";
        write(&root.path().join("CLAUDE.md"), body);

        let plan = plan_for(root.path(), home.path(), &[]);
        // create AGENTS.md must precede rewrite CLAUDE.md.
        assert!(plan.items[0].path.ends_with("AGENTS.md"));
        assert_eq!(plan.items[0].action, "create");
        assert!(plan.items[1].path.ends_with("CLAUDE.md"));
        assert_eq!(plan.items[1].action, "rewrite");

        apply_in(&plan, root.path(), &snaps);
        // Canonical is the promoted body verbatim; entry is a pure wrapper.
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            body
        );
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    // ── missing_entry: create wrapper ───────────────────────────────────────

    #[test]
    fn missing_entry_creates_the_wrapper() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# canonical\n");

        let plan = plan_for(root.path(), home.path(), &[]);
        let it = item_for(&plan.items, "CLAUDE.md");
        assert_eq!(it.rule, "instructions.missing_entry");
        assert_eq!(it.action, "create");
        assert!(!it.snapshot); // nothing to back up

        let (_r, snap) = apply_in(&plan, root.path(), &snaps);
        assert!(snap.is_none(), "a pure create captures no snapshot");
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    // ── symlink_entry: replace_link ─────────────────────────────────────────

    #[test]
    fn symlink_entry_becomes_a_wrapper_and_snapshots_the_link() {
        // The design's core claim — "symlink is the variant, the wrapper is the
        // canonical form, privilege-free on Windows" — is precisely about this
        // conversion, so it is the last test that should be skipped on Windows.
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# canonical\n");
        crate::core::test_support::expect_symlink_file(
            std::path::Path::new("AGENTS.md"),
            &root.path().join("CLAUDE.md"),
        );

        let plan = plan_for(root.path(), home.path(), &[]);
        let it = item_for(&plan.items, "CLAUDE.md");
        assert_eq!(it.rule, "instructions.symlink_entry");
        assert_eq!(it.action, "replace_link");

        let (results, snap) = apply_in(&plan, root.path(), &snaps);
        assert_eq!(item_for(&results, "CLAUDE.md").action, "replace_link");
        assert!(snap.is_some());
        // The entry is a regular wrapper file now (not a symlink); canonical intact.
        let md = std::fs::symlink_metadata(root.path().join("CLAUDE.md")).unwrap();
        assert!(!md.file_type().is_symlink());
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            "# canonical\n"
        );
    }

    // ── idempotency (AC) ────────────────────────────────────────────────────

    #[test]
    fn second_normalize_pass_is_all_noop() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        // A body-in-entry project: first pass promotes + wrappers.
        write(&root.path().join("CLAUDE.md"), "# body\n\nnote\n");

        let plan1 = plan_for(root.path(), home.path(), &[]);
        let (r1, _) = apply_in(&plan1, root.path(), &snaps);
        assert!(r1.iter().all(|i| is_success(&i.action)));

        // Second pass: nothing left to fix.
        let plan2 = plan_for(root.path(), home.path(), &[]);
        assert!(
            plan2.items.iter().all(|i| i.action == "noop"),
            "second pass must be all-noop, got {:?}",
            plan2.items.iter().map(|i| &i.action).collect::<Vec<_>>()
        );
        // In practice the fixed findings vanish entirely, so the plan is empty.
        assert!(plan2.items.is_empty());
    }

    // ── TOCTOU (AC) ──────────────────────────────────────────────────────────

    #[test]
    fn tampered_target_conflicts_and_is_not_written() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# canonical\n\nshared\n");
        write(
            &root.path().join("CLAUDE.md"),
            "# canonical\n\nshared\n\nclaude note\n",
        );

        let plan = plan_for(root.path(), home.path(), &[]);
        assert_eq!(item_for(&plan.items, "CLAUDE.md").action, "rewrite");

        // Tamper with the entry AFTER planning.
        write(&root.path().join("CLAUDE.md"), "totally different now\n");
        let (results, _snap) = apply_in(&plan, root.path(), &snaps);
        let it = item_for(&results, "CLAUDE.md");
        assert_eq!(it.action, "conflict");
        assert!(it.message.as_deref().unwrap().contains("guard"));
        // The tampered content is left exactly as-is; nothing overwrote it.
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "totally different now\n"
        );
    }

    #[test]
    fn dual_body_canonical_drift_conflicts_and_leaves_entry_untouched() {
        // The merge's dropped-block set is decided by the canonical. If the
        // canonical drifts between plan and apply, the frozen merge is stale — the
        // write must be refused, never applied from a stale computation.
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# Shared\n\nshared block\n");
        let entry_original = "# Shared\n\nshared block\n\nclaude note\n";
        write(&root.path().join("CLAUDE.md"), entry_original);

        let plan = plan_for(root.path(), home.path(), &[]);
        let it = item_for(&plan.items, "CLAUDE.md");
        assert_eq!(it.action, "rewrite");
        assert!(it.depends_on.is_some(), "the canonical is guarded");

        // The canonical drifts (a shared block deleted) — the ENTRY is untouched,
        // so its own baseline still matches; only the source guard can catch this.
        write(&root.path().join("AGENTS.md"), "# Shared\n");

        let (results, _snap) = apply_in(&plan, root.path(), &snaps);
        let it = item_for(&results, "CLAUDE.md");
        assert_eq!(it.action, "conflict");
        assert!(it.message.as_deref().unwrap().contains("source changed"));
        // The entry is left exactly as it was — no stale merge written.
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            entry_original
        );
    }

    #[test]
    fn missing_canonical_entry_drift_refuses_both_items() {
        // The promoted body is the entry's content; drift in the entry must refuse
        // the AGENTS.md create (its source) as well as the entry rewrite.
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("CLAUDE.md"), "# body\n\noriginal\n");

        let plan = plan_for(root.path(), home.path(), &[]);
        let create = item_for(&plan.items, "AGENTS.md");
        assert_eq!(create.action, "create");
        assert!(create.depends_on.is_some(), "the entry source is guarded");

        // Entry drifts after planning.
        write(&root.path().join("CLAUDE.md"), "wholly rewritten\n");
        let (results, _snap) = apply_in(&plan, root.path(), &snaps);
        assert!(
            results.iter().all(|i| i.action == "conflict"),
            "both items refuse on entry drift: {:?}",
            results.iter().map(|i| &i.action).collect::<Vec<_>>()
        );
        // No stale canonical was materialized.
        assert!(!root.path().join("AGENTS.md").exists());
    }

    #[test]
    fn snapshot_failure_writes_nothing() {
        // "快照先行 ⇒ 全或无": if capture cannot run, no item writes — not even a
        // create that needs no backup of its own.
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# shared\n\ncommon\n");
        write(
            &root.path().join("CLAUDE.md"),
            "# shared\n\ncommon\n\nclaude note\n",
        );
        let plan = plan_for(root.path(), home.path(), &[]);
        assert_eq!(item_for(&plan.items, "CLAUDE.md").action, "rewrite");

        // A snapshot ROOT that is a regular file makes `capture` fail.
        let bad_root = tempdir().unwrap();
        let bad_snap_root = bad_root.path().join("not-a-dir");
        fs::write(&bad_snap_root, "x").unwrap();

        let (results, snap_id) = apply(&plan, root.path(), &bad_snap_root, 1);
        assert!(snap_id.is_none());
        assert_eq!(item_for(&results, "CLAUDE.md").action, "conflict");
        // The entry is untouched.
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "# shared\n\ncommon\n\nclaude note\n"
        );
    }

    // ── conflict classes (AC) ───────────────────────────────────────────────

    #[test]
    fn conflict_non_utf8_entry() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        // Non-UTF-8 body with no canonical → missing_canonical fires, plan conflicts.
        fs::write(root.path().join("CLAUDE.md"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
        let plan = plan_for(root.path(), home.path(), &[]);
        let it = item_for(&plan.items, "CLAUDE.md");
        assert_eq!(it.action, "conflict");
        assert!(it.message.as_deref().unwrap().contains("UTF-8"));
    }

    #[test]
    fn conflict_directory_occupies_canonical_path() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("CLAUDE.md"), "# body\n");
        // A directory sits where AGENTS.md would be created.
        fs::create_dir(root.path().join("AGENTS.md")).unwrap();
        let plan = plan_for(root.path(), home.path(), &[]);
        let it = item_for(&plan.items, "AGENTS.md");
        assert_eq!(it.action, "conflict");
        assert!(it.message.as_deref().unwrap().contains("directory"));
    }

    #[test]
    fn conflict_symlink_retargeted_between_plan_and_apply() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# canonical\n");
        crate::core::test_support::expect_symlink_file(
            std::path::Path::new("AGENTS.md"),
            &root.path().join("CLAUDE.md"),
        );

        let plan = plan_for(root.path(), home.path(), &[]);
        assert_eq!(item_for(&plan.items, "CLAUDE.md").action, "replace_link");

        // Re-point the symlink elsewhere after planning (off-canonical / drift).
        write(&root.path().join("OTHER.md"), "x\n");
        fs::remove_file(root.path().join("CLAUDE.md")).unwrap();
        crate::core::test_support::expect_symlink_file(
            std::path::Path::new("OTHER.md"),
            &root.path().join("CLAUDE.md"),
        );

        let (results, _snap) = apply_in(&plan, root.path(), &snaps);
        let it = item_for(&results, "CLAUDE.md");
        assert_eq!(it.action, "conflict");
        // The retargeted link is untouched.
        assert_eq!(
            std::fs::read_link(root.path().join("CLAUDE.md")).unwrap(),
            PathBuf::from("OTHER.md")
        );
    }

    // ── --fingerprint targeting + unsupported ───────────────────────────────

    #[test]
    fn fingerprint_targets_only_the_named_finding() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        // Two independent fixables: a body-in-entry (missing_canonical) — but that
        // is one finding touching two files. Use missing_entry + a broken import is
        // not fixable, so instead: dual_body + an unrelated project is overkill.
        // Simplest: one dual_body finding; target it by fingerprint, and target a
        // bogus fingerprint to exercise `unsupported`.
        write(&root.path().join("AGENTS.md"), "# a\n\nshared\n");
        write(&root.path().join("CLAUDE.md"), "# a\n\nshared\n\nextra\n");
        let findings = findings_for(root.path(), home.path());
        let fp = findings
            .iter()
            .find(|f| f.rule == "instructions.dual_body")
            .unwrap()
            .fingerprint
            .clone();

        let targeted = plan(&findings, root.path(), &[fp.clone()], 0);
        assert_eq!(targeted.items.len(), 1);
        assert_eq!(targeted.items[0].fingerprint, fp);
        assert!(targeted.unsupported.is_empty());

        let bogus = plan(&findings, root.path(), &["nope".to_string()], 0);
        assert!(bogus.items.is_empty());
        assert_eq!(bogus.unsupported, vec!["nope".to_string()]);
    }

    // ── snapshot content is recoverable ─────────────────────────────────────

    #[test]
    fn snapshot_captures_original_entry_bytes_for_recovery() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        let original = "# canonical\n\nshared\n\nclaude note\n";
        write(&root.path().join("AGENTS.md"), "# canonical\n\nshared\n");
        write(&root.path().join("CLAUDE.md"), original);

        let plan = plan_for(root.path(), home.path(), &[]);
        let (_results, snap_id) = apply_in(&plan, root.path(), &snaps);
        let id = snap_id.expect("rewrite is snapshotted");

        // The snapshot dir holds a manifest and a payload with the original bytes.
        let manifest_raw = fs::read(snaps.path().join(&id).join(snapshot::MANIFEST_NAME)).unwrap();
        let manifest: snapshot::SnapshotManifest = serde_json::from_slice(&manifest_raw).unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|e| e.original_path.ends_with("CLAUDE.md"))
            .unwrap();
        let payload = snaps.path().join(&id).join(entry.payload.as_ref().unwrap());
        assert_eq!(fs::read_to_string(payload).unwrap(), original);
    }

    // ── verify ───────────────────────────────────────────────────────────────

    #[test]
    fn verify_true_when_clean_and_findings_gone() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        let snaps = tempdir().unwrap();
        write(&root.path().join("CLAUDE.md"), "# body\n");
        let plan = plan_for(root.path(), home.path(), &[]);
        let (results, _) = apply_in(&plan, root.path(), &snaps);
        let rediag = findings_for(root.path(), home.path());
        assert!(verify(&results, &rediag));
    }

    #[test]
    fn verify_false_on_any_conflict() {
        let conflict = NormalizeItem {
            fingerprint: "fp".into(),
            rule: "instructions.dual_body".into(),
            project: "/p".into(),
            path: "/p/CLAUDE.md".into(),
            action: "conflict".into(),
            before: WriteEvidence::Absent,
            after_content: None,
            snapshot: false,
            depends_on: None,
            message: Some("drift".into()),
        };
        assert!(!verify(&[conflict], &[]));
    }
}
