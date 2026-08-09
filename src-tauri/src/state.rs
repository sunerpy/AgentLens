use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use agentlens_core::archive::{default_archive_path, Archive};
use agentlens_core::host::{HostKind, HostRegistry};
use agentlens_core::hostsource::{
    execute_action, set_archive_busy_timeout, ClaudeCodeLocalSource, Clock, CodexLocalSource,
    HermesLocalSource, LocalHostSource, RefreshAction, RefreshScheduler, RoundReport, RoundResult,
    SourceKey, SourceRegistration, SshHostSource, SystemClock, TriggerOutcome,
    DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS,
};
use agentlens_core::ingest::OPENCODE_SOURCE;
use agentlens_core::pricing::{default_prices_path, PriceTable};
use agentlens_core::source::claude_code::CLAUDE_CODE_SOURCE;
use agentlens_core::source::codex::CODEX_SOURCE;
use agentlens_core::source::hermes::HERMES_SOURCE;
use agentlens_core::transport::ssh::{CollectorArtifacts, SshTransport, StdCommandRunner};

use tauri::{AppHandle, Emitter};

use crate::contract::{IpcError, IpcErrorCode, RefreshEvent, SourceStatus, TriggerRefreshResult};
use crate::credentials::{askpass_helper_path, ssh_authentication_for_host, OsKeyringStore};

/// Emitted to the webview after a refresh round commits new rows.
///
/// The frontend listens for it in `frontend/src/lib/archiveQueries.ts` and invalidates the
/// archive query family. Without this signal the scheduler and the dashboard cache are
/// disconnected: a round can commit 155k rows while the overview keeps serving its cached
/// pre-collection result (F3 DEFECT-2). Payload is the host id whose round committed.
///
/// The literal is duplicated in TypeScript on purpose — an event name is a wire string, not a
/// ts-rs DTO — so each side names the other in a comment.
pub const EVENT_ARCHIVE_COMMITTED: &str = "agentlens://archive-committed";

pub const EVENT_REFRESH_COMPLETED: &str = "agentlens://refresh-completed";

/// True when a finished round actually wrote something the aggregates can see.
///
/// A committed round that changed no records cannot move any aggregate, so announcing it would
/// make every idle scheduler tick refetch the whole dashboard for nothing.
fn round_changed_archive(report: &RoundReport) -> bool {
    match &report.result {
        RoundResult::Collected(summary) => summary.is_success() && summary.changed_records > 0,
        RoundResult::Failed { .. } => false,
    }
}

pub struct AppState {
    pub(crate) archive: Mutex<Archive>,
    pub(crate) scheduler: Arc<Mutex<RefreshScheduler>>,
    archive_path: PathBuf,
    prices_path: PathBuf,
    /// Filled once by [`crate::tray::install`]. `lib.rs` starts the refresh loop before the
    /// Tauri builder exists, so the handle cannot be a constructor argument; a round that
    /// finishes before it lands simply skips the notification.
    app_handle: Arc<OnceLock<AppHandle>>,
}

impl AppState {
    pub fn open_default() -> Result<Self, IpcError> {
        let archive_path = default_archive_path().map_err(database_error)?;
        let prices_path = default_prices_path().map_err(pricing_error)?;
        Self::open_paths(archive_path, prices_path)
    }

    #[cfg(test)]
    pub fn open_in_data_dir(data_dir: impl AsRef<std::path::Path>) -> Result<Self, IpcError> {
        let archive = Archive::open_in_data_dir(data_dir.as_ref()).map_err(database_error)?;
        let archive_path = archive.path().to_path_buf();
        let prices_path = agentlens_core::pricing::prices_path_in(data_dir);
        Self::from_archive(archive, archive_path, prices_path)
    }

    fn open_paths(archive_path: PathBuf, prices_path: PathBuf) -> Result<Self, IpcError> {
        let archive = Archive::open(&archive_path).map_err(database_error)?;
        Self::from_archive(archive, archive_path, prices_path)
    }

    fn from_archive(
        archive: Archive,
        archive_path: PathBuf,
        prices_path: PathBuf,
    ) -> Result<Self, IpcError> {
        set_archive_busy_timeout(&archive, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS)
            .map_err(refresh_error)?;
        let hosts = HostRegistry::new(archive.connection())
            .list()
            .map_err(host_error)?;
        let mut scheduler = RefreshScheduler::new();
        for host in &hosts {
            for registration in SourceRegistration::all_for_host(host) {
                scheduler.register(registration).map_err(refresh_error)?;
            }
        }
        Ok(Self {
            archive: Mutex::new(archive),
            scheduler: Arc::new(Mutex::new(scheduler)),
            archive_path,
            prices_path,
            app_handle: Arc::new(OnceLock::new()),
        })
    }

    pub(crate) fn lock_archive(&self) -> Result<MutexGuard<'_, Archive>, IpcError> {
        self.archive.lock().map_err(|_| {
            IpcError::new(
                IpcErrorCode::Internal,
                "archive state lock is poisoned by a previous failed operation",
            )
        })
    }

    pub(crate) fn lock_scheduler(&self) -> Result<MutexGuard<'_, RefreshScheduler>, IpcError> {
        self.scheduler.lock().map_err(|_| {
            IpcError::new(
                IpcErrorCode::Internal,
                "refresh scheduler lock is poisoned by a previous failed operation",
            )
        })
    }

    pub(crate) fn load_prices(&self) -> Result<PriceTable, IpcError> {
        PriceTable::load(&self.prices_path).map_err(pricing_error)
    }

    pub(crate) fn save_prices(&self, prices: &PriceTable) -> Result<(), IpcError> {
        prices.save(&self.prices_path).map_err(pricing_error)
    }

    pub(crate) fn tick_due(&self) -> Result<(), IpcError> {
        let now = SystemClock.now_utc_ms();
        let actions = self.lock_scheduler()?.tick(now);
        for action in actions {
            drop(self.spawn_action(action));
        }
        Ok(())
    }

    /// Publishes the Tauri handle the refresh loop needs to emit [`EVENT_ARCHIVE_COMMITTED`].
    ///
    /// Idempotent: a second call is ignored rather than replacing a live handle.
    pub(crate) fn attach_app_handle(&self, app: AppHandle) {
        let _ = self.app_handle.set(app);
    }

    pub fn start_refresh_loop(&self) {
        let archive_path = self.archive_path.clone();
        let scheduler = Arc::clone(&self.scheduler);
        let app_handle = Arc::clone(&self.app_handle);
        thread::spawn(move || loop {
            let Some(handles) = spawn_due_actions(
                &archive_path,
                &scheduler,
                &app_handle,
                SystemClock.now_utc_ms(),
            ) else {
                break;
            };
            drop(handles);
            thread::sleep(Duration::from_secs(1));
        });
    }

    /// Refreshes every source of one host, in the background.
    ///
    /// This is the "一键刷新" entry point: a host with OpenCode and Claude Code enabled produces one
    /// result per source, and each round runs on its own thread with its own archive handle, so a
    /// slow Claude Code walk does not delay the OpenCode round.
    pub(crate) fn trigger_refresh(
        &self,
        host_id: &str,
    ) -> Result<Vec<TriggerRefreshResult>, IpcError> {
        let mut results = Vec::new();
        for outcome in self.admit_manual_refresh(host_id)? {
            match outcome {
                TriggerOutcome::Started(action) => {
                    results.push(started_response(&action));
                    drop(self.spawn_action(action));
                }
                other => results.push(TriggerRefreshResult::try_from(other)?),
            }
        }
        Ok(results)
    }

    /// Runs one manually admitted round inline and reports its lifecycle to the command channel.
    ///
    /// The caller is already on Tauri's blocking pool. Keeping execution here, rather than
    /// spawning another detached thread, keeps the channel scoped to its invoke and guarantees
    /// that dropping the command argument closes the stream only after `Finished` was sent.
    pub(crate) fn trigger_refresh_with_events(
        &self,
        host_id: &str,
        mut on_event: impl FnMut(RefreshEvent),
    ) -> Result<Vec<TriggerRefreshResult>, IpcError> {
        let outcomes = self.admit_manual_refresh(host_id)?;
        let mut results = Vec::with_capacity(outcomes.len());
        let mut admitted = Vec::new();
        for outcome in outcomes {
            match outcome {
                TriggerOutcome::Started(action) => {
                    results.push(started_response(&action));
                    admitted.push(action);
                }
                other => results.push(TriggerRefreshResult::try_from(other)?),
            }
        }

        // Rounds run one after another rather than on worker threads: the caller is already on
        // Tauri's blocking pool and owns the channel, so finishing here is what guarantees every
        // `Finished` is sent before the channel argument is dropped and the stream closes.
        for action in admitted {
            let key = action.key();
            let started = self.refresh_status(&key)?.ok_or_else(|| {
                IpcError::new(
                    IpcErrorCode::Internal,
                    "refresh source disappeared immediately after admission",
                )
                .with_field("hostId", &key.host_id)
                .with_field("source", &key.source)
            })?;
            on_event(RefreshEvent::Started { status: started });

            self.execute_action(action);
            let status = self.refresh_status(&key)?;
            on_event(RefreshEvent::Finished {
                host_id: key.host_id,
                source: key.source,
                status,
            });
        }

        Ok(results)
    }

    /// Admits one manual round per source of `host_id`.
    ///
    /// An unregistered host yields an empty list, which the callers turn into the same
    /// `NotFound` the single-source version returned.
    fn admit_manual_refresh(&self, host_id: &str) -> Result<Vec<TriggerOutcome>, IpcError> {
        if host_id.trim().is_empty() {
            return Err(IpcError::invalid_input(
                "hostId",
                "host_id must not be empty",
            ));
        }
        let now = SystemClock.now_utc_ms();
        let mut scheduler = self.lock_scheduler()?;
        let keys = scheduler.keys_for_host(host_id);
        if keys.is_empty() {
            return Err(IpcError::not_found("host", host_id));
        }
        Ok(keys
            .iter()
            .map(|key| scheduler.trigger_manual(key, now))
            .collect())
    }

    fn refresh_status(&self, key: &SourceKey) -> Result<Option<SourceStatus>, IpcError> {
        Ok(self.lock_scheduler()?.status(key).map(Into::into))
    }

    pub(crate) fn register_host(&self, registration: SourceRegistration) -> Result<(), IpcError> {
        self.lock_scheduler()?
            .register(registration)
            .map_err(refresh_error)
    }

    /// Rebuilds every scheduler slot of one host from its stored record.
    ///
    /// Every slot of the host is dropped first, so disabling a source in the host editor also
    /// unschedules it instead of leaving a slot that keeps scanning a source the user turned off.
    pub(crate) fn replace_host_registrations(
        &self,
        registrations: Vec<SourceRegistration>,
        host_id: &str,
    ) -> Result<(), IpcError> {
        {
            let mut scheduler = self.lock_scheduler()?;
            scheduler.remove_host(host_id);
            for registration in registrations {
                scheduler.register(registration).map_err(refresh_error)?;
            }
        }
        crate::tray::apply_refresh_intervals(self);
        Ok(())
    }

    pub(crate) fn remove_host_registration(&self, host_id: &str) -> Result<(), IpcError> {
        self.lock_scheduler()?.remove_host(host_id);
        Ok(())
    }

    fn spawn_action(&self, action: RefreshAction) -> thread::JoinHandle<()> {
        spawn_action(
            self.archive_path.clone(),
            Arc::clone(&self.scheduler),
            Arc::clone(&self.app_handle),
            action,
        )
    }

    fn execute_action(&self, action: RefreshAction) {
        RefreshRuntime {
            archive_path: self.archive_path.clone(),
            scheduler: Arc::clone(&self.scheduler),
            app_handle: Arc::clone(&self.app_handle),
        }
        .execute(action);
    }
}

fn started_response(action: &RefreshAction) -> TriggerRefreshResult {
    TriggerRefreshResult::Started {
        host_id: action.host_id.clone(),
        source: action.source.clone(),
        started_at_utc: action.started_at_utc,
    }
}

fn spawn_action(
    archive_path: PathBuf,
    scheduler: Arc<Mutex<RefreshScheduler>>,
    app_handle: Arc<OnceLock<AppHandle>>,
    action: RefreshAction,
) -> thread::JoinHandle<()> {
    let runtime = RefreshRuntime {
        archive_path,
        scheduler,
        app_handle,
    };
    thread::spawn(move || runtime.execute(action))
}

fn spawn_due_actions(
    archive_path: &std::path::Path,
    scheduler: &Arc<Mutex<RefreshScheduler>>,
    app_handle: &Arc<OnceLock<AppHandle>>,
    now_utc_ms: i64,
) -> Option<Vec<thread::JoinHandle<()>>> {
    let actions = scheduler.lock().ok()?.tick(now_utc_ms);
    Some(
        actions
            .into_iter()
            .map(|action| {
                spawn_action(
                    archive_path.to_path_buf(),
                    Arc::clone(scheduler),
                    Arc::clone(app_handle),
                    action,
                )
            })
            .collect(),
    )
}

struct RefreshRuntime {
    archive_path: PathBuf,
    scheduler: Arc<Mutex<RefreshScheduler>>,
    app_handle: Arc<OnceLock<AppHandle>>,
}

impl RefreshRuntime {
    fn execute(self, action: RefreshAction) {
        let app_handle = Arc::clone(&self.app_handle);
        self.execute_with_completion_notifier(action, move |host_id| {
            if let Some(app) = app_handle.get() {
                if let Err(error) = app.emit(EVENT_REFRESH_COMPLETED, host_id.to_owned()) {
                    tracing::warn!(%error, "unable to announce the completed round");
                }
            }
        });
    }

    fn execute_with_completion_notifier(
        self,
        action: RefreshAction,
        notify_completed: impl FnOnce(&str),
    ) {
        let clock = SystemClock;
        let report = self.execute_round(&clock, &action);
        let changed = round_changed_archive(&report);
        if let Ok(mut scheduler) = self.scheduler.lock() {
            let _ = scheduler.complete(&action.key(), clock.now_utc_ms(), report);
        }
        // Emitted after `complete`, so a listener that immediately re-reads
        // `get_refresh_status` sees the finished round rather than one still marked running.
        if changed {
            if let Some(app) = self.app_handle.get() {
                if let Err(error) = app.emit(EVENT_ARCHIVE_COMMITTED, action.host_id.clone()) {
                    tracing::warn!(%error, "unable to announce the committed round");
                }
            }
        }
        notify_completed(&action.host_id);
    }

    fn execute_round(&self, clock: &SystemClock, action: &RefreshAction) -> RoundReport {
        let mut archive = match Archive::open(&self.archive_path) {
            Ok(archive) => archive,
            Err(error) => return RoundReport::failed(0, error.to_string()),
        };
        if let Err(error) = set_archive_busy_timeout(&archive, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS) {
            return RoundReport::failed(0, error.to_string());
        }
        let host = match HostRegistry::new(archive.connection()).get(&action.host_id) {
            Ok(Some(host)) => host,
            Ok(None) => return RoundReport::failed(0, "host disappeared before refresh started"),
            Err(error) => return RoundReport::failed(0, error.to_string()),
        };
        if !host.enabled_sources().iter().any(|s| s == &action.source) {
            return RoundReport::failed(
                0,
                format!(
                    "采集源 {} 已在主机 {} 上停用，本轮不执行",
                    action.source, action.host_id
                ),
            );
        }
        match host.kind {
            HostKind::Local => match action.source.as_str() {
                HERMES_SOURCE => match HermesLocalSource::discover(&action.host_id) {
                    Ok(source) => execute_action(&source, &mut archive, clock, action),
                    Err(error) => RoundReport::failed(0, error.to_string()),
                },
                CODEX_SOURCE => match CodexLocalSource::discover(&action.host_id) {
                    Ok(source) => execute_action(&source, &mut archive, clock, action),
                    Err(error) => RoundReport::failed(0, error.to_string()),
                },
                CLAUDE_CODE_SOURCE => match ClaudeCodeLocalSource::discover(&action.host_id) {
                    Ok(source) => execute_action(&source, &mut archive, clock, action),
                    Err(error) => RoundReport::failed(0, error.to_string()),
                },
                OPENCODE_SOURCE => match LocalHostSource::discover(&action.host_id) {
                    Ok(source) => execute_action(&source, &mut archive, clock, action),
                    Err(error) => RoundReport::failed(0, error.to_string()),
                },
                other => RoundReport::failed(0, format!("未知的本地采集源 {other}")),
            },
            HostKind::Ssh => {
                // A host record carries no identity file, so the prompt this round can expect
                // is the login password; `ssh_authentication_for_host` still falls back to a
                // stored passphrase when that is the only entry the host has.
                let authentication = ssh_authentication_for_host(
                    Arc::new(OsKeyringStore),
                    host.host_id(),
                    None,
                    askpass_helper_path(),
                );
                let transport = match SshTransport::discover(
                    StdCommandRunner,
                    None,
                    authentication,
                    collector_artifacts(),
                ) {
                    Ok(transport) => transport,
                    Err(error) => return RoundReport::failed(0, error.to_string()),
                };
                match SshHostSource::new(&host, transport) {
                    Ok(source) => execute_action(
                        &source.with_source(action.source.clone()),
                        &mut archive,
                        clock,
                        action,
                    ),
                    Err(error) => RoundReport::failed(0, error.to_string()),
                }
            }
        }
    }
}

/// `pub(crate)` so the todo-18 SSH connection probe reuses the exact same artifact
/// discovery a real refresh round uses; a second copy could drift and make the probe
/// report success for an architecture the refresh path cannot actually serve.
pub(crate) fn collector_artifacts() -> CollectorArtifacts {
    let mut artifacts = CollectorArtifacts::default();
    if let Some(path) = artifact_path(
        "AGENTLENS_COLLECTOR_X86_64",
        "agentlens-collector-x86_64-unknown-linux-musl",
    ) {
        artifacts = artifacts.with_x86_64(path);
    }
    if let Some(path) = artifact_path(
        "AGENTLENS_COLLECTOR_AARCH64",
        "agentlens-collector-aarch64-unknown-linux-musl",
    ) {
        artifacts = artifacts.with_aarch64(path);
    }
    artifacts
}

fn artifact_path(environment_key: &str, bundled_name: &str) -> Option<PathBuf> {
    env::var_os(environment_key)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let path = env::current_exe().ok()?.parent()?.join(bundled_name);
            path.is_file().then_some(path)
        })
}

fn database_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Database, error.to_string())
}

fn pricing_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Pricing, error.to_string())
}

fn host_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Database, error.to_string())
}

fn refresh_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Refresh, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use agentlens_core::hostsource::{CollectSummary, SourceSchedule, SourceState, TriggerMode};
    use tempfile::TempDir;

    use super::*;
    use crate::commands::{hosts_create_impl, hosts_update_impl, set_settings_impl};
    use crate::contract::{
        AppSettings, HostCreateInput, HostKind as ContractHostKind, HostUpdateInput,
    };
    use crate::tray::{SETTING_KEY_AUTO_REFRESH_ENABLED, SETTING_KEY_REMOTE_INTERVAL_MS};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                env::set_var(self.key, value);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open test app state");
        (data_dir, state)
    }

    fn opencode_slot(host_id: &str) -> SourceKey {
        SourceKey::opencode(host_id)
    }

    fn manual_registration(host_id: &str) -> SourceRegistration {
        SourceRegistration {
            host_id: host_id.to_owned(),
            source: OPENCODE_SOURCE.to_owned(),
            display_name: format!("fixture {host_id}"),
            kind: HostKind::Local,
            schedule: SourceSchedule::for_kind(HostKind::Local).with_trigger(TriggerMode::Manual),
        }
    }

    fn summary(reached_eof: bool, committed: bool, changed_records: u64) -> CollectSummary {
        CollectSummary {
            reached_eof,
            committed,
            changed_records,
            eligible_count: changed_records,
            cursor_time_updated: Some(1),
        }
    }

    /// The gate deciding whether the dashboard is told to refetch. Getting it wrong is F3
    /// DEFECT-2 in one direction (no notification, a dashboard frozen at its stale values) and
    /// a dashboard-wide refetch on every idle tick in the other.
    #[test]
    fn only_a_committed_round_that_changed_rows_announces_itself() {
        assert!(round_changed_archive(&RoundReport::collected(
            10,
            summary(true, true, 155_498)
        )));

        for (label, report) in [
            (
                "committed but nothing changed",
                RoundReport::collected(10, summary(true, true, 0)),
            ),
            (
                "changed rows but the transaction never committed",
                RoundReport::collected(10, summary(true, false, 42)),
            ),
            (
                "changed rows but the stream never reached EOF",
                RoundReport::collected(10, summary(false, true, 42)),
            ),
            ("failed round", RoundReport::failed(10, "ssh timed out")),
        ] {
            assert!(
                !round_changed_archive(&report),
                "{label} must not announce a commit"
            );
        }
    }

    #[test]
    fn reopening_state_restores_persisted_hosts_into_the_scheduler() {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let host_id = {
            let state = AppState::open_in_data_dir(data_dir.path()).expect("open initial state");
            hosts_create_impl(
                &state,
                HostCreateInput {
                    display_name: "Persisted workstation".to_owned(),
                    kind: ContractHostKind::Local,
                    machine_id_hash: "d".repeat(64),
                    ssh_target: None,
                    remote_data_dir: Some("/fixtures/opencode".to_owned()),
                    enabled_sources: None,
                },
            )
            .expect("persist host")
            .host_id
        };

        let reopened = AppState::open_in_data_dir(data_dir.path()).expect("reopen state");
        let statuses = reopened
            .lock_scheduler()
            .expect("lock scheduler")
            .statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].host_id, host_id);
        assert_eq!(statuses[0].display_name, "Persisted workstation");
        assert_eq!(statuses[0].kind, HostKind::Local);
    }

    #[test]
    fn updating_an_ssh_host_preserves_the_persisted_auto_refresh_schedule() {
        let (_data_dir, state) = state();
        let host = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Remote workstation".to_owned(),
                kind: ContractHostKind::Ssh,
                machine_id_hash: "a".repeat(64),
                ssh_target: Some("agent@example.test".to_owned()),
                remote_data_dir: Some("/srv/opencode".to_owned()),
                enabled_sources: None,
            },
        )
        .expect("create SSH host");
        let configured_interval_ms = 1_200_000_u64;
        set_settings_impl(
            &state,
            AppSettings {
                values: [
                    (
                        SETTING_KEY_AUTO_REFRESH_ENABLED.to_owned(),
                        "true".to_owned(),
                    ),
                    (
                        SETTING_KEY_REMOTE_INTERVAL_MS.to_owned(),
                        configured_interval_ms.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            },
        )
        .expect("enable persisted SSH auto refresh");

        hosts_update_impl(
            &state,
            HostUpdateInput {
                host_id: host.host_id.clone(),
                display_name: "Renamed remote workstation".to_owned(),
                kind: ContractHostKind::Ssh,
                ssh_target: Some("agent@example.test".to_owned()),
                remote_data_dir: Some("/srv/opencode".to_owned()),
                enabled_sources: None,
            },
        )
        .expect("update SSH host");

        let status = state
            .lock_scheduler()
            .expect("lock scheduler")
            .status(&opencode_slot(&host.host_id))
            .expect("updated SSH slot remains registered");
        assert_eq!(status.trigger, TriggerMode::Auto);
        assert_eq!(status.interval_ms, configured_interval_ms);
    }

    #[test]
    fn explicit_paths_are_retained_and_an_idle_tick_starts_no_manual_source() {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let archive_path = data_dir.path().join("custom.sqlite3");
        let prices_path = data_dir.path().join("custom-prices.json");
        let state = AppState::open_paths(archive_path.clone(), prices_path.clone())
            .expect("open explicit paths");
        state
            .register_host(manual_registration("manual-host"))
            .expect("register manual source");

        state.tick_due().expect("tick scheduler");

        assert_eq!(state.archive_path, archive_path);
        assert_eq!(state.prices_path, prices_path);
        let status = state
            .lock_scheduler()
            .expect("lock scheduler")
            .status(&opencode_slot("manual-host"))
            .expect("manual source remains registered");
        assert_eq!(status.state, SourceState::Idle);
        assert_eq!(status.trigger, TriggerMode::Manual);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_state_uses_the_platform_data_directory_for_both_stores() {
        let _environment = ENV_LOCK.lock().expect("serialize environment mutation");
        let directory = tempfile::tempdir().expect("create default data directory");
        let _xdg = EnvGuard::set("XDG_DATA_HOME", directory.path());

        let state = AppState::open_default().expect("open default state");

        assert!(state.archive_path.starts_with(directory.path()));
        assert!(state.prices_path.starts_with(directory.path()));
        assert!(state.archive_path.is_file());
    }

    #[test]
    fn poisoned_state_locks_become_structured_internal_errors() {
        let (_data_dir, archive_state) = state();
        let archive_panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = archive_state.archive.lock().expect("lock archive");
            panic!("poison archive lock");
        }));
        assert!(archive_panic.is_err());
        let archive_error = match archive_state.lock_archive() {
            Ok(_) => panic!("poisoned archive lock must fail"),
            Err(error) => error,
        };
        assert_eq!(archive_error.code, IpcErrorCode::Internal);
        assert!(archive_error.message.contains("archive state lock"));

        let (_data_dir, scheduler_state) = state();
        let scheduler_panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = scheduler_state.scheduler.lock().expect("lock scheduler");
            panic!("poison scheduler lock");
        }));
        assert!(scheduler_panic.is_err());
        let scheduler_error = scheduler_state
            .lock_scheduler()
            .expect_err("poisoned scheduler lock must fail");
        assert_eq!(scheduler_error.code, IpcErrorCode::Internal);
        assert!(scheduler_error.message.contains("refresh scheduler lock"));
    }

    #[test]
    fn manual_refresh_rejects_blank_unknown_and_already_running_hosts() {
        let (_data_dir, state) = state();
        let blank = state
            .trigger_refresh("  ")
            .expect_err("blank host id must fail before scheduling");
        assert_eq!(blank.code, IpcErrorCode::InvalidInput);
        assert_eq!(
            blank.fields.get("field").map(String::as_str),
            Some("hostId")
        );

        let unknown = state
            .trigger_refresh("missing-host")
            .expect_err("unknown host must be reported");
        assert_eq!(unknown.code, IpcErrorCode::NotFound);
        assert_eq!(
            unknown.fields.get("identifier").map(String::as_str),
            Some("missing-host")
        );

        state
            .register_host(manual_registration("busy-host"))
            .expect("register source");
        let admitted_at = 1_234_567;
        let first = state
            .lock_scheduler()
            .expect("lock scheduler")
            .trigger_manual(&opencode_slot("busy-host"), admitted_at);
        assert!(matches!(first, TriggerOutcome::Started(_)));

        assert_eq!(
            state
                .trigger_refresh("busy-host")
                .expect("a duplicate refresh is a value, not an error"),
            vec![TriggerRefreshResult::AlreadyRunning {
                host_id: "busy-host".to_owned(),
                source: OPENCODE_SOURCE.to_owned(),
                started_at_utc: admitted_at,
            }]
        );
    }

    #[test]
    fn admitted_manual_and_scheduled_refreshes_dispatch_background_rounds() {
        let (_data_dir, manual_state) = state();
        manual_state
            .register_host(manual_registration("manual-dispatch"))
            .expect("register manual source");

        let started = manual_state
            .trigger_refresh("manual-dispatch")
            .expect("manual refresh is admitted");
        assert!(matches!(
            started.as_slice(),
            [TriggerRefreshResult::Started { host_id, source, .. }]
                if host_id == "manual-dispatch" && source == OPENCODE_SOURCE
        ));

        let (_data_dir, scheduled_state) = state();
        scheduled_state
            .register_host(SourceRegistration {
                host_id: "scheduled-dispatch".to_owned(),
                source: OPENCODE_SOURCE.to_owned(),
                display_name: "Scheduled fixture".to_owned(),
                kind: HostKind::Local,
                schedule: SourceSchedule::for_kind(HostKind::Local),
            })
            .expect("register scheduled source");
        scheduled_state.tick_due().expect("dispatch due source");
        let state = scheduled_state
            .lock_scheduler()
            .expect("lock scheduler")
            .status(&opencode_slot("scheduled-dispatch"))
            .expect("scheduled status")
            .state;
        assert!(
            matches!(state, SourceState::Running | SourceState::Error { .. }),
            "the due source must leave idle before tick_due returns"
        );
    }

    /// Drives the exact production chain — `hosts_create_impl` → `SourceRegistration::all_for_host`
    /// → `tick_due` → `CodexLocalSource::discover` → ingest → `query_summary` — against the real
    /// `~/.codex` of the machine running the test.
    ///
    /// Ignored because it reads private developer data and asserts nothing about its contents, so
    /// it can never be a CI gate. Its value is diagnostic: it is the only check that proves the
    /// enable-a-source path reaches the archive and the dashboard aggregate, rather than proving
    /// each hop in isolation the way the synthetic tests do.
    ///
    /// `cargo test -p agentlens-tauri codex_real_data_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore = "reads the developer's real ~/.codex; run explicitly"]
    fn codex_real_data_end_to_end_reaches_the_archive_and_the_summary() {
        let (_data_dir, state) = state();
        let host = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Real codex probe".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "e".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: Some(vec![CODEX_SOURCE.to_owned()]),
            },
        )
        .expect("register a local host with codex enabled");

        let started = std::time::Instant::now();
        state.tick_due().expect("dispatch the due codex slot");
        let key = SourceKey::new(host.host_id.clone(), CODEX_SOURCE.to_owned());
        loop {
            let status = state
                .refresh_status(&key)
                .expect("read status")
                .expect("codex slot is registered");
            if status.state != crate::contract::SourceState::Running {
                println!(
                    "codex round finished in {:?}: {:?}",
                    started.elapsed(),
                    status.state
                );
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(600),
                "codex round did not finish within ten minutes"
            );
            thread::sleep(Duration::from_millis(200));
        }

        let mut archived_codex = 0_i64;
        let mut first_utc = i64::MAX;
        let mut last_utc = 0_i64;
        {
            let archive = state.lock_archive().expect("lock archive");
            let mut statement = archive
                .connection()
                .prepare(
                    "SELECT source, is_incomplete, count(*), coalesce(sum(tok_input + tok_output
                     + tok_reasoning + tok_cache_read + tok_cache_write), 0),
                     min(time_created_utc), max(time_created_utc)
                 FROM usage_record GROUP BY source, is_incomplete ORDER BY source, is_incomplete",
                )
                .expect("prepare per-source rollup");
            let mut rows = statement.query([]).expect("run per-source rollup");
            while let Some(row) = rows.next().expect("read rollup row") {
                let source: String = row.get(0).expect("source");
                let incomplete: i64 = row.get(1).expect("is_incomplete");
                let count: i64 = row.get(2).expect("count");
                let tokens: i64 = row.get(3).expect("tokens");
                let first: i64 = row.get(4).expect("first");
                let last: i64 = row.get(5).expect("last");
                println!(
                    "archive: source={source} is_incomplete={incomplete} rows={count} \
                     tokens={tokens} first={first} last={last}"
                );
                if source == CODEX_SOURCE && incomplete == 0 {
                    archived_codex = count;
                    first_utc = first;
                    last_utc = last;
                }
            }
        }

        let day = |utc_ms: i64| {
            chrono::DateTime::from_timestamp_millis(utc_ms)
                .expect("archived timestamp is representable")
                .format("%Y-%m-%d")
                .to_string()
        };
        let summary = crate::commands::get_summary_impl(
            &state,
            crate::contract::DateRange {
                start_date: day(first_utc),
                end_date_exclusive: day(last_utc + 86_400_000),
                week_start: crate::contract::WeekStart::Monday,
            },
            "UTC".to_owned(),
            crate::contract::AggregateFilters {
                host_id: None,
                source: Some(CODEX_SOURCE.to_owned()),
                agent_key: None,
                provider_id: None,
                model_id: None,
            },
        )
        .expect("aggregate the codex slice the dashboard would read");
        println!(
            "summary(source=codex, {}..{}): messages={} tokens={:?} cost={:?}",
            day(first_utc),
            day(last_utc + 86_400_000),
            summary.message_count,
            summary.tokens,
            summary.cost
        );

        assert!(
            archived_codex > 0,
            "no complete codex rows reached the archive; the enable-a-source path is broken"
        );
        assert_eq!(
            u64::try_from(archived_codex).expect("non-negative"),
            summary.message_count,
            "the dashboard aggregate must see every complete codex row"
        );
    }

    #[test]
    fn refresh_worker_dispatches_due_actions_and_stops_on_a_poisoned_scheduler() {
        let directory = tempfile::tempdir().expect("create missing archive parent");
        let scheduler = Arc::new(Mutex::new(RefreshScheduler::new()));
        scheduler
            .lock()
            .expect("lock scheduler")
            .register(SourceRegistration {
                host_id: "due-host".to_owned(),
                source: OPENCODE_SOURCE.to_owned(),
                display_name: "Due host".to_owned(),
                kind: HostKind::Local,
                schedule: SourceSchedule::for_kind(HostKind::Local),
            })
            .expect("register due source");
        let app_handle = Arc::new(OnceLock::new());

        let handles = spawn_due_actions(
            &directory.path().join("missing.sqlite3"),
            &scheduler,
            &app_handle,
            100,
        )
        .expect("healthy scheduler produces a dispatch list");
        assert_eq!(handles.len(), 1);
        for handle in handles {
            handle.join().expect("refresh worker must not panic");
        }
        assert!(matches!(
            scheduler
                .lock()
                .expect("lock scheduler")
                .status(&opencode_slot("due-host"))
                .expect("due host status")
                .state,
            SourceState::Error { .. }
        ));

        let (_data_dir, poisoned) = state();
        let poison = catch_unwind(AssertUnwindSafe(|| {
            let _guard = poisoned.scheduler.lock().expect("lock scheduler");
            panic!("poison refresh-loop scheduler");
        }));
        assert!(poison.is_err());
        assert!(spawn_due_actions(
            &poisoned.archive_path,
            &poisoned.scheduler,
            &poisoned.app_handle,
            200,
        )
        .is_none());
        poisoned.start_refresh_loop();
    }

    #[test]
    fn refresh_runtime_records_open_and_missing_host_failures_in_the_scheduler() {
        let execute = |archive_path: PathBuf, host_id: &str| {
            let scheduler = Arc::new(Mutex::new(RefreshScheduler::new()));
            scheduler
                .lock()
                .expect("lock scheduler")
                .register(manual_registration(host_id))
                .expect("register source");
            let action = match scheduler
                .lock()
                .expect("lock scheduler")
                .trigger_manual(&opencode_slot(host_id), 100)
            {
                TriggerOutcome::Started(action) => action,
                outcome => panic!("unexpected trigger outcome: {outcome:?}"),
            };
            RefreshRuntime {
                archive_path,
                scheduler: Arc::clone(&scheduler),
                app_handle: Arc::new(OnceLock::new()),
            }
            .execute(action);
            let status = scheduler
                .lock()
                .expect("lock scheduler")
                .status(&opencode_slot(host_id))
                .expect("source status");
            assert!(matches!(status.state, SourceState::Error { .. }));
            status.last_error.expect("runtime failure text")
        };

        let directory = tempfile::tempdir().expect("create temporary directory");
        let open_error = execute(directory.path().to_path_buf(), "unopenable-archive");
        assert!(!open_error.is_empty());

        let (_data_dir, state) = state();
        let missing_host = execute(state.archive_path.clone(), "missing-from-registry");
        assert_eq!(missing_host, "host disappeared before refresh started");
    }

    #[test]
    fn refresh_runtime_announces_completion_when_a_round_changes_no_archive_rows() {
        let directory = tempfile::tempdir().expect("create missing archive parent");
        let scheduler = Arc::new(Mutex::new(RefreshScheduler::new()));
        scheduler
            .lock()
            .expect("lock scheduler")
            .register(manual_registration("unchanged-host"))
            .expect("register source");
        let action = match scheduler
            .lock()
            .expect("lock scheduler")
            .trigger_manual(&opencode_slot("unchanged-host"), 100)
        {
            TriggerOutcome::Started(action) => action,
            outcome => panic!("unexpected trigger outcome: {outcome:?}"),
        };
        let mut completed_hosts = Vec::new();

        RefreshRuntime {
            archive_path: directory.path().join("missing.sqlite3"),
            scheduler,
            app_handle: Arc::new(OnceLock::new()),
        }
        .execute_with_completion_notifier(action, |host_id| {
            completed_hosts.push(host_id.to_owned());
        });

        assert_eq!(completed_hosts, ["unchanged-host"]);
    }

    #[test]
    fn refresh_runtime_maps_local_discovery_and_ssh_startup_failures_into_round_reports() {
        let _environment = ENV_LOCK.lock().expect("serialize environment mutation");
        let (data_dir, state) = state();
        let local = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local source".to_owned(),
                kind: ContractHostKind::Local,
                machine_id_hash: "2".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create local host");
        let source_dir = data_dir.path().join("opencode-source");
        std::fs::create_dir(&source_dir).expect("create source directory");
        std::fs::write(source_dir.join("opencode.db"), b"not a sqlite database")
            .expect("write invalid source database");
        let _opencode = EnvGuard::set("OPENCODE_DATA_DIR", &source_dir);
        let local_action = match state
            .lock_scheduler()
            .expect("lock scheduler")
            .trigger_manual(&opencode_slot(&local.host_id), 100)
        {
            TriggerOutcome::Started(action) => action,
            outcome => panic!("unexpected local trigger outcome: {outcome:?}"),
        };
        let runtime = RefreshRuntime {
            archive_path: state.archive_path.clone(),
            scheduler: Arc::clone(&state.scheduler),
            app_handle: Arc::new(OnceLock::new()),
        };
        assert!(matches!(
            runtime.execute_round(&SystemClock, &local_action).result,
            RoundResult::Failed { .. }
        ));

        let remote = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "SSH source".to_owned(),
                kind: ContractHostKind::Ssh,
                machine_id_hash: "3".repeat(64),
                ssh_target: Some("ci@example.test".to_owned()),
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create SSH host");
        let _path = EnvGuard::set("PATH", "");
        let remote_action = match state
            .lock_scheduler()
            .expect("lock scheduler")
            .trigger_manual(&opencode_slot(&remote.host_id), 200)
        {
            TriggerOutcome::Started(action) => action,
            outcome => panic!("unexpected SSH trigger outcome: {outcome:?}"),
        };
        let runtime = RefreshRuntime {
            archive_path: state.archive_path.clone(),
            scheduler: Arc::clone(&state.scheduler),
            app_handle: Arc::new(OnceLock::new()),
        };
        let report = runtime.execute_round(&SystemClock, &remote_action);
        assert!(matches!(report.result, RoundResult::Failed { ref error } if !error.is_empty()));
    }

    #[test]
    fn collector_artifact_discovery_accepts_existing_environment_overrides() {
        let _environment = ENV_LOCK.lock().expect("serialize environment mutation");
        let directory = tempfile::tempdir().expect("create artifact directory");
        let x86_64 = directory.path().join("collector-x86_64");
        let aarch64 = directory.path().join("collector-aarch64");
        std::fs::write(&x86_64, b"x86 fixture").expect("write x86 fixture");
        std::fs::write(&aarch64, b"arm fixture").expect("write arm fixture");
        let _x86_guard = EnvGuard::set("AGENTLENS_COLLECTOR_X86_64", &x86_64);
        let _arm_guard = EnvGuard::set("AGENTLENS_COLLECTOR_AARCH64", &aarch64);

        let artifacts = collector_artifacts();
        assert_eq!(artifacts.x86_64.as_deref(), Some(x86_64.as_path()));
        assert_eq!(artifacts.aarch64.as_deref(), Some(aarch64.as_path()));

        let current_exe = env::current_exe().expect("current test executable");
        let executable_name = current_exe
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("UTF-8 executable name");
        assert_eq!(
            artifact_path("AGENTLENS_UNUSED_ARTIFACT", executable_name).as_deref(),
            Some(current_exe.as_path()),
            "without an override the bundled artifact beside the executable is used"
        );
        assert_eq!(
            artifact_path("AGENTLENS_UNUSED_ARTIFACT", "definitely-not-bundled"),
            None
        );
    }

    #[test]
    fn state_error_adapters_preserve_category_and_message() {
        for (error, code) in [
            (database_error("database fixture"), IpcErrorCode::Database),
            (pricing_error("pricing fixture"), IpcErrorCode::Pricing),
            (host_error("host fixture"), IpcErrorCode::Database),
            (refresh_error("refresh fixture"), IpcErrorCode::Refresh),
        ] {
            assert_eq!(error.code, code);
            assert!(error.message.ends_with("fixture"));
        }
    }
}
