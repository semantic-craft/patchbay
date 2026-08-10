use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{Emitter, Manager};

pub mod commands;
pub mod core;

/// Shared flag: when true, CloseRequested should NOT be prevented.
pub static QUITTING: AtomicBool = AtomicBool::new(false);

const MAIN_TRAY_ID: &str = "main-tray";

#[cfg(target_os = "macos")]
const CUSTOM_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/tray-icon-32.png");
#[cfg(not(target_os = "macos"))]
const CUSTOM_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/tray-icon-color-32.png");

fn parse_bool_setting(value: Option<String>, default: bool) -> bool {
    match value.as_deref().map(str::trim).map(str::to_ascii_lowercase) {
        Some(v) if matches!(v.as_str(), "true" | "1" | "yes" | "on") => true,
        Some(v) if matches!(v.as_str(), "false" | "0" | "no" | "off") => false,
        _ => default,
    }
}

fn is_tray_icon_enabled(store: &Arc<core::skill_store::SkillStore>) -> bool {
    let value = store.get_setting("show_tray_icon").ok().flatten();
    parse_bool_setting(value, true)
}

fn restore_main_window(app: &tauri::AppHandle) {
    let app_for_main = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        {
            if let Err(err) = app_for_main.set_dock_visibility(true) {
                log::error!("Failed to show Dock icon on macOS: {err}");
            }
            if let Err(err) = app_for_main.set_activation_policy(tauri::ActivationPolicy::Regular) {
                log::error!("Failed to set activation policy to Regular on macOS: {err}");
            }
            if let Err(err) = app_for_main.show() {
                log::error!("Failed to show app on macOS: {err}");
            }
        }

        if let Some(w) = app_for_main.get_webview_window("main") {
            if let Err(err) = w.show() {
                log::error!("Failed to show main window: {err}");
            }
            if let Err(err) = w.unminimize() {
                log::error!("Failed to unminimize main window: {err}");
            }
            if let Err(err) = w.set_focus() {
                log::error!("Failed to focus main window: {err}");
            }
        } else {
            log::error!("Main window not found while restoring from tray");
        }
    }) {
        log::error!("Failed to schedule restore_main_window on main thread: {err}");
    }
}

fn request_quit(app: &tauri::AppHandle) {
    let app_for_main = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        quit_app(&app_for_main);
    }) {
        log::error!("Failed to schedule quit on main thread: {err}");
        // Fallback: attempt quit anyway.
        quit_app(app);
    }
}

fn load_custom_tray_icon() -> Option<tauri::image::Image<'static>> {
    let img = image::load_from_memory_with_format(CUSTOM_TRAY_ICON_BYTES, image::ImageFormat::Png)
        .ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(tauri::image::Image::new_owned(
        rgba.into_raw(),
        width,
        height,
    ))
}

/// The tray is deliberately minimal: show the window, or quit. Everything the
/// app does now needs a project selected, so there is nothing meaningful to
/// drive from a global menu.
fn build_tray_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<(tauri::menu::Menu<R>, String)> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let menu = Menu::new(app)?;

    let app_name = MenuItem::with_id(app, "tray-app-name", "Patchbay", false, None::<&str>)?;
    menu.append(&app_name)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let show_item = MenuItem::with_id(app, "show", "Open Patchbay", true, None::<&str>)?;
    menu.append(&show_item)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    menu.append(&quit_item)?;

    Ok((menu, "Patchbay".to_string()))
}

pub(crate) fn refresh_tray_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    if app.tray_by_id(MAIN_TRAY_ID).is_none() {
        return Ok(());
    }
    // `Menu::new`, `set_menu` and `set_tooltip` hit native macOS (NSMenu /
    // NSStatusItem) APIs and MUST run on the main thread — calling them from a
    // worker thread is UB and crashed the app with a `slice::from_raw_parts`
    // alignment panic during repeated tray actions.
    let app_for_main = app.clone();
    app.run_on_main_thread(move || {
        let Some(tray) = app_for_main.tray_by_id(MAIN_TRAY_ID) else {
            return;
        };
        match build_tray_menu(&app_for_main) {
            Ok((menu, tooltip)) => {
                if let Err(err) = tray.set_menu(Some(menu)) {
                    log::warn!("tray set_menu failed: {err}");
                }
                if let Err(err) = tray.set_tooltip(Some(&tooltip)) {
                    log::warn!("tray set_tooltip failed: {err}");
                }
            }
            Err(err) => log::warn!("build_tray_menu failed: {err}"),
        }
    })
    .map_err(|e| e.to_string())
}

fn ensure_tray_icon(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(MAIN_TRAY_ID).is_some() {
        return Ok(());
    }

    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let (menu, tooltip) = build_tray_menu(app)?;

    let mut builder = TrayIconBuilder::with_id(MAIN_TRAY_ID)
        .tooltip(tooltip)
        .menu(&menu)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            match id {
                "show" => {
                    log::debug!("Tray menu clicked: show");
                    restore_main_window(app)
                }
                "quit" => {
                    log::debug!("Tray menu clicked: quit");
                    request_quit(app)
                }
                _ => {}
            }
        });

    if let Some(icon) = load_custom_tray_icon().or_else(|| app.default_window_icon().cloned()) {
        builder = builder.icon(icon);
    }

    #[cfg(target_os = "macos")]
    {
        // Render the original white PNG directly for maximum brightness.
        builder = builder.icon_as_template(false);
    }

    // On macOS, left-click on tray icon opens the menu by default;
    // on Windows/Linux, left-click restores the window directly.
    if !cfg!(target_os = "macos") {
        builder = builder
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    restore_main_window(tray.app_handle());
                }
            });
    }

    let _tray = builder.build(app)?;
    log::debug!("Tray icon created");
    Ok(())
}

pub fn set_tray_icon_enabled(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let app_for_main = app.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let result = if enabled {
            ensure_tray_icon(&app_for_main).map_err(|e| e.to_string())
        } else {
            let _ = app_for_main.remove_tray_by_id(MAIN_TRAY_ID);
            log::debug!("Tray icon removed");
            Ok(())
        };
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;

    rx.recv()
        .map_err(|e| format!("Failed to receive tray update result: {e}"))?
}

/// Quit the application cleanly: destroy the main window, then exit.
///
/// Do NOT signal our process group here (e.g. `kill(-pgid, SIGTERM)`).
/// On Linux the app inherits the launcher's pgid — that may be the user's
/// desktop session (issue #47, tearing down GNOME) or the developer's shell
/// (terminating the parent terminal and its sibling jobs). Either is
/// catastrophic and not worth the convenience of auto-cleaning a stray
/// `tauri dev` vite process.
pub fn quit_app(app: &tauri::AppHandle) {
    QUITTING.store(true, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("main") {
        if let Err(err) = w.destroy() {
            log::error!("Failed to destroy main window while quitting: {err}");
        }
    }
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pre_builder_start = Instant::now();
    let (store, startup_timings) =
        core::app_state::initialize_store().expect("Failed to initialize app state");
    let pre_builder_ms = pre_builder_start.elapsed().as_millis();
    let store_for_setup = store.clone();

    let builder_start = Instant::now();
    tauri::Builder::default()
        .manage(store)
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            restore_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            // Snapshot the builder->setup gap BEFORE doing any work in setup,
            // so the label reflects only the time Tauri spent constructing
            // the App between Builder::default() and invoking this callback.
            let builder_to_setup_ms = builder_start.elapsed().as_millis();
            let setup_start = Instant::now();
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .level_for("tao", log::LevelFilter::Warn)
                    .level_for("wry", log::LevelFilter::Warn)
                    .level_for("hyper", log::LevelFilter::Warn)
                    .level_for("reqwest", log::LevelFilter::Warn)
                    .level_for("rustls", log::LevelFilter::Warn)
                    .max_file_size(5 * 1024 * 1024)
                    .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
                    .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                    .format(|out, message, record| {
                        out.finish(format_args!(
                            "{} {:5} [{}] {}",
                            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
                            record.level(),
                            record.target(),
                            message
                        ))
                    })
                    .build(),
            )?;

            core::panic_log::install_panic_hook(app.handle().clone());
            log::info!(
                "app start: version={} os={} arch={}",
                app.config().version.clone().unwrap_or_default(),
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            log::info!(
                "startup: pre_builder {} ms, builder_to_setup {} ms",
                pre_builder_ms,
                builder_to_setup_ms
            );
            startup_timings.log();

            let step = Instant::now();
            if is_tray_icon_enabled(&store_for_setup) {
                ensure_tray_icon(app.handle())?;
                log::info!(
                    "startup: ensure_tray_icon done in {} ms",
                    step.elapsed().as_millis()
                );
            } else {
                log::info!("startup: tray icon disabled");
            }

            let step = Instant::now();
            core::file_watcher::start_file_watcher(app.handle().clone(), store_for_setup.clone());
            log::info!(
                "startup: start_file_watcher done in {} ms",
                step.elapsed().as_millis()
            );

            // Fleet P2 automatic rounds are independently double-opt-in:
            // global setting plus per-repository manifest flag. The scheduler
            // is inert by default and yields immediately to manual fleet work.
            core::fleet::auto_round::start(app.handle().clone(), store_for_setup.clone());

            // Intercept window close — let frontend decide (close vs hide to tray)
            // When QUITTING is set, allow the close to proceed so the process fully exits.
            let step = Instant::now();
            let win = app.get_webview_window("main").unwrap();

            // Window-level glass (#37): apply once behind the webview and
            // record which tier actually took effect. The frontend only drops
            // its opaque CSS wallpaper when a native material is really there,
            // so a failed apply degrades to the pre-#37 look instead of a bare
            // transparent window.
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_liquid_glass::LiquidGlassExt;
                app.handle().plugin(tauri_plugin_liquid_glass::init())?;
                let supported = app.liquid_glass().is_supported();
                let apply_ok = match app.liquid_glass().set_effect(
                    &win,
                    tauri_plugin_liquid_glass::LiquidGlassConfig::default(),
                ) {
                    Ok(()) => true,
                    Err(err) => {
                        log::warn!("window glass apply failed, keeping CSS wallpaper: {err}");
                        false
                    }
                };
                let tier = commands::window_glass::resolve_tier(true, supported, apply_ok);
                commands::window_glass::record_tier(tier);
                log::info!("startup: window glass tier = {tier}");
            }
            #[cfg(not(target_os = "macos"))]
            commands::window_glass::record_tier(commands::window_glass::resolve_tier(
                false, false, false,
            ));

            let win_for_event = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if QUITTING.load(Ordering::SeqCst) {
                        return; // allow close
                    }
                    win_for_event.emit("window-close-requested", ()).ok();
                    api.prevent_close();
                }
            });
            log::info!(
                "startup: window handle + close hook in {} ms",
                step.elapsed().as_millis()
            );

            log::info!(
                "startup: setup() body total {} ms",
                setup_start.elapsed().as_millis()
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Chain (three-tier links)
            commands::chain::chain_get_topology,
            commands::chain::chain_doctor_report,
            commands::chain::chain_ignore_finding,
            commands::chain::chain_restore_finding,
            commands::chain::chain_duplicate_checkouts,
            commands::chain::chain_register_project,
            commands::chain::chain_plan_link,
            commands::chain::chain_apply_link,
            commands::chain::chain_plan_unlink,
            commands::chain::chain_apply_unlink,
            commands::chain::chain_locate_candidates,
            commands::chain::chain_plan_repair,
            commands::chain::chain_repo_moves,
            commands::chain::chain_repo_dirty_diff,
            commands::chain::chain_presets_list,
            commands::chain::chain_preset_save,
            commands::chain::chain_preset_rename,
            commands::chain::chain_preset_delete,
            commands::chain::chain_repair_live,
            commands::chain::chain_repair_live_control,
            commands::chain::chain_repair_journal,
            commands::chain::chain_undo_repair,
            commands::chain::chain_dismiss_repair_record,
            commands::chain::chain_apply_repair,
            commands::chain::chain_plan_pull,
            commands::chain::chain_apply_pull,
            commands::chain::chain_plan_fork_sync,
            commands::chain::chain_apply_fork_sync,
            commands::chain::chain_plan_remediate,
            commands::chain::chain_apply_remediate,
            commands::chain::chain_get_warehouse_roots,
            commands::chain::chain_set_warehouse_roots,
            // Fleet (multi-machine repo sync)
            commands::fleet::fleet_status,
            commands::fleet::fleet_discover,
            commands::fleet::fleet_auto_status,
            commands::fleet::fleet_set_repo_auto_sync,
            commands::fleet::fleet_manifest_get,
            commands::fleet::fleet_manifest_update,
            commands::fleet::fleet_plan_push,
            commands::fleet::fleet_apply_push,
            commands::fleet::fleet_plan_pull,
            commands::fleet::fleet_apply_pull,
            commands::fleet::fleet_plan_bootstrap,
            commands::fleet::fleet_apply_bootstrap,
            commands::fleet::fleet_report,
            // Instructions (AGENTS.md governance)
            commands::instructions::instructions_scan,
            commands::instructions::instructions_doctor_report,
            commands::instructions::instructions_ignore_finding,
            commands::instructions::instructions_restore_finding,
            commands::instructions::instructions_plan_normalize,
            commands::instructions::instructions_apply_normalize,
            commands::instructions::instructions_plan_init,
            commands::instructions::instructions_apply_init,
            // Agent surfaces
            commands::tools::get_tool_status,
            commands::tools::set_tool_enabled,
            commands::tools::set_all_tools_enabled,
            commands::tools::set_tool_order_cmd,
            commands::tools::set_custom_tool_path,
            commands::tools::reset_custom_tool_path,
            commands::tools::set_custom_tool_project_path,
            commands::tools::reset_custom_tool_project_path,
            commands::tools::add_custom_tool,
            commands::tools::remove_custom_tool,
            // Projects
            commands::projects::get_projects,
            commands::projects::add_project,
            commands::projects::add_linked_workspace,
            commands::projects::remove_project,
            commands::projects::scan_projects,
            // Window glass
            commands::window_glass::window_glass_status,
            // Settings and diagnostics
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::check_app_update,
            commands::settings::get_diagnostic_info,
            commands::settings::get_recent_log_excerpt,
            commands::settings::export_logs_zip,
            commands::settings::log_startup_event,
            commands::settings::check_last_panic,
            commands::settings::clear_last_panic,
            commands::settings::app_exit,
            commands::settings::hide_to_tray,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
