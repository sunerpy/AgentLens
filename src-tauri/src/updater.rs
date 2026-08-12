use std::sync::Mutex;

use agentlens_core::archive::read_app_settings;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::contract::{IpcError, IpcErrorCode, UpdateMetadata, UpdateProgress};
use crate::state::AppState;

pub const SETTING_KEY_AUTO_UPDATE_ENABLED: &str = "update.autoInstallEnabled";

const PENDING_UPDATE_LOCK_ERROR: &str = "pending update lock is poisoned";

/// A checked update is retained in Rust so the frontend can never bypass the persisted install
/// policy by acquiring the updater plugin's resource id directly.
#[derive(Default)]
pub struct PendingUpdateState {
    update: Mutex<Option<Update>>,
}

impl PendingUpdateState {
    fn replace(&self, update: Option<Update>) -> Result<(), IpcError> {
        *self
            .update
            .lock()
            .map_err(|_| IpcError::new(IpcErrorCode::Internal, PENDING_UPDATE_LOCK_ERROR))? =
            update;
        Ok(())
    }

    #[cfg(any(target_os = "windows", test))]
    fn pending(&self) -> Result<Update, IpcError> {
        self.update
            .lock()
            .map_err(|_| IpcError::new(IpcErrorCode::Internal, PENDING_UPDATE_LOCK_ERROR))?
            .clone()
            .ok_or_else(|| IpcError::new(IpcErrorCode::NotFound, "没有可安装的已检查更新"))
    }
}

pub fn resolve_auto_update_enabled(raw: Option<&String>) -> bool {
    raw.is_none_or(|value| !matches!(value.trim(), "false" | "0" | "off" | "no"))
}

pub(crate) fn ensure_auto_update_enabled(state: &AppState) -> Result<(), IpcError> {
    let archive = state.lock_archive()?;
    let values = read_app_settings(archive.connection())
        .map_err(|error| IpcError::new(IpcErrorCode::Database, error.to_string()))?;
    if resolve_auto_update_enabled(values.get(SETTING_KEY_AUTO_UPDATE_ENABLED)) {
        Ok(())
    } else {
        Err(
            IpcError::new(IpcErrorCode::Conflict, "自动安装更新已在设置中关闭")
                .with_field("settingKey", SETTING_KEY_AUTO_UPDATE_ENABLED),
        )
    }
}

fn updater_error(context: &str, error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Internal, format!("{context}: {error}"))
}

fn ensure_auto_install_supported(auto_install_supported: bool) -> Result<(), IpcError> {
    if auto_install_supported {
        Ok(())
    } else {
        Err(IpcError::new(
            IpcErrorCode::Conflict,
            "当前平台仅支持提示更新，请从发布页下载安装包",
        )
        .with_field("platform", std::env::consts::OS))
    }
}

pub(crate) fn metadata_from_parts(
    current_version: impl Into<String>,
    version: impl Into<String>,
    date: Option<String>,
    body: Option<String>,
    auto_install_supported: bool,
) -> UpdateMetadata {
    UpdateMetadata {
        current_version: current_version.into(),
        version: Some(version.into()),
        date,
        body,
        auto_install_supported,
    }
}

fn current_metadata(current_version: impl Into<String>) -> UpdateMetadata {
    UpdateMetadata {
        current_version: current_version.into(),
        version: None,
        date: None,
        body: None,
        auto_install_supported: cfg!(target_os = "windows"),
    }
}

fn checked_metadata(current_version: String, update: Option<&Update>) -> UpdateMetadata {
    update
        .map(metadata_from_update)
        .unwrap_or_else(|| current_metadata(current_version))
}

fn metadata_from_update(update: &Update) -> UpdateMetadata {
    metadata_from_parts(
        update.current_version.clone(),
        update.version.clone(),
        update.date.map(|value| value.to_string()),
        update.body.clone(),
        cfg!(target_os = "windows"),
    )
}

#[tauri::command]
pub async fn updater_check<R: Runtime>(app: AppHandle<R>) -> Result<UpdateMetadata, IpcError> {
    let current_version = app.package_info().version.to_string();
    let update = app
        .updater()
        .map_err(|error| updater_error("无法初始化更新检查", error))?
        .check()
        .await
        .map_err(|error| updater_error("无法检查更新", error))?;

    let metadata = checked_metadata(current_version, update.as_ref());
    app.state::<PendingUpdateState>().replace(update)?;
    Ok(metadata)
}

#[cfg(any(target_os = "windows", test))]
fn next_download_progress(
    downloaded: &mut u64,
    chunk_length: usize,
    total: Option<u64>,
) -> UpdateProgress {
    *downloaded = downloaded.saturating_add(chunk_length as u64);
    UpdateProgress::Downloading {
        downloaded: *downloaded,
        total,
    }
}

#[cfg(any(target_os = "windows", test))]
fn send_progress(on_event: &Channel<UpdateProgress>, event: UpdateProgress) {
    if let Err(error) = on_event.send(event) {
        tracing::debug!(%error, "unable to send updater progress");
    }
}

#[cfg(target_os = "windows")]
async fn download_and_install(
    update: Update,
    on_event: Channel<UpdateProgress>,
) -> Result<(), IpcError> {
    send_progress(&on_event, UpdateProgress::Started);
    let mut downloaded = 0_u64;
    let progress = on_event.clone();
    update
        .download_and_install(
            move |chunk_length, total| {
                let event = next_download_progress(&mut downloaded, chunk_length, total);
                send_progress(&progress, event);
            },
            move || send_progress(&on_event, UpdateProgress::Downloaded),
        )
        .await
        .map_err(|error| updater_error("无法下载或安装更新", error))
}

/// The persisted switch is re-read immediately before download so turning it off after a check
/// cannot leave a stale frontend button capable of installing anyway.
#[tauri::command]
pub async fn updater_install<R: Runtime>(
    app: AppHandle<R>,
    on_event: Channel<UpdateProgress>,
) -> Result<(), IpcError> {
    {
        let state = app.state::<AppState>();
        ensure_auto_update_enabled(&state)?;
    }
    #[cfg(target_os = "windows")]
    {
        ensure_auto_install_supported(true)?;
        let update = app.state::<PendingUpdateState>().pending()?;
        download_and_install(update, on_event).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = on_event;
        ensure_auto_install_supported(false)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use serde_json::json;
    use tauri::{ipc::Channel, Manager};
    use tempfile::TempDir;

    use super::{
        current_metadata, ensure_auto_install_supported, ensure_auto_update_enabled,
        metadata_from_parts, next_download_progress, resolve_auto_update_enabled, send_progress,
        updater_check, updater_error, updater_install, PendingUpdateState,
        SETTING_KEY_AUTO_UPDATE_ENABLED,
    };
    use crate::commands::set_settings_impl;
    use crate::contract::{AppSettings, IpcErrorCode, UpdateProgress};
    use crate::state::AppState;

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create updater test data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open updater test state");
        (data_dir, state)
    }

    fn channel() -> Channel<UpdateProgress> {
        Channel::new(|_| Ok(()))
    }

    fn mock_app(
        updater_endpoint: Option<&str>,
    ) -> (TempDir, tauri::AppHandle<tauri::test::MockRuntime>) {
        let (data_dir, state) = state();
        let mut context: tauri::Context<tauri::test::MockRuntime> =
            tauri::test::mock_context(tauri::test::noop_assets());
        let mut builder = tauri::test::mock_builder()
            .manage(state)
            .manage(PendingUpdateState::default());
        if let Some(endpoint) = updater_endpoint {
            context.config_mut().plugins.0.insert(
                "updater".to_owned(),
                json!({
                    "dangerousInsecureTransportProtocol": true,
                    "endpoints": [endpoint],
                    "pubkey": "test-public-key"
                }),
            );
            builder = builder.plugin(
                tauri_plugin_updater::Builder::new()
                    .target("test-target")
                    .build(),
            );
        }
        let app = builder.build(context).expect("build updater mock app");
        let handle = app.handle().clone();
        (data_dir, handle)
    }

    fn serve_release(version: &str, notes: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind updater test endpoint");
        let address = listener
            .local_addr()
            .expect("read updater endpoint address");
        let body = json!({
            "version": version,
            "notes": notes,
            "pub_date": "2026-08-12T03:00:00Z",
            "platforms": {
                "test-target": {
                    "url": format!("http://{address}/artifact"),
                    "signature": "test-signature"
                }
            }
        })
        .to_string();
        let endpoint = format!("http://{address}/latest.json");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept updater request");
            let mut request = [0_u8; 2048];
            let bytes = stream.read(&mut request).expect("read updater request");
            assert!(String::from_utf8_lossy(&request[..bytes]).starts_with("GET /latest.json "));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write updater response");
        });
        (endpoint, server)
    }

    #[test]
    fn auto_update_defaults_to_enabled_and_recognizes_disabled_spellings() {
        assert!(resolve_auto_update_enabled(None));
        assert!(resolve_auto_update_enabled(Some(&"true".to_owned())));
        assert!(!resolve_auto_update_enabled(Some(&"false".to_owned())));
        assert!(!resolve_auto_update_enabled(Some(&"0".to_owned())));
        assert!(!resolve_auto_update_enabled(Some(&"off".to_owned())));
        assert!(!resolve_auto_update_enabled(Some(&"no".to_owned())));
    }

    #[test]
    fn install_policy_reloads_the_persisted_switch_and_rejects_disabled_updates() {
        let (_data_dir, state) = state();
        let mut values = BTreeMap::new();
        values.insert(
            SETTING_KEY_AUTO_UPDATE_ENABLED.to_owned(),
            "false".to_owned(),
        );
        set_settings_impl(&state, AppSettings { values }).expect("persist disabled updater switch");

        let error =
            ensure_auto_update_enabled(&state).expect_err("disabled updater must not install");

        assert_eq!(error.code, IpcErrorCode::Conflict);
        assert_eq!(
            error.fields.get("settingKey").map(String::as_str),
            Some(SETTING_KEY_AUTO_UPDATE_ENABLED)
        );
    }

    #[test]
    fn install_policy_allows_the_default_enabled_setting() {
        let (_data_dir, state) = state();

        ensure_auto_update_enabled(&state).expect("default updater switch must permit install");
    }

    #[test]
    fn platform_policy_allows_windows_and_identifies_an_unsupported_platform() {
        ensure_auto_install_supported(true).expect("Windows must support automatic install");

        let error = ensure_auto_install_supported(false)
            .expect_err("other platforms must direct users to the release page");
        assert_eq!(error.code, IpcErrorCode::Conflict);
        assert_eq!(
            error.fields.get("platform").map(String::as_str),
            Some(std::env::consts::OS)
        );
    }

    #[test]
    fn updater_errors_preserve_the_operation_and_source_message() {
        let error = updater_error("无法检查更新", "endpoint returned 503");

        assert_eq!(error.code, IpcErrorCode::Internal);
        assert_eq!(error.message, "无法检查更新: endpoint returned 503");
    }

    #[test]
    fn update_metadata_preserves_the_versions_release_notes_and_platform_policy() {
        let metadata = metadata_from_parts(
            "0.0.4",
            "0.0.5",
            Some("2026-08-12T03:00:00Z".to_owned()),
            Some("signed updater release".to_owned()),
            true,
        );

        assert_eq!(metadata.current_version, "0.0.4");
        assert_eq!(metadata.version.as_deref(), Some("0.0.5"));
        assert_eq!(metadata.date.as_deref(), Some("2026-08-12T03:00:00Z"));
        assert_eq!(metadata.body.as_deref(), Some("signed updater release"));
        assert!(metadata.auto_install_supported);
    }

    #[test]
    fn current_metadata_reports_that_no_new_version_was_found() {
        let metadata = current_metadata("0.0.5");

        assert_eq!(metadata.current_version, "0.0.5");
        assert_eq!(metadata.version, None);
        assert_eq!(metadata.date, None);
        assert_eq!(metadata.body, None);
        assert_eq!(metadata.auto_install_supported, cfg!(target_os = "windows"));
    }

    #[test]
    fn check_command_returns_current_metadata_and_clears_pending_when_versions_match() {
        let (endpoint, server) = serve_release("0.1.0", "already current");
        let (_data_dir, app) = mock_app(Some(&endpoint));

        let metadata = tauri::async_runtime::block_on(updater_check(app.clone()))
            .expect("same-version check must succeed");
        server.join().expect("updater endpoint must complete");

        assert_eq!(metadata.current_version, "0.1.0");
        assert_eq!(metadata.version, None);
        assert_eq!(
            app.state::<PendingUpdateState>()
                .pending()
                .err()
                .map(|error| error.code),
            Some(IpcErrorCode::NotFound)
        );
    }

    #[test]
    fn check_command_preserves_available_release_metadata_and_pending_update() {
        let (endpoint, server) = serve_release("0.2.0", "signed updater release");
        let (_data_dir, app) = mock_app(Some(&endpoint));

        let metadata = tauri::async_runtime::block_on(updater_check(app.clone()))
            .expect("new-version check must succeed");
        server.join().expect("updater endpoint must complete");

        assert_eq!(metadata.current_version, "0.1.0");
        assert_eq!(metadata.version.as_deref(), Some("0.2.0"));
        assert_eq!(
            metadata.date.as_deref(),
            Some("2026-08-12 3:00:00.0 +00:00:00")
        );
        assert_eq!(metadata.body.as_deref(), Some("signed updater release"));
        let pending = app
            .state::<PendingUpdateState>()
            .pending()
            .expect("available release must remain pending");
        assert_eq!(pending.version, "0.2.0");
    }

    #[test]
    fn install_command_reloads_the_disabled_switch_before_platform_admission() {
        let (_data_dir, app) = mock_app(None);
        let mut values = BTreeMap::new();
        values.insert(
            SETTING_KEY_AUTO_UPDATE_ENABLED.to_owned(),
            "false".to_owned(),
        );
        set_settings_impl(&app.state::<AppState>(), AppSettings { values })
            .expect("persist disabled updater switch");

        let error = tauri::async_runtime::block_on(updater_install(app, channel()))
            .expect_err("disabled updater must reject the command");

        assert_eq!(error.code, IpcErrorCode::Conflict);
        assert_eq!(
            error.fields.get("settingKey").map(String::as_str),
            Some(SETTING_KEY_AUTO_UPDATE_ENABLED)
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn install_command_rejects_automatic_install_on_an_unsupported_platform() {
        let (_data_dir, app) = mock_app(None);

        let error = tauri::async_runtime::block_on(updater_install(app, channel()))
            .expect_err("non-Windows updater must only offer release-page guidance");

        assert_eq!(error.code, IpcErrorCode::Conflict);
        assert_eq!(
            error.fields.get("platform").map(String::as_str),
            Some(std::env::consts::OS)
        );
    }

    #[test]
    fn pending_state_reports_not_found_before_a_successful_check() {
        let state = PendingUpdateState::default();

        state
            .replace(None)
            .expect("empty pending state must be writable");
        let error = state
            .pending()
            .err()
            .expect("install requires a previously checked update");
        assert_eq!(error.code, IpcErrorCode::NotFound);
        assert_eq!(error.message, "没有可安装的已检查更新");
    }

    #[test]
    fn download_progress_accumulates_chunks_and_preserves_the_optional_total() {
        let mut downloaded = u64::MAX - 2;

        assert_eq!(
            next_download_progress(&mut downloaded, 2, Some(99)),
            UpdateProgress::Downloading {
                downloaded: u64::MAX,
                total: Some(99),
            }
        );
        assert_eq!(
            next_download_progress(&mut downloaded, 1, None),
            UpdateProgress::Downloading {
                downloaded: u64::MAX,
                total: None,
            }
        );
    }

    #[test]
    fn progress_sender_delivers_the_exact_event_to_the_channel() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&messages);
        let channel = Channel::new(move |body| {
            captured.lock().expect("capture progress body").push(body);
            Ok(())
        });

        send_progress(&channel, UpdateProgress::Downloaded);

        let bodies = messages.lock().expect("read captured progress body");
        assert_eq!(bodies.len(), 1);
        let event: UpdateProgress = bodies[0]
            .clone()
            .deserialize()
            .expect("progress event must be valid JSON IPC");
        assert_eq!(event, UpdateProgress::Downloaded);
    }

    #[test]
    fn progress_sender_tolerates_a_closed_frontend_channel() {
        let callback_ran = Arc::new(Mutex::new(false));
        let observed = Arc::clone(&callback_ran);
        let channel = Channel::new(move |_| {
            *observed.lock().expect("record failed channel callback") = true;
            Err(tauri::Error::FailedToReceiveMessage)
        });

        send_progress(&channel, UpdateProgress::Started);

        assert!(*callback_ran.lock().expect("read failed channel callback"));
    }
}
