use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use agentlens_core::archive::{default_archive_path, Archive};
use agentlens_core::host::{HostKind, HostRegistry};
use agentlens_core::hostsource::{
    execute_action, set_archive_busy_timeout, Clock, LocalHostSource, RefreshAction,
    RefreshScheduler, RoundReport, RoundResult, SourceRegistration, SshHostSource, SystemClock,
    TriggerOutcome, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS,
};
use agentlens_core::pricing::{default_prices_path, PriceTable};
use agentlens_core::transport::ssh::{
    CollectorArtifacts, SshAuthentication, SshTransport, StdCommandRunner,
};

use tauri::{AppHandle, Emitter};

use crate::contract::{IpcError, IpcErrorCode, TriggerRefreshResult};

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
            scheduler
                .register(SourceRegistration::from_host(host))
                .map_err(refresh_error)?;
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
            self.spawn_action(action);
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
            let actions = match scheduler.lock() {
                Ok(mut scheduler) => scheduler.tick(SystemClock.now_utc_ms()),
                Err(_) => break,
            };
            for action in actions {
                spawn_action(
                    archive_path.clone(),
                    Arc::clone(&scheduler),
                    Arc::clone(&app_handle),
                    action,
                );
            }
            thread::sleep(Duration::from_secs(1));
        });
    }

    pub(crate) fn trigger_refresh(&self, host_id: &str) -> Result<TriggerRefreshResult, IpcError> {
        if host_id.trim().is_empty() {
            return Err(IpcError::invalid_input(
                "hostId",
                "host_id must not be empty",
            ));
        }
        let now = SystemClock.now_utc_ms();
        let outcome = self.lock_scheduler()?.trigger_manual(host_id, now);
        match outcome {
            TriggerOutcome::Started(action) => {
                let response = TriggerRefreshResult::Started {
                    host_id: action.host_id.clone(),
                    started_at_utc: action.started_at_utc,
                };
                self.spawn_action(action);
                Ok(response)
            }
            other => TriggerRefreshResult::try_from(other),
        }
    }

    pub(crate) fn register_host(&self, registration: SourceRegistration) -> Result<(), IpcError> {
        self.lock_scheduler()?
            .register(registration)
            .map_err(refresh_error)
    }

    pub(crate) fn replace_host_registration(
        &self,
        registration: SourceRegistration,
    ) -> Result<(), IpcError> {
        let mut scheduler = self.lock_scheduler()?;
        scheduler.remove(&registration.host_id);
        scheduler.register(registration).map_err(refresh_error)
    }

    pub(crate) fn remove_host_registration(&self, host_id: &str) -> Result<(), IpcError> {
        self.lock_scheduler()?.remove(host_id);
        Ok(())
    }

    fn spawn_action(&self, action: RefreshAction) {
        spawn_action(
            self.archive_path.clone(),
            Arc::clone(&self.scheduler),
            Arc::clone(&self.app_handle),
            action,
        );
    }
}

fn spawn_action(
    archive_path: PathBuf,
    scheduler: Arc<Mutex<RefreshScheduler>>,
    app_handle: Arc<OnceLock<AppHandle>>,
    action: RefreshAction,
) {
    let runtime = RefreshRuntime {
        archive_path,
        scheduler,
        app_handle,
    };
    thread::spawn(move || runtime.execute(action));
}

struct RefreshRuntime {
    archive_path: PathBuf,
    scheduler: Arc<Mutex<RefreshScheduler>>,
    app_handle: Arc<OnceLock<AppHandle>>,
}

impl RefreshRuntime {
    fn execute(self, action: RefreshAction) {
        let clock = SystemClock;
        let report = self.execute_round(&clock, &action);
        let changed = round_changed_archive(&report);
        if let Ok(mut scheduler) = self.scheduler.lock() {
            let _ = scheduler.complete(&action.host_id, clock.now_utc_ms(), report);
        }
        // Emitted after `complete`, so a listener that immediately re-reads
        // `get_refresh_status` sees the finished round rather than one still marked running.
        if changed {
            if let Some(app) = self.app_handle.get() {
                if let Err(error) = app.emit(EVENT_ARCHIVE_COMMITTED, action.host_id.clone()) {
                    eprintln!("agentlens: unable to announce the committed round: {error}");
                }
            }
        }
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
        match host.kind {
            HostKind::Local => match LocalHostSource::discover(&action.host_id) {
                Ok(source) => execute_action(&source, &mut archive, clock, action),
                Err(error) => RoundReport::failed(0, error.to_string()),
            },
            HostKind::Ssh => {
                let transport = match SshTransport::discover(
                    StdCommandRunner,
                    None,
                    SshAuthentication::Batch {
                        identity_file: None,
                    },
                    collector_artifacts(),
                ) {
                    Ok(transport) => transport,
                    Err(error) => return RoundReport::failed(0, error.to_string()),
                };
                match SshHostSource::new(&host, transport) {
                    Ok(source) => execute_action(&source, &mut archive, clock, action),
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
    use agentlens_core::hostsource::CollectSummary;

    use super::*;

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
}
