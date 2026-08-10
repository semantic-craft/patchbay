use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, State};

use crate::core::error::AppError;
use crate::core::skill_store::SkillStore;
use crate::core::timing::should_log_first_or_slow;
use crate::core::tool_adapters::{self, CustomToolDef, ToolCategory};
use crate::core::tool_service::{
    self, get_custom_tool_paths, get_custom_tool_project_paths, get_custom_tools,
    get_disabled_tools, get_tool_order, normalize_project_relative_skills_dir_input,
    normalize_skills_dir_input, set_custom_tool_paths, set_custom_tool_project_paths,
    set_custom_tools, set_disabled_tools, set_tool_order, ToolInfo,
};

#[derive(Debug, Serialize)]
pub struct ToolInfoDto {
    pub key: String,
    pub display_name: String,
    pub installed: bool,
    pub skills_dir: String,
    pub enabled: bool,
    pub is_custom: bool,
    pub has_path_override: bool,
    pub project_relative_skills_dir: Option<String>,
    pub has_project_path_override: bool,
    pub category: ToolCategory,
}

static GET_TOOL_STATUS_FIRST_CALL: AtomicBool = AtomicBool::new(true);

#[tauri::command]
pub async fn get_tool_status(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<ToolInfoDto>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let start = Instant::now();
        let infos = tool_service::list_tool_info(&store);
        let count = infos.len();
        let result: Vec<ToolInfoDto> = infos
            .into_iter()
            .map(|info: ToolInfo| ToolInfoDto {
                key: info.key,
                display_name: info.display_name,
                installed: info.installed,
                skills_dir: info.skills_dir,
                enabled: info.enabled,
                is_custom: info.is_custom,
                has_path_override: info.has_path_override,
                project_relative_skills_dir: info.project_relative_skills_dir,
                has_project_path_override: info.has_project_path_override,
                category: info.category,
            })
            .collect();
        let elapsed_ms = start.elapsed().as_millis();
        if should_log_first_or_slow(&GET_TOOL_STATUS_FIRST_CALL, elapsed_ms, 100) {
            log::info!("get_tool_status: {count} tools in {elapsed_ms} ms");
        }
        Ok(result)
    })
    .await?
}

fn refresh_tray_menu_best_effort(app: &AppHandle) {
    if let Err(err) = crate::refresh_tray_menu(app) {
        log::warn!("Failed to refresh tray menu after tool mutation: {err}");
    }
}

#[tauri::command]
pub async fn set_tool_enabled(
    app: AppHandle,
    key: String,
    enabled: bool,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut disabled = get_disabled_tools(&store);
        if enabled {
            disabled.retain(|k| k != &key);
            set_disabled_tools(&store, &disabled)
        } else {
            if !disabled.contains(&key) {
                disabled.push(key.clone());
            }
            set_disabled_tools(&store, &disabled)
        }
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn set_all_tools_enabled(
    app: AppHandle,
    enabled: bool,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        if enabled {
            set_disabled_tools(&store, &[])
        } else {
            let adapters = tool_adapters::all_tool_adapters(&store);
            let all_keys: Vec<String> = adapters.iter().map(|a| a.key.clone()).collect();
            set_disabled_tools(&store, &all_keys)
        }
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn get_tool_order_cmd(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<String>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || Ok(get_tool_order(&store))).await?
}

#[tauri::command]
pub async fn set_tool_order_cmd(
    order: Vec<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || set_tool_order(&store, &order)).await?
}

#[tauri::command]
pub async fn set_custom_tool_path(
    key: String,
    path: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let key = key.trim().to_string();
        let path = normalize_skills_dir_input(&path)?;
        if key.is_empty() || path.is_empty() {
            return Err(AppError::invalid_input("Key and path are required"));
        }

        let old_adapter = tool_adapters::find_adapter_with_store(&store, &key)
            .ok_or_else(|| AppError::not_found(format!("Unknown tool: {key}")))?;
        let old_skills_dir = old_adapter.skills_dir();

        let mut customs = get_custom_tools(&store);
        if let Some(custom) = customs.iter_mut().find(|c| c.key == key) {
            custom.skills_dir = path;
            set_custom_tools(&store, &customs)?;
        } else {
            let mut paths = get_custom_tool_paths(&store);
            paths.insert(key.clone(), path);
            set_custom_tool_paths(&store, &paths)?;
        }

        let new_adapter = tool_adapters::find_adapter_with_store(&store, &key)
            .ok_or_else(|| AppError::not_found(format!("Unknown tool: {key}")))?;
        if old_skills_dir != new_adapter.skills_dir() {}
        Ok(())
    })
    .await?
}

#[tauri::command]
pub async fn reset_custom_tool_path(
    key: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let old_adapter = tool_adapters::find_adapter_with_store(&store, &key)
            .ok_or_else(|| AppError::not_found(format!("Unknown tool: {key}")))?;
        let old_skills_dir = old_adapter.skills_dir();

        let mut paths = get_custom_tool_paths(&store);
        paths.remove(&key);
        set_custom_tool_paths(&store, &paths)?;

        let new_adapter = tool_adapters::find_adapter_with_store(&store, &key)
            .ok_or_else(|| AppError::not_found(format!("Unknown tool: {key}")))?;
        if old_skills_dir != new_adapter.skills_dir() {}
        Ok(())
    })
    .await?
}

#[tauri::command]
pub async fn set_custom_tool_project_path(
    key: String,
    project_relative_skills_dir: Option<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(AppError::invalid_input("Key is required"));
        }
        let normalized = normalize_project_relative_skills_dir_input(
            project_relative_skills_dir.as_deref().unwrap_or_default(),
        )?;

        // Custom tools store the project path on their definition; clearing it
        // (None) drops project-workspace support for that agent.
        let mut customs = get_custom_tools(&store);
        if let Some(custom) = customs.iter_mut().find(|c| c.key == key) {
            custom.project_relative_skills_dir = normalized;
            return set_custom_tools(&store, &customs);
        }

        // Built-in tools keep overrides in a side map keyed by tool key.
        // Resolve the built-in default project path (no store overrides) to
        // validate the key and to detect no-op edits: an empty value, or one
        // equal to the default, removes the override and restores the default.
        let default_project_path = tool_adapters::default_tool_adapters()
            .into_iter()
            .find(|a| a.key == key)
            .map(|a| a.project_relative_skills_dir().to_string())
            .ok_or_else(|| AppError::not_found(format!("Unknown tool: {key}")))?;
        let mut project_paths = get_custom_tool_project_paths(&store);
        match normalized {
            Some(path) if path != default_project_path => {
                project_paths.insert(key, path);
            }
            _ => {
                project_paths.remove(&key);
            }
        }
        set_custom_tool_project_paths(&store, &project_paths)
    })
    .await?
}

#[tauri::command]
pub async fn reset_custom_tool_project_path(
    key: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(AppError::invalid_input("Key is required"));
        }
        if tool_adapters::find_adapter_with_store(&store, &key).is_none() {
            return Err(AppError::not_found(format!("Unknown tool: {key}")));
        }
        let mut project_paths = get_custom_tool_project_paths(&store);
        if project_paths.remove(&key).is_some() {
            set_custom_tool_project_paths(&store, &project_paths)?;
        }
        Ok(())
    })
    .await?
}

#[tauri::command]
pub async fn add_custom_tool(
    key: String,
    display_name: String,
    skills_dir: String,
    project_relative_skills_dir: Option<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let key = key.trim().to_string();
        let display_name = display_name.trim().to_string();
        let skills_dir = normalize_skills_dir_input(&skills_dir)?;
        let project_relative_skills_dir = normalize_project_relative_skills_dir_input(
            project_relative_skills_dir.as_deref().unwrap_or_default(),
        )?;
        if key.is_empty() || display_name.is_empty() || skills_dir.is_empty() {
            return Err(AppError::invalid_input(
                "Agent key, name and skills path are required",
            ));
        }

        // Validate key uniqueness
        let all = tool_adapters::all_tool_adapters(&store);
        if all.iter().any(|a| a.key == key) {
            return Err(AppError::invalid_input(format!(
                "Agent key \"{key}\" already exists"
            )));
        }
        let mut customs = get_custom_tools(&store);
        customs.push(CustomToolDef {
            key: key.clone(),
            display_name,
            skills_dir,
            project_relative_skills_dir,
            category: Default::default(),
        });
        set_custom_tools(&store, &customs)?;
        Ok(())
    })
    .await?
}

#[tauri::command]
pub async fn remove_custom_tool(
    key: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Remove from custom_tools list
        let mut customs = get_custom_tools(&store);
        customs.retain(|c| c.key != key);
        set_custom_tools(&store, &customs)?;
        // Remove any stale override for this key.
        let mut custom_paths = get_custom_tool_paths(&store);
        custom_paths.remove(&key);
        set_custom_tool_paths(&store, &custom_paths)?;
        // Also remove from disabled_tools if present
        let mut disabled = get_disabled_tools(&store);
        disabled.retain(|k| k != &key);
        set_disabled_tools(&store, &disabled)
    })
    .await?
}

pub fn migrate_legacy_tool_keys(store: &SkillStore) -> Result<(), AppError> {
    tool_service::migrate_legacy_tool_keys(store)
}
