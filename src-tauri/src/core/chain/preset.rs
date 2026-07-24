//! Chain assembly presets (issue #35): a named set of warehouse skill
//! references, saved from a project's current links and consumed by the
//! onboarding wizard (#36) as batch-link starting points.
//!
//! A reference names the skill, the Original it resolves to, and (when known)
//! the repository it was scanned from. References are stored verbatim —
//! whether a referenced Original still exists is the wizard's (and Doctor's)
//! concern at APPLY time, not the preset's at save time.

use serde::{Deserialize, Serialize};

use crate::core::error::AppError;
use crate::core::skill_store::{ChainPresetRow, SkillStore};

/// One warehouse skill reference inside a preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainPresetSkill {
    /// Skill (directory) name.
    pub name: String,
    /// Absolute path of the Original the chain resolved to at save time.
    pub path: String,
    /// Repository display name the Original was scanned from, when known.
    pub repo: Option<String>,
}

/// A named chain assembly preset.
#[derive(Debug, Clone, Serialize)]
pub struct ChainPreset {
    pub id: i64,
    pub name: String,
    pub skills: Vec<ChainPresetSkill>,
    pub created_at: i64,
}

fn parse(row: &ChainPresetRow) -> Option<ChainPreset> {
    Some(ChainPreset {
        id: row.id,
        name: row.name.clone(),
        skills: serde_json::from_str(&row.skills).ok()?,
        created_at: row.created_at,
    })
}

/// All presets, name order. A row whose JSON no longer parses is skipped
/// rather than blanking the whole bar.
pub fn list(store: &SkillStore) -> Result<Vec<ChainPreset>, AppError> {
    let rows = store.list_chain_presets().map_err(AppError::db)?;
    Ok(rows.iter().filter_map(parse).collect())
}

/// Save a new preset. The name must be non-empty and unused; the reference
/// set must be non-empty (an empty preset can assemble nothing).
pub fn save(
    store: &SkillStore,
    name: &str,
    skills: &[ChainPresetSkill],
) -> Result<ChainPreset, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid_input("preset name must not be empty"));
    }
    if skills.is_empty() {
        return Err(AppError::invalid_input(
            "a preset needs at least one skill reference",
        ));
    }
    let skills_json = serde_json::to_string(skills)
        .map_err(|e| AppError::invalid_input(format!("unserializable skills: {e}")))?;
    let id = store
        .insert_chain_preset(name, &skills_json)
        .map_err(|e| db_or_duplicate(e, name))?;
    Ok(ChainPreset {
        id,
        name: name.to_string(),
        skills: skills.to_vec(),
        created_at: chrono::Utc::now().timestamp(),
    })
}

/// Rename a preset; the new name must be non-empty and unused.
pub fn rename(store: &SkillStore, id: i64, name: &str) -> Result<(), AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid_input("preset name must not be empty"));
    }
    let renamed = store
        .rename_chain_preset(id, name)
        .map_err(|e| db_or_duplicate(e, name))?;
    if !renamed {
        return Err(AppError::not_found("no such preset"));
    }
    Ok(())
}

pub fn delete(store: &SkillStore, id: i64) -> Result<(), AppError> {
    let deleted = store.delete_chain_preset(id).map_err(AppError::db)?;
    if !deleted {
        return Err(AppError::not_found("no such preset"));
    }
    Ok(())
}

/// Map the UNIQUE(name) violation to a human-readable input error; anything
/// else stays a database error.
fn db_or_duplicate(e: anyhow::Error, name: &str) -> AppError {
    if e.to_string().contains("UNIQUE constraint failed") {
        AppError::invalid_input(format!("a preset named \"{name}\" already exists"))
    } else {
        AppError::db(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn skill(name: &str) -> ChainPresetSkill {
        ChainPresetSkill {
            name: name.to_string(),
            path: format!("/wh/repo/skills/{name}"),
            repo: Some("repo".to_string()),
        }
    }

    fn store() -> (tempfile::TempDir, SkillStore) {
        let temp = tempdir().unwrap();
        let store = SkillStore::new(&temp.path().join("patchbay.db")).unwrap();
        (temp, store)
    }

    #[test]
    fn save_list_rename_delete_round_trip() {
        let (_temp, store) = store();

        let saved = save(
            &store,
            " 写作全套 ",
            &[skill("zotero"), skill("ppt-master")],
        )
        .unwrap();
        assert_eq!(saved.name, "写作全套", "names are trimmed");

        let listed = list(&store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].skills.len(), 2);
        assert_eq!(listed[0].skills[0].path, "/wh/repo/skills/zotero");

        rename(&store, saved.id, "文献流水线").unwrap();
        assert_eq!(list(&store).unwrap()[0].name, "文献流水线");

        delete(&store, saved.id).unwrap();
        assert!(list(&store).unwrap().is_empty());
    }

    #[test]
    fn duplicate_names_are_rejected_for_save_and_rename() {
        let (_temp, store) = store();
        save(&store, "工程基础", &[skill("tdd")]).unwrap();
        let second = save(&store, "写作全套", &[skill("zotero")]).unwrap();

        assert!(save(&store, "工程基础", &[skill("prototype")]).is_err());
        assert!(rename(&store, second.id, "工程基础").is_err());
        // The failed operations changed nothing.
        assert_eq!(list(&store).unwrap().len(), 2);
    }

    #[test]
    fn empty_name_or_empty_skills_are_rejected_and_ids_must_exist() {
        let (_temp, store) = store();
        assert!(save(&store, "  ", &[skill("tdd")]).is_err());
        assert!(save(&store, "空套装", &[]).is_err());
        assert!(rename(&store, 999, "x").is_err());
        assert!(delete(&store, 999).is_err());
    }
}
