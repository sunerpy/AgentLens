use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use agentlens_core::archive::{read_app_settings, write_app_settings};
use agentlens_core::host::{
    local_machine_identity as core_local_machine_identity, HostError, HostRecord, HostRegistry,
    MachineIdentity, SUPPORTED_SOURCES,
};
use agentlens_core::hostsource::{SourceRegistration, MIN_AUTO_REFRESH_INTERVAL_MS};
use agentlens_core::pricing::{builtin_price_catalog, PriceTable as CorePriceTable};
use agentlens_core::query::{
    query_breakdown, query_details, query_series_bundle, query_summary, BreakdownOptions,
    LocalDateRange, QueryError,
};
use agentlens_core::source::opencode_legacy::CoverageStore;
use agentlens_core::transport::ssh::{
    SshAuthentication, SshError, SshTransport, StdCommandRunner, SSH_PROBE_WALL_TIMEOUT,
};
use chrono::NaiveDate;
use serde_json::Value;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};

use crate::contract::{
    AggregateFilters, AppSettings, BreakdownDimensions, BreakdownRow, DateRange, DetailFilters,
    Granularity, Host, HostCreateInput, HostKind, HostUpdateInput, IpcError, IpcErrorCode,
    MessageFilters, MessagePage, ObservedModelPrice, PriceCatalog, PriceMatchKind, PriceTable,
    RefreshEvent, SeriesQueryResult, SourceStatus, Summary, TriggerRefreshResult,
};
use crate::credentials::{
    askpass_helper_path, ssh_authentication_for_host, CredentialError, CredentialKind,
    CredentialRef, CredentialStatus, CredentialStore, LocalIdentity, OsKeyringStore, Secret,
    SshProbeInput, SshProbeResult,
};
use crate::logging::{
    diagnostics_snapshot, read_recent, DiagnosticsReport, LogTail, LOG_TAIL_DEFAULT_LIMIT,
};
use crate::state::AppState;
use crate::tray::{SETTING_KEY_LOCAL_INTERVAL_MS, SETTING_KEY_REMOTE_INTERVAL_MS};

pub type IpcResult<T> = Result<T, IpcError>;

type ProbeCancellation = Arc<AtomicBool>;

static SSH_PROBE_CANCELLATIONS: OnceLock<Mutex<BTreeMap<String, ProbeCancellation>>> =
    OnceLock::new();

fn probe_cancellations() -> &'static Mutex<BTreeMap<String, ProbeCancellation>> {
    SSH_PROBE_CANCELLATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

struct ProbeRegistration {
    request_id: String,
    cancellation: ProbeCancellation,
}

impl ProbeRegistration {
    fn register(request_id: String) -> IpcResult<Self> {
        validate_probe_request_id(&request_id)?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut registrations = probe_cancellations().lock().map_err(|_| {
            IpcError::new(IpcErrorCode::Internal, "SSH probe registry lock poisoned")
        })?;
        if registrations.contains_key(&request_id) {
            return Err(IpcError::new(
                IpcErrorCode::Conflict,
                "SSH probe request_id is already running",
            )
            .with_field("requestId", request_id));
        }
        registrations.insert(request_id.clone(), Arc::clone(&cancellation));
        drop(registrations);
        Ok(Self {
            request_id,
            cancellation,
        })
    }
}

impl Drop for ProbeRegistration {
    fn drop(&mut self) {
        if let Ok(mut registrations) = probe_cancellations().lock() {
            registrations.remove(&self.request_id);
        }
    }
}

/// Runs a synchronous command body on Tauri's blocking pool.
///
/// A `#[tauri::command] pub fn` runs **on the main thread**, so every millisecond it spends
/// in SQLite, the OS keyring or a platform lookup is a millisecond the webview cannot paint
/// or accept input. Opening the hosts view fires four or more queries at once, which used to
/// queue up on that one thread and freeze the window. An `async fn` command is polled on the
/// async runtime instead, and `spawn_blocking` moves the synchronous body to a worker thread.
async fn on_blocking_pool<T, F>(command: &'static str, task: F) -> IpcResult<T>
where
    F: FnOnce() -> IpcResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            IpcError::new(
                IpcErrorCode::Internal,
                format!("{command} worker failed: {error}"),
            )
        })?
}

/// Same, for bodies that need the managed [`AppState`].
///
/// `State<'_, AppState>` borrows the invoke message and is therefore not `'static`, so it
/// cannot cross `spawn_blocking`. `AppHandle` is `'static` and cheap to clone, so the shell
/// moves the handle and re-resolves the managed state on the worker thread instead.
async fn with_state<R, T, F>(command: &'static str, app: AppHandle<R>, task: F) -> IpcResult<T>
where
    R: Runtime,
    F: FnOnce(&AppState) -> IpcResult<T> + Send + 'static,
    T: Send + 'static,
{
    on_blocking_pool(command, move || task(&app.state::<AppState>())).await
}

#[tauri::command]
pub async fn get_summary<R: Runtime>(
    app: AppHandle<R>,
    range: DateRange,
    tz: String,
    filters: AggregateFilters,
) -> IpcResult<Summary> {
    with_state("get_summary", app, move |state: &AppState| {
        state.tick_due()?;
        get_summary_impl(state, range, tz, filters)
    })
    .await
}

pub(crate) fn get_summary_impl(
    state: &AppState,
    range: DateRange,
    tz: String,
    filters: AggregateFilters,
) -> IpcResult<Summary> {
    let range = parse_range(&range, &tz)?;
    let filters = filters.into();
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    query_summary(&archive, &range, &filters, &prices)
        .map(Into::into)
        .map_err(query_error)
}

#[tauri::command]
pub async fn get_trend<R: Runtime>(
    app: AppHandle<R>,
    range: DateRange,
    tz: String,
    granularity: Granularity,
    filters: Option<AggregateFilters>,
) -> IpcResult<SeriesQueryResult> {
    with_state("get_trend", app, move |state: &AppState| {
        state.tick_due()?;
        get_trend_impl(state, range, tz, granularity, filters.unwrap_or_default())
    })
    .await
}

pub(crate) fn get_trend_impl(
    state: &AppState,
    range: DateRange,
    tz: String,
    granularity: Granularity,
    filters: AggregateFilters,
) -> IpcResult<SeriesQueryResult> {
    let range = parse_range(&range, &tz)?;
    let filters = filters.into();
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    let coverage = CoverageStore::load(archive.connection()).map_err(database_error)?;
    query_series_bundle(
        &archive,
        &range,
        granularity.into(),
        &filters,
        &prices,
        &coverage,
    )
    .map(Into::into)
    .map_err(query_error)
}

#[tauri::command]
pub async fn get_breakdown<R: Runtime>(
    app: AppHandle<R>,
    range: DateRange,
    dims: BreakdownDimensions,
) -> IpcResult<Vec<BreakdownRow>> {
    with_state("get_breakdown", app, move |state: &AppState| {
        state.tick_due()?;
        get_breakdown_impl(state, range, dims)
    })
    .await
}

pub(crate) fn get_breakdown_impl(
    state: &AppState,
    range: DateRange,
    dims: BreakdownDimensions,
) -> IpcResult<Vec<BreakdownRow>> {
    let range = parse_range(&range, &dims.timezone)?;
    let filters = dims.filters.into();
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    query_breakdown(
        &archive,
        &range,
        &filters,
        BreakdownOptions {
            expand_variant: dims.expand_variant,
        },
        &prices,
    )
    .map(|rows| rows.into_iter().map(Into::into).collect())
    .map_err(query_error)
}

#[tauri::command]
pub async fn query_messages<R: Runtime>(
    app: AppHandle<R>,
    filters: MessageFilters,
    limit: Value,
    offset: Value,
) -> IpcResult<MessagePage> {
    with_state("query_messages", app, move |state: &AppState| {
        state.tick_due()?;
        query_messages_impl(state, filters, limit, offset)
    })
    .await
}

pub(crate) fn query_messages_impl(
    state: &AppState,
    filters: MessageFilters,
    limit: Value,
    offset: Value,
) -> IpcResult<MessagePage> {
    let limit = parse_u32(&limit, "limit")?;
    let offset = parse_i64(&offset, "offset")?;
    let range = parse_range(&filters.range, &filters.timezone)?;
    let detail: DetailFilters = filters.detail;
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    query_details(&archive, &range, &detail.into(), limit, offset, &prices)
        .map(Into::into)
        .map_err(query_error)
}

#[tauri::command]
pub async fn hosts_list<R: Runtime>(app: AppHandle<R>) -> IpcResult<Vec<Host>> {
    with_state("hosts_list", app, |state: &AppState| {
        state.tick_due()?;
        hosts_list_impl(state)
    })
    .await
}

pub(crate) fn hosts_list_impl(state: &AppState) -> IpcResult<Vec<Host>> {
    HostRegistry::new(state.lock_archive()?.connection())
        .list()
        .map(|hosts| hosts.into_iter().map(Into::into).collect())
        .map_err(host_error)
}

#[tauri::command]
pub async fn hosts_get<R: Runtime>(app: AppHandle<R>, host_id: String) -> IpcResult<Host> {
    with_state("hosts_get", app, move |state: &AppState| {
        state.tick_due()?;
        hosts_get_impl(state, &host_id)
    })
    .await
}

pub(crate) fn hosts_get_impl(state: &AppState, host_id: &str) -> IpcResult<Host> {
    validate_host_id(host_id)?;
    HostRegistry::new(state.lock_archive()?.connection())
        .get(host_id)
        .map_err(host_error)?
        .map(Into::into)
        .ok_or_else(|| IpcError::not_found("host", host_id))
}

#[tauri::command]
pub async fn hosts_create<R: Runtime>(
    app: AppHandle<R>,
    input: HostCreateInput,
) -> IpcResult<Host> {
    with_state("hosts_create", app, move |state: &AppState| {
        hosts_create_impl(state, input)
    })
    .await
}

pub(crate) fn hosts_create_impl(state: &AppState, input: HostCreateInput) -> IpcResult<Host> {
    let identity =
        MachineIdentity::from_machine_id_hash(&input.machine_id_hash).map_err(host_error)?;
    let host = build_host_record(
        input.display_name,
        input.kind,
        input.ssh_target,
        input.remote_data_dir,
        input.enabled_sources,
        &identity,
    )?;
    {
        let archive = state.lock_archive()?;
        HostRegistry::new(archive.connection())
            .insert(&host)
            .map_err(host_error)?;
    }
    for registration in SourceRegistration::all_for_host(&host) {
        state.register_host(registration)?;
    }
    Ok(host.into())
}

#[tauri::command]
pub async fn hosts_update<R: Runtime>(
    app: AppHandle<R>,
    input: HostUpdateInput,
) -> IpcResult<Host> {
    with_state("hosts_update", app, move |state: &AppState| {
        hosts_update_impl(state, input)
    })
    .await
}

pub(crate) fn hosts_update_impl(state: &AppState, input: HostUpdateInput) -> IpcResult<Host> {
    validate_host_id(&input.host_id)?;
    let existing = {
        let archive = state.lock_archive()?;
        HostRegistry::new(archive.connection())
            .get(&input.host_id)
            .map_err(host_error)?
            .ok_or_else(|| IpcError::not_found("host", &input.host_id))?
    };
    let identity =
        MachineIdentity::from_machine_id_hash(existing.machine_id_hash()).map_err(host_error)?;
    let host = build_host_record(
        input.display_name,
        input.kind,
        input.ssh_target,
        input.remote_data_dir,
        // `None` means "leave the configured sources alone", so an edit that only renames a host
        // cannot silently disable an adapter it never mentioned.
        Some(
            input
                .enabled_sources
                .unwrap_or_else(|| existing.enabled_sources().to_vec()),
        ),
        &identity,
    )?;
    {
        let archive = state.lock_archive()?;
        HostRegistry::new(archive.connection())
            .update(&host)
            .map_err(host_error)?;
    }
    state.replace_host_registrations(SourceRegistration::all_for_host(&host), host.host_id())?;
    hosts_get_impl(state, &input.host_id)
}

/// Every source key a host may be configured to collect, in canonical order.
///
/// A read-only export of [`SUPPORTED_SOURCES`] so the source picker in the hosts view offers
/// exactly what [`HostRecord::validate`] accepts. The alternative — a list hard-coded in
/// TypeScript — drifts silently the moment a fifth adapter lands, and the drift only surfaces as
/// a rejected `hosts_update` the user cannot explain. Takes no `AppHandle`: the value is a
/// compile-time constant, so there is no archive to lock and no failure mode.
#[tauri::command]
pub async fn hosts_supported_sources() -> IpcResult<Vec<String>> {
    Ok(SUPPORTED_SOURCES
        .iter()
        .map(|source| (*source).to_owned())
        .collect())
}

#[tauri::command]
pub async fn hosts_delete<R: Runtime>(app: AppHandle<R>, host_id: String) -> IpcResult<()> {
    with_state("hosts_delete", app, move |state: &AppState| {
        hosts_delete_impl(state, &host_id)
    })
    .await
}

pub(crate) fn hosts_delete_impl(state: &AppState, host_id: &str) -> IpcResult<()> {
    validate_host_id(host_id)?;
    {
        let archive = state.lock_archive()?;
        let registry = HostRegistry::new(archive.connection());
        if registry.get(host_id).map_err(host_error)?.is_none() {
            return Err(IpcError::not_found("host", host_id));
        }
        registry.delete(host_id).map_err(host_error)?;
    }
    state.remove_host_registration(host_id)
}

#[tauri::command]
pub async fn trigger_refresh<R: Runtime>(
    app: AppHandle<R>,
    host_id: String,
    on_event: Channel<RefreshEvent>,
) -> IpcResult<Vec<TriggerRefreshResult>> {
    with_state("trigger_refresh", app, move |state: &AppState| {
        state.trigger_refresh_with_events(&host_id, |event| {
            if let Err(error) = on_event.send(event) {
                tracing::debug!(%error, "unable to send refresh progress");
            }
        })
    })
    .await
}

#[tauri::command]
pub async fn get_refresh_status<R: Runtime>(app: AppHandle<R>) -> IpcResult<Vec<SourceStatus>> {
    with_state("get_refresh_status", app, |state: &AppState| {
        state.tick_due()?;
        get_refresh_status_impl(state)
    })
    .await
}

pub(crate) fn get_refresh_status_impl(state: &AppState) -> IpcResult<Vec<SourceStatus>> {
    Ok(state
        .lock_scheduler()?
        .statuses()
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub async fn get_settings<R: Runtime>(app: AppHandle<R>) -> IpcResult<AppSettings> {
    with_state("get_settings", app, get_settings_impl).await
}

pub(crate) fn get_settings_impl(state: &AppState) -> IpcResult<AppSettings> {
    let values = read_app_settings(state.lock_archive()?.connection()).map_err(database_error)?;
    Ok(AppSettings { values })
}

#[tauri::command]
pub async fn set_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: AppSettings,
) -> IpcResult<AppSettings> {
    with_state("set_settings", app, move |state: &AppState| {
        set_settings_impl(state, settings)
    })
    .await
}

pub(crate) fn set_settings_impl(state: &AppState, settings: AppSettings) -> IpcResult<AppSettings> {
    validate_refresh_settings(&settings.values)?;
    let mut archive = state.lock_archive()?;
    write_app_settings(archive.connection_mut(), &settings.values).map_err(database_error)?;
    drop(archive);
    let applied = get_settings_impl(state)?;
    crate::tray::apply_refresh_intervals(state);
    Ok(applied)
}

/// Rejects a refresh interval below [`MIN_AUTO_REFRESH_INTERVAL_MS`] before it reaches the store.
///
/// The backend is authoritative here. A silent clamp was rejected: a user who typed one minute and
/// saw the value accepted would believe the app polls every minute for as long as they use it.
fn validate_refresh_settings(values: &BTreeMap<String, String>) -> IpcResult<()> {
    for key in [
        SETTING_KEY_LOCAL_INTERVAL_MS,
        SETTING_KEY_REMOTE_INTERVAL_MS,
    ] {
        let Some(raw) = values.get(key) else {
            continue;
        };
        let parsed = raw.trim().parse::<i64>().map_err(|_| {
            IpcError::invalid_input(key, format!("刷新间隔必须是毫秒整数，收到 {raw:?}"))
        })?;
        if parsed < MIN_AUTO_REFRESH_INTERVAL_MS as i64 {
            return Err(IpcError::invalid_input(
                key,
                format!(
                    "刷新间隔不能小于 {MIN_AUTO_REFRESH_INTERVAL_MS} 毫秒（10 分钟），收到 {parsed}"
                ),
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn price_catalog_get<R: Runtime>(app: AppHandle<R>) -> IpcResult<PriceCatalog> {
    with_state("price_catalog_get", app, price_catalog_get_impl).await
}

pub(crate) fn price_catalog_get_impl(state: &AppState) -> IpcResult<PriceCatalog> {
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    let mut statement = archive
        .connection()
        .prepare(
            "SELECT provider_id, model_id, count(*)
             FROM usage_record
             WHERE tok_input > 0 OR tok_output > 0 OR tok_cache_read > 0 OR tok_cache_write > 0
             GROUP BY provider_id, model_id
             ORDER BY provider_id, model_id",
        )
        .map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let mut observed_models = Vec::new();
    while let Some(row) = rows.next().map_err(database_error)? {
        let provider_id: String = row.get(0).map_err(database_error)?;
        let model_id: String = row.get(1).map_err(database_error)?;
        let usage_count =
            u64::try_from(row.get::<_, i64>(2).map_err(database_error)?).map_err(|_| {
                IpcError::new(
                    IpcErrorCode::Database,
                    "usage_record returned a negative model usage count",
                )
            })?;
        let (match_kind, matched_price) = match prices.lookup_match(&provider_id, &model_id) {
            Some(matched) => (matched.kind.into(), Some(matched.entry.clone().into())),
            None => (PriceMatchKind::Unknown, None),
        };
        observed_models.push(ObservedModelPrice {
            provider_id,
            model_id,
            usage_count,
            match_kind,
            matched_price,
        });
    }

    Ok(PriceCatalog::from_core(
        builtin_price_catalog(),
        observed_models,
    ))
}

#[tauri::command]
pub async fn prices_get<R: Runtime>(app: AppHandle<R>) -> IpcResult<PriceTable> {
    with_state("prices_get", app, prices_get_impl).await
}

pub(crate) fn prices_get_impl(state: &AppState) -> IpcResult<PriceTable> {
    state.load_prices().map(Into::into)
}

#[tauri::command]
pub async fn prices_set<R: Runtime>(app: AppHandle<R>, prices: Value) -> IpcResult<PriceTable> {
    with_state("prices_set", app, move |state: &AppState| {
        prices_set_impl(state, prices)
    })
    .await
}

pub(crate) fn prices_set_impl(state: &AppState, prices: Value) -> IpcResult<PriceTable> {
    let prices: PriceTable = serde_json::from_value(prices).map_err(|error| {
        IpcError::invalid_input("prices", format!("malformed price payload: {error}"))
    })?;
    let prices: CorePriceTable = prices.into();
    state.save_prices(&prices)?;
    state.load_prices().map(Into::into)
}

/// Identity behind the auto-registered local host card.
///
/// Only Rust can answer this: `machine_id_hash` is SHA-256 over the trimmed machine-id,
/// and a wrong value would register the same machine twice and double-count its usage.
#[tauri::command]
pub async fn local_machine_identity() -> IpcResult<LocalIdentity> {
    on_blocking_pool("local_machine_identity", local_machine_identity_impl).await
}

pub(crate) fn local_machine_identity_impl() -> IpcResult<LocalIdentity> {
    let identity = core_local_machine_identity().map_err(host_error)?;
    Ok(LocalIdentity {
        host_id: identity.host_id().to_owned(),
        machine_id_hash: identity.machine_id_hash().to_owned(),
        hostname: local_hostname(),
    })
}

/// Probe one SSH target for the "测试连接" button.
///
/// `AppHandle` is injected by Tauri rather than sent from the webview, so taking it here does
/// not change the invoke payload. It is needed because a re-test of an already-registered host
/// must authenticate the same way a refresh round does: the form has no `host_id` — the machine
/// identity is what the probe *discovers* — so the host is resolved by its SSH target instead.
#[tauri::command]
pub async fn ssh_probe<R: Runtime>(
    app: AppHandle<R>,
    input: SshProbeInput,
    request_id: String,
) -> IpcResult<SshProbeResult> {
    let registration = ProbeRegistration::register(request_id)?;
    let cancellation = Arc::clone(&registration.cancellation);
    let result = on_blocking_pool("SSH probe", move || {
        ssh_probe_impl_with_cancel(Some(&app.state::<AppState>()), input, &|| {
            cancellation.load(Ordering::Acquire)
        })
    })
    .await;
    drop(registration);
    result
}

#[cfg(test)]
pub(crate) fn ssh_probe_impl(input: SshProbeInput) -> IpcResult<SshProbeResult> {
    ssh_probe_impl_with_cancel(None, input, &|| false)
}

fn ssh_probe_impl_with_cancel(
    state: Option<&AppState>,
    input: SshProbeInput,
    is_cancelled: &dyn Fn() -> bool,
) -> IpcResult<SshProbeResult> {
    let ssh_target = input.ssh_target.trim();
    if ssh_target.is_empty() {
        return Err(IpcError::invalid_input(
            "sshTarget",
            "ssh_target must not be empty",
        ));
    }
    let identity_file = optional_path(input.identity_file.as_deref());
    let authentication = probe_authentication(state, ssh_target, identity_file);
    let probe = SshTransport::probe_connection_with_timeout(
        StdCommandRunner,
        None,
        authentication,
        crate::state::collector_artifacts(),
        ssh_target,
        SSH_PROBE_WALL_TIMEOUT,
        is_cancelled,
    )
    .map_err(ssh_error)?;
    Ok(SshProbeResult::from_probe(
        &probe,
        input.remote_data_dir.as_deref(),
    ))
}

/// Authentication for a probe: the stored secret of the host that already owns this target.
///
/// A target nobody has registered yet has no keyring entry to read — a first probe necessarily
/// runs in `BatchMode`, which is honest rather than a regression: the operator saves the host,
/// stores its password, and the next probe and every refresh round use it.
fn probe_authentication(
    state: Option<&AppState>,
    ssh_target: &str,
    identity_file: Option<PathBuf>,
) -> SshAuthentication {
    match state.and_then(|state| registered_ssh_host_id(state, ssh_target)) {
        Some(host_id) => ssh_authentication_for_host(
            Arc::new(OsKeyringStore),
            &host_id,
            identity_file,
            askpass_helper_path(),
        ),
        None => SshAuthentication::Batch { identity_file },
    }
}

fn registered_ssh_host_id(state: &AppState, ssh_target: &str) -> Option<String> {
    let archive = state.lock_archive().ok()?;
    let hosts = HostRegistry::new(archive.connection()).list().ok()?;
    hosts
        .into_iter()
        .find(|host| {
            host.kind == agentlens_core::host::HostKind::Ssh
                && host.ssh_target.as_deref().map(str::trim) == Some(ssh_target)
        })
        .map(|host| host.host_id().to_owned())
}

/// Deliberately the one command that stays synchronous.
///
/// It validates an ASCII id and stores an `AtomicBool` — nanoseconds, no I/O, so a worker-thread
/// hop would cost more than the work. It also must not queue: the probe it cancels is itself
/// occupying a blocking-pool thread, so routing the cancellation through that same pool would
/// let a saturated pool delay the signal behind the very task it is meant to stop.
#[tauri::command]
pub fn ssh_probe_cancel(request_id: String) -> IpcResult<()> {
    validate_probe_request_id(&request_id)?;
    if let Some(cancellation) = probe_cancellations()
        .lock()
        .map_err(|_| IpcError::new(IpcErrorCode::Internal, "SSH probe registry lock poisoned"))?
        .get(&request_id)
    {
        cancellation.store(true, Ordering::Release);
    }
    Ok(())
}

fn validate_probe_request_id(request_id: &str) -> IpcResult<()> {
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(IpcError::invalid_input(
            "requestId",
            "request_id must be 1-128 ASCII letters, digits, '-' or '_'",
        ))
    } else {
        Ok(())
    }
}

/// Store a password or key passphrase in the OS keyring. It never reaches a config file,
/// a log line or a DTO; the response only reports presence.
#[tauri::command]
pub async fn credential_set(
    host_id: String,
    kind: CredentialKind,
    secret: String,
) -> IpcResult<CredentialStatus> {
    on_blocking_pool("credential_set", move || {
        credential_set_impl(
            &OsKeyringStore,
            &host_id,
            kind,
            secret,
            credentials_are_deliverable(),
        )
    })
    .await
}

/// `present` reports whether the secret **will be used**, not merely whether it was written.
///
/// The two used to be independent: a secret could sit in the keyring while every SSH command
/// ran with `BatchMode=yes`, which forbids password prompts outright — so the hosts view said
/// "saved" about something that could never be sent. Now that the transport delivers it, the
/// one remaining local reason a stored secret stays inert is an installation without the
/// bundled askpass helper, and this refuses to call that state present. Whether the *remote*
/// accepts the secret is not knowable without connecting, and is reported by the connection.
///
/// `deliverable` is a parameter rather than a lookup so both branches are testable without
/// mutating process-wide environment state from parallel tests.
fn delivered_status(status: CredentialStatus, deliverable: bool) -> CredentialStatus {
    CredentialStatus {
        present: status.present && deliverable,
        ..status
    }
}

/// `true` when this installation can hand a stored secret to `ssh`.
fn credentials_are_deliverable() -> bool {
    askpass_helper_path().is_some()
}

pub(crate) fn credential_set_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
    secret: String,
    deliverable: bool,
) -> IpcResult<CredentialStatus> {
    store
        .store(&CredentialRef::new(host_id, kind), &Secret::new(secret))
        .map(|status| delivered_status(status, deliverable))
        .map_err(credential_error)
}

#[tauri::command]
pub async fn credential_status(
    host_id: String,
    kind: CredentialKind,
) -> IpcResult<CredentialStatus> {
    on_blocking_pool("credential_status", move || {
        credential_status_impl(
            &OsKeyringStore,
            &host_id,
            kind,
            credentials_are_deliverable(),
        )
    })
    .await
}

pub(crate) fn credential_status_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
    deliverable: bool,
) -> IpcResult<CredentialStatus> {
    store
        .status(&CredentialRef::new(host_id, kind))
        .map(|status| delivered_status(status, deliverable))
        .map_err(credential_error)
}

#[tauri::command]
pub async fn credential_delete(
    host_id: String,
    kind: CredentialKind,
) -> IpcResult<CredentialStatus> {
    on_blocking_pool("credential_delete", move || {
        credential_delete_impl(
            &OsKeyringStore,
            &host_id,
            kind,
            credentials_are_deliverable(),
        )
    })
    .await
}

pub(crate) fn credential_delete_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
    deliverable: bool,
) -> IpcResult<CredentialStatus> {
    let reference = CredentialRef::new(host_id, kind);
    store.delete(&reference).map_err(credential_error)?;
    store
        .status(&reference)
        .map(|status| delivered_status(status, deliverable))
        .map_err(credential_error)
}

/// Newest log records, for the diagnostics view.
///
/// Reads at most [`LOG_TAIL_MAX_LIMIT`] entries and never the whole file: a rotated
/// generation is 2 MiB, and shipping that over IPC to render 500 visible rows would stall
/// the webview for no benefit.
#[tauri::command]
pub async fn logs_tail<R: Runtime>(app: AppHandle<R>, limit: Option<u32>) -> IpcResult<LogTail> {
    on_blocking_pool("logs_tail", move || {
        let directory = app
            .path()
            .app_log_dir()
            .map_err(|error| IpcError::new(IpcErrorCode::Internal, error.to_string()))?;
        let limit = limit
            .map(|value| value as usize)
            .unwrap_or(LOG_TAIL_DEFAULT_LIMIT);
        Ok(read_recent(&directory, limit))
    })
    .await
}

/// Environment facts for a bug report.
///
/// Deliberately carries no hostname, user name, machine-id hash, archive path or credential:
/// its output is meant to be pasted into a public issue tracker, so identifying data must not
/// be able to reach it. See [`crate::logging::DiagnosticsReport`].
#[tauri::command]
pub async fn diagnostics_report() -> IpcResult<DiagnosticsReport> {
    on_blocking_pool("diagnostics_report", || Ok(diagnostics_snapshot())).await
}

fn optional_path(value: Option<&str>) -> Option<PathBuf> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Best-effort display name for the local card. `None` lets the view fall back to its
/// own dictionary string rather than embedding user-visible text in Rust.
fn local_hostname() -> Option<String> {
    ["HOSTNAME", "COMPUTERNAME"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter_map(|value| value.into_string().ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

fn parse_range(range: &DateRange, timezone: &str) -> IpcResult<LocalDateRange> {
    let start = parse_date(&range.start_date, "range.startDate")?;
    let end = parse_date(&range.end_date_exclusive, "range.endDateExclusive")?;
    LocalDateRange::from_timezone_name(start, end, timezone, range.week_start.into())
        .map_err(query_error)
}

fn parse_date(value: &str, field: &str) -> IpcResult<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|error| {
        IpcError::invalid_input(field, format!("expected YYYY-MM-DD date: {error}"))
    })
}

fn parse_u32(value: &Value, field: &str) -> IpcResult<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            IpcError::invalid_input(
                field,
                format!("{field} must be an integer from 0 through {}", u32::MAX),
            )
        })
}

fn parse_i64(value: &Value, field: &str) -> IpcResult<i64> {
    value
        .as_i64()
        .ok_or_else(|| IpcError::invalid_input(field, format!("{field} must be a signed integer")))
}

fn validate_host_id(host_id: &str) -> IpcResult<()> {
    if host_id.trim().is_empty() {
        Err(IpcError::invalid_input(
            "hostId",
            "host_id must not be empty",
        ))
    } else {
        Ok(())
    }
}

fn build_host_record(
    display_name: String,
    kind: HostKind,
    ssh_target: Option<String>,
    remote_data_dir: Option<String>,
    enabled_sources: Option<Vec<String>>,
    identity: &MachineIdentity,
) -> IpcResult<HostRecord> {
    let host = match kind {
        HostKind::Local => {
            if ssh_target.is_some() {
                return Err(IpcError::invalid_input(
                    "sshTarget",
                    "local hosts must not define ssh_target",
                ));
            }
            HostRecord::local(display_name, identity)
        }
        HostKind::Ssh => HostRecord::ssh(display_name, ssh_target.unwrap_or_default(), identity),
    };
    let host = host.with_remote_data_dir(remote_data_dir);
    Ok(match enabled_sources {
        Some(sources) => host.with_enabled_sources(sources),
        None => host,
    })
}

fn query_error(error: QueryError) -> IpcError {
    let code = match error {
        QueryError::InvalidTimezone(_) => IpcErrorCode::InvalidTimezone,
        QueryError::InvalidDateRange { .. } => IpcErrorCode::InvalidRange,
        QueryError::InvalidLimit | QueryError::NegativeOffset(_) => IpcErrorCode::InvalidInput,
        QueryError::Sqlite(_) => IpcErrorCode::Database,
        _ => IpcErrorCode::InvalidInput,
    };
    IpcError::new(code, error.to_string())
}

fn ssh_error(error: SshError) -> IpcError {
    let variant = match error {
        SshError::ArchMismatch { .. } => "archMismatch",
        SshError::NoWritableCache { .. } => "noWritableCache",
        SshError::TransferCorrupted { .. } => "transferCorrupted",
        SshError::AuthFailed { .. } => "authFailed",
        SshError::NoDataDir { .. } => "noDataDir",
        SshError::WalUnreadable { .. } => "walUnreadable",
        SshError::ClientCancelled => "clientCancelled",
        SshError::TimedOut { .. } => "timedOut",
        SshError::SshUnavailable { .. } => "sshUnavailable",
        SshError::InvalidInput { .. } => "invalidInput",
        SshError::InvalidResponse { .. } => "invalidResponse",
        SshError::Runner { .. } => "runner",
    };
    // The typed variant and its Chinese remediation travel in `fields` rather than as a
    // new `IpcErrorCode`: the frontend's shared `isIpcError` guard validates `code`
    // against a fixed list, so inventing a code would make every SSH failure decay into
    // `internal` and lose the remediation text the view must render.
    IpcError::new(IpcErrorCode::Refresh, error.to_string())
        .with_field("variant", variant)
        .with_field("remediation", error.remediation())
}

fn credential_error(error: CredentialError) -> IpcError {
    let code = match error {
        CredentialError::BlankHostId | CredentialError::EmptySecret => IpcErrorCode::InvalidInput,
        _ => IpcErrorCode::Internal,
    };
    IpcError::new(code, error.to_string())
        .with_field("variant", error.variant())
        .with_field("remediation", error.remediation())
}

fn host_error(error: HostError) -> IpcError {
    let code = match error {
        HostError::DuplicateMachine { .. } | HostError::HostAlreadyExists { .. } => {
            IpcErrorCode::Conflict
        }
        HostError::HostNotFound { .. } => IpcErrorCode::NotFound,
        HostError::Sqlite(_) => IpcErrorCode::Database,
        _ => IpcErrorCode::InvalidInput,
    };
    IpcError::new(code, error.to_string())
}

fn database_error(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(IpcErrorCode::Database, error.to_string())
}

#[cfg(test)]
pub const REGISTERED_COMMANDS: [&str; 25] = [
    "get_summary",
    "get_trend",
    "get_breakdown",
    "query_messages",
    "hosts_list",
    "hosts_get",
    "hosts_create",
    "hosts_update",
    "hosts_supported_sources",
    "hosts_delete",
    "trigger_refresh",
    "get_refresh_status",
    "get_settings",
    "set_settings",
    "price_catalog_get",
    "prices_get",
    "prices_set",
    "local_machine_identity",
    "ssh_probe",
    "ssh_probe_cancel",
    "credential_set",
    "credential_status",
    "credential_delete",
    "logs_tail",
    "diagnostics_report",
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use agentlens_core::host::HostKind as CoreHostKind;
    use agentlens_core::hostsource::{SourceSchedule, TriggerMode};
    use agentlens_core::transport::ssh::CommandStage;
    use serde_json::json;
    use tauri::ipc::{Channel, InvokeResponseBody};
    use tempfile::TempDir;

    use super::*;
    use crate::contract::{PriceEntry, PriceMatchKind, WeekStart};
    use crate::credentials::InMemoryCredentialStore;
    use agentlens_core::ingest::OPENCODE_SOURCE;
    use agentlens_core::source::claude_code::CLAUDE_CODE_SOURCE;

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open test app state");
        (data_dir, state)
    }

    /// A real Tauri app on the mock runtime, so the `async` command shells can be invoked the
    /// way the webview invokes them — no GTK, no WebView2, no window.
    ///
    /// Without this the shells could only be checked by type, and "does `AppHandle` still
    /// resolve the managed `AppState` after the closure has been moved onto a worker thread?"
    /// is precisely the question a type check cannot answer.
    fn mock_app() -> (TempDir, AppHandle<tauri::test::MockRuntime>) {
        let (data_dir, state) = state();
        let app = tauri::test::mock_builder()
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock tauri app");
        let handle = app.handle().clone();
        (data_dir, handle)
    }

    fn range(start_date: &str, end_date_exclusive: &str) -> DateRange {
        DateRange {
            start_date: start_date.to_owned(),
            end_date_exclusive: end_date_exclusive.to_owned(),
            week_start: WeekStart::Monday,
        }
    }

    fn message_filters() -> MessageFilters {
        MessageFilters {
            range: range("2026-01-01", "2026-02-01"),
            timezone: "UTC".to_owned(),
            detail: DetailFilters::default(),
        }
    }

    #[test]
    fn registered_command_surface_is_complete_and_stable() {
        assert_eq!(REGISTERED_COMMANDS.len(), 25);
        assert!(REGISTERED_COMMANDS.contains(&"query_messages"));
        assert!(REGISTERED_COMMANDS.contains(&"hosts_supported_sources"));
        assert!(REGISTERED_COMMANDS.contains(&"trigger_refresh"));
        assert!(REGISTERED_COMMANDS.contains(&"price_catalog_get"));
        assert!(REGISTERED_COMMANDS.contains(&"prices_set"));
        assert!(REGISTERED_COMMANDS.contains(&"ssh_probe_cancel"));
        assert!(REGISTERED_COMMANDS.contains(&"logs_tail"));
        assert!(REGISTERED_COMMANDS.contains(&"diagnostics_report"));
    }

    #[test]
    fn logs_tail_shell_resolves_the_tauri_log_directory() {
        let (_data_dir, app) = mock_app();

        let tail = tauri::async_runtime::block_on(logs_tail(app.clone(), Some(10)))
            .expect("logs_tail must resolve the app log directory");

        let expected = app
            .path()
            .app_log_dir()
            .expect("mock runtime resolves a log directory");
        assert_eq!(tail.directory, expected.display().to_string());
        assert!(tail.entries.len() <= 10);
    }

    #[test]
    fn logs_tail_without_a_limit_falls_back_to_the_default() {
        let (_data_dir, app) = mock_app();

        let tail = tauri::async_runtime::block_on(logs_tail(app, None))
            .expect("logs_tail must accept an absent limit");

        assert!(tail.entries.len() <= LOG_TAIL_DEFAULT_LIMIT);
    }

    #[test]
    fn diagnostics_report_shell_returns_only_publishable_environment_facts() {
        let report = tauri::async_runtime::block_on(diagnostics_report())
            .expect("diagnostics_report has no failure path");

        assert_eq!(report.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.arch, std::env::consts::ARCH);
    }

    #[test]
    fn price_catalog_reports_observed_exact_approximate_and_unknown_models() {
        let (_data_dir, state) = state();
        {
            let archive = state.lock_archive().expect("lock archive");
            for (index, provider_id, model_id) in [
                (1, "anthropic", "claude-sonnet-4-5-20250929"),
                (2, "aws", "us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
                (3, "private-provider", "private-model-v7"),
            ] {
                archive
                    .connection()
                    .execute(
                        "INSERT INTO usage_record (
                            host_id, source, message_id, session_id,
                            time_created_utc, time_completed_utc, source_time_updated,
                            origin, origin_priority, agent_raw, agent_key,
                            provider_id, model_id, variant,
                            tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
                            cost, cost_source, is_incomplete, project_dir
                        ) VALUES (
                            'host-price-test', 'opencode', ?1, 'ses-price-test',
                            1785468844419, 1785468845419, 1785468846419,
                            'live', 3, 'Sisyphus', 'sisyphus',
                            ?2, ?3, NULL,
                            10, 20, 0, 0, 0,
                            NULL, 'unavailable', 0, '/tmp/price-test'
                        )",
                        rusqlite::params![format!("msg-price-{index}"), provider_id, model_id],
                    )
                    .expect("seed observed model");
            }
        }

        let catalog = price_catalog_get_impl(&state).expect("load price catalog");

        assert_eq!(catalog.schema_version, 1);
        assert!(!catalog.catalog_version.is_empty());
        assert!(!catalog.updated_at.is_empty());
        assert_eq!(catalog.currency, "USD");
        assert!(catalog
            .entries
            .iter()
            .any(|entry| entry.provider_id == "amazon-bedrock"));

        let exact = catalog
            .observed_models
            .iter()
            .find(|model| model.provider_id == "anthropic")
            .expect("exact observed model");
        assert_eq!(exact.match_kind, PriceMatchKind::Exact);
        assert_eq!(exact.usage_count, 1);

        let approximate = catalog
            .observed_models
            .iter()
            .find(|model| model.provider_id == "aws")
            .expect("approximate observed model");
        assert_eq!(approximate.match_kind, PriceMatchKind::Normalized);
        assert_eq!(
            approximate
                .matched_price
                .as_ref()
                .map(|entry| entry.provider_id.as_str()),
            Some("amazon-bedrock")
        );

        let unknown = catalog
            .observed_models
            .iter()
            .find(|model| model.provider_id == "private-provider")
            .expect("unknown observed model");
        assert_eq!(unknown.match_kind, PriceMatchKind::Unknown);
        assert!(unknown.matched_price.is_none());
    }

    #[test]
    fn price_catalog_omits_models_without_billable_tokens() {
        let (_data_dir, state) = state();
        {
            let archive = state.lock_archive().expect("lock archive");
            archive
                .connection()
                .execute(
                    "INSERT INTO usage_record (
                        host_id, source, message_id, session_id,
                        time_created_utc, time_completed_utc, source_time_updated,
                        origin, origin_priority, agent_raw, agent_key,
                        provider_id, model_id, variant,
                        tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
                        cost, cost_source, is_incomplete, project_dir
                    ) VALUES (
                        'host-price-test', 'opencode', 'msg-zero-price', 'ses-price-test',
                        1785468844419, 1785468845419, 1785468846419,
                        'live', 3, 'Sisyphus', 'sisyphus',
                        'kiro-auth', 'auto', NULL,
                        0, 0, 99, 0, 0,
                        NULL, 'unavailable', 0, '/tmp/price-test'
                    )",
                    [],
                )
                .expect("seed zero-billable observed model");
        }

        let catalog = price_catalog_get_impl(&state).expect("load price catalog");

        assert!(!catalog
            .observed_models
            .iter()
            .any(|model| model.provider_id == "kiro-auth" && model.model_id == "auto"));
    }

    #[test]
    fn price_catalog_shell_resolves_state_on_the_blocking_pool() {
        let (_data_dir, app) = mock_app();

        let catalog = tauri::async_runtime::block_on(price_catalog_get(app))
            .expect("catalog command must resolve managed state");

        assert_eq!(catalog.currency, "USD");
        assert!(catalog.observed_models.is_empty());
    }

    #[test]
    fn local_identity_command_returns_a_stable_hash_and_prefers_the_unix_hostname() {
        struct HostnameGuard {
            previous: Option<std::ffi::OsString>,
        }

        impl Drop for HostnameGuard {
            fn drop(&mut self) {
                if let Some(value) = &self.previous {
                    std::env::set_var("HOSTNAME", value);
                } else {
                    std::env::remove_var("HOSTNAME");
                }
            }
        }

        static HOSTNAME_LOCK: Mutex<()> = Mutex::new(());
        let _environment = HOSTNAME_LOCK
            .lock()
            .expect("serialize hostname environment mutation");
        let guard = HostnameGuard {
            previous: std::env::var_os("HOSTNAME"),
        };
        std::env::set_var("HOSTNAME", "  fixture-workstation  ");

        match local_machine_identity_impl() {
            Ok(identity) => {
                assert_eq!(identity.host_id.len(), 16);
                assert_eq!(identity.machine_id_hash.len(), 64);
                assert!(identity
                    .machine_id_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
                assert_eq!(identity.hostname.as_deref(), Some("fixture-workstation"));
                assert_eq!(
                    tauri::async_runtime::block_on(local_machine_identity())
                        .expect("public command delegates to the same source"),
                    identity
                );
            }
            Err(error) => {
                assert_eq!(error.code, IpcErrorCode::InvalidInput);
                assert!(!error.message.is_empty());
                assert_eq!(
                    tauri::async_runtime::block_on(local_machine_identity())
                        .expect_err("a missing platform identity stays missing")
                        .code,
                    error.code
                );
            }
        }
        drop(guard);
    }

    #[test]
    fn query_messages_caps_limit_and_always_returns_total_count() {
        let (_data_dir, state) = state();

        let page = query_messages_impl(&state, message_filters(), json!(50), json!(0))
            .expect("query 50-row page");
        assert!(page.rows.len() <= 50);
        assert_eq!(page.limit, 50);
        assert_eq!(page.total_count, 0);
        assert_eq!(page.offset, 0);

        let capped = query_messages_impl(&state, message_filters(), json!(500), json!(0))
            .expect("query capped page");
        assert!(capped.rows.len() <= 200);
        assert_eq!(capped.limit, 200);
        assert_eq!(capped.total_count, 0);
    }

    #[test]
    fn query_messages_rejects_malformed_pagination_as_structured_error() {
        let (_data_dir, state) = state();
        let error = query_messages_impl(&state, message_filters(), json!("500"), json!(0))
            .expect_err("string limit must be rejected");

        assert_eq!(error.code, IpcErrorCode::InvalidInput);
        assert_eq!(error.fields.get("field"), Some(&"limit".to_owned()));
        assert!(serde_json::to_value(error)
            .expect("serialize error")
            .is_object());
    }

    #[test]
    fn reversed_range_returns_structured_error_without_panicking() {
        let (_data_dir, state) = state();
        let error = get_summary_impl(
            &state,
            range("2026-02-01", "2026-01-01"),
            "UTC".to_owned(),
            AggregateFilters::default(),
        )
        .expect_err("reversed range must fail");

        assert_eq!(error.code, IpcErrorCode::InvalidRange);
        let encoded = serde_json::to_value(error).expect("serialize error");
        assert_eq!(encoded["code"], "invalidRange");
        assert!(encoded["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn settings_updates_merge_without_erasing_unmentioned_keys() {
        let (_data_dir, state) = state();
        let initial = AppSettings {
            values: BTreeMap::from([
                ("theme".to_owned(), "dark".to_owned()),
                ("timezone".to_owned(), "UTC".to_owned()),
            ]),
        };
        set_settings_impl(&state, initial).expect("write initial settings");

        let merged = set_settings_impl(
            &state,
            AppSettings {
                values: BTreeMap::from([("theme".to_owned(), "light".to_owned())]),
            },
        )
        .expect("merge settings");

        assert_eq!(
            merged.values.get("theme").map(String::as_str),
            Some("light")
        );
        assert_eq!(
            merged.values.get("timezone").map(String::as_str),
            Some("UTC")
        );
    }

    /// The backend, not the settings view, is what makes the 10-minute floor real. A rejected write
    /// must also leave the store untouched, otherwise the UI would show a value that was refused.
    #[test]
    fn settings_reject_a_refresh_interval_below_the_ten_minute_floor() {
        let (_data_dir, state) = state();
        set_settings_impl(
            &state,
            AppSettings {
                values: BTreeMap::from([(
                    SETTING_KEY_LOCAL_INTERVAL_MS.to_owned(),
                    MIN_AUTO_REFRESH_INTERVAL_MS.to_string(),
                )]),
            },
        )
        .expect("the floor itself is accepted");

        for (key, value) in [
            (SETTING_KEY_LOCAL_INTERVAL_MS, "60000"),
            (SETTING_KEY_LOCAL_INTERVAL_MS, "599999"),
            (SETTING_KEY_LOCAL_INTERVAL_MS, "0"),
            (SETTING_KEY_LOCAL_INTERVAL_MS, "-600000"),
            (SETTING_KEY_REMOTE_INTERVAL_MS, "300000"),
        ] {
            let error = set_settings_impl(
                &state,
                AppSettings {
                    values: BTreeMap::from([(key.to_owned(), value.to_owned())]),
                },
            )
            .expect_err("a sub-floor interval must be refused, not clamped");
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
            assert_eq!(error.fields.get("field").map(String::as_str), Some(key));
            assert!(
                error.message.contains("600000"),
                "the message must name the floor: {}",
                error.message
            );
        }

        let malformed = set_settings_impl(
            &state,
            AppSettings {
                values: BTreeMap::from([(
                    SETTING_KEY_REMOTE_INTERVAL_MS.to_owned(),
                    "ten minutes".to_owned(),
                )]),
            },
        )
        .expect_err("a non-numeric interval must be refused");
        assert_eq!(malformed.code, IpcErrorCode::InvalidInput);

        let stored = get_settings_impl(&state).expect("read settings back");
        assert_eq!(
            stored.values.get(SETTING_KEY_LOCAL_INTERVAL_MS),
            Some(&MIN_AUTO_REFRESH_INTERVAL_MS.to_string()),
            "a rejected write must leave the accepted value in place"
        );
        assert_eq!(stored.values.get(SETTING_KEY_REMOTE_INTERVAL_MS), None);

        set_settings_impl(
            &state,
            AppSettings {
                values: BTreeMap::from([("theme".to_owned(), "dark".to_owned())]),
            },
        )
        .expect("settings unrelated to refresh are unaffected by the floor");
    }

    /// One host with two adapters must produce one admitted round per adapter, and the host editor
    /// must be able to disable one of them again without leaving a slot behind.
    #[test]
    fn hosts_with_two_sources_register_and_refresh_both_slots() {
        let (_data_dir, state) = state();
        let created = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Two-source workstation".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "e".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: Some(vec![
                    OPENCODE_SOURCE.to_owned(),
                    CLAUDE_CODE_SOURCE.to_owned(),
                ]),
            },
        )
        .expect("create a two-source host");
        assert_eq!(
            created.enabled_sources,
            vec![OPENCODE_SOURCE.to_owned(), CLAUDE_CODE_SOURCE.to_owned()]
        );

        let statuses = get_refresh_status_impl(&state).expect("read refresh status");
        assert_eq!(statuses.len(), 2);
        let mut sources: Vec<String> = statuses.iter().map(|s| s.source.clone()).collect();
        sources.sort();
        assert_eq!(
            sources,
            vec![CLAUDE_CODE_SOURCE.to_owned(), OPENCODE_SOURCE.to_owned()]
        );
        assert!(statuses
            .iter()
            .all(|status| status.host_id == created.host_id));

        let renamed = hosts_update_impl(
            &state,
            HostUpdateInput {
                host_id: created.host_id.clone(),
                display_name: "Renamed".to_owned(),
                kind: HostKind::Local,
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("rename the host");
        assert_eq!(
            renamed.enabled_sources,
            vec![OPENCODE_SOURCE.to_owned(), CLAUDE_CODE_SOURCE.to_owned()],
            "an omitted source list must leave the configured adapters alone"
        );
        assert_eq!(
            get_refresh_status_impl(&state)
                .expect("read refresh status")
                .len(),
            2
        );

        let narrowed = hosts_update_impl(
            &state,
            HostUpdateInput {
                host_id: created.host_id.clone(),
                display_name: "Renamed".to_owned(),
                kind: HostKind::Local,
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: Some(vec![OPENCODE_SOURCE.to_owned()]),
            },
        )
        .expect("disable the second source");
        assert_eq!(narrowed.enabled_sources, vec![OPENCODE_SOURCE.to_owned()]);
        let statuses = get_refresh_status_impl(&state).expect("read refresh status");
        assert_eq!(statuses.len(), 1, "a disabled source must be unscheduled");
        assert_eq!(statuses[0].source, OPENCODE_SOURCE);

        hosts_delete_impl(&state, &created.host_id).expect("delete the host");
        assert!(get_refresh_status_impl(&state)
            .expect("read refresh status")
            .is_empty());
    }

    /// 界面的采集源勾选框只能来自这条命令，所以它必须与 `HostRecord::validate` 接受的集合
    /// 逐字一致：少一个源，用户就永远勾不到那个适配器；多一个，保存时才会被后端拒绝。
    #[test]
    fn hosts_supported_sources_exports_the_core_constant_verbatim() {
        let exported = tauri::async_runtime::block_on(hosts_supported_sources())
            .expect("a constant export cannot fail");

        assert_eq!(exported, SUPPORTED_SOURCES.map(str::to_owned).to_vec());
        for source in [OPENCODE_SOURCE, "claude-code", "codex", "hermes"] {
            assert!(
                exported.contains(&source.to_owned()),
                "{source} 已实现适配器，界面必须能勾选它：{exported:?}"
            );
        }

        let (_data_dir, state) = state();
        let created = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Every source".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "e".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: Some(exported.clone()),
            },
        )
        .expect("每个导出的源都必须被后端接受");
        assert_eq!(created.enabled_sources, exported);
    }

    #[test]
    fn hosts_reject_an_unknown_or_empty_source_list() {
        let (_data_dir, state) = state();
        for sources in [vec![], vec!["nonexistent-source".to_owned()]] {
            let error = hosts_create_impl(
                &state,
                HostCreateInput {
                    display_name: "Rejected".to_owned(),
                    kind: HostKind::Local,
                    machine_id_hash: "f".repeat(64),
                    ssh_target: None,
                    remote_data_dir: None,
                    enabled_sources: Some(sources),
                },
            )
            .expect_err("an invalid source list must be refused");
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
            assert!(
                error.message.contains("采集源配置无效"),
                "{}",
                error.message
            );
        }
        assert!(get_refresh_status_impl(&state)
            .expect("read refresh status")
            .is_empty());
    }

    #[test]
    fn prices_set_round_trips_through_atomic_core_store() {
        let (_data_dir, state) = state();
        let input = PriceTable {
            schema_version: 1,
            entries: vec![PriceEntry {
                provider_id: "openai".to_owned(),
                model_id: "gpt-test".to_owned(),
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 0.5,
                extra: BTreeMap::new(),
            }],
            extra: BTreeMap::new(),
        };

        let saved = prices_set_impl(
            &state,
            serde_json::to_value(&input).expect("serialize price input"),
        )
        .expect("save price table");
        assert_eq!(saved, input);
        assert_eq!(prices_get_impl(&state).expect("reload prices"), input);

        let error = prices_set_impl(&state, json!({ "schemaVersion": 1, "entries": "bad" }))
            .expect_err("malformed prices must fail");
        assert_eq!(error.code, IpcErrorCode::InvalidInput);
    }

    #[test]
    fn credential_commands_round_trip_and_never_echo_the_secret() {
        let store = InMemoryCredentialStore::new();
        let host_id = "0123456789abcdef";
        let secret = "s3cret-passphrase";

        let stored = credential_set_impl(
            &store,
            host_id,
            CredentialKind::Passphrase,
            secret.to_owned(),
            true,
        )
        .expect("store passphrase");
        assert!(stored.present);
        let encoded = serde_json::to_string(&stored).expect("serialize credential status");
        assert!(
            !encoded.contains(secret),
            "credential_set response leaked the secret: {encoded}"
        );

        assert!(
            credential_status_impl(&store, host_id, CredentialKind::Passphrase, true)
                .expect("read status")
                .present
        );
        assert!(
            !credential_status_impl(&store, host_id, CredentialKind::Password, true)
                .expect("read unrelated status")
                .present,
            "a password entry must not be implied by a stored passphrase"
        );

        assert!(
            !credential_delete_impl(&store, host_id, CredentialKind::Passphrase, true)
                .expect("delete passphrase")
                .present
        );
        assert!(
            !credential_delete_impl(&store, host_id, CredentialKind::Passphrase, true)
                .expect("second delete is idempotent")
                .present
        );
    }

    /// The lie this change exists to remove: the hosts view rendered "已存入钥匙串" from
    /// `present`, while the transport could not send the secret at all. `present` must now mean
    /// stored **and** deliverable, so an installation without the bundled helper reports false.
    #[test]
    fn credential_status_reports_a_stored_but_undeliverable_secret_as_absent() {
        let store = InMemoryCredentialStore::new();
        let host_id = "0123456789abcdef";

        let undeliverable = credential_set_impl(
            &store,
            host_id,
            CredentialKind::Password,
            "s3cret".to_owned(),
            false,
        )
        .expect("store password");
        assert!(
            !undeliverable.present,
            "without the askpass helper a stored secret can never reach ssh"
        );
        assert!(
            !credential_status_impl(&store, host_id, CredentialKind::Password, false)
                .expect("read status")
                .present
        );
        assert!(
            credential_status_impl(&store, host_id, CredentialKind::Password, true)
                .expect("read status")
                .present,
            "the same entry is present once it can actually be delivered"
        );
        assert!(
            !credential_status_impl(&store, host_id, CredentialKind::Passphrase, true)
                .expect("read status")
                .present,
            "deliverability must never invent an entry that was never stored"
        );
        assert!(
            !credential_delete_impl(&store, host_id, CredentialKind::Password, true)
                .expect("delete password")
                .present
        );
    }

    /// A re-test of a saved host must authenticate the way its refresh rounds do, otherwise the
    /// button reports a failure the actual collection would not have hit. The host is matched by
    /// SSH target because the form carries no host id.
    #[test]
    fn ssh_probe_resolves_the_registered_host_behind_an_ssh_target() {
        let (_data_dir, state) = state();
        let created = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Build box".to_owned(),
                kind: HostKind::Ssh,
                machine_id_hash: "7".repeat(64),
                ssh_target: Some("ci@build-box.internal".to_owned()),
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("register SSH host");

        assert_eq!(
            registered_ssh_host_id(&state, "ci@build-box.internal").as_deref(),
            Some(created.host_id.as_str())
        );
        assert_eq!(
            registered_ssh_host_id(&state, "  ci@build-box.internal  "),
            None,
            "the caller trims the target; an untrimmed value must not silently match"
        );
        assert_eq!(registered_ssh_host_id(&state, "ci@other.internal"), None);

        hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "This machine".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "8".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("register local host");
        assert_eq!(
            registered_ssh_host_id(&state, "ci@build-box.internal").as_deref(),
            Some(created.host_id.as_str()),
            "a local host must never be mistaken for an SSH target"
        );

        // No state and an unknown target are the two paths that must stay BatchMode: a first
        // probe has no keyring entry to read yet, which is honest rather than a regression.
        for authentication in [
            probe_authentication(None, "ci@build-box.internal", None),
            probe_authentication(Some(&state), "ci@never-registered.internal", None),
        ] {
            assert!(matches!(authentication, SshAuthentication::Batch { .. }));
        }
    }

    #[test]
    fn credential_command_errors_carry_variant_and_chinese_remediation() {
        let store = InMemoryCredentialStore::new();
        let blank =
            credential_set_impl(&store, "  ", CredentialKind::Password, "x".to_owned(), true)
                .expect_err("a blank host id must be rejected");
        assert_eq!(blank.code, IpcErrorCode::InvalidInput);
        assert_eq!(
            blank.fields.get("variant").map(String::as_str),
            Some("blankHostId")
        );

        let empty = credential_set_impl(
            &store,
            "0123456789abcdef",
            CredentialKind::Password,
            String::new(),
            true,
        )
        .expect_err("an empty secret must be rejected");
        assert_eq!(
            empty.fields.get("variant").map(String::as_str),
            Some("emptySecret")
        );

        let unavailable = credential_set_impl(
            &InMemoryCredentialStore::failing("session bus has no secret service"),
            "0123456789abcdef",
            CredentialKind::Password,
            "x".to_owned(),
            true,
        )
        .expect_err("an unavailable keyring must be reported");
        assert_eq!(unavailable.code, IpcErrorCode::Internal);
        assert!(unavailable
            .fields
            .get("remediation")
            .is_some_and(|text| text.contains("libsecret")));
    }

    #[test]
    fn ssh_probe_rejects_a_blank_target_before_touching_the_network() {
        let error = ssh_probe_impl(SshProbeInput {
            ssh_target: "   ".to_owned(),
            identity_file: None,
            remote_data_dir: None,
        })
        .expect_err("a blank ssh target must be rejected");

        assert_eq!(error.code, IpcErrorCode::InvalidInput);
        assert_eq!(
            error.fields.get("field").map(String::as_str),
            Some("sshTarget")
        );
    }

    #[test]
    fn ssh_probe_command_returns_a_future_instead_of_blocking_the_main_thread() {
        fn assert_probe_future(_: impl Future<Output = IpcResult<SshProbeResult>>) {}

        let (_data_dir, app) = mock_app();
        assert_probe_future(ssh_probe(
            app,
            SshProbeInput {
                ssh_target: "fixture.invalid".to_owned(),
                identity_file: None,
                remote_data_dir: None,
            },
            "probe_async_contract".to_owned(),
        ));
    }

    /// Type-level guard over the whole command surface.
    ///
    /// A `#[tauri::command] pub fn` runs on the main thread and freezes the webview for its
    /// whole duration — the defect this file was restructured to remove. Reverting any command
    /// below to `pub fn` makes `F` a plain `IpcResult<_>`, which does not implement `Future`,
    /// so the reversion cannot compile. A comment could not enforce that.
    #[test]
    fn every_blocking_command_returns_a_future_so_the_webview_never_stalls() {
        macro_rules! assert_command_is_async {
            ($command:path, ($($argument:ty),* $(,)?)) => {{
                fn assert_async<T, F>(_: fn($($argument),*) -> F)
                where
                    F: Future<Output = IpcResult<T>>,
                {
                }
                assert_async($command);
            }};
        }

        assert_command_is_async!(
            get_summary,
            (AppHandle, DateRange, String, AggregateFilters)
        );
        assert_command_is_async!(
            get_trend,
            (
                AppHandle,
                DateRange,
                String,
                Granularity,
                Option<AggregateFilters>,
            )
        );
        assert_command_is_async!(get_breakdown, (AppHandle, DateRange, BreakdownDimensions));
        assert_command_is_async!(query_messages, (AppHandle, MessageFilters, Value, Value));
        assert_command_is_async!(hosts_list, (AppHandle));
        assert_command_is_async!(hosts_get, (AppHandle, String));
        assert_command_is_async!(hosts_create, (AppHandle, HostCreateInput));
        assert_command_is_async!(hosts_update, (AppHandle, HostUpdateInput));
        assert_command_is_async!(hosts_supported_sources, ());
        assert_command_is_async!(hosts_delete, (AppHandle, String));
        assert_command_is_async!(trigger_refresh, (AppHandle, String, Channel<RefreshEvent>));
        assert_command_is_async!(get_refresh_status, (AppHandle));
        assert_command_is_async!(get_settings, (AppHandle));
        assert_command_is_async!(set_settings, (AppHandle, AppSettings));
        assert_command_is_async!(price_catalog_get, (AppHandle));
        assert_command_is_async!(prices_get, (AppHandle));
        assert_command_is_async!(prices_set, (AppHandle, Value));
        assert_command_is_async!(local_machine_identity, ());
        assert_command_is_async!(ssh_probe, (AppHandle, SshProbeInput, String));
        assert_command_is_async!(credential_set, (String, CredentialKind, String));
        assert_command_is_async!(credential_status, (String, CredentialKind));
        assert_command_is_async!(credential_delete, (String, CredentialKind));
        assert_command_is_async!(logs_tail, (AppHandle, Option<u32>));
        assert_command_is_async!(diagnostics_report, ());

        // `ssh_probe_cancel` is the deliberate exception: pure in-memory, and it must not queue
        // behind the blocking-pool task it exists to cancel. This binding pins that choice, so
        // making it `async` breaks the build just as loudly as un-asyncing the rest.
        let _: fn(String) -> IpcResult<()> = ssh_probe_cancel;

        assert_eq!(REGISTERED_COMMANDS.len(), 25);
    }

    #[test]
    fn credential_commands_reject_a_blank_host_id_without_reaching_the_os_keyring() {
        for error in [
            tauri::async_runtime::block_on(credential_set(
                "  ".to_owned(),
                CredentialKind::Password,
                "x".to_owned(),
            ))
            .expect_err("a blank host id must fail in the worker"),
            tauri::async_runtime::block_on(credential_status(
                "  ".to_owned(),
                CredentialKind::Password,
            ))
            .expect_err("a blank host id must fail in the worker"),
            tauri::async_runtime::block_on(credential_delete(
                "  ".to_owned(),
                CredentialKind::Passphrase,
            ))
            .expect_err("a blank host id must fail in the worker"),
        ] {
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
            assert_eq!(
                error.fields.get("variant").map(String::as_str),
                Some("blankHostId")
            );
        }
    }

    /// Behavioural half of the async-command guard.
    ///
    /// Every shell here is invoked exactly as the webview invokes it, so this proves the
    /// `AppHandle` still resolves the managed `AppState` after the body was moved onto a
    /// worker thread — and, because `block_on` only accepts a `Future`, reverting any of
    /// these commands to `pub fn` also breaks this test.
    #[test]
    fn state_backed_command_shells_work_end_to_end_on_a_worker_thread() {
        use tauri::async_runtime::block_on;

        let (_data_dir, app) = mock_app();

        let created = block_on(hosts_create(
            app.clone(),
            HostCreateInput {
                display_name: "Mock workstation".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "c".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        ))
        .expect("create a host through the async shell");
        assert_eq!(
            block_on(hosts_list(app.clone())).expect("list hosts"),
            vec![created.clone()]
        );
        assert_eq!(
            block_on(hosts_get(app.clone(), created.host_id.clone())).expect("get host"),
            created
        );
        assert_eq!(
            block_on(hosts_update(
                app.clone(),
                HostUpdateInput {
                    host_id: created.host_id.clone(),
                    display_name: "Renamed workstation".to_owned(),
                    kind: HostKind::Local,
                    ssh_target: None,
                    remote_data_dir: None,
                    enabled_sources: None,
                },
            ))
            .expect("update host")
            .display_name,
            "Renamed workstation"
        );
        assert_eq!(
            block_on(get_refresh_status(app.clone()))
                .expect("read refresh status")
                .len(),
            1
        );
        assert_eq!(
            block_on(trigger_refresh(
                app.clone(),
                "   ".to_owned(),
                Channel::new(|_| Ok(())),
            ))
            .expect_err("a blank host id must fail in the worker")
            .code,
            IpcErrorCode::InvalidInput
        );
        block_on(hosts_delete(app.clone(), created.host_id.clone())).expect("delete host");
        assert!(block_on(hosts_list(app.clone()))
            .expect("list hosts after delete")
            .is_empty());

        let window = range("2026-01-01", "2026-02-01");
        assert_eq!(
            block_on(get_summary(
                app.clone(),
                window.clone(),
                "UTC".to_owned(),
                AggregateFilters::default(),
            ))
            .expect("summarise an empty archive")
            .message_count,
            0
        );
        assert!(block_on(get_trend(
            app.clone(),
            window.clone(),
            "UTC".to_owned(),
            Granularity::Day,
            None,
        ))
        .expect("trend over an empty archive")
        .total
        .iter()
        .all(|point| point.message_count.unwrap_or_default() == 0));
        assert!(block_on(get_breakdown(
            app.clone(),
            window,
            BreakdownDimensions {
                timezone: "UTC".to_owned(),
                filters: AggregateFilters::default(),
                expand_variant: false,
            },
        ))
        .expect("breakdown over an empty archive")
        .is_empty());
        assert_eq!(
            block_on(query_messages(
                app.clone(),
                message_filters(),
                json!(50),
                json!(0),
            ))
            .expect("page an empty archive")
            .total_count,
            0
        );

        let settings = block_on(set_settings(
            app.clone(),
            AppSettings {
                values: BTreeMap::from([("theme".to_owned(), "dark".to_owned())]),
            },
        ))
        .expect("write settings through the async shell");
        assert_eq!(
            block_on(get_settings(app.clone())).expect("read settings back"),
            settings
        );

        let empty_table = PriceTable {
            schema_version: 1,
            entries: Vec::new(),
            extra: BTreeMap::new(),
        };
        let prices = block_on(prices_set(
            app.clone(),
            serde_json::to_value(&empty_table).expect("serialize price input"),
        ))
        .expect("reset the price table");
        assert_eq!(prices, empty_table);
        assert_eq!(
            block_on(prices_get(app.clone())).expect("read prices back"),
            prices
        );
    }

    #[test]
    fn refresh_channel_delivers_ordered_statuses_and_finishes_the_stream() {
        use std::sync::Arc;

        use tauri::async_runtime::block_on;

        let (_data_dir, app) = mock_app();
        app.state::<AppState>()
            .register_host(SourceRegistration {
                host_id: "stream-host".to_owned(),
                source: agentlens_core::ingest::OPENCODE_SOURCE.to_owned(),
                display_name: "Stream fixture".to_owned(),
                kind: CoreHostKind::Local,
                schedule: SourceSchedule::for_kind(CoreHostKind::Local)
                    .with_trigger(TriggerMode::Manual),
            })
            .expect("register stream fixture");

        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let received = Arc::clone(&events);
        let channel = Channel::<RefreshEvent>::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("refresh events must use the JSON channel body")
            };
            received
                .lock()
                .expect("lock received refresh events")
                .push(serde_json::from_str(&json).expect("decode refresh event"));
            Ok(())
        });

        let result = block_on(trigger_refresh(app, "stream-host".to_owned(), channel))
            .expect("trigger refresh through the async channel command");
        assert!(matches!(
            result.as_slice(),
            [TriggerRefreshResult::Started { host_id, .. }] if host_id == "stream-host"
        ));

        let events = events.lock().expect("lock final refresh events");
        let event_names = events
            .iter()
            .map(|event| event["event"].as_str().expect("tagged refresh event"))
            .collect::<Vec<_>>();
        assert_eq!(event_names, ["started", "finished"]);
        assert_eq!(events[0]["data"]["status"]["state"]["state"], "running");
        assert_eq!(events[1]["data"]["status"]["state"]["state"], "error");
        assert_eq!(
            event_names.iter().position(|event| *event == "finished"),
            Some(event_names.len() - 1),
            "Finished must be terminal: no channel message may follow it"
        );
    }

    #[test]
    fn ssh_probe_request_ids_are_validated_and_cancellation_is_idempotent() {
        for invalid in ["", "has space", "slash/value", &"x".repeat(129)] {
            let error = validate_probe_request_id(invalid).expect_err("invalid request id");
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
        }
        validate_probe_request_id("probe_01-abc").expect("valid request id");

        let registration = ProbeRegistration::register("probe_cancel_test".to_owned())
            .expect("register probe cancellation");
        ssh_probe_cancel("probe_cancel_test".to_owned()).expect("cancel active probe");
        assert!(registration.cancellation.load(Ordering::Acquire));
        drop(registration);
        ssh_probe_cancel("probe_cancel_test".to_owned()).expect("repeat cancellation is harmless");
    }

    #[test]
    fn ssh_timeout_maps_to_typed_refresh_error_with_remediation() {
        let error = ssh_error(SshError::TimedOut {
            stage: agentlens_core::transport::ssh::CommandStage::Stage1,
            timeout_ms: 20_000,
            detail: "fixture timeout".to_owned(),
        });

        assert_eq!(error.code, IpcErrorCode::Refresh);
        assert_eq!(
            error.fields.get("variant").map(String::as_str),
            Some("timedOut")
        );
        assert!(error
            .fields
            .get("remediation")
            .is_some_and(|text| text.contains("进程树")));
    }

    #[test]
    fn host_crud_keeps_registry_and_scheduler_in_sync() {
        let (_data_dir, state) = state();
        let machine_id_hash = "a".repeat(64);
        let created = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Local workstation".to_owned(),
                kind: HostKind::Local,
                machine_id_hash,
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create host");
        assert_eq!(
            hosts_list_impl(&state).expect("list hosts"),
            vec![created.clone()]
        );
        assert_eq!(
            get_refresh_status_impl(&state)
                .expect("list source status")
                .len(),
            1
        );

        let updated = hosts_update_impl(
            &state,
            HostUpdateInput {
                host_id: created.host_id.clone(),
                display_name: "Remote workstation".to_owned(),
                kind: HostKind::Ssh,
                ssh_target: Some("user@example.test".to_owned()),
                remote_data_dir: Some("/srv/opencode".to_owned()),
                enabled_sources: None,
            },
        )
        .expect("update host");
        assert_eq!(updated.display_name, "Remote workstation");
        assert_eq!(updated.kind, HostKind::Ssh);

        hosts_delete_impl(&state, &created.host_id).expect("delete host");
        assert!(hosts_list_impl(&state)
            .expect("list empty hosts")
            .is_empty());
        assert!(get_refresh_status_impl(&state)
            .expect("list empty status")
            .is_empty());
        assert_eq!(
            hosts_get_impl(&state, &created.host_id)
                .expect_err("deleted host must be absent")
                .code,
            IpcErrorCode::NotFound
        );
    }

    #[test]
    fn aggregate_queries_return_contract_shapes_for_an_empty_archive() {
        let (_data_dir, state) = state();
        let summary = get_summary_impl(
            &state,
            range("2026-01-01", "2026-01-03"),
            "UTC".to_owned(),
            AggregateFilters::default(),
        )
        .expect("query empty summary");
        assert_eq!(summary.message_count, 0);
        assert_eq!(summary.active_session_count, 0);
        assert_eq!(summary.tokens.total_input, 0);

        let trend = get_trend_impl(
            &state,
            range("2026-01-01", "2026-01-03"),
            "UTC".to_owned(),
            Granularity::Day,
            AggregateFilters::default(),
        )
        .expect("query empty trend");
        assert_eq!(trend.total.len(), 2);
        assert!(trend.total.iter().all(|point| {
            point.coverage == crate::contract::CoverageStatus::None
                && point.message_count.is_none()
                && point.tokens.is_none()
        }));
        assert!(trend.groups.is_empty());

        let breakdown = get_breakdown_impl(
            &state,
            range("2026-01-01", "2026-01-03"),
            BreakdownDimensions {
                timezone: "UTC".to_owned(),
                filters: AggregateFilters::default(),
                expand_variant: true,
            },
        )
        .expect("query empty breakdown");
        assert!(breakdown.is_empty());
    }

    #[test]
    fn scalar_parsers_reject_lossy_values_and_preserve_field_names() {
        assert_eq!(
            parse_u32(&json!(u32::MAX), "limit").expect("u32 max"),
            u32::MAX
        );
        for value in [
            json!(-1),
            json!(1.5),
            json!(u64::from(u32::MAX) + 1),
            json!("1"),
        ] {
            let error = parse_u32(&value, "limit").expect_err("lossy u32 must fail");
            assert_eq!(error.code, IpcErrorCode::InvalidInput);
            assert_eq!(error.fields.get("field").map(String::as_str), Some("limit"));
        }

        assert_eq!(
            parse_i64(&json!(-42), "offset").expect("signed offset"),
            -42
        );
        for value in [json!(1.5), json!("-42"), json!(u64::MAX)] {
            let error = parse_i64(&value, "offset").expect_err("non-i64 must fail");
            assert_eq!(
                error.fields.get("field").map(String::as_str),
                Some("offset")
            );
        }

        assert_eq!(
            optional_path(Some("  /tmp/key  ")),
            Some(PathBuf::from("/tmp/key"))
        );
        assert_eq!(optional_path(Some("   ")), None);
        assert_eq!(optional_path(None), None);

        let bad_start = parse_range(&range("01-01-2026", "2026-01-02"), "UTC")
            .expect_err("non-ISO start date must fail");
        assert_eq!(
            bad_start.fields.get("field").map(String::as_str),
            Some("range.startDate")
        );
        let bad_end = parse_range(&range("2026-01-01", "tomorrow"), "UTC")
            .expect_err("non-ISO end date must fail");
        assert_eq!(
            bad_end.fields.get("field").map(String::as_str),
            Some("range.endDateExclusive")
        );
        let bad_timezone = parse_range(&range("2026-01-01", "2026-01-02"), "Mars/Olympus")
            .expect_err("unknown timezone must fail");
        assert_eq!(bad_timezone.code, IpcErrorCode::InvalidTimezone);
    }

    #[test]
    fn host_mutations_reject_invalid_conflicting_and_missing_records() {
        let (_data_dir, state) = state();
        let blank = hosts_get_impl(&state, "   ").expect_err("blank host id must fail");
        assert_eq!(blank.code, IpcErrorCode::InvalidInput);

        let invalid_hash = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Invalid identity".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "not-a-sha256".to_owned(),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect_err("invalid machine hash must fail");
        assert_eq!(invalid_hash.code, IpcErrorCode::InvalidInput);

        let local_with_ssh = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Misrouted local".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "e".repeat(64),
                ssh_target: Some("user@example.test".to_owned()),
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect_err("a local host cannot carry an SSH target");
        assert_eq!(local_with_ssh.code, IpcErrorCode::InvalidInput);
        assert_eq!(
            local_with_ssh.fields.get("field").map(String::as_str),
            Some("sshTarget")
        );

        let first = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "First registration".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "f".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect("create first host");
        let duplicate = hosts_create_impl(
            &state,
            HostCreateInput {
                display_name: "Duplicate registration".to_owned(),
                kind: HostKind::Local,
                machine_id_hash: "f".repeat(64),
                ssh_target: None,
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect_err("duplicate physical machine must fail");
        assert_eq!(duplicate.code, IpcErrorCode::Conflict);

        let missing_update = hosts_update_impl(
            &state,
            HostUpdateInput {
                host_id: "missing-host".to_owned(),
                display_name: "Missing".to_owned(),
                kind: HostKind::Ssh,
                ssh_target: Some("user@example.test".to_owned()),
                remote_data_dir: None,
                enabled_sources: None,
            },
        )
        .expect_err("updating a missing host must fail");
        assert_eq!(missing_update.code, IpcErrorCode::NotFound);

        let missing_delete = hosts_delete_impl(&state, "missing-host")
            .expect_err("deleting a missing host must fail");
        assert_eq!(missing_delete.code, IpcErrorCode::NotFound);
        assert_eq!(
            hosts_get_impl(&state, &first.host_id)
                .expect("failed mutations leave the original host intact")
                .display_name,
            "First registration"
        );
    }

    #[test]
    fn probe_registration_rejects_duplicates_and_async_blank_probe_cleans_up() {
        let registration = ProbeRegistration::register("probe_duplicate_test".to_owned())
            .expect("register first probe");
        let duplicate = match ProbeRegistration::register("probe_duplicate_test".to_owned()) {
            Ok(_) => panic!("duplicate request id must conflict"),
            Err(error) => error,
        };
        assert_eq!(duplicate.code, IpcErrorCode::Conflict);
        assert_eq!(
            duplicate.fields.get("requestId").map(String::as_str),
            Some("probe_duplicate_test")
        );
        drop(registration);
        ProbeRegistration::register("probe_duplicate_test".to_owned())
            .expect("dropping a probe releases its request id");

        let request_id = "probe_async_blank_test";
        let (_data_dir, app) = mock_app();
        let error = tauri::async_runtime::block_on(ssh_probe(
            app,
            SshProbeInput {
                ssh_target: "   ".to_owned(),
                identity_file: Some("  /tmp/id_fixture  ".to_owned()),
                remote_data_dir: None,
            },
            request_id.to_owned(),
        ))
        .expect_err("blank target must fail in the worker");
        assert_eq!(error.code, IpcErrorCode::InvalidInput);
        assert!(
            !probe_cancellations()
                .lock()
                .expect("lock probe registry")
                .contains_key(request_id),
            "the async command must release its request id after completion"
        );
    }

    #[test]
    fn every_ssh_failure_maps_to_a_stable_variant_and_actionable_remediation() {
        let cases = [
            (
                SshError::ArchMismatch {
                    remote_arch: "riscv64".to_owned(),
                    available: vec!["x86_64".to_owned()],
                },
                "archMismatch",
            ),
            (
                SshError::NoWritableCache {
                    detail: "read-only".to_owned(),
                },
                "noWritableCache",
            ),
            (
                SshError::TransferCorrupted {
                    detail: "checksum".to_owned(),
                },
                "transferCorrupted",
            ),
            (
                SshError::AuthFailed {
                    detail: "denied".to_owned(),
                },
                "authFailed",
            ),
            (
                SshError::NoDataDir {
                    detail: "missing".to_owned(),
                },
                "noDataDir",
            ),
            (
                SshError::WalUnreadable {
                    detail: "permissions".to_owned(),
                },
                "walUnreadable",
            ),
            (SshError::ClientCancelled, "clientCancelled"),
            (
                SshError::SshUnavailable {
                    detail: "not installed".to_owned(),
                },
                "sshUnavailable",
            ),
            (
                SshError::InvalidInput {
                    detail: "bad target".to_owned(),
                },
                "invalidInput",
            ),
            (
                SshError::InvalidResponse {
                    stage: CommandStage::Stage1,
                    detail: "bad probe".to_owned(),
                },
                "invalidResponse",
            ),
            (
                SshError::Runner {
                    stage: CommandStage::Stage4,
                    detail: "spawn failed".to_owned(),
                },
                "runner",
            ),
        ];

        for (source, variant) in cases {
            let error = ssh_error(source);
            assert_eq!(error.code, IpcErrorCode::Refresh);
            assert_eq!(
                error.fields.get("variant").map(String::as_str),
                Some(variant)
            );
            assert!(error
                .fields
                .get("remediation")
                .is_some_and(
                    |remediation| remediation.starts_with('请') || remediation.starts_with('可')
                ));
        }
    }

    #[test]
    fn command_error_adapters_keep_domain_specific_codes() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).expect("fixture date");
        for (source, expected) in [
            (
                QueryError::InvalidTimezone("bad/tz".to_owned()),
                IpcErrorCode::InvalidTimezone,
            ),
            (
                QueryError::InvalidDateRange {
                    start_date: date,
                    end_date_exclusive: date,
                },
                IpcErrorCode::InvalidRange,
            ),
            (QueryError::InvalidLimit, IpcErrorCode::InvalidInput),
            (QueryError::NegativeOffset(-1), IpcErrorCode::InvalidInput),
            (
                QueryError::Sqlite(rusqlite::Error::InvalidQuery),
                IpcErrorCode::Database,
            ),
            (
                QueryError::InvalidTimestamp(i64::MAX),
                IpcErrorCode::InvalidInput,
            ),
        ] {
            let error = query_error(source);
            assert_eq!(error.code, expected);
            assert!(!error.message.is_empty());
        }

        for (source, expected) in [
            (
                HostError::DuplicateMachine {
                    machine_id_hash: "a".repeat(64),
                    existing_host_id: "host-a".to_owned(),
                    existing_display_name: "Host A".to_owned(),
                },
                IpcErrorCode::Conflict,
            ),
            (
                HostError::HostAlreadyExists {
                    host_id: "host-a".to_owned(),
                    display_name: "Host A".to_owned(),
                },
                IpcErrorCode::Conflict,
            ),
            (
                HostError::HostNotFound {
                    host_id: "missing".to_owned(),
                },
                IpcErrorCode::NotFound,
            ),
            (
                HostError::Sqlite(rusqlite::Error::InvalidQuery),
                IpcErrorCode::Database,
            ),
            (HostError::MachineIdBlank, IpcErrorCode::InvalidInput),
        ] {
            let error = host_error(source);
            assert_eq!(error.code, expected);
            assert!(!error.message.is_empty());
        }

        let database = database_error("fixture database failure");
        assert_eq!(database.code, IpcErrorCode::Database);
        assert_eq!(database.message, "fixture database failure");
    }
}
