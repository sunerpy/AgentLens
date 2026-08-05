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
    SourceSchedule, DEFAULT_LOCAL_MIN_INTERVAL_MS, DEFAULT_REMOTE_INTERVAL_MS,
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

/// Hard floor for any configured refresh interval (5 minutes), mirroring the UI clamp.
pub const MIN_REFRESH_INTERVAL_MS: u64 = DEFAULT_LOCAL_MIN_INTERVAL_MS;

const MENU_ID_OPEN: &str = "tray-open";
const MENU_ID_REFRESH: &str = "tray-refresh";
const MENU_ID_QUIT: &str = "tray-quit";

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
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_ID_OPEN => show_main_window(app),
            MENU_ID_REFRESH => refresh_all_hosts(app),
            MENU_ID_QUIT => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    println!("agentlens: tray icon installed (open / refresh / quit)");
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
            eprintln!("agentlens: failed to hide the main window: {error}");
            return;
        }
        println!("agentlens: main window hidden to tray (webview kept alive)");
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
    let Ok(host_ids) = state.lock_scheduler().map(|scheduler| scheduler.host_ids()) else {
        return;
    };
    for host_id in host_ids {
        if let Err(error) = state.trigger_refresh(&host_id) {
            eprintln!(
                "agentlens: tray refresh for {host_id} failed: {}",
                error.message
            );
        }
    }
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
            eprintln!("agentlens: archive unavailable: {}", error.message);
            return;
        }
    };
    let mut values = BTreeMap::new();
    values.insert(
        SETTING_KEY_ARCHIVE_PATH.to_owned(),
        archive.path().display().to_string(),
    );
    if let Err(error) = write_app_settings(archive.connection_mut(), &values) {
        eprintln!("agentlens: failed to publish the archive location: {error}");
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
pub fn apply_refresh_intervals(state: &AppState) {
    let settings = match state
        .lock_archive()
        .map_err(|error| error.message)
        .and_then(|archive| read_app_settings(archive.connection()).map_err(|e| e.to_string()))
    {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("agentlens: unable to read persisted refresh intervals: {error}");
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
    let Ok(mut scheduler) = state.lock_scheduler() else {
        return;
    };
    for status in scheduler.statuses() {
        let interval_ms = match status.kind {
            HostKind::Local => local_ms,
            HostKind::Ssh => remote_ms,
        };
        let schedule = SourceSchedule::for_kind(status.kind)
            .with_trigger(status.trigger)
            .with_min_interval_ms(interval_ms);
        if let Err(error) = scheduler.set_schedule(&status.host_id, schedule) {
            eprintln!(
                "agentlens: unable to apply interval to {}: {error}",
                status.host_id
            );
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
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    const STEP: Duration = Duration::from_millis(100);

    let poll = |deadline: Duration, mut ready: Box<dyn FnMut() -> bool>| -> bool {
        let started = Instant::now();
        while started.elapsed() < deadline {
            if ready() {
                return true;
            }
            sleep(STEP);
        }
        false
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::commands::{
        get_settings_impl, get_trend_impl, hosts_create_impl, set_settings_impl,
    };
    use crate::contract::{
        AggregateFilters, AppSettings, DateRange, Granularity, HostCreateInput,
        HostKind as ContractHostKind, WeekStart,
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

        assert_eq!(utc.len(), 2);
        assert_eq!(shanghai.len(), 2);
        assert_eq!(utc[0].bucket.start_utc_ms, JAN_1_UTC_MS);
        assert_eq!(utc[0].bucket.end_utc_ms, JAN_1_UTC_MS + 86_400_000);
        assert_eq!(
            shanghai[0].bucket.start_utc_ms,
            JAN_1_UTC_MS - EIGHT_HOURS_MS
        );
        assert_eq!(
            shanghai[0].bucket.end_utc_ms,
            JAN_1_UTC_MS + 86_400_000 - EIGHT_HOURS_MS
        );
        assert_ne!(utc[0].bucket.start_utc_ms, shanghai[0].bucket.start_utc_ms);
        assert_ne!(utc[1].bucket.start_utc_ms, shanghai[1].bucket.start_utc_ms);

        // Hour labels carry the numeric offset, so the timezone change is visible in the label
        // too, not only in the epoch boundaries.
        let utc_hours = persist_and_trend("UTC", Granularity::Hour, ("2026-01-01", "2026-01-02"));
        let shanghai_hours = persist_and_trend(
            "Asia/Shanghai",
            Granularity::Hour,
            ("2026-01-01", "2026-01-02"),
        );
        assert_eq!(utc_hours[0].bucket.label, "2026-01-01T00:00+00:00");
        assert_eq!(shanghai_hours[0].bucket.label, "2026-01-01T00:00+08:00");
    }

    #[test]
    fn refresh_interval_floor_rejects_anything_below_five_minutes() {
        let sixty_seconds = "60000".to_owned();
        assert_eq!(
            resolve_interval_ms(Some(&sixty_seconds), DEFAULT_LOCAL_MIN_INTERVAL_MS),
            300_000
        );
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
        let long = "450000".to_owned();
        assert_eq!(
            resolve_interval_ms(Some(&long), DEFAULT_LOCAL_MIN_INTERVAL_MS),
            450_000
        );
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
            },
        )
        .expect("create local host");

        set_settings_impl(
            &state,
            settings(&[(SETTING_KEY_LOCAL_INTERVAL_MS, "450000")]),
        )
        .expect("persist local interval");
        apply_refresh_intervals(&state);
        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .interval_ms(&host.host_id),
            Some(450_000)
        );

        // A configured 60 s must not reach the scheduler even if it somehow bypassed the UI.
        set_settings_impl(
            &state,
            settings(&[(SETTING_KEY_LOCAL_INTERVAL_MS, "60000")]),
        )
        .expect("persist sub-floor interval");
        apply_refresh_intervals(&state);
        assert_eq!(
            state
                .lock_scheduler()
                .expect("lock scheduler")
                .interval_ms(&host.host_id),
            Some(MIN_REFRESH_INTERVAL_MS)
        );
    }
}
