mod commands;
pub mod contract;
pub mod credentials;
pub mod logging;
mod state;
mod tray;
mod updater;

mod bindings;

use tauri::Manager;

/// Expands to the full invoke handler, plus any extra command paths passed in.
///
/// The production command list appears exactly once. The debug-only tray automation commands
/// are `#[cfg(debug_assertions)]`, so they must not be named in a release build's handler —
/// this macro lets the two `invoke_handler` branches differ without duplicating the list.
macro_rules! agentlens_handler {
    ($($debug_command:path),* $(,)?) => {
        tauri::generate_handler![
            commands::get_summary,
            commands::get_trend,
            commands::get_breakdown,
            commands::query_messages,
            commands::hosts_list,
            commands::hosts_get,
            commands::hosts_create,
            commands::hosts_update,
            commands::hosts_supported_sources,
            commands::hosts_delete,
            commands::trigger_refresh,
            commands::get_refresh_status,
            commands::get_settings,
            commands::set_settings,
            commands::price_catalog_get,
            commands::prices_get,
            commands::prices_set,
            commands::local_machine_identity,
            commands::ssh_probe,
            commands::ssh_probe_cancel,
            commands::credential_set,
            commands::credential_status,
            commands::credential_delete,
            commands::logs_tail,
            commands::diagnostics_report,
            updater::updater_check,
            updater::updater_install,
            $($debug_command),*
        ]
    };
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let state = state::AppState::open_default()?;

    let builder = tauri::Builder::default()
        // 「在系统文件管理器里定位归档库」与「在默认浏览器里打开反馈页」的唯一通道。
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .manage(updater::PendingUpdateState::default())
        .on_window_event(tray::handle_window_event)
        .setup(|app| {
            // Logging comes first so that everything below it is diagnosable. The log
            // directory is resolved by Tauri (`app_log_dir`) rather than hand-assembled,
            // because the three platforms disagree on where logs belong.
            if let Ok(directory) = app.path().app_log_dir() {
                logging::init(directory);
            }
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH,
                "AgentLens starting"
            );

            // `app_settings` is the only settings store; the shell applies what it holds
            // before the first refresh round can fire, and publishes the archive location
            // for the settings view. Both steps report failures, so they run after logging.
            let state = app.state::<state::AppState>();
            tray::publish_archive_location(&state);
            tray::apply_refresh_intervals(&state);
            state.start_refresh_loop();

            tray::install(app.handle())?;
            #[cfg(debug_assertions)]
            tray::spawn_selftest_driver(app.handle());
            Ok(())
        });

    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(agentlens_handler![
        tray::test_close_main_window,
        tray::test_quit
    ]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(agentlens_handler![]);

    builder.run(tauri::generate_context!())?;
    Ok(())
}
