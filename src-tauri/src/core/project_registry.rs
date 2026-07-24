//! Single entry point for persisting a directory as a registered project.
//!
//! Both the Project Workspace `add_project` command and the chain module's
//! enrolment path go through here, so project identity — a canonical absolute
//! path — is defined in exactly one place and there is only ever one registry.
//! Aliases of the same directory (`.`/`..` segments, symlinks, trailing
//! slashes) resolve to a single record; same-named directories at different
//! locations stay distinct.

use std::path::{Path, PathBuf};

use crate::core::error::AppError;
use crate::core::skill_store::{ProjectRecord, SkillStore};

/// Canonical identity key for a project path: resolve symlinks and `.`/`..`
/// against the filesystem, falling back to the path as given when it can't be
/// canonicalized (e.g. it no longer exists). This is only ever an identity /
/// dedup key — never a stored or scanned path, because the chain's symlink
/// tracer resolves targets lexically (never canonicalized) and rewriting a
/// project's path to its canonical form would misclassify those targets.
pub fn canonical_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Persist `path` as a registered project, reusing an existing record when the
/// same directory (by canonical path) is already registered. Returns the
/// registered record, whether freshly inserted or already present, so callers
/// can treat registration as idempotent.
///
/// `scaffold_workspace_dirs` creates the Project Workspace `.claude/skills` and
/// `.claude/skills-disabled` directories used by the skill-toggle UI. The chain
/// enrolment path passes `false`: the three-tier link operations create agent
/// entry surfaces themselves, and a pre-existing physical `.claude/skills`
/// would block the standard `.claude/skills -> ../.agents/skills` dir link. The
/// scaffold runs before the dedup check so a Project Workspace add still creates
/// those directories even for a folder the chain module enrolled first.
pub fn register_project(
    store: &SkillStore,
    path: &Path,
    scaffold_workspace_dirs: bool,
) -> Result<ProjectRecord, AppError> {
    if !path.is_dir() {
        return Err(AppError::invalid_input("Directory does not exist"));
    }

    if scaffold_workspace_dirs {
        // Idempotent, and intentionally ahead of the dedup return so an existing
        // chain-enrolled record still gains the workspace directories.
        let claude_dir = path.join(".claude");
        std::fs::create_dir_all(claude_dir.join("skills"))?;
        std::fs::create_dir_all(claude_dir.join("skills-disabled"))?;
    }

    // Project identity is the canonical absolute path: dedupe aliases of the
    // same directory while keeping same-named directories at different paths
    // distinct. The stored `path` stays as given (see `canonical_key`).
    let key = canonical_key(path);
    if let Some(existing) = store
        .get_all_projects()
        .map_err(AppError::db)?
        .into_iter()
        .find(|record| {
            record.workspace_type == "project" && canonical_key(Path::new(&record.path)) == key
        })
    {
        return Ok(existing);
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let now = chrono::Utc::now().timestamp_millis();
    let record = ProjectRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        path: path.to_string_lossy().to_string(),
        workspace_type: "project".to_string(),
        linked_agent_key: None,
        linked_agent_name: None,
        disabled_path: None,
        sort_order: 0,
        created_at: now,
        updated_at: now,
    };
    store.insert_project(&record).map_err(AppError::db)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store_in(dir: &Path) -> SkillStore {
        SkillStore::new(&dir.join("patchbay.db")).unwrap()
    }

    #[test]
    fn registers_new_project_without_scaffolding_workspace_dirs() {
        let temp = tempdir().unwrap();
        // A project outside any default discovery root (e.g. under Dropbox).
        let proj = temp.path().join("Dropbox").join("alpha");
        std::fs::create_dir_all(&proj).unwrap();
        let store = store_in(temp.path());

        let record = register_project(&store, &proj, false).unwrap();
        assert_eq!(record.name, "alpha");
        assert_eq!(record.workspace_type, "project");
        assert_eq!(record.path, proj.to_string_lossy());
        assert_eq!(store.get_all_projects().unwrap().len(), 1);
        // Chain enrolment must not pre-create the workspace skills dir, or the
        // `.claude/skills -> ../.agents/skills` dir link can't be established.
        assert!(!proj.join(".claude").exists());
    }

    #[test]
    fn scaffolds_workspace_dirs_when_requested() {
        let temp = tempdir().unwrap();
        let proj = temp.path().join("alpha");
        std::fs::create_dir_all(&proj).unwrap();
        let store = store_in(temp.path());

        register_project(&store, &proj, true).unwrap();
        assert!(proj.join(".claude/skills").is_dir());
        assert!(proj.join(".claude/skills-disabled").is_dir());
    }

    #[test]
    fn deduplicates_path_aliases_by_canonical_identity() {
        let temp = tempdir().unwrap();
        let proj = temp.path().join("alpha");
        std::fs::create_dir_all(proj.join("nested")).unwrap();
        let store = store_in(temp.path());

        let first = register_project(&store, &proj, false).unwrap();
        // The same directory reached through a `.`/`..` alias is one project.
        let alias = proj.join("nested").join("..");
        let second = register_project(&store, &alias, false).unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(store.get_all_projects().unwrap().len(), 1);
    }

    #[test]
    fn keeps_same_named_projects_at_different_paths_distinct() {
        let temp = tempdir().unwrap();
        let a = temp.path().join("a").join("web");
        let b = temp.path().join("b").join("web");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let store = store_in(temp.path());

        let ra = register_project(&store, &a, false).unwrap();
        let rb = register_project(&store, &b, false).unwrap();

        assert_ne!(ra.id, rb.id);
        assert_eq!(ra.name, "web");
        assert_eq!(rb.name, "web");
        assert_eq!(store.get_all_projects().unwrap().len(), 2);
    }

    #[test]
    fn re_registering_a_chain_enrolled_project_scaffolds_workspace_dirs() {
        let temp = tempdir().unwrap();
        let proj = temp.path().join("alpha");
        std::fs::create_dir_all(&proj).unwrap();
        let store = store_in(temp.path());

        // First enrolled by the chain module: no workspace scaffold.
        let enrolled = register_project(&store, &proj, false).unwrap();
        assert!(!proj.join(".claude").exists());

        // Later added through the Project Workspace: same record, but now the
        // skill-toggle directories exist (AC5: existing workspace behaviour).
        let added = register_project(&store, &proj, true).unwrap();
        assert_eq!(enrolled.id, added.id);
        assert_eq!(store.get_all_projects().unwrap().len(), 1);
        assert!(proj.join(".claude/skills").is_dir());
        assert!(proj.join(".claude/skills-disabled").is_dir());
    }
}
