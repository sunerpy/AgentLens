mod commands;
pub mod contract;
pub mod credentials;
mod state;
mod tray;

mod bindings;

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
            $($debug_command),*
        ]
    };
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let state = state::AppState::open_default()?;
    // `app_settings` is the only settings store; the shell applies what it holds before the
    // first refresh round can fire, and publishes the archive location for the settings view.
    tray::publish_archive_location(&state);
    tray::apply_refresh_intervals(&state);
    state.start_refresh_loop();

    let builder = tauri::Builder::default()
        // 只为「在系统文件管理器里定位归档库」而装：设置页的路径否则只能靠手工复制。
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .on_window_event(tray::handle_window_event)
        .setup(|app| {
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
