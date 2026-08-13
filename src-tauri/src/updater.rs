use std::{collections::BTreeMap, sync::Mutex, time::Duration};

use agentlens_core::archive::read_app_settings;
use reqwest::Url;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Error as UpdaterError, Update, Updater, UpdaterExt};

use crate::contract::{IpcError, IpcErrorCode, UpdateMetadata, UpdateProgress};
use crate::state::AppState;

pub const SETTING_KEY_AUTO_UPDATE_ENABLED: &str = "update.autoInstallEnabled";
pub const SETTING_KEY_UPDATE_PROXY_URL: &str = "update.proxyUrl";

const PENDING_UPDATE_LOCK_ERROR: &str = "pending update lock is poisoned";
const UPDATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
enum UpdateProxy {
    /// Let reqwest resolve HTTP_PROXY / HTTPS_PROXY / ALL_PROXY / NO_PROXY and, when the
    /// `system-proxy` feature is enabled, the operating system proxy configuration.
    System,
    Custom(Url),
}

impl UpdateProxy {
    fn parse(raw: Option<&str>) -> Result<Self, IpcError> {
        let raw = raw.unwrap_or_default().trim();
        if raw.is_empty() {
            return Ok(Self::System);
        }
        if !raw.contains("://") {
            return Err(IpcError::invalid_input(
                SETTING_KEY_UPDATE_PROXY_URL,
                "更新代理必须以 http://、https:// 或 socks5:// 开头",
            ));
        }

        let url = Url::parse(raw).map_err(|error| {
            IpcError::invalid_input(
                SETTING_KEY_UPDATE_PROXY_URL,
                format!("更新代理必须是完整 URL（支持 http://、https://、socks5://）：{error}"),
            )
        })?;
        if !matches!(url.scheme(), "http" | "https" | "socks5") {
            return Err(IpcError::invalid_input(
                SETTING_KEY_UPDATE_PROXY_URL,
                format!(
                    "更新代理协议不受支持：{}；仅支持 http://、https://、socks5://",
                    url.scheme()
                ),
            ));
        }
        if url.host_str().is_none() {
            return Err(IpcError::invalid_input(
                SETTING_KEY_UPDATE_PROXY_URL,
                "更新代理 URL 必须包含主机名或 IP 地址",
            ));
        }
        if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
            return Err(IpcError::invalid_input(
                SETTING_KEY_UPDATE_PROXY_URL,
                "更新代理 URL 只能包含协议、认证信息、主机和端口，不能包含路径、查询参数或片段",
            ));
        }

        Ok(Self::Custom(url))
    }

    fn mode(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Custom(_) => "custom",
        }
    }

    fn diagnostic_label(&self) -> String {
        match self {
            Self::System => "系统代理 / 环境变量（未配置时直连）".to_owned(),
            Self::Custom(url) => {
                let port = url
                    .port()
                    .map_or_else(String::new, |port| format!(":{port}"));
                format!(
                    "自定义代理 {}://{}{}（认证信息已隐藏）",
                    url.scheme(),
                    url.host_str().unwrap_or("<unknown>"),
                    port
                )
            }
        }
    }
}

pub(crate) fn validate_update_settings(values: &BTreeMap<String, String>) -> Result<(), IpcError> {
    UpdateProxy::parse(values.get(SETTING_KEY_UPDATE_PROXY_URL).map(String::as_str))?;
    Ok(())
}

fn resolve_update_proxy(state: &AppState) -> Result<UpdateProxy, IpcError> {
    let archive = state.lock_archive()?;
    let values = read_app_settings(archive.connection())
        .map_err(|error| IpcError::new(IpcErrorCode::Database, error.to_string()))?;
    UpdateProxy::parse(values.get(SETTING_KEY_UPDATE_PROXY_URL).map(String::as_str))
}

fn configured_updater<R: Runtime>(
    app: &AppHandle<R>,
    proxy: &UpdateProxy,
) -> Result<Updater, IpcError> {
    let builder = app.updater_builder().timeout(UPDATE_REQUEST_TIMEOUT);
    let builder = match proxy {
        UpdateProxy::System => builder,
        UpdateProxy::Custom(url) => builder.proxy(url.clone()),
    };
    builder
        .build()
        .map_err(|error| updater_error("无法初始化更新检查", error, proxy))
}

#[derive(Clone)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
struct PendingUpdate {
    update: Update,
    proxy: UpdateProxy,
}

/// A checked update is retained in Rust so the frontend can never bypass the persisted install
/// policy by acquiring the updater plugin's resource id directly.
#[derive(Default)]
pub struct PendingUpdateState {
    update: Mutex<Option<PendingUpdate>>,
}

impl PendingUpdateState {
    fn replace(&self, update: Option<Update>, proxy: UpdateProxy) -> Result<(), IpcError> {
        *self
            .update
            .lock()
            .map_err(|_| IpcError::new(IpcErrorCode::Internal, PENDING_UPDATE_LOCK_ERROR))? =
            update.map(|update| PendingUpdate { update, proxy });
        Ok(())
    }

    #[cfg(any(target_os = "windows", test))]
    fn pending(&self, proxy: &UpdateProxy) -> Result<Update, IpcError> {
        let pending = self
            .update
            .lock()
            .map_err(|_| IpcError::new(IpcErrorCode::Internal, PENDING_UPDATE_LOCK_ERROR))?
            .clone()
            .ok_or_else(|| IpcError::new(IpcErrorCode::NotFound, "没有可安装的已检查更新"))?;
        if &pending.proxy != proxy {
            return Err(IpcError::new(
                IpcErrorCode::Conflict,
                "更新代理设置已变化，请重新检查更新后再安装",
            )
            .with_field("settingKey", SETTING_KEY_UPDATE_PROXY_URL));
        }
        Ok(pending.update)
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

fn updater_error(context: &str, error: UpdaterError, proxy: &UpdateProxy) -> IpcError {
    let source = error.to_string();
    if matches!(&error, UpdaterError::Reqwest(_) | UpdaterError::Network(_)) {
        let proxy_label = proxy.diagnostic_label();
        IpcError::new(
            IpcErrorCode::Network,
            format!(
                "{context}：网络请求失败。当前代理模式：{proxy_label}。请检查网络或代理是否可用，也可在“设置 → 应用更新”中配置代理。底层错误：{source}"
            ),
        )
        .with_field("proxyMode", proxy.mode())
        .with_field("proxy", proxy_label)
        .with_field("cause", source)
    } else {
        IpcError::new(IpcErrorCode::Internal, format!("{context}: {source}"))
    }
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
    let proxy = resolve_update_proxy(&app.state::<AppState>())?;
    let update = configured_updater(&app, &proxy)?
        .check()
        .await
        .map_err(|error| updater_error("无法检查更新", error, &proxy))?;

    let metadata = checked_metadata(current_version, update.as_ref());
    app.state::<PendingUpdateState>().replace(update, proxy)?;
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
    proxy: &UpdateProxy,
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
        .map_err(|error| updater_error("无法下载或安装更新", error, proxy))
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
        let proxy = resolve_update_proxy(&app.state::<AppState>())?;
        let update = app.state::<PendingUpdateState>().pending(&proxy)?;
        download_and_install(update, &proxy, on_event).await
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
        time::Duration,
    };

    use agentlens_core::archive::read_app_settings;
    use serde_json::json;
    use tauri::{ipc::Channel, Manager};
    use tauri_plugin_updater::Error as UpdaterError;
    use tempfile::TempDir;

    use super::{
        current_metadata, ensure_auto_install_supported, ensure_auto_update_enabled,
        metadata_from_parts, next_download_progress, resolve_auto_update_enabled, send_progress,
        updater_check, updater_error, updater_install, validate_update_settings,
        PendingUpdateState, UpdateProxy, SETTING_KEY_AUTO_UPDATE_ENABLED,
        SETTING_KEY_UPDATE_PROXY_URL,
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

    fn serve_proxy_release(
        version: &str,
    ) -> (
        String,
        String,
        std::sync::mpsc::Receiver<String>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind updater proxy");
        let address = listener.local_addr().expect("read updater proxy address");
        let body = json!({
            "version": version,
            "notes": "proxied release",
            "pub_date": "2026-08-12T03:00:00Z",
            "platforms": {
                "test-target": {
                    "url": "http://release.invalid/artifact",
                    "signature": "test-signature"
                }
            }
        })
        .to_string();
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            for response_body in [body.as_bytes(), b"signed artifact bytes"] {
                let (mut stream, _) = listener.accept().expect("accept updater proxy request");
                let mut request = [0_u8; 4096];
                let bytes = stream
                    .read(&mut request)
                    .expect("read updater proxy request");
                let request = String::from_utf8_lossy(&request[..bytes]);
                let request_line = request.lines().next().unwrap_or_default().to_owned();
                request_tx
                    .send(request_line)
                    .expect("record updater proxy request");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_body.len()
                )
                .expect("write updater proxy response headers");
                stream
                    .write_all(response_body)
                    .expect("write updater proxy response body");
            }
        });
        (
            "http://release.invalid/latest.json".to_owned(),
            format!("http://{address}"),
            request_rx,
            server,
        )
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
    fn updater_errors_distinguish_network_failures_and_preserve_diagnostics() {
        let error = updater_error(
            "无法检查更新",
            UpdaterError::Network("endpoint returned 503".to_owned()),
            &UpdateProxy::System,
        );

        assert_eq!(error.code, IpcErrorCode::Network);
        assert!(error.message.contains("系统代理 / 环境变量"));
        assert!(error.message.contains("设置 → 应用更新"));
        assert!(error.message.contains("endpoint returned 503"));
        assert_eq!(
            error.fields.get("proxyMode").map(String::as_str),
            Some("system")
        );
        assert_eq!(
            error.fields.get("cause").map(String::as_str),
            Some("`endpoint returned 503`")
        );
    }

    #[test]
    fn updater_errors_keep_non_network_failures_internal() {
        let error = updater_error(
            "无法检查更新",
            UpdaterError::ReleaseNotFound,
            &UpdateProxy::System,
        );

        assert_eq!(error.code, IpcErrorCode::Internal);
        assert!(error.message.contains("无法检查更新"));
        assert!(error
            .message
            .contains("Could not fetch a valid release JSON from the remote"));
    }

    #[test]
    fn network_diagnostics_never_expose_proxy_credentials() {
        let proxy = UpdateProxy::parse(Some("http://alice:secret@proxy.example:8080"))
            .expect("authenticated proxy must parse");

        let error = updater_error(
            "无法检查更新",
            UpdaterError::Network("connection refused".to_owned()),
            &proxy,
        );

        assert_eq!(error.code, IpcErrorCode::Network);
        assert!(error.message.contains("http://proxy.example:8080"));
        assert!(!error.message.contains("alice"));
        assert!(!error.message.contains("secret"));
        assert_eq!(
            error.fields.get("proxyMode").map(String::as_str),
            Some("custom")
        );
    }

    #[test]
    fn proxy_setting_accepts_supported_urls_and_rejects_invalid_values_before_persisting() {
        for raw in [
            "",
            "http://127.0.0.1:7890",
            "https://proxy.example:8443",
            "socks5://127.0.0.1:1080",
            "http://user:pass@proxy.example:8080",
        ] {
            let values =
                BTreeMap::from([(SETTING_KEY_UPDATE_PROXY_URL.to_owned(), raw.to_owned())]);
            validate_update_settings(&values).expect("supported proxy URL must be accepted");
        }

        for raw in [
            "proxy.example:8080",
            "ftp://proxy.example:21",
            "http://",
            "http://proxy.example:8080/path",
        ] {
            let values =
                BTreeMap::from([(SETTING_KEY_UPDATE_PROXY_URL.to_owned(), raw.to_owned())]);
            let error = validate_update_settings(&values)
                .expect_err("invalid proxy URL must be rejected before persistence");
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
            assert_eq!(
                error.fields.get("field").map(String::as_str),
                Some(SETTING_KEY_UPDATE_PROXY_URL)
            );
        }
    }

    #[test]
    fn settings_command_rejects_an_invalid_proxy_url_without_writing_it() {
        let (_data_dir, state) = state();
        let values = BTreeMap::from([(
            SETTING_KEY_UPDATE_PROXY_URL.to_owned(),
            "not-a-proxy-url".to_owned(),
        )]);

        let error = set_settings_impl(&state, AppSettings { values })
            .expect_err("settings command must reject an invalid proxy URL");

        assert_eq!(error.code, IpcErrorCode::InvalidInput);
        assert_eq!(
            error.fields.get("field").map(String::as_str),
            Some(SETTING_KEY_UPDATE_PROXY_URL)
        );
        let persisted = read_app_settings(state.lock_archive().unwrap().connection()).unwrap();
        assert!(!persisted.contains_key(SETTING_KEY_UPDATE_PROXY_URL));
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
                .pending(&UpdateProxy::System)
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
            .pending(&UpdateProxy::System)
            .expect("available release must remain pending");
        assert_eq!(pending.version, "0.2.0");
    }

    #[test]
    fn pending_update_rejects_install_after_the_proxy_setting_changes() {
        let (endpoint, server) = serve_release("0.2.0", "signed updater release");
        let (_data_dir, app) = mock_app(Some(&endpoint));
        tauri::async_runtime::block_on(updater_check(app.clone()))
            .expect("new-version check must succeed");
        server.join().expect("updater endpoint must complete");
        let changed_proxy =
            UpdateProxy::parse(Some("http://127.0.0.1:7890")).expect("parse changed proxy");

        let error = app
            .state::<PendingUpdateState>()
            .pending(&changed_proxy)
            .err()
            .expect("stale pending update must not use the previous proxy");

        assert_eq!(error.code, IpcErrorCode::Conflict);
        assert_eq!(
            error.fields.get("settingKey").map(String::as_str),
            Some(SETTING_KEY_UPDATE_PROXY_URL)
        );
    }

    #[test]
    fn check_and_download_route_through_the_configured_http_proxy() {
        let (endpoint, proxy_url, request_rx, server) = serve_proxy_release("0.2.0");
        let (_data_dir, app) = mock_app(Some(&endpoint));
        let values = BTreeMap::from([(SETTING_KEY_UPDATE_PROXY_URL.to_owned(), proxy_url.clone())]);
        set_settings_impl(&app.state::<AppState>(), AppSettings { values })
            .expect("persist updater proxy");

        let metadata = tauri::async_runtime::block_on(updater_check(app.clone()))
            .expect("proxied update check must succeed");
        let proxy = UpdateProxy::parse(Some(&proxy_url)).expect("parse persisted updater proxy");
        let update = app
            .state::<PendingUpdateState>()
            .pending(&proxy)
            .expect("checked update must remain pending");
        let download_error = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))
            .expect_err("fixture signature is intentionally invalid after the proxied download");
        assert!(
            matches!(
                download_error,
                UpdaterError::Minisign(_)
                    | UpdaterError::Base64(_)
                    | UpdaterError::SignatureUtf8(_)
            ),
            "download must reach signature verification after proxying: {download_error:?}"
        );

        let manifest_request = request_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("proxy must observe the updater manifest request");
        let artifact_request = request_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("proxy must observe the updater artifact request");
        server.join().expect("updater proxy must complete");

        println!(
            "proxy observed updater requests via {proxy_url}: {manifest_request} | {artifact_request}"
        );
        assert_eq!(metadata.version.as_deref(), Some("0.2.0"));
        assert_eq!(
            manifest_request,
            "GET http://release.invalid/latest.json HTTP/1.1"
        );
        assert_eq!(
            artifact_request,
            "GET http://release.invalid/artifact HTTP/1.1"
        );
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
            .replace(None, UpdateProxy::System)
            .expect("empty pending state must be writable");
        let error = state
            .pending(&UpdateProxy::System)
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
