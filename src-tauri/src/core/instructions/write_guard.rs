//! Write-safety base for instructions normalize/init (design §8).
//!
//! Chain only ever creates/removes *symlinks*; instructions rewrites *file
//! content*, so this module makes §8's hard constraints mechanical rather than
//! conventional. Every mutation goes through one of three guarded ops, and each
//! op refuses (reports a [`WriteOutcome::Conflict`], writes nothing) rather than
//! risk data loss:
//!
//! * **TOCTOU content guard** — [`WriteEvidence`] carries a file's content hash,
//!   captured at plan time; [`verify_unchanged`] re-checks it immediately before
//!   a write. One `PartialEq` covers every drift: content edited, file replaced
//!   by a symlink/dir, or removed. (Chain's `EntryEvidence` only compares link
//!   targets; instructions must compare content, so the evidence lives here.)
//! * **Write-target whitelist** — [`classify_write_target`] narrows below
//!   `path_guard`'s project boundary to exactly `AGENTS.md`, `CLAUDE.md`, and
//!   `docs/agents/*.md`. Anything else — including every global-surface path,
//!   which sits outside the project root — is rejected in code, enforcing the
//!   global read-only rule.
//! * **The canonical is never rewritten** — `AGENTS.md` classifies as
//!   [`WritePolicy::CreateOnly`], and [`create_only`] refuses an existing target,
//!   so the canonical body can be created but never overwritten.
//! * **No content file is ever deleted** — the only removal, [`remove_symlink`],
//!   refuses anything that is not (and was not) a symlink. No path here deletes a
//!   regular file.
//!
//! Snapshot-first (§8) is a sequencing rule the caller upholds: capture originals
//! via [`super::snapshot`] before invoking [`rewrite`]/[`remove_symlink`].

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::snapshot::sha256_hex;
use crate::core::audit_log::AuditDraft;
use crate::core::path_guard;

/// Audit action for a normalize apply (design §7).
pub const ACTION_NORMALIZE: &str = "instructions_normalize";
/// Audit action for an init apply (design §7).
pub const ACTION_INIT: &str = "instructions_init";

/// On-disk state of a write target, captured at plan time and re-checked before
/// apply. Modeled on chain's `EntryEvidence`, but the `File` variant carries the
/// content hash so a single comparison is the full time-of-check/time-of-use
/// guard for content — not just existence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WriteEvidence {
    /// Nothing exists at the path.
    Absent,
    /// A symlink; `target` is its raw (lexical) link target.
    Symlink { target: String },
    /// A physical directory (a conflict for any write target).
    Dir,
    /// A regular file; `sha256` fingerprints its exact bytes. An empty hash means
    /// the bytes could not be read — treated as "changed" by [`verify_unchanged`]
    /// so an unreadable target never passes the guard.
    File { sha256: String },
}

/// Observe a target without following its final component, so a symlink reads as
/// a symlink rather than its destination. Infallible: an unreadable regular file
/// yields `File { sha256: "" }`, which [`verify_unchanged`] rejects (fail-closed).
pub fn observe_target(path: &Path) -> WriteEvidence {
    match std::fs::symlink_metadata(path) {
        Err(_) => WriteEvidence::Absent,
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            WriteEvidence::Symlink { target }
        }
        Ok(meta) if meta.is_dir() => WriteEvidence::Dir,
        Ok(_) => {
            let sha256 = std::fs::read(path)
                .map(|b| sha256_hex(&b))
                .unwrap_or_default();
            WriteEvidence::File { sha256 }
        }
    }
}

/// True only when `path` still matches `baseline` exactly. An unreadable file
/// (empty hash) never matches, so the guard fails closed.
pub fn verify_unchanged(baseline: &WriteEvidence, path: &Path) -> bool {
    let current = observe_target(path);
    if let WriteEvidence::File { sha256 } = &current {
        if sha256.is_empty() {
            return false;
        }
    }
    current == *baseline
}

/// What a permitted write target may undergo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// May be created when absent, but an existing file is never overwritten —
    /// the canonical `AGENTS.md` and init-created `docs/agents/*.md`.
    CreateOnly,
    /// May be created or rewritten — the per-agent entry `CLAUDE.md`.
    CreateOrRewrite,
}

/// Classify `target` against the write-target whitelist (design §8), narrower
/// than `path_guard`'s project boundary. Returns the target's [`WritePolicy`],
/// or `None` when it is not a permitted instructions write path — which includes
/// every path outside `project_root` (all global surfaces) and any nested or
/// misnamed candidate (`.claude/CLAUDE.md`, a subdir `AGENTS.md`, …).
pub fn classify_write_target(project_root: &Path, target: &Path) -> Option<WritePolicy> {
    // Canonical containment first: rejects escapes, symlinked parents, and every
    // out-of-project (global) path.
    if !path_guard::is_path_safe(project_root, target) {
        return None;
    }
    // Structural match on the *given* relative path. Only plain path components
    // are allowed, so `..`/`.`/absolute segments can never smuggle a match.
    let rel = target.strip_prefix(project_root).ok()?;
    let mut names: Vec<String> = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(s) => names.push(s.to_string_lossy().to_string()),
            _ => return None,
        }
    }
    match names.as_slice() {
        [only] if only == "AGENTS.md" => Some(WritePolicy::CreateOnly),
        [only] if only == "CLAUDE.md" => Some(WritePolicy::CreateOrRewrite),
        [dir, sub, name] if dir == "docs" && sub == "agents" && is_docs_agent_file(name) => {
            Some(WritePolicy::CreateOnly)
        }
        _ => None,
    }
}

/// A `docs/agents/*.md` leaf: a plain `.md` filename with a non-empty stem and no
/// path trickery (`sanitize_name` is a no-op on a safe name).
fn is_docs_agent_file(name: &str) -> bool {
    name.len() > 3 && name.ends_with(".md") && path_guard::sanitize_name(name) == name
}

/// The outcome of a guarded write. `Conflict` means the op was refused and
/// nothing was written (design §8: "conflict 只报告不覆盖").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Created,
    Rewritten,
    RemovedLink,
    Conflict(String),
}

impl WriteOutcome {
    pub fn is_conflict(&self) -> bool {
        matches!(self, WriteOutcome::Conflict(_))
    }
}

fn conflict(msg: impl Into<String>) -> WriteOutcome {
    WriteOutcome::Conflict(msg.into())
}

/// Create a whitelisted target from `content`, refusing if anything already
/// exists there. This is the only path that writes `AGENTS.md`, so the canonical
/// body can be created but — being create-only — never overwritten (§8). New
/// files are written with `\n` line endings; a needed parent (`docs/agents/`) is
/// created inside the project.
pub fn create_only(
    project_root: &Path,
    target: &Path,
    content: &str,
) -> std::io::Result<WriteOutcome> {
    if classify_write_target(project_root, target).is_none() {
        return Ok(conflict(
            "target is not a permitted instructions write path",
        ));
    }
    if !matches!(observe_target(target), WriteEvidence::Absent) {
        return Ok(conflict(
            "target already exists; create-only never overwrites",
        ));
    }
    if let Some(parent) = target.parent() {
        if path_guard::is_path_safe(project_root, parent) {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(target, content.as_bytes())?;
    Ok(WriteOutcome::Created)
}

/// Rewrite an existing entry file to `new_content`, guarded against drift since
/// `baseline`. Refuses any target that is not [`WritePolicy::CreateOrRewrite`] —
/// so `AGENTS.md` (create-only) can never be rewritten here — and any baseline
/// that is not a file. The original's EOL style is preserved (CRLF in ⇒ CRLF
/// out); `new_content` is expected in `\n` form.
pub fn rewrite(
    project_root: &Path,
    target: &Path,
    new_content: &str,
    baseline: &WriteEvidence,
) -> std::io::Result<WriteOutcome> {
    if classify_write_target(project_root, target) != Some(WritePolicy::CreateOrRewrite) {
        return Ok(conflict("target is not a rewritable instructions entry"));
    }
    if !matches!(baseline, WriteEvidence::File { .. }) {
        return Ok(conflict("rewrite requires an existing-file baseline"));
    }
    if !verify_unchanged(baseline, target) {
        return Ok(conflict("target changed since preview (content guard)"));
    }
    // verify_unchanged confirmed a regular file matching `baseline`, so this read
    // neither follows a symlink nor races a swap.
    let current = std::fs::read(target)?;
    let had_crlf = current.windows(2).any(|w| w == b"\r\n");
    let lf = new_content.replace("\r\n", "\n");
    let out = if had_crlf {
        lf.replace('\n', "\r\n")
    } else {
        lf
    };
    std::fs::write(target, out.as_bytes())?;
    Ok(WriteOutcome::Rewritten)
}

/// Remove a symlink entry (the symlink→wrapper conversion of §4.1). Refuses
/// anything that is not, and was not, a symlink — so no regular file is ever
/// deleted (§8). Removes the link itself; its target file is never followed or
/// touched.
pub fn remove_symlink(
    project_root: &Path,
    target: &Path,
    baseline: &WriteEvidence,
) -> std::io::Result<WriteOutcome> {
    if classify_write_target(project_root, target) != Some(WritePolicy::CreateOrRewrite) {
        return Ok(conflict("target is not a removable instructions entry"));
    }
    if !matches!(baseline, WriteEvidence::Symlink { .. }) {
        return Ok(conflict(
            "remove requires a symlink baseline; no content file is deleted",
        ));
    }
    if !verify_unchanged(baseline, target) {
        return Ok(conflict("target changed since preview (content guard)"));
    }
    std::fs::remove_file(target)?;
    Ok(WriteOutcome::RemovedLink)
}

/// Free-form audit detail for a write apply: the snapshot id and the files it
/// covers (design §7).
pub fn audit_detail(snapshot_id: &str, files: &[String]) -> String {
    format!("snapshot {snapshot_id}; files: {}", files.join(", "))
}

/// Build the audit draft for a normalize/init apply. `action` is
/// [`ACTION_NORMALIZE`] or [`ACTION_INIT`]; `tool` is the agent key when the
/// apply targets one. The caller marks success with `.ok()` once writes land.
pub fn audit_draft(
    action: &str,
    tool: Option<&str>,
    snapshot_id: &str,
    files: &[String],
) -> AuditDraft {
    let mut draft = AuditDraft::new(action).detail(audit_detail(snapshot_id, files));
    if let Some(t) = tool {
        draft = draft.tool(t);
    }
    draft
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        (tmp, root)
    }

    // ── whitelist ────────────────────────────────────────────────────────

    #[test]
    fn whitelist_admits_the_three_shapes_with_right_policy() {
        let (_g, root) = project();
        assert_eq!(
            classify_write_target(&root, &root.join("AGENTS.md")),
            Some(WritePolicy::CreateOnly)
        );
        assert_eq!(
            classify_write_target(&root, &root.join("CLAUDE.md")),
            Some(WritePolicy::CreateOrRewrite)
        );
        assert_eq!(
            classify_write_target(&root, &root.join("docs/agents/testing.md")),
            Some(WritePolicy::CreateOnly)
        );
    }

    #[test]
    fn whitelist_rejects_nested_misnamed_and_traversal() {
        let (_g, root) = project();
        // Nested CLAUDE.md (design: only project-root; .claude/CLAUDE.md is read-only).
        assert!(classify_write_target(&root, &root.join(".claude/CLAUDE.md")).is_none());
        // AGENTS.md in a subdir is not the canonical.
        assert!(classify_write_target(&root, &root.join("sub/AGENTS.md")).is_none());
        // Wrong extension / deeper nesting under docs/agents.
        assert!(classify_write_target(&root, &root.join("docs/agents/notes.txt")).is_none());
        assert!(classify_write_target(&root, &root.join("docs/agents/x/y.md")).is_none());
        // An unrelated file.
        assert!(classify_write_target(&root, &root.join("README.md")).is_none());
        // Traversal out of the project.
        assert!(classify_write_target(&root, &root.join("../AGENTS.md")).is_none());
    }

    #[test]
    fn whitelist_rejects_paths_outside_the_project() {
        let (_g, root) = project();
        // A global-surface style path outside the project root — read-only in code.
        let outside = root.parent().unwrap().join("global").join("CLAUDE.md");
        assert!(classify_write_target(&root, &outside).is_none());
    }

    // ── TOCTOU guard ─────────────────────────────────────────────────────

    #[test]
    fn observe_and_verify_track_content_drift() {
        let (_g, root) = project();
        let f = root.join("CLAUDE.md");
        fs::write(&f, "one").unwrap();
        let baseline = observe_target(&f);
        assert!(matches!(baseline, WriteEvidence::File { .. }));
        assert!(verify_unchanged(&baseline, &f));

        fs::write(&f, "two").unwrap();
        assert!(!verify_unchanged(&baseline, &f));
    }

    #[test]
    fn verify_catches_file_replaced_by_absence() {
        let (_g, root) = project();
        let f = root.join("CLAUDE.md");
        fs::write(&f, "x").unwrap();
        let baseline = observe_target(&f);
        fs::remove_file(&f).unwrap();
        assert!(!verify_unchanged(&baseline, &f));
    }

    // ── guarded writes: hard constraints ─────────────────────────────────

    #[test]
    fn create_only_writes_then_refuses_existing() {
        let (_g, root) = project();
        let agents = root.join("AGENTS.md");
        assert_eq!(
            create_only(&root, &agents, "# body\n").unwrap(),
            WriteOutcome::Created
        );
        assert_eq!(fs::read_to_string(&agents).unwrap(), "# body\n");
        // The canonical is never overwritten.
        assert!(create_only(&root, &agents, "# clobber\n")
            .unwrap()
            .is_conflict());
        assert_eq!(fs::read_to_string(&agents).unwrap(), "# body\n");
    }

    #[test]
    fn create_only_makes_docs_agents_parent() {
        let (_g, root) = project();
        let doc = root.join("docs/agents/testing.md");
        assert_eq!(
            create_only(&root, &doc, "See ...\n").unwrap(),
            WriteOutcome::Created
        );
        assert!(doc.exists());
    }

    #[test]
    fn create_only_rejects_non_whitelisted() {
        let (_g, root) = project();
        assert!(create_only(&root, &root.join("README.md"), "x")
            .unwrap()
            .is_conflict());
    }

    #[test]
    fn rewrite_refuses_the_canonical() {
        let (_g, root) = project();
        let agents = root.join("AGENTS.md");
        fs::write(&agents, "canonical\n").unwrap();
        let baseline = observe_target(&agents);
        // AGENTS.md is create-only: rewrite must refuse it outright.
        assert!(rewrite(&root, &agents, "hijacked\n", &baseline)
            .unwrap()
            .is_conflict());
        assert_eq!(fs::read_to_string(&agents).unwrap(), "canonical\n");
    }

    #[test]
    fn rewrite_applies_when_unchanged_and_refuses_on_drift() {
        let (_g, root) = project();
        let entry = root.join("CLAUDE.md");
        fs::write(&entry, "old\n").unwrap();
        let baseline = observe_target(&entry);
        assert_eq!(
            rewrite(&root, &entry, "@AGENTS.md\n", &baseline).unwrap(),
            WriteOutcome::Rewritten
        );
        assert_eq!(fs::read_to_string(&entry).unwrap(), "@AGENTS.md\n");

        // A stale baseline (content already moved on) is refused.
        assert!(rewrite(&root, &entry, "again\n", &baseline)
            .unwrap()
            .is_conflict());
    }

    #[test]
    fn rewrite_preserves_crlf_eol() {
        let (_g, root) = project();
        let entry = root.join("CLAUDE.md");
        fs::write(&entry, "old line one\r\nold line two\r\n").unwrap();
        let baseline = observe_target(&entry);
        rewrite(&root, &entry, "new one\nnew two\n", &baseline).unwrap();
        assert_eq!(
            fs::read_to_string(&entry).unwrap(),
            "new one\r\nnew two\r\n"
        );
    }

    #[test]
    fn remove_symlink_takes_link_only_and_never_a_file() {
        let (_g, root) = project();
        let canonical = root.join("AGENTS.md");
        fs::write(&canonical, "body").unwrap();
        let entry = root.join("CLAUDE.md");
        crate::core::test_support::expect_symlink_file(&canonical, &entry);

        let baseline = observe_target(&entry);
        assert!(matches!(baseline, WriteEvidence::Symlink { .. }));
        assert_eq!(
            remove_symlink(&root, &entry, &baseline).unwrap(),
            WriteOutcome::RemovedLink
        );
        assert!(!entry.exists());
        // The link's target file was never touched.
        assert_eq!(fs::read_to_string(&canonical).unwrap(), "body");
    }

    #[test]
    fn remove_symlink_refuses_a_regular_file() {
        let (_g, root) = project();
        let entry = root.join("CLAUDE.md");
        fs::write(&entry, "real content").unwrap();
        let baseline = observe_target(&entry); // File, not Symlink
        assert!(remove_symlink(&root, &entry, &baseline)
            .unwrap()
            .is_conflict());
        // No content file is ever deleted.
        assert!(entry.exists());
    }

    // ── audit wiring ─────────────────────────────────────────────────────

    #[test]
    fn audit_draft_carries_action_tool_and_snapshot_detail() {
        let draft = audit_draft(
            ACTION_NORMALIZE,
            Some("claude_code"),
            "1700000000000",
            &["/p/AGENTS.md".to_string(), "/p/CLAUDE.md".to_string()],
        )
        .ok();
        assert_eq!(draft.action, ACTION_NORMALIZE);
        assert_eq!(draft.tool.as_deref(), Some("claude_code"));
        assert!(draft.success);
        let detail = draft.detail.unwrap();
        assert!(detail.contains("snapshot 1700000000000"));
        assert!(detail.contains("/p/AGENTS.md"));
        assert!(detail.contains("/p/CLAUDE.md"));
    }
}
