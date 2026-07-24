//! Ordered set of Original Repository (warehouse) roots.
//!
//! Persisted as a JSON array under `chain_warehouse_roots`. The legacy scalar
//! `chain_warehouse_root` is migrated losslessly and, when nothing is
//! configured, the historical default is used — so existing single-root users
//! see no change.
//!
//! Roots are de-duplicated by canonical identity before scanning, so aliases
//! (symlinks, trailing slashes, repeated entries) are inspected once.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::core::{error::AppError, skill_store::SkillStore};

const ROOTS_KEY: &str = "chain_warehouse_roots";
const LEGACY_ROOT_KEY: &str = "chain_warehouse_root";

/// Historical default warehouse root (`~/Projects/xw-skills`).
pub fn default_warehouse_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("Projects")
        .join("xw-skills")
}

/// Default projects root (`~/Projects`).
pub fn default_projects_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Projects")
}

/// Ordered configured warehouse roots, honoring (in priority order) the
/// persisted array, the legacy scalar, then the built-in default. Never empty.
pub fn warehouse_roots(store: &SkillStore) -> Result<Vec<PathBuf>, AppError> {
    if let Some(raw) = store.get_setting(ROOTS_KEY).map_err(AppError::db)? {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
            let roots: Vec<PathBuf> = list
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect();
            if !roots.is_empty() {
                return Ok(roots);
            }
        }
    }
    if let Some(scalar) = store.get_setting(LEGACY_ROOT_KEY).map_err(AppError::db)? {
        let trimmed = scalar.trim();
        if !trimmed.is_empty() {
            return Ok(vec![PathBuf::from(trimmed)]);
        }
    }
    Ok(vec![default_warehouse_root()])
}

/// Persist an ordered list of warehouse roots (blank entries dropped).
pub fn set_warehouse_roots(store: &SkillStore, roots: &[String]) -> Result<(), AppError> {
    let cleaned: Vec<String> = roots
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let json = serde_json::to_string(&cleaned)
        .map_err(|e| AppError::internal(format!("serialize warehouse roots: {e}")))?;
    store.set_setting(ROOTS_KEY, &json).map_err(AppError::db)
}

/// One-shot lossless migration: seed the array from the legacy scalar when the
/// array key is absent. The scalar is left intact for rollback safety. A no-op
/// once the array exists or when there is nothing to migrate.
pub fn migrate_chain_roots(store: &SkillStore) -> Result<(), AppError> {
    if store
        .get_setting(ROOTS_KEY)
        .map_err(AppError::db)?
        .is_some()
    {
        return Ok(());
    }
    if let Some(scalar) = store.get_setting(LEGACY_ROOT_KEY).map_err(AppError::db)? {
        if !scalar.trim().is_empty() {
            set_warehouse_roots(store, std::slice::from_ref(&scalar))?;
        }
    }
    Ok(())
}

/// De-duplicate roots by canonical identity, preserving first-seen order and
/// the caller's original spelling. Canonical duplicates collapse so each
/// distinct root is scanned once.
pub fn dedupe(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        if seen.insert(dedup_key(root)) {
            out.push(root.clone());
        }
    }
    out
}

/// Canonical de-dup key. Uses the real filesystem identity when the root
/// exists; otherwise a lexical normalization so missing roots still collapse
/// exact/trailing-slash duplicates without touching the filesystem.
fn dedup_key(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| lexical_key(root))
}

fn lexical_key(root: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in root.components() {
        match comp {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        out
    }
}

/// Classify a root's readability for explicit per-root status reporting.
/// Returns `("ok" | "missing" | "unreadable", optional error detail)` so a
/// misconfigured root surfaces as a status rather than silently empty.
pub fn root_status(root: &Path) -> (&'static str, Option<String>) {
    match std::fs::read_dir(root) {
        Ok(_) => ("ok", None),
        Err(e) => {
            let kind = if root.exists() {
                "unreadable"
            } else {
                "missing"
            };
            (kind, Some(e.to_string()))
        }
    }
}

/// A configured root plus a lightweight readability status, for the settings
/// editor. Unlike the topology scan this does not recurse into repos.
#[derive(Debug, Clone, Serialize)]
pub struct RootConfigEntry {
    pub path: String,
    /// "ok" | "missing" | "unreadable"
    pub status: String,
    pub error: Option<String>,
}

/// The configured roots (as stored, order preserved, not de-duplicated), each
/// annotated with a readability status so the settings UI can flag bad roots
/// instead of letting them appear as empty.
pub fn warehouse_roots_config(store: &SkillStore) -> Result<Vec<RootConfigEntry>, AppError> {
    let roots = warehouse_roots(store)?;
    Ok(roots
        .iter()
        .map(|root| {
            let (status, error) = root_status(root);
            RootConfigEntry {
                path: root.to_string_lossy().to_string(),
                status: status.to_string(),
                error,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store_in(dir: &Path) -> SkillStore {
        SkillStore::new(&dir.join("patchbay.db")).unwrap()
    }

    #[test]
    fn defaults_to_builtin_when_unconfigured() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        assert_eq!(
            warehouse_roots(&store).unwrap(),
            vec![default_warehouse_root()]
        );
    }

    #[test]
    fn legacy_scalar_is_honored_and_migrated_losslessly() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        store
            .set_setting(LEGACY_ROOT_KEY, "/warehouse/one")
            .unwrap();

        // Read path already honors the scalar...
        assert_eq!(
            warehouse_roots(&store).unwrap(),
            vec![PathBuf::from("/warehouse/one")]
        );

        // ...and migration persists it as the array without dropping the scalar.
        migrate_chain_roots(&store).unwrap();
        assert_eq!(
            store.get_setting(ROOTS_KEY).unwrap().as_deref(),
            Some("[\"/warehouse/one\"]")
        );
        assert_eq!(
            store.get_setting(LEGACY_ROOT_KEY).unwrap().as_deref(),
            Some("/warehouse/one")
        );
    }

    #[test]
    fn migration_is_noop_when_array_present_or_nothing_to_migrate() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());

        // Nothing configured: no array written.
        migrate_chain_roots(&store).unwrap();
        assert_eq!(store.get_setting(ROOTS_KEY).unwrap(), None);

        // Existing array is not clobbered by a later scalar.
        set_warehouse_roots(&store, &["/a".to_string(), "/b".to_string()]).unwrap();
        store.set_setting(LEGACY_ROOT_KEY, "/legacy").unwrap();
        migrate_chain_roots(&store).unwrap();
        assert_eq!(
            warehouse_roots(&store).unwrap(),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn array_takes_priority_and_blanks_are_dropped() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        store.set_setting(LEGACY_ROOT_KEY, "/legacy").unwrap();
        set_warehouse_roots(
            &store,
            &["  /a  ".to_string(), "".to_string(), "/b".to_string()],
        )
        .unwrap();
        assert_eq!(
            warehouse_roots(&store).unwrap(),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn empty_array_falls_back_to_scalar_then_default() {
        let temp = tempdir().unwrap();
        let store = store_in(temp.path());
        set_warehouse_roots(&store, &["   ".to_string()]).unwrap(); // persists "[]"
        store.set_setting(LEGACY_ROOT_KEY, "/legacy").unwrap();
        assert_eq!(
            warehouse_roots(&store).unwrap(),
            vec![PathBuf::from("/legacy")]
        );
    }

    #[test]
    fn dedupe_collapses_repeated_and_trailing_slash_and_symlinked_roots() {
        // Exact + trailing-slash duplicates collapse for missing paths.
        let deduped = dedupe(&[
            PathBuf::from("/warehouse/a"),
            PathBuf::from("/warehouse/a/"),
            PathBuf::from("/warehouse/a"),
            PathBuf::from("/warehouse/b"),
        ]);
        assert_eq!(
            deduped,
            vec![PathBuf::from("/warehouse/a"), PathBuf::from("/warehouse/b")]
        );

        // Symlinked alias of a real dir collapses to one scan target.
        let temp = tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let alias = temp.path().join("alias");
        crate::core::test_support::symlink_dir(&real, &alias).unwrap();
        let deduped = dedupe(&[real.clone(), alias]);
        assert_eq!(deduped, vec![real]);
    }

    #[test]
    fn root_status_reports_missing_and_ok() {
        let temp = tempdir().unwrap();
        assert_eq!(root_status(temp.path()).0, "ok");
        let (kind, detail) = root_status(&temp.path().join("does-not-exist"));
        assert_eq!(kind, "missing");
        assert!(detail.is_some());
    }
}
