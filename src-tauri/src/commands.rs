use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use agentlens_core::archive::{read_app_settings, write_app_settings};
use agentlens_core::host::{
    local_machine_identity as core_local_machine_identity, HostError, HostRecord, HostRegistry,
    MachineIdentity,
};
use agentlens_core::hostsource::SourceRegistration;
use agentlens_core::pricing::PriceTable as CorePriceTable;
use agentlens_core::query::{
    query_breakdown, query_details, query_series, query_summary, BreakdownOptions, LocalDateRange,
    QueryError,
};
use agentlens_core::source::opencode_legacy::CoverageStore;
use agentlens_core::transport::ssh::{
    SshAuthentication, SshError, SshTransport, StdCommandRunner, SSH_PROBE_WALL_TIMEOUT,
};
use chrono::NaiveDate;
use serde_json::Value;
use tauri::State;

use crate::contract::{
    AggregateFilters, AppSettings, BreakdownDimensions, BreakdownRow, DateRange, DetailFilters,
    Granularity, Host, HostCreateInput, HostKind, HostUpdateInput, IpcError, IpcErrorCode,
    MessageFilters, MessagePage, PriceTable, SeriesPoint, SourceStatus, Summary,
    TriggerRefreshResult,
};
use crate::credentials::{
    CredentialError, CredentialKind, CredentialRef, CredentialStatus, CredentialStore,
    LocalIdentity, OsKeyringStore, Secret, SshProbeInput, SshProbeResult,
};
use crate::state::AppState;

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

#[tauri::command]
pub fn get_summary(
    state: State<'_, AppState>,
    range: DateRange,
    tz: String,
    filters: AggregateFilters,
) -> IpcResult<Summary> {
    state.tick_due()?;
    get_summary_impl(&state, range, tz, filters)
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
pub fn get_trend(
    state: State<'_, AppState>,
    range: DateRange,
    tz: String,
    granularity: Granularity,
    filters: Option<AggregateFilters>,
) -> IpcResult<Vec<SeriesPoint>> {
    state.tick_due()?;
    get_trend_impl(&state, range, tz, granularity, filters.unwrap_or_default())
}

pub(crate) fn get_trend_impl(
    state: &AppState,
    range: DateRange,
    tz: String,
    granularity: Granularity,
    filters: AggregateFilters,
) -> IpcResult<Vec<SeriesPoint>> {
    let range = parse_range(&range, &tz)?;
    let filters = filters.into();
    let prices = state.load_prices()?;
    let archive = state.lock_archive()?;
    let coverage = CoverageStore::load(archive.connection()).map_err(database_error)?;
    query_series(
        &archive,
        &range,
        granularity.into(),
        &filters,
        &prices,
        &coverage,
    )
    .map(|rows| rows.into_iter().map(Into::into).collect())
    .map_err(query_error)
}

#[tauri::command]
pub fn get_breakdown(
    state: State<'_, AppState>,
    range: DateRange,
    dims: BreakdownDimensions,
) -> IpcResult<Vec<BreakdownRow>> {
    state.tick_due()?;
    get_breakdown_impl(&state, range, dims)
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
pub fn query_messages(
    state: State<'_, AppState>,
    filters: MessageFilters,
    limit: Value,
    offset: Value,
) -> IpcResult<MessagePage> {
    state.tick_due()?;
    query_messages_impl(&state, filters, limit, offset)
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
pub fn hosts_list(state: State<'_, AppState>) -> IpcResult<Vec<Host>> {
    state.tick_due()?;
    hosts_list_impl(&state)
}

pub(crate) fn hosts_list_impl(state: &AppState) -> IpcResult<Vec<Host>> {
    HostRegistry::new(state.lock_archive()?.connection())
        .list()
        .map(|hosts| hosts.into_iter().map(Into::into).collect())
        .map_err(host_error)
}

#[tauri::command]
pub fn hosts_get(state: State<'_, AppState>, host_id: String) -> IpcResult<Host> {
    state.tick_due()?;
    hosts_get_impl(&state, &host_id)
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
pub fn hosts_create(state: State<'_, AppState>, input: HostCreateInput) -> IpcResult<Host> {
    hosts_create_impl(&state, input)
}

pub(crate) fn hosts_create_impl(state: &AppState, input: HostCreateInput) -> IpcResult<Host> {
    let identity =
        MachineIdentity::from_machine_id_hash(&input.machine_id_hash).map_err(host_error)?;
    let host = build_host_record(
        input.display_name,
        input.kind,
        input.ssh_target,
        input.remote_data_dir,
        &identity,
    )?;
    {
        let archive = state.lock_archive()?;
        HostRegistry::new(archive.connection())
            .insert(&host)
            .map_err(host_error)?;
    }
    state.register_host(SourceRegistration::from_host(&host))?;
    Ok(host.into())
}

#[tauri::command]
pub fn hosts_update(state: State<'_, AppState>, input: HostUpdateInput) -> IpcResult<Host> {
    hosts_update_impl(&state, input)
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
        &identity,
    )?;
    {
        let archive = state.lock_archive()?;
        HostRegistry::new(archive.connection())
            .update(&host)
            .map_err(host_error)?;
    }
    state.replace_host_registration(SourceRegistration::from_host(&host))?;
    hosts_get_impl(state, &input.host_id)
}

#[tauri::command]
pub fn hosts_delete(state: State<'_, AppState>, host_id: String) -> IpcResult<()> {
    hosts_delete_impl(&state, &host_id)
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
pub fn trigger_refresh(
    state: State<'_, AppState>,
    host_id: String,
) -> IpcResult<TriggerRefreshResult> {
    state.trigger_refresh(&host_id)
}

#[tauri::command]
pub fn get_refresh_status(state: State<'_, AppState>) -> IpcResult<Vec<SourceStatus>> {
    state.tick_due()?;
    get_refresh_status_impl(&state)
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
pub fn get_settings(state: State<'_, AppState>) -> IpcResult<AppSettings> {
    get_settings_impl(&state)
}

pub(crate) fn get_settings_impl(state: &AppState) -> IpcResult<AppSettings> {
    let values = read_app_settings(state.lock_archive()?.connection()).map_err(database_error)?;
    Ok(AppSettings { values })
}

#[tauri::command]
pub fn set_settings(state: State<'_, AppState>, settings: AppSettings) -> IpcResult<AppSettings> {
    set_settings_impl(&state, settings)
}

pub(crate) fn set_settings_impl(state: &AppState, settings: AppSettings) -> IpcResult<AppSettings> {
    let mut archive = state.lock_archive()?;
    write_app_settings(archive.connection_mut(), &settings.values).map_err(database_error)?;
    drop(archive);
    get_settings_impl(state)
}

#[tauri::command]
pub fn prices_get(state: State<'_, AppState>) -> IpcResult<PriceTable> {
    prices_get_impl(&state)
}

pub(crate) fn prices_get_impl(state: &AppState) -> IpcResult<PriceTable> {
    state.load_prices().map(Into::into)
}

#[tauri::command]
pub fn prices_set(state: State<'_, AppState>, prices: Value) -> IpcResult<PriceTable> {
    prices_set_impl(&state, prices)
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
pub fn local_machine_identity() -> IpcResult<LocalIdentity> {
    local_machine_identity_impl()
}

pub(crate) fn local_machine_identity_impl() -> IpcResult<LocalIdentity> {
    let identity = core_local_machine_identity().map_err(host_error)?;
    Ok(LocalIdentity {
        host_id: identity.host_id().to_owned(),
        machine_id_hash: identity.machine_id_hash().to_owned(),
        hostname: local_hostname(),
    })
}

#[tauri::command]
pub async fn ssh_probe(input: SshProbeInput, request_id: String) -> IpcResult<SshProbeResult> {
    let registration = ProbeRegistration::register(request_id)?;
    let cancellation = Arc::clone(&registration.cancellation);
    let result = tauri::async_runtime::spawn_blocking(move || {
        ssh_probe_impl_with_cancel(input, &|| cancellation.load(Ordering::Acquire))
    })
    .await
    .map_err(|error| {
        IpcError::new(
            IpcErrorCode::Internal,
            format!("SSH probe worker failed: {error}"),
        )
    })?;
    drop(registration);
    result
}

#[cfg(test)]
pub(crate) fn ssh_probe_impl(input: SshProbeInput) -> IpcResult<SshProbeResult> {
    ssh_probe_impl_with_cancel(input, &|| false)
}

fn ssh_probe_impl_with_cancel(
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
    let probe = SshTransport::probe_connection_with_timeout(
        StdCommandRunner,
        None,
        SshAuthentication::Batch { identity_file },
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
pub fn credential_set(
    host_id: String,
    kind: CredentialKind,
    secret: String,
) -> IpcResult<CredentialStatus> {
    credential_set_impl(&OsKeyringStore, &host_id, kind, secret)
}

pub(crate) fn credential_set_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
    secret: String,
) -> IpcResult<CredentialStatus> {
    store
        .store(&CredentialRef::new(host_id, kind), &Secret::new(secret))
        .map_err(credential_error)
}

#[tauri::command]
pub fn credential_status(host_id: String, kind: CredentialKind) -> IpcResult<CredentialStatus> {
    credential_status_impl(&OsKeyringStore, &host_id, kind)
}

pub(crate) fn credential_status_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
) -> IpcResult<CredentialStatus> {
    store
        .status(&CredentialRef::new(host_id, kind))
        .map_err(credential_error)
}

#[tauri::command]
pub fn credential_delete(host_id: String, kind: CredentialKind) -> IpcResult<CredentialStatus> {
    credential_delete_impl(&OsKeyringStore, &host_id, kind)
}

pub(crate) fn credential_delete_impl(
    store: &dyn CredentialStore,
    host_id: &str,
    kind: CredentialKind,
) -> IpcResult<CredentialStatus> {
    let reference = CredentialRef::new(host_id, kind);
    store.delete(&reference).map_err(credential_error)?;
    store.status(&reference).map_err(credential_error)
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
    Ok(host.with_remote_data_dir(remote_data_dir))
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
pub const REGISTERED_COMMANDS: [&str; 21] = [
    "get_summary",
    "get_trend",
    "get_breakdown",
    "query_messages",
    "hosts_list",
    "hosts_get",
    "hosts_create",
    "hosts_update",
    "hosts_delete",
    "trigger_refresh",
    "get_refresh_status",
    "get_settings",
    "set_settings",
    "prices_get",
    "prices_set",
    "local_machine_identity",
    "ssh_probe",
    "ssh_probe_cancel",
    "credential_set",
    "credential_status",
    "credential_delete",
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::contract::{PriceEntry, WeekStart};
    use crate::credentials::InMemoryCredentialStore;

    fn state() -> (TempDir, AppState) {
        let data_dir = tempfile::tempdir().expect("create temporary data directory");
        let state = AppState::open_in_data_dir(data_dir.path()).expect("open test app state");
        (data_dir, state)
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
        assert_eq!(REGISTERED_COMMANDS.len(), 21);
        assert!(REGISTERED_COMMANDS.contains(&"query_messages"));
        assert!(REGISTERED_COMMANDS.contains(&"trigger_refresh"));
        assert!(REGISTERED_COMMANDS.contains(&"prices_set"));
        assert!(REGISTERED_COMMANDS.contains(&"ssh_probe_cancel"));
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
        )
        .expect("store passphrase");
        assert!(stored.present);
        let encoded = serde_json::to_string(&stored).expect("serialize credential status");
        assert!(
            !encoded.contains(secret),
            "credential_set response leaked the secret: {encoded}"
        );

        assert!(
            credential_status_impl(&store, host_id, CredentialKind::Passphrase)
                .expect("read status")
                .present
        );
        assert!(
            !credential_status_impl(&store, host_id, CredentialKind::Password)
                .expect("read unrelated status")
                .present,
            "a password entry must not be implied by a stored passphrase"
        );

        assert!(
            !credential_delete_impl(&store, host_id, CredentialKind::Passphrase)
                .expect("delete passphrase")
                .present
        );
        assert!(
            !credential_delete_impl(&store, host_id, CredentialKind::Passphrase)
                .expect("second delete is idempotent")
                .present
        );
    }

    #[test]
    fn credential_command_errors_carry_variant_and_chinese_remediation() {
        let store = InMemoryCredentialStore::new();
        let blank = credential_set_impl(&store, "  ", CredentialKind::Password, "x".to_owned())
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

        assert_probe_future(ssh_probe(
            SshProbeInput {
                ssh_target: "fixture.invalid".to_owned(),
                identity_file: None,
                remote_data_dir: None,
            },
            "probe_async_contract".to_owned(),
        ));
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
}
