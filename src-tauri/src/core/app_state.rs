use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};

use super::{app_dirs, chain, skill_store::SkillStore, tool_service};

/// Per-stage timings collected during `initialize_store`. The struct is
/// returned to the caller so the log lines can be emitted once
/// `tauri_plugin_log` is registered — anything logged from inside this
/// function would otherwise be dropped because the logger isn't installed
/// until later in `tauri::Builder::setup`. See issue #153.
#[derive(Debug, Clone)]
pub struct StartupTimings {
    pub ensure_app_dirs_ms: u128,
    pub open_store_ms: u128,
    pub migrate_legacy_tool_keys_ms: u128,
    pub total_ms: u128,
}

impl Default for StartupTimings {
    fn default() -> Self {
        Self {
            ensure_app_dirs_ms: 0,
            open_store_ms: 0,
            migrate_legacy_tool_keys_ms: 0,
            total_ms: 0,
        }
    }
}

pub fn initialize_store() -> Result<(Arc<SkillStore>, StartupTimings)> {
    initialize_store_inner()
}

pub fn initialize_cli_store() -> Result<Arc<SkillStore>> {
    initialize_store_inner().map(|(store, _)| store)
}

fn initialize_store_inner() -> Result<(Arc<SkillStore>, StartupTimings)> {
    let total_start = Instant::now();
    let mut timings = StartupTimings::default();

    let step = Instant::now();
    app_dirs::ensure_app_dirs().context("Failed to create the application data directory")?;
    timings.ensure_app_dirs_ms = step.elapsed().as_millis();

    let db_path = app_dirs::db_path();
    let step = Instant::now();
    let store = Arc::new(SkillStore::new(&db_path).context("Failed to initialize database")?);
    timings.open_store_ms = step.elapsed().as_millis();

    let step = Instant::now();
    tool_service::migrate_legacy_tool_keys(&store)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("Failed to migrate legacy tool keys")?;
    timings.migrate_legacy_tool_keys_ms = step.elapsed().as_millis();

    // Seed the ordered warehouse-roots array from the legacy scalar (lossless).
    chain::roots::migrate_chain_roots(&store)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("Failed to migrate chain warehouse roots")?;

    timings.total_ms = total_start.elapsed().as_millis();
    Ok((store, timings))
}

impl StartupTimings {
    /// Emit a single human-readable log block from the captured timings.
    /// Called from `tauri::Builder::setup` once `tauri_plugin_log` is
    /// installed; calling it before that point would lose the output to
    /// the no-op default logger.
    pub fn log(&self) {
        log::info!("startup: initialize_store total {} ms", self.total_ms);
        log::info!(
            "startup: ensure_app_dirs {} ms, open_store {} ms, migrate_legacy_tool_keys {} ms",
            self.ensure_app_dirs_ms,
            self.open_store_ms,
            self.migrate_legacy_tool_keys_ms
        );
    }
}
