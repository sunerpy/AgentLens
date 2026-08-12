use std::sync::Mutex;

use agentlens_core::archive::read_app_settings;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::contract::{IpcError, IpcErrorCode, UpdateMetadata, UpdateProgress};
use crate::state::AppState;

pub const SETTING_KEY_AUTO_UPDATE_ENABLED: &str = "update.autoInstallEnabled";

/// A checked update is retained in Rust so the frontend can never bypass the persisted install
/// policy by acquiring the updater plugin's resource id directly.
#[derive(Default)]
pub struct PendingUpdateState {
    update: Mutex<Option<Update>>,
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

    let metadata = update
        .as_ref()
        .map(metadata_from_update)
        .unwrap_or_else(|| current_metadata(current_version));
    *app.state::<PendingUpdateState>()
        .update
        .lock()
        .map_err(|_| IpcError::new(IpcErrorCode::Internal, "pending update lock is poisoned"))? =
        update;
    Ok(metadata)
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
    if !cfg!(target_os = "windows") {
        return Err(IpcError::new(
            IpcErrorCode::Conflict,
            "当前平台仅支持提示更新，请从发布页下载安装包",
        )
        .with_field("platform", std::env::consts::OS));
    }

    let update = app
        .state::<PendingUpdateState>()
        .update
        .lock()
        .map_err(|_| IpcError::new(IpcErrorCode::Internal, "pending update lock is poisoned"))?
        .clone()
        .ok_or_else(|| IpcError::new(IpcErrorCode::NotFound, "没有可安装的已检查更新"))?;

    let _ = on_event.send(UpdateProgress::Started);
    let mut downloaded = 0_u64;
    let progress = on_event.clone();
    update
        .download_and_install(
            move |chunk_length, total| {
                downloaded = downloaded.saturating_add(chunk_length as u64);
                if let Err(error) = progress.send(UpdateProgress::Downloading { downloaded, total })
                {
                    tracing::debug!(%error, "unable to send updater download progress");
                }
            },
            move || {
                if let Err(error) = on_event.send(UpdateProgress::Downloaded) {
                    tracing::debug!(%error, "unable to send updater download completion");
                }
            },
        )
        .await
        .map_err(|error| updater_error("无法下载或安装更新", error))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;

    use super::{
        ensure_auto_update_enabled, metadata_from_parts, resolve_auto_update_enabled,
        SETTING_KEY_AUTO_UPDATE_ENABLED,
    };
    use crate::commands::set_settings_impl;
    use crate::contract::{AppSettings, IpcErrorCode};
    use crate::state::AppState;

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create updater test data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open updater test state");
        (data_dir, state)
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
}
