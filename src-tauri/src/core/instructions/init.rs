//! Plan/apply `init` — scaffold a project's instructions surface (design §4.2).
//!
//! Where [`super::normalize`] fixes an existing surface, `init` bootstraps a bare
//! project: it creates the canonical `AGENTS.md` skeleton (only when absent —
//! **never overwritten**), the per-agent wrapper entries (v1: Claude's
//! `CLAUDE.md`), and, with `--docs-dir`, an empty `docs/agents/` directory plus a
//! pointer line in the skeleton's Conventions section. Every file write goes
//! through [`super::write_guard::create_only`] (whitelist + create-only), so init
//! can only ever *add* files; it has no rewrite or delete path, and therefore
//! needs no snapshot. It is fully idempotent: anything already present is a `noop`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::normalize::WRAPPER;
use super::surfaces::Agent;
use super::write_guard::{self, WriteEvidence, WriteOutcome};
use crate::core::path_guard;

/// One planned (or realized) scaffold action. `create` writes; `noop` means the
/// target already exists (never overwritten); `conflict` is a refusal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitItem {
    /// Absolute path of the file or directory this item creates.
    pub path: String,
    /// `canonical` | `entry` | `docs_dir` — what role the target plays.
    pub kind: String,
    /// `create` | `noop` | `conflict`.
    pub action: String,
    /// On-disk state at plan time.
    pub before: WriteEvidence,
    /// The full content a `create` file item will write; `None` for a directory
    /// item, a `noop`, or a `conflict`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_content: Option<String>,
    /// Conflict reason (or note); `None` on a clean create/noop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A previewed init operation. Produced by [`plan`], consumed by [`apply`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitPlan {
    pub items: Vec<InitItem>,
    pub scanned_at: i64,
}

/// The result of applying an init plan: per-item outcomes and — set by the
/// service — whether every intended target now exists.
#[derive(Debug, Clone, Serialize)]
pub struct InitOutcome {
    pub items: Vec<InitItem>,
    pub verified: bool,
    pub scanned_at: i64,
}

// ── template (design §4.2 — byte-exact) ─────────────────────────────────────

/// The default Conventions body: a placeholder prompt the user fills in.
const CONVENTIONS_PLACEHOLDER: &str =
    "<key conventions; keep this file short and link out for detail>";
/// The `--docs-dir` Conventions body: a pointer into the docs directory.
const CONVENTIONS_POINTER: &str = "See docs/agents/<topic>.md";
/// The docs directory `--docs-dir` creates (empty; the user adds topic files).
const DOCS_AGENTS_DIR: &str = "docs/agents";

/// The `AGENTS.md` skeleton (design §4.2): the project directory name as the H1,
/// then Overview / Commands / Conventions with placeholder bodies the user fills.
/// With `docs_dir`, the Conventions body becomes the docs pointer. English by
/// design (instructions are read by agents; conventions are cross-project).
pub fn skeleton(project_name: &str, docs_dir: bool) -> String {
    let conventions = if docs_dir {
        CONVENTIONS_POINTER
    } else {
        CONVENTIONS_PLACEHOLDER
    };
    format!(
        "# {project_name}\n\n\
         ## Overview\n\n\
         <one line: what this project is>\n\n\
         ## Commands\n\n\
         <build / test / lint commands agents should run>\n\n\
         ## Conventions\n\n\
         {conventions}\n"
    )
}

// ── plan (read-only) ────────────────────────────────────────────────────────

/// Build the init plan for `project_root`. Scaffolds the canonical skeleton, a
/// Claude wrapper entry (when Claude is installed), and — with `docs_dir` — the
/// empty docs directory. Read-only apart from [`write_guard::observe_target`].
pub fn plan(project_root: &Path, installed: &[Agent], docs_dir: bool, scanned_at: i64) -> InitPlan {
    let mut items = Vec::new();
    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.to_string_lossy().to_string());

    // ① Canonical skeleton (create-only; an existing canonical is never touched).
    let canonical = project_root.join("AGENTS.md");
    items.push(create_file_item(
        &canonical,
        "canonical",
        skeleton(&project_name, docs_dir),
    ));

    // ② Per-agent entries: v1 is Claude's project-root wrapper.
    if installed.contains(&Agent::Claude) {
        let entry = project_root.join("CLAUDE.md");
        items.push(create_file_item(&entry, "entry", WRAPPER.to_string()));
    }

    // ③ `--docs-dir`: an empty docs/agents/ directory.
    if docs_dir {
        let dir = project_root.join(DOCS_AGENTS_DIR);
        items.push(dir_item(&dir));
    }

    InitPlan { items, scanned_at }
}

/// A create-or-noop plan item for a whitelisted file target.
fn create_file_item(path: &Path, kind: &str, content: String) -> InitItem {
    let before = write_guard::observe_target(path);
    match &before {
        WriteEvidence::Absent => InitItem {
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            action: "create".to_string(),
            before,
            after_content: Some(content),
            message: None,
        },
        // A directory occupying a file path cannot be scaffolded over.
        WriteEvidence::Dir => item_conflict(path, kind, before, "path is occupied by a directory"),
        // Any existing file/symlink is preserved untouched (idempotent, §4.2).
        _ => InitItem {
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            action: "noop".to_string(),
            before,
            after_content: None,
            message: None,
        },
    }
}

/// A create-or-noop plan item for the docs directory.
fn dir_item(path: &Path) -> InitItem {
    let before = write_guard::observe_target(path);
    let action = match &before {
        WriteEvidence::Absent => "create",
        WriteEvidence::Dir => "noop",
        // A file (or symlink) sitting where the directory should be is a conflict.
        _ => {
            return item_conflict(path, "docs_dir", before, "path is occupied by a file");
        }
    };
    InitItem {
        path: path.to_string_lossy().to_string(),
        kind: "docs_dir".to_string(),
        action: action.to_string(),
        before,
        after_content: None,
        message: None,
    }
}

fn item_conflict(path: &Path, kind: &str, before: WriteEvidence, message: &str) -> InitItem {
    InitItem {
        path: path.to_string_lossy().to_string(),
        kind: kind.to_string(),
        action: "conflict".to_string(),
        before,
        after_content: None,
        message: Some(message.to_string()),
    }
}

// ── apply (writes, guarded) ─────────────────────────────────────────────────

/// Apply an init plan under the guard stack. File creates go through
/// [`write_guard::create_only`] (whitelist + refuse-if-exists); the docs directory
/// is created in-project (path-guarded). `noop`/`conflict` items are reflected
/// verbatim. Idempotent: a re-apply of a scaffolded project writes nothing.
pub fn apply(plan: &InitPlan, project_root: &Path) -> Vec<InitItem> {
    plan.items
        .iter()
        .map(|it| {
            if it.action != "create" {
                return it.clone(); // noop / conflict verbatim
            }
            match it.kind.as_str() {
                "docs_dir" => apply_dir(it, project_root),
                _ => apply_file(it, project_root),
            }
        })
        .collect()
}

fn apply_file(it: &InitItem, project_root: &Path) -> InitItem {
    let path = PathBuf::from(&it.path);
    let content = it.after_content.as_deref().unwrap_or("");
    match write_guard::create_only(project_root, &path, content) {
        Ok(WriteOutcome::Created) => it.clone(),
        Ok(WriteOutcome::Conflict(msg)) => refuse(it, &msg),
        Ok(_) => refuse(it, "unexpected write outcome"),
        Err(e) => refuse(it, &e.to_string()),
    }
}

/// Create the docs directory, guarded to stay inside the project (a symlinked
/// parent cannot redirect it out). Init only ever scaffolds the one fixed
/// `docs/agents` directory, so a deserialized/forged plan pointing `docs_dir`
/// elsewhere is refused (defense-in-depth: `InitPlan` crosses the wire for #19).
/// Creating an already-existing directory is harmless and idempotent.
fn apply_dir(it: &InitItem, project_root: &Path) -> InitItem {
    let path = PathBuf::from(&it.path);
    if path != project_root.join(DOCS_AGENTS_DIR) {
        return refuse(it, "not the init docs directory");
    }
    if !path_guard::is_path_safe(project_root, &path) {
        return refuse(it, "directory escapes the project boundary");
    }
    match std::fs::create_dir_all(&path) {
        Ok(()) => it.clone(),
        Err(e) => refuse(it, &e.to_string()),
    }
}

fn refuse(it: &InitItem, message: &str) -> InitItem {
    InitItem {
        action: "conflict".to_string(),
        message: Some(message.to_string()),
        ..it.clone()
    }
}

/// Verify an init apply: clean only when no item was refused AND every intended
/// target now exists on disk (each `create` landed; each `noop` was already
/// there). Existence is checked WITHOUT following the final component — matching
/// how `observe_target` classified a `noop` — so a dangling symlink entry (a
/// legitimate never-overwrite noop) still verifies rather than reporting a
/// spurious failure.
pub fn verify(results: &[InitItem]) -> bool {
    results.iter().all(|it| match it.action.as_str() {
        "conflict" => false,
        _ => std::fs::symlink_metadata(&it.path).is_ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn plan_apply(root: &Path, installed: &[Agent], docs_dir: bool) -> Vec<InitItem> {
        let plan = plan(root, installed, docs_dir, 0);
        apply(&plan, root)
    }

    fn item<'a>(items: &'a [InitItem], kind: &str) -> &'a InitItem {
        items
            .iter()
            .find(|i| i.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} item"))
    }

    // ── skeleton template (byte-exact, design §4.2) ─────────────────────────

    #[test]
    fn skeleton_matches_the_design_template_verbatim() {
        let expected = "\
# my-project

## Overview

<one line: what this project is>

## Commands

<build / test / lint commands agents should run>

## Conventions

<key conventions; keep this file short and link out for detail>
";
        assert_eq!(skeleton("my-project", false), expected);
    }

    #[test]
    fn skeleton_docs_dir_variant_uses_the_pointer() {
        let out = skeleton("my-project", true);
        assert!(out.ends_with("## Conventions\n\nSee docs/agents/<topic>.md\n"));
        assert!(!out.contains("keep this file short"));
    }

    // ── scaffolding a bare project ──────────────────────────────────────────

    #[test]
    fn init_creates_canonical_and_claude_wrapper() {
        let root = tempdir().unwrap();
        let results = plan_apply(root.path(), &[Agent::Claude], false);
        assert_eq!(item(&results, "canonical").action, "create");
        assert_eq!(item(&results, "entry").action, "create");

        // Canonical is the skeleton with the project dir name as the H1.
        let dir_name = root
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            skeleton(&dir_name, false)
        );
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn no_claude_no_wrapper_entry() {
        let root = tempdir().unwrap();
        let results = plan_apply(root.path(), &[Agent::Codex], false);
        assert!(results.iter().all(|i| i.kind != "entry"));
        // The canonical is still scaffolded (native agents read it directly).
        assert!(root.path().join("AGENTS.md").exists());
        assert!(!root.path().join("CLAUDE.md").exists());
    }

    // ── --docs-dir variant ──────────────────────────────────────────────────

    #[test]
    fn docs_dir_creates_empty_dir_and_pointer_conventions() {
        let root = tempdir().unwrap();
        let results = plan_apply(root.path(), &[Agent::Claude], true);
        assert_eq!(item(&results, "docs_dir").action, "create");

        // The directory exists and is empty.
        let dir = root.path().join("docs/agents");
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        // The canonical's Conventions section points into it.
        let canonical = fs::read_to_string(root.path().join("AGENTS.md")).unwrap();
        assert!(canonical.contains("## Conventions\n\nSee docs/agents/<topic>.md\n"));
    }

    // ── never overwrite (AC) ────────────────────────────────────────────────

    #[test]
    fn existing_canonical_is_left_byte_identical() {
        let root = tempdir().unwrap();
        let original = "# Hand-written\n\nmy real instructions\n";
        fs::write(root.path().join("AGENTS.md"), original).unwrap();

        let plan = plan(root.path(), &[Agent::Claude], false, 0);
        assert_eq!(item(&plan.items, "canonical").action, "noop");
        apply(&plan, root.path());
        // The existing canonical is never overwritten.
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
            original
        );
        // The missing wrapper is still scaffolded.
        assert!(root.path().join("CLAUDE.md").exists());
    }

    // ── idempotency (AC) ────────────────────────────────────────────────────

    #[test]
    fn second_init_pass_is_all_noop() {
        let root = tempdir().unwrap();
        let first = plan_apply(root.path(), &[Agent::Claude], true);
        assert!(first.iter().any(|i| i.action == "create"));

        let plan2 = plan(root.path(), &[Agent::Claude], true, 0);
        assert!(
            plan2.items.iter().all(|i| i.action == "noop"),
            "second pass must be all-noop: {:?}",
            plan2
                .items
                .iter()
                .map(|i| (&i.kind, &i.action))
                .collect::<Vec<_>>()
        );
        let results = apply(&plan2, root.path());
        assert!(verify(&results));
    }

    // ── conflicts ────────────────────────────────────────────────────────────

    #[test]
    fn directory_occupying_canonical_path_conflicts() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("AGENTS.md")).unwrap();
        let plan = plan(root.path(), &[Agent::Claude], false, 0);
        assert_eq!(item(&plan.items, "canonical").action, "conflict");
        let results = apply(&plan, root.path());
        assert!(!verify(&results));
    }

    #[test]
    fn verify_true_after_clean_scaffold() {
        let root = tempdir().unwrap();
        let results = plan_apply(root.path(), &[Agent::Claude], true);
        assert!(verify(&results));
    }

    #[test]
    fn dangling_symlink_entry_is_a_noop_that_still_verifies() {
        // A CLAUDE.md that is a dangling symlink is a legitimate never-overwrite
        // noop; verify must not follow the link and report a spurious failure.
        let root = tempdir().unwrap();
        crate::core::test_support::expect_symlink_file(
            std::path::Path::new("does-not-exist.md"),
            &root.path().join("CLAUDE.md"),
        );

        let plan = plan(root.path(), &[Agent::Claude], false, 0);
        assert_eq!(item(&plan.items, "entry").action, "noop");
        let results = apply(&plan, root.path());
        // The dangling link is untouched, and the outcome verifies clean.
        assert!(
            verify(&results),
            "a dangling-symlink noop must still verify"
        );
        assert!(std::fs::symlink_metadata(root.path().join("CLAUDE.md")).is_ok());
    }

    #[test]
    fn apply_dir_refuses_a_forged_docs_path() {
        // A plan whose docs_dir item points outside the fixed docs/agents path
        // (only possible via a forged/deserialized plan) is refused.
        let root = tempdir().unwrap();
        let forged = InitPlan {
            items: vec![InitItem {
                path: root.path().join("evil-dir").to_string_lossy().to_string(),
                kind: "docs_dir".to_string(),
                action: "create".to_string(),
                before: WriteEvidence::Absent,
                after_content: None,
                message: None,
            }],
            scanned_at: 0,
        };
        let results = apply(&forged, root.path());
        assert_eq!(results[0].action, "conflict");
        assert!(!root.path().join("evil-dir").exists());
    }
}
