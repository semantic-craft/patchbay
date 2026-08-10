use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tauri::State;

use crate::core::skill_store::{ProjectRecord, SkillRecord, SkillStore};
use crate::core::timing::should_log_first_or_slow;
use crate::core::{error::AppError, project_registry, project_scanner, tool_adapters};

#[derive(Serialize, Default)]
pub struct SyncHealthDto {
    pub in_sync: usize,
    pub project_newer: usize,
    pub center_newer: usize,
    pub diverged: usize,
    pub project_only: usize,
}

#[derive(Serialize)]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub workspace_type: String,
    pub linked_agent_name: Option<String>,
    pub supports_skill_toggle: bool,
    pub sort_order: i32,
    pub skill_count: usize,
    pub sync_health: SyncHealthDto,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize)]
pub struct ProjectSkillDocumentDto {
    pub skill_name: String,
    pub filename: String,
    pub content: String,
}

#[derive(Serialize, Clone)]
pub struct ProjectAgentTargetDto {
    pub key: String,
    pub display_name: String,
    pub enabled: bool,
    pub installed: bool,
    pub is_custom: bool,
}

fn agent_skill_configs(store: &SkillStore) -> Vec<project_scanner::AgentSkillConfig> {
    let mut grouped: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for adapter in tool_adapters::all_tool_adapters(store) {
        let project_dir = adapter.project_relative_skills_dir().to_string();
        if project_dir.is_empty() {
            continue;
        }
        if let Some((_, agents)) = grouped.iter_mut().find(|(dir, _)| *dir == project_dir) {
            agents.push((adapter.key, adapter.display_name));
        } else {
            grouped.push((project_dir, vec![(adapter.key, adapter.display_name)]));
        }
    }

    grouped
        .into_iter()
        .filter_map(|(relative_skills_dir, agents)| {
            let (key, first_display_name) = agents.first()?.clone();
            let display_name = if agents.len() == 1 {
                first_display_name
            } else {
                agents
                    .into_iter()
                    .map(|(_, display_name)| display_name)
                    .collect::<Vec<_>>()
                    .join(" / ")
            };
            Some(project_scanner::AgentSkillConfig {
                key,
                display_name,
                relative_skills_dir,
            })
        })
        .collect()
}

fn linked_workspace_agent_key(rec: &ProjectRecord) -> String {
    rec.linked_agent_key
        .clone()
        .unwrap_or_else(|| slugify_skill_dir_name(&rec.name))
}

fn linked_workspace_agent_name(rec: &ProjectRecord) -> String {
    rec.linked_agent_name
        .clone()
        .unwrap_or_else(|| rec.name.clone())
}

fn read_workspace_skills(
    rec: &ProjectRecord,
    configs: &[project_scanner::AgentSkillConfig],
) -> Vec<project_scanner::ProjectSkillInfo> {
    if rec.workspace_type == "linked" {
        return project_scanner::read_linked_workspace_skills(
            Path::new(&rec.path),
            rec.disabled_path.as_deref().map(Path::new),
            &linked_workspace_agent_key(rec),
            &linked_workspace_agent_name(rec),
            true,
        );
    }
    project_scanner::read_project_skills(Path::new(&rec.path), configs)
}

fn project_to_dto(
    rec: &ProjectRecord,
    all_managed: &[SkillRecord],
    configs: &[project_scanner::AgentSkillConfig],
) -> ProjectDto {
    let skills = read_workspace_skills(rec, configs);
    let skill_count = skills.len();

    let mut health = SyncHealthDto::default();
    for skill in &skills {
        let matched = find_best_center_match(skill, all_managed);
        let status = classify_sync_status(skill, matched);
        match status.as_str() {
            "in_sync" => health.in_sync += 1,
            "project_newer" => health.project_newer += 1,
            "center_newer" => health.center_newer += 1,
            "diverged" => health.diverged += 1,
            _ => health.project_only += 1,
        }
    }

    ProjectDto {
        id: rec.id.clone(),
        name: rec.name.clone(),
        path: rec.path.clone(),
        workspace_type: rec.workspace_type.clone(),
        linked_agent_name: rec.linked_agent_name.clone(),
        supports_skill_toggle: rec.workspace_type != "linked" || rec.disabled_path.is_some(),
        sort_order: rec.sort_order,
        skill_count,
        sync_health: health,
        created_at: rec.created_at,
        updated_at: rec.updated_at,
    }
}

fn ensure_distinct_linked_workspace_roots(
    skills_root: &Path,
    disabled_root: &Path,
) -> Result<(), AppError> {
    let skills_canonical = std::fs::canonicalize(skills_root)?;
    let disabled_canonical = std::fs::canonicalize(disabled_root)?;

    if skills_canonical == disabled_canonical
        || skills_canonical.starts_with(&disabled_canonical)
        || disabled_canonical.starts_with(&skills_canonical)
    {
        return Err(AppError::invalid_input(
            "Skills directory and disabled skills directory must not overlap",
        ));
    }

    Ok(())
}

pub(crate) fn slugify_skill_dir_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        let valid = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.';
        if valid {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '_' || c == '.');
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn source_ref_matches_skill_path(
    skill_path: &str,
    skill_canonical: Option<&PathBuf>,
    managed: &SkillRecord,
) -> bool {
    let Some(source_ref) = managed.source_ref.as_deref() else {
        return false;
    };
    if source_ref == skill_path {
        return true;
    }
    let Some(skill_canonical) = skill_canonical else {
        return false;
    };
    let Ok(source_canonical) = std::fs::canonicalize(source_ref) else {
        return false;
    };
    source_canonical == *skill_canonical
}

pub(crate) fn find_best_center_match<'a>(
    skill: &project_scanner::ProjectSkillInfo,
    all_managed: &'a [SkillRecord],
) -> Option<&'a SkillRecord> {
    let skill_hash = skill.content_hash.as_deref();
    let canonical_skill_path = std::fs::canonicalize(&skill.path).ok();

    all_managed
        .iter()
        .filter_map(|managed| {
            if source_ref_matches_skill_path(&skill.path, canonical_skill_path.as_ref(), managed) {
                return Some((managed, 3));
            }
            if skill_hash.is_some() && managed.content_hash.as_deref() == skill_hash {
                return Some((managed, 2));
            }
            let managed_dir_name = slugify_skill_dir_name(&managed.name);
            if managed_dir_name.eq_ignore_ascii_case(&skill.dir_name) {
                return Some((managed, 1));
            }
            None
        })
        .max_by_key(|(_, score)| *score)
        .map(|(managed, _)| managed)
}

pub(crate) fn classify_sync_status(
    skill: &project_scanner::ProjectSkillInfo,
    managed: Option<&SkillRecord>,
) -> String {
    let Some(managed) = managed else {
        return "project_only".to_string();
    };

    // Fast path: compare project hash against DB-stored center hash
    if skill.content_hash.is_some()
        && managed.content_hash.as_deref() == skill.content_hash.as_deref()
    {
        return "in_sync".to_string();
    }

    // DB hash may be stale — recompute center hash from disk as fallback
    if let Some(project_hash) = skill.content_hash.as_deref() {
        if let Ok(live_center_hash) =
            crate::core::content_hash::hash_directory(Path::new(&managed.central_path))
        {
            if project_hash == live_center_hash {
                return "in_sync".to_string();
            }
        }
    }

    let Some(project_modified_at) = skill.last_modified_at else {
        return "diverged".to_string();
    };

    let center_updated_at = managed.updated_at;
    let threshold_ms = 1_000;
    if project_modified_at > center_updated_at + threshold_ms {
        "project_newer".to_string()
    } else if center_updated_at > project_modified_at + threshold_ms {
        "center_newer".to_string()
    } else {
        "diverged".to_string()
    }
}

static GET_PROJECTS_FIRST_CALL: AtomicBool = AtomicBool::new(true);

#[tauri::command]
pub async fn get_projects(store: State<'_, Arc<SkillStore>>) -> Result<Vec<ProjectDto>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let start = Instant::now();
        let records = store.get_all_projects().map_err(AppError::db)?;
        let all_managed = store.get_all_skills().map_err(AppError::db)?;
        let configs = agent_skill_configs(&store);
        let count = records.len();
        let dtos: Vec<ProjectDto> = records
            .iter()
            .map(|r| project_to_dto(r, &all_managed, &configs))
            .collect();
        let elapsed_ms = start.elapsed().as_millis();
        if should_log_first_or_slow(&GET_PROJECTS_FIRST_CALL, elapsed_ms, 100) {
            log::info!("get_projects: {count} projects in {elapsed_ms} ms");
        }
        Ok(dtos)
    })
    .await?
}

#[tauri::command]
pub async fn add_project(
    store: State<'_, Arc<SkillStore>>,
    path: String,
) -> Result<ProjectDto, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Registration goes through the shared registry so project identity — a
        // canonical absolute path — and alias de-duplication live in one place.
        // `true` scaffolds the Project Workspace `.claude/skills` directories,
        // preserving the existing behaviour of enrolling an empty directory.
        let record = project_registry::register_project(&store, Path::new(&path), true)?;
        let all_managed = store.get_all_skills().map_err(AppError::db)?;
        let configs = agent_skill_configs(&store);
        Ok(project_to_dto(&record, &all_managed, &configs))
    })
    .await?
}

#[tauri::command]
pub async fn add_linked_workspace(
    store: State<'_, Arc<SkillStore>>,
    name: String,
    path: String,
    disabled_path: Option<String>,
) -> Result<ProjectDto, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid_input("Workspace name is required"));
        }

        let skills_root = PathBuf::from(path.trim());
        if !skills_root.is_dir() {
            return Err(AppError::invalid_input("Skills directory does not exist"));
        }

        let disabled_path = disabled_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let disabled_path = if let Some(disabled) = disabled_path {
            let disabled_root = PathBuf::from(&disabled);
            if !disabled_root.is_dir() {
                return Err(AppError::invalid_input(
                    "Disabled skills directory does not exist",
                ));
            }
            ensure_distinct_linked_workspace_roots(&skills_root, &disabled_root)?;
            Some(disabled)
        } else {
            let mut disabled_root = skills_root.clone();
            let derived = disabled_root
                .file_name()
                .and_then(|n| n.to_str())
                .map(|name| format!("{}-disabled", name));
            match derived {
                Some(name) => {
                    disabled_root.set_file_name(name);
                    match std::fs::create_dir_all(&disabled_root) {
                        Ok(()) => {
                            ensure_distinct_linked_workspace_roots(&skills_root, &disabled_root)?;
                            Some(disabled_root.to_string_lossy().to_string())
                        }
                        Err(_) => None,
                    }
                }
                None => None,
            }
        };

        let now = chrono::Utc::now().timestamp_millis();
        let record = ProjectRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
            path: skills_root.to_string_lossy().to_string(),
            workspace_type: "linked".to_string(),
            linked_agent_key: Some(slugify_skill_dir_name(&name)),
            linked_agent_name: Some(name),
            disabled_path,
            sort_order: 0,
            created_at: now,
            updated_at: now,
        };

        store.insert_project(&record).map_err(AppError::db)?;
        let all_managed = store.get_all_skills().map_err(AppError::db)?;
        let configs = agent_skill_configs(&store);
        Ok(project_to_dto(&record, &all_managed, &configs))
    })
    .await?
}

#[tauri::command]
pub async fn remove_project(store: State<'_, Arc<SkillStore>>, id: String) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.delete_project(&id).map_err(AppError::db))
        .await?
}

#[tauri::command]
pub async fn scan_projects(
    root: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<String>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root_path = Path::new(&root);
        if !root_path.is_dir() {
            return Err(AppError::invalid_input("Directory does not exist"));
        }
        let configs = agent_skill_configs(&store);
        Ok(project_scanner::scan_projects_in_dir(
            root_path, 4, &configs,
        ))
    })
    .await?
}
