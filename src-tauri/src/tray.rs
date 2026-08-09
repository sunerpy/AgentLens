//! Desktop-shell integration owned by todo 19: the resident tray icon, close-to-tray
//! behaviour, the boot-time application of persisted `app_settings`, and the debug-only
//! automation hooks that make the tray QA sequence fully unattended.
//!
//! EXCLUSIVE FILE BOUNDARY — todo 19 owns `src-tauri/src/tray.rs`. The only other Rust file
//! this worker touches is `lib.rs`, where the tray is wired in.
//!
//! Design notes:
//! - Closing the main window **hides** it (`prevent_close` + `hide`) instead of destroying the
//!   webview. The plan defers the lightweight "destroy the webview and keep only the tray"
//!   mode to a later phase, so the running webview must survive a close.
//! - `app_settings` is the single settings store. This module reads it at boot to apply the
//!   persisted refresh intervals to the scheduler and writes exactly one derived, read-only
//!   key (`archive.path`) so the settings view can display the archive location without a new
//!   IPC command.
//! - The refresh-interval floor is enforced here as well as in the UI. The floor exists because
//!   a full scan of a real 43 GB archive measured 23.3 s, so a sub-300 s poll was rejected in
//!   review as guaranteeing overlapping scans. An SSH round is strictly slower than a local
//!   one, so the same floor is the minimum sane value for both kinds.

use std::collections::BTreeMap;

use agentlens_core::archive::{read_app_settings, write_app_settings};
use agentlens_core::host::HostKind;
use agentlens_core::hostsource::{
    SourceKey, SourceSchedule, TriggerMode, DEFAULT_LOCAL_MIN_INTERVAL_MS,
    DEFAULT_REMOTE_INTERVAL_MS, MIN_AUTO_REFRESH_INTERVAL_MS,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Window, WindowEvent};

use crate::state::AppState;

/// Label of the window declared in `tauri.conf.json`.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// `app_settings` key holding the archive database path. Written by the shell at boot and
/// rendered read-only by the settings view; it is derived state, not a user preference.
pub const SETTING_KEY_ARCHIVE_PATH: &str = "archive.path";
/// `app_settings` key holding the configured local refresh interval, in milliseconds.
pub const SETTING_KEY_LOCAL_INTERVAL_MS: &str = "refresh.localIntervalMs";
/// `app_settings` key holding the configured remote refresh interval, in milliseconds.
pub const SETTING_KEY_REMOTE_INTERVAL_MS: &str = "refresh.remoteIntervalMs";
/// `app_settings` key holding whether timer-driven refresh runs at all.
///
/// Absent means enabled, so an installation that predates the toggle keeps refreshing.
pub const SETTING_KEY_AUTO_REFRESH_ENABLED: &str = "refresh.autoRefreshEnabled";

/// Hard floor for any configured refresh interval (10 minutes).
///
/// The authoritative rejection lives in `agentlens_core`; this is the same number, used here to
/// resolve a value already stored in `app_settings` — including one written by an older build with
/// the previous 5-minute floor, which is why this path clamps instead of failing to start.
pub const MIN_REFRESH_INTERVAL_MS: u64 = MIN_AUTO_REFRESH_INTERVAL_MS;

const MENU_ID_OPEN: &str = "tray-open";
const MENU_ID_REFRESH: &str = "tray-refresh";
const MENU_ID_QUIT: &str = "tray-quit";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayMenuAction {
    Open,
    Refresh,
    Quit,
    Ignore,
}

fn tray_menu_action(menu_id: &str) -> TrayMenuAction {
    match menu_id {
        MENU_ID_OPEN => TrayMenuAction::Open,
        MENU_ID_REFRESH => TrayMenuAction::Refresh,
        MENU_ID_QUIT => TrayMenuAction::Quit,
        _ => TrayMenuAction::Ignore,
    }
}

// Native tray menu labels live here rather than in `frontend/src/i18n/zh.ts`: the menu is an
// OS-level widget built by Rust, so it cannot read the frontend dictionary. `check-i18n` only
// governs `frontend/src/**`, which is where every browser-rendered string still comes from.
const MENU_LABEL_OPEN: &str = "打开 AgentLens";
const MENU_LABEL_REFRESH: &str = "立即刷新";
const MENU_LABEL_QUIT: &str = "退出";
const TRAY_TOOLTIP: &str = "AgentLens";

/// Builds the resident tray icon with the open / refresh / quit menu.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // First point in startup where an `AppHandle` exists: `lib.rs` must start the refresh loop
    // before the Tauri builder is constructed, so the loop cannot emit
    // `EVENT_ARCHIVE_COMMITTED` until the handle is handed to it here.
    app.state::<AppState>().attach_app_handle(app.clone());

    let open = MenuItem::with_id(app, MENU_ID_OPEN, MENU_LABEL_OPEN, true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, MENU_ID_REFRESH, MENU_LABEL_REFRESH, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_ID_QUIT, MENU_LABEL_QUIT, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;

    let mut builder = TrayIconBuilder::new()
        .tooltip(TRAY_TOOLTIP)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match tray_menu_action(event.id().as_ref()) {
            TrayMenuAction::Open => show_main_window(app),
            TrayMenuAction::Refresh => refresh_all_hosts(app),
            TrayMenuAction::Quit => app.exit(0),
            TrayMenuAction::Ignore => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    tracing::info!("tray icon installed (open / refresh / quit)");
    Ok(())
}

/// Turns a close request into "hide to tray" so the webview keeps running.
pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        if window.label() != MAIN_WINDOW_LABEL {
            return;
        }
        api.prevent_close();
        if let Err(error) = window.hide() {
            tracing::error!(%error, "failed to hide the main window");
            return;
        }
        tracing::info!("main window hidden to tray (webview kept alive)");
    }
}

fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn refresh_all_hosts(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(outcomes) = trigger_all_hosts(&state) else {
        return;
    };
    for (host_id, outcome) in outcomes {
        if let Err(error) = outcome {
            tracing::error!(host_id = %host_id, message = %error.message, "tray refresh failed");
        }
    }
}

type HostRefreshOutcome = (
    String,
    Result<Vec<crate::contract::TriggerRefreshResult>, crate::contract::IpcError>,
);

fn trigger_all_hosts(
    state: &AppState,
) -> Result<Vec<HostRefreshOutcome>, crate::contract::IpcError> {
    let host_ids = state.lock_scheduler()?.host_ids();
    Ok(host_ids
        .into_iter()
        .map(|host_id| {
            let outcome = state.trigger_refresh(&host_id);
            (host_id, outcome)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Persisted settings applied by the shell at boot
// ---------------------------------------------------------------------------

/// Publishes the archive database path into `app_settings` so the settings view can display it.
///
/// Best-effort: a failure must not stop the desktop shell from starting, so the error is logged
/// and the settings view falls back to "location unavailable".
pub fn publish_archive_location(state: &AppState) {
    let mut archive = match state.lock_archive() {
        Ok(archive) => archive,
        Err(error) => {
            tracing::error!(message = %error.message, "archive unavailable");
            return;
        }
    };
    let mut values = BTreeMap::new();
    values.insert(
        SETTING_KEY_ARCHIVE_PATH.to_owned(),
        archive.path().display().to_string(),
    );
    if let Err(error) = write_app_settings(archive.connection_mut(), &values) {
        tracing::error!(%error, "failed to publish the archive location");
    }
}

/// Clamps an interval string from `app_settings` into a usable millisecond value.
///
/// Anything unparseable, non-positive, or below [`MIN_REFRESH_INTERVAL_MS`] resolves to the
/// floor rather than to a hot loop over SQLite.
pub fn resolve_interval_ms(raw: Option<&String>, default_ms: u64) -> u64 {
    let configured = raw
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.unsigned_abs())
        .unwrap_or(default_ms);
    configured.max(MIN_REFRESH_INTERVAL_MS)
}

/// Applies the persisted refresh intervals to every registered source.
///
/// Runs once at boot, before the refresh loop starts, so a configured interval is honoured
/// instead of silently falling back to the per-kind default.
pub fn resolve_auto_refresh_enabled(raw: Option<&String>) -> bool {
    raw.is_none_or(|value| !matches!(value.trim(), "false" | "0" | "off" | "no"))
}

pub fn apply_refresh_intervals(state: &AppState) {
    let settings = match state
        .lock_archive()
        .map_err(|error| error.message)
        .and_then(|archive| read_app_settings(archive.connection()).map_err(|e| e.to_string()))
    {
        Ok(settings) => settings,
        Err(error) => {
            tracing::error!(%error, "unable to read persisted refresh intervals");
            return;
        }
    };
    let local_ms = resolve_interval_ms(
        settings.get(SETTING_KEY_LOCAL_INTERVAL_MS),
        DEFAULT_LOCAL_MIN_INTERVAL_MS,
    );
    let remote_ms = resolve_interval_ms(
        settings.get(SETTING_KEY_REMOTE_INTERVAL_MS),
        DEFAULT_REMOTE_INTERVAL_MS,
    );
    let auto_refresh_enabled =
        resolve_auto_refresh_enabled(settings.get(SETTING_KEY_AUTO_REFRESH_ENABLED));
    let Ok(mut scheduler) = state.lock_scheduler() else {
        return;
    };
    for status in scheduler.statuses() {
        let interval_ms = match status.kind {
            HostKind::Local => local_ms,
            HostKind::Ssh => remote_ms,
        };
        let trigger = if auto_refresh_enabled {
            TriggerMode::Auto
        } else {
            TriggerMode::Manual
        };
        let schedule = SourceSchedule::for_kind(status.kind)
            .with_trigger(trigger)
            .with_min_interval_ms(interval_ms);
        let key = SourceKey::new(status.host_id.clone(), status.source.clone());
        if let Err(error) = scheduler.set_schedule(&key, schedule) {
            tracing::warn!(source = %key, %error, "unable to apply refresh interval");
        }
    }
}

// ---------------------------------------------------------------------------
// Debug-only automation hooks
//
// Both commands are `#[cfg(debug_assertions)]`, so they do not exist in a release build; the
// invoke handler in `lib.rs` registers them only on the debug branch.
// ---------------------------------------------------------------------------

/// Requests a close of the main window exactly as a user clicking the window chrome would.
///
/// The close travels through [`handle_window_event`], so a successful call proves the
/// hide-to-tray path rather than a shortcut that hides the window directly.
#[cfg(debug_assertions)]
#[tauri::command]
pub fn test_close_main_window(app: AppHandle) -> Result<(), crate::contract::IpcError> {
    let window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| crate::contract::IpcError::not_found("window", MAIN_WINDOW_LABEL))?;
    window.close().map_err(|error| {
        crate::contract::IpcError::new(crate::contract::IpcErrorCode::Internal, error.to_string())
    })
}

/// Terminates the process, the tray "quit" action's programmatic equivalent.
#[cfg(debug_assertions)]
#[tauri::command]
pub fn test_quit(app: AppHandle) {
    println!("agentlens: test_quit received, exiting");
    app.exit(0);
}

/// Drives the unattended tray sequence when `AGENTLENS_TRAY_SELFTEST_DIR` is set.
///
/// The driver issues both debug commands as **real Tauri invokes from the webview**
/// (`window.__TAURI_INTERNALS__.invoke`), which is the same channel WebdriverIO would use; the
/// external QA script owns the `kill -0` liveness assertions and hands control back by creating
/// the `proceed-quit` file, so there is no timing race between the two halves.
#[cfg(debug_assertions)]
pub fn spawn_selftest_driver(app: &AppHandle) {
    use std::path::PathBuf;

    let Some(dir) = std::env::var_os("AGENTLENS_TRAY_SELFTEST_DIR") else {
        return;
    };
    let app = app.clone();
    let dir = PathBuf::from(dir);
    std::thread::spawn(move || run_selftest(&app, &dir));
}

#[cfg(debug_assertions)]
fn run_selftest(app: &AppHandle, dir: &std::path::Path) {
    use std::time::Duration;

    const STEP: Duration = Duration::from_millis(100);

    let poll = |deadline: Duration, ready: Box<dyn FnMut() -> bool>| -> bool {
        poll_until(deadline, STEP, ready)
    };

    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        println!("SELFTEST FAIL: main window missing");
        return;
    };

    let visible_probe = window.clone();
    if !poll(
        Duration::from_secs(30),
        Box::new(move || visible_probe.is_visible().unwrap_or(false)),
    ) {
        println!("SELFTEST FAIL: main window never became visible");
        return;
    }
    println!("SELFTEST STEP window-visible=true");

    if let Err(error) = window.eval("window.__TAURI_INTERNALS__.invoke('test_close_main_window')") {
        println!("SELFTEST FAIL: eval of test_close_main_window failed: {error}");
        return;
    }
    println!("SELFTEST STEP invoked=test_close_main_window");

    let hidden_probe = window.clone();
    if !poll(
        Duration::from_secs(15),
        Box::new(move || !hidden_probe.is_visible().unwrap_or(true)),
    ) {
        println!("SELFTEST FAIL: main window is still visible after close");
        return;
    }
    println!("SELFTEST STEP window-visible=false");

    let proceed = dir.join("proceed-quit");
    if !poll(Duration::from_secs(120), Box::new(move || proceed.exists())) {
        println!("SELFTEST FAIL: proceed-quit was never created");
        return;
    }
    println!("SELFTEST STEP proceed-quit=observed");

    // Succeeding here also proves the hidden webview was not destroyed: the invoke is issued
    // from inside it.
    if let Err(error) = window.eval("window.__TAURI_INTERNALS__.invoke('test_quit')") {
        println!("SELFTEST FAIL: eval of test_quit failed: {error}");
        return;
    }
    println!("SELFTEST STEP invoked=test_quit");
}

#[cfg(debug_assertions)]
fn poll_until(
    deadline: std::time::Duration,
    step: std::time::Duration,
    mut ready: impl FnMut() -> bool,
) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(step);
    }
    false
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::commands::{
        get_settings_impl, get_trend_impl, hosts_create_impl, set_settings_impl,
    };
    use crate::contract::{
        AggregateFilters, AppSettings, DateRange, Granularity, HostCreateInput,
        HostKind as ContractHostKind, TriggerRefreshResult, WeekStart,
    };
    use crate::state::AppState;

    /// 2026-01-01T00:00:00Z.
    const JAN_1_UTC_MS: i64 = 1_767_225_600_000;
    const EIGHT_HOURS_MS: i64 = 8 * 3_600_000;

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open test app state");
        (data_dir, state)
    }

    fn settings(pairs: &[(&str, &str)]) -> AppSettings {
        AppSettings {
            values: pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    fn range(start_date: &str, end_date_exclusive: &str) -> DateRange {
        DateRange {
            start_date: start_date.to_owned(),
            end_date_exclusive: end_date_exclusive.to_owned(),
            week_start: WeekStart::Monday,
        }
    }

    /// Persists a report timezone, reads it back out of `app_settings`, and feeds exactly that
    /// value into `get_trend` — the same path the shell takes — then asserts the day-bucket
    /// boundaries and the hour labels actually move.
    #[test]
    fn persisted_report_timezone_shifts_get_trend_bucket_boundaries() {
        let (_data_dir, state) = state();

        let persist_and_trend = |tz: &str, granularity: Granularity, days: (&str, &str)| {
            set_settings_impl(&state, settings(&[("report.timezone", tz)]))
                .expect("persist report timezone");
            let stored = get_settings_impl(&state)
                .expect("read settings back")
                .values
                .get("report.timezone")
                .expect("timezone key present")
                .clone();
            assert_eq!(stored, tz);
            get_trend_impl(
                &state,
                range(days.0, days.1),
                stored,
                granularity,
                AggregateFilters::default(),
            )
            .expect("query trend")
        };

        let utc = persist_and_trend("UTC", Granularity::Day, ("2026-01-01", "2026-01-03"));
        let shanghai = persist_and_trend(
            "Asia/Shanghai",
            Granularity::Day,
            ("2026-01-01", "2026-01-03"),
        );

        assert_eq!(utc.total.len(), 2);
        assert_eq!(shanghai.total.len(), 2);
        assert_eq!(utc.total[0].bucket.start_utc_ms, JAN_1_UTC_MS);
        assert_eq!(utc.total[0].bucket.end_utc_ms, JAN_1_UTC_MS + 86_400_000);
        assert_eq!(
            shanghai.total[0].bucket.start_utc_ms,
            JAN_1_UTC_MS - EIGHT_HOURS_MS
        );
        assert_eq!(
            shanghai.total[0].bucket.end_utc_ms,
            JAN_1_UTC_MS + 86_400_000 - EIGHT_HOURS_MS
        );
        assert_ne!(
            utc.total[0].bucket.start_utc_ms,
            shanghai.total[0].bucket.start_utc_ms
        );
        assert_ne!(
            utc.total[1].bucket.start_utc_ms,
            shanghai.total[1].bucket.start_utc_ms
        );

        // Hour labels carry the numeric offset, so the timezone change is visible in the label
        // too, not only in the epoch boundaries.
        let utc_hours = persist_and_trend("UTC", Granularity::Hour, ("2026-01-01", "2026-01-02"));
        let shanghai_hours = persist_and_trend(
            "Asia/Shanghai",
            Granularity::Hour,
            ("2026-01-01", "2026-01-02"),
        );
        assert_eq!(utc_hours.total[0].bucket.label, "2026-01-01T00:00+00:00");
        assert_eq!(
            shanghai_hours.total[0].bucket.label,
            "2026-01-01T00:00+08:00"
        );
    }

    #[test]
    fn stored_refresh_interval_below_the_ten_minute_floor_is_clamped_on_read() {
        assert_eq!(MIN_REFRESH_INTERVAL_MS, 600_000);
        for below in ["60000", "300000", "599999"] {
            let raw = below.to_owned();
            assert_eq!(
                resolve_interval_ms(Some(&raw), DEFAULT_LOCAL_MIN_INTERVAL_MS),
                MIN_REFRESH_INTERVAL_MS,
                "a stored {below:?} predating the floor must resolve to the floor"
            );
        }
        for malformed in ["0", "-1", "abc", "", "  "] {
            let raw = malformed.to_owned();
            assert_eq!(
                resolve_interval_ms(Some(&raw), DEFAULT_REMOTE_INTERVAL_MS),
                DEFAULT_REMOTE_INTERVAL_MS,
                "malformed interval {malformed:?} must fall back to the default"
            );
        }
        assert_eq!(
            resolve_interval_ms(None, DEFAULT_REMOTE_INTERVAL_MS),
            900_000
        );
        let long = "1800000".to_owned();
        assert_eq!(
            resolve_interval_ms(Some(&long), DEFAULT_LOCAL_MIN_INTERVAL_MS),
            1_800_000
        );
    }

    #[test]
    fn auto_refresh_stays_enabled_unless_the_setting_explicitly_turns_it_off() {
        assert!(
            resolve_auto_refresh_enabled(None),
            "an installation predating the toggle keeps refreshing"
        );
        for enabled in ["true", "1", "on", "yes", " true "] {
            assert!(resolve_auto_refresh_enabled(Some(&enabled.to_owned())));
        }
        for disabled in ["false", "0", "off", "no", " false "] {
            assert!(!resolve_auto_refresh_enabled(Some(&disabled.to_owned())));
        }
    }

    #[test]
    fn disabling_auto_refresh_switches_every_slot_to_manual_and_stops_ticking() {
        let (_data_dir, state) = state();
        let host = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local workstation".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "d".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: Some(vec![
                    agentlens_core::ingest::OPENCODE_SOURCE.to_owned(),
                    agentlens_core::source::claude_code::CLAUDE_CODE_SOURCE.to_owned(),
                ]),
            },
        )
        .expect("create a two-source local host");
        let slots = [
            SourceKey::opencode(&host.host_id),
            SourceKey::new(
                &host.host_id,
                agentlens_core::source::claude_code::CLAUDE_CODE_SOURCE,
            ),
        ];
        for slot in &slots {
            assert_eq!(
                state
                    .lock_scheduler()
                    .expect("lock scheduler")
                    .status(slot)
                    .expect("slot is registered")
                    .trigger,
                TriggerMode::Auto
            );
        }

        set_settings_impl(
            &state,
            settings(&[(SETTING_KEY_AUTO_REFRESH_ENABLED, "false")]),
        )
        .expect("persist the disabled toggle");

        for slot in &slots {
            let status = state
                .lock_scheduler()
                .expect("lock scheduler")
                .status(slot)
                .expect("slot is still registered");
            assert_eq!(status.trigger, TriggerMode::Manual);
            assert_eq!(status.next_due_utc, None, "a manual slot is never auto-due");
        }
        assert!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .tick(i64::MAX / 2)
                .is_empty(),
            "no slot may be admitted by the timer while auto refresh is off"
        );
    }

    #[test]
    fn tray_menu_ids_map_to_explicit_actions_and_unknown_ids_are_ignored() {
        assert_eq!(tray_menu_action(MENU_ID_OPEN), TrayMenuAction::Open);
        assert_eq!(tray_menu_action(MENU_ID_REFRESH), TrayMenuAction::Refresh);
        assert_eq!(tray_menu_action(MENU_ID_QUIT), TrayMenuAction::Quit);
        assert_eq!(tray_menu_action("future-menu-item"), TrayMenuAction::Ignore);
    }

    #[test]
    fn tray_refresh_dispatches_every_registered_host_without_double_starting_running_rounds() {
        let (_data_dir, state) = state();
        let local = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local workstation".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "6".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create local host");
        let remote = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Remote workstation".to_owned(),
                kind: ContractHostKind::Ssh,
                machine_id_hash: "7".repeat(64),
                ssh_target: Some("ci@example.test".to_owned()),
                remote_data_dir: Some("/srv/opencode".to_owned()),
                enabled_sources: None,
            },
        )
        .expect("create remote host");

        {
            let mut scheduler = state.lock_scheduler().expect("lock scheduler");
            assert!(matches!(
                scheduler.trigger_manual(&SourceKey::opencode(&local.host_id), 100),
                agentlens_core::hostsource::TriggerOutcome::Started(_)
            ));
            assert!(matches!(
                scheduler.trigger_manual(&SourceKey::opencode(&remote.host_id), 200),
                agentlens_core::hostsource::TriggerOutcome::Started(_)
            ));
        }

        let outcomes = trigger_all_hosts(&state).expect("dispatch tray refresh");
        assert_eq!(outcomes.len(), 2);
        for (host_id, outcome) in outcomes {
            let expected_started_at = if host_id == local.host_id { 100 } else { 200 };
            assert_eq!(
                outcome.expect("already-running is a successful command result"),
                vec![TriggerRefreshResult::AlreadyRunning {
                    host_id,
                    source: agentlens_core::ingest::OPENCODE_SOURCE.to_owned(),
                    started_at_utc: expected_started_at,
                }]
            );
        }
    }

    #[test]
    fn selftest_poll_stops_on_success_and_does_not_call_after_an_expired_deadline() {
        let calls = std::cell::Cell::new(0_u32);
        assert!(poll_until(
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            }
        ));
        assert_eq!(
            calls.get(),
            3,
            "polling stops as soon as the condition is ready"
        );

        let expired_called = std::cell::Cell::new(false);
        assert!(!poll_until(
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            || {
                expired_called.set(true);
                true
            }
        ));
        assert!(!expired_called.get());
    }

    /// The settings view renders whatever `prices_set` rejects verbatim, so each rejection must
    /// carry an actionable message and must leave `prices.json` untouched.
    #[test]
    fn price_editor_rejections_carry_a_readable_message_and_write_nothing() {
        use crate::commands::{prices_get_impl, prices_set_impl};
        use crate::contract::IpcErrorCode;

        let (_data_dir, state) = state();
        let entry = |provider: &str, model: &str, input: f64| {
            serde_json::json!({
                "providerId": provider,
                "modelId": model,
                "inputPerMtok": input,
                "outputPerMtok": 1.0,
                "cacheReadPerMtok": 0.1,
                "cacheWritePerMtok": 1.0,
                "extra": {},
            })
        };
        let table = |entries: serde_json::Value| serde_json::json!({ "schemaVersion": 1, "entries": entries, "extra": {} });

        for (needle, payload) in [
            (
                "must not be negative",
                table(serde_json::json!([entry(
                    "kiro-auth",
                    "claude-opus-5-max",
                    -1.0
                )])),
            ),
            (
                "blank provider_id",
                table(serde_json::json!([entry("", "claude-opus-5-max", 3.0)])),
            ),
            (
                "must be unique",
                table(serde_json::json!([
                    entry("kiro-auth", "claude-opus-5-max", 3.0),
                    entry("kiro-auth", "claude-opus-5-max", 4.0),
                ])),
            ),
        ] {
            let error = prices_set_impl(&state, payload).expect_err("invalid table must fail");
            assert_eq!(error.code, IpcErrorCode::Pricing);
            assert!(
                error.message.contains(needle),
                "message {:?} should explain {needle:?}",
                error.message
            );
            assert!(prices_get_impl(&state)
                .expect("reload prices")
                .entries
                .is_empty());
        }
    }

    #[test]
    fn archive_location_is_published_into_app_settings() {
        let (_data_dir, state) = state();
        let expected = state
            .lock_archive()
            .expect("lock archive")
            .path()
            .display()
            .to_string();

        publish_archive_location(&state);

        let values = get_settings_impl(&state).expect("read settings").values;
        assert_eq!(values.get(SETTING_KEY_ARCHIVE_PATH), Some(&expected));
    }

    #[test]
    fn persisted_intervals_are_applied_and_floored_at_boot() {
        let (_data_dir, state) = state();
        let host = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local workstation".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "b".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create local host");
        let remote = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Remote workstation".to_owned(),
                kind: ContractHostKind::Ssh,
                machine_id_hash: "c".repeat(64),
                ssh_target: Some("ci@example.test".to_owned()),
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create remote host");

        set_settings_impl(
            &state,
            settings(&[
                (SETTING_KEY_LOCAL_INTERVAL_MS, "900000"),
                (SETTING_KEY_REMOTE_INTERVAL_MS, "1200000"),
                (SETTING_KEY_AUTO_REFRESH_ENABLED, "true"),
            ]),
        )
        .expect("persist local interval");
        apply_refresh_intervals(&state);
        {
            let scheduler = state.lock_scheduler().expect("lock scheduler");
            assert_eq!(
                scheduler.interval_ms(&SourceKey::opencode(&host.host_id)),
                Some(900_000)
            );
            assert_eq!(
                scheduler.interval_ms(&SourceKey::opencode(&remote.host_id)),
                Some(1_200_000)
            );
            assert_eq!(
                scheduler
                    .status(&SourceKey::opencode(&host.host_id))
                    .expect("local status")
                    .trigger,
                agentlens_core::hostsource::TriggerMode::Auto
            );
            assert_eq!(
                scheduler
                    .status(&SourceKey::opencode(&remote.host_id))
                    .expect("remote status")
                    .trigger,
                agentlens_core::hostsource::TriggerMode::Auto
            );
        }

        set_settings_impl(
            &state,
            settings(&[(SETTING_KEY_AUTO_REFRESH_ENABLED, "false")]),
        )
        .expect("disable automatic refresh");
        apply_refresh_intervals(&state);
        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .status(&SourceKey::opencode(&remote.host_id))
                .expect("remote status")
                .trigger,
            TriggerMode::Manual
        );

        // A configured 60 s is refused by the write path, so it can never reach the scheduler.
        let rejected = set_settings_impl(
            &state,
            settings(&[(SETTING_KEY_LOCAL_INTERVAL_MS, "60000")]),
        )
        .expect_err("a sub-floor interval must be rejected, not clamped");
        assert_eq!(rejected.code, crate::contract::IpcErrorCode::InvalidInput);
        apply_refresh_intervals(&state);
        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .interval_ms(&SourceKey::opencode(&host.host_id)),
            Some(900_000),
            "a rejected write must leave the previously applied interval in force"
        );

        // A value persisted by an older build with the previous 5-minute floor is clamped on read
        // rather than blocking startup: `resolve_interval_ms` is the compatibility path.
        state
            .lock_archive()
            .expect("lock archive")
            .connection()
            .execute(
                "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, '300000')",
                [SETTING_KEY_LOCAL_INTERVAL_MS],
            )
            .expect("simulate a legacy stored interval");
        apply_refresh_intervals(&state);
        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .interval_ms(&SourceKey::opencode(&host.host_id)),
            Some(MIN_REFRESH_INTERVAL_MS)
        );
    }

    #[test]
    fn persisted_interval_read_failures_leave_existing_schedules_untouched() {
        let (_data_dir, state) = state();
        let host = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local workstation".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "8".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create local host");
        let before = state
            .lock_scheduler()
            .expect("lock scheduler")
            .interval_ms(&SourceKey::opencode(&host.host_id));
        state
            .lock_archive()
            .expect("lock archive")
            .connection()
            .execute_batch("DROP TABLE app_settings")
            .expect("remove settings table");

        apply_refresh_intervals(&state);

        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .interval_ms(&SourceKey::opencode(&host.host_id)),
            before,
            "a settings read failure must not partially rewrite schedules"
        );
    }

    #[test]
    fn archive_location_publication_is_best_effort_when_the_settings_table_is_unavailable() {
        let (_data_dir, state) = state();
        state
            .lock_archive()
            .expect("lock archive")
            .connection()
            .execute_batch("DROP TABLE app_settings")
            .expect("remove settings table");

        let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish_archive_location(&state);
        }));

        assert!(
            publication.is_ok(),
            "desktop startup must survive this best-effort write"
        );
        assert!(
            state.lock_archive().is_ok(),
            "the archive remains usable afterwards"
        );
    }

    #[test]
    fn shell_settings_hooks_return_cleanly_when_state_locks_are_poisoned() {
        let (_data_dir, archive_state) = state();
        let archive_poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = archive_state.archive.lock().expect("lock archive");
            panic!("poison archive for shell hook");
        }));
        assert!(archive_poison.is_err());
        let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish_archive_location(&archive_state);
        }));
        assert!(publication.is_ok(), "archive publication is best-effort");

        let (_data_dir, scheduler_state) = state();
        let scheduler_poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = scheduler_state.scheduler.lock().expect("lock scheduler");
            panic!("poison scheduler for shell hook");
        }));
        assert!(scheduler_poison.is_err());
        let application = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply_refresh_intervals(&scheduler_state);
        }));
        assert!(
            application.is_ok(),
            "startup must survive a poisoned scheduler"
        );

        let refresh_error = trigger_all_hosts(&scheduler_state)
            .expect_err("tray refresh must expose the poisoned scheduler");
        assert_eq!(refresh_error.code, crate::contract::IpcErrorCode::Internal);
    }
}
