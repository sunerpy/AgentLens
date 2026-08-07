use std::collections::BTreeMap;

use agentlens_core::host::{HostKind as CoreHostKind, HostRecord};
use agentlens_core::hostsource::{
    SourceState as CoreSourceState, SourceStatus as CoreSourceStatus,
    TriggerMode as CoreTriggerMode, TriggerOutcome as CoreTriggerOutcome,
};
use agentlens_core::pricing::{
    CostTotals as CoreCostTotals, PriceCatalog as CorePriceCatalog, PriceEntry as CorePriceEntry,
    PriceMatchKind as CorePriceMatchKind, PriceTable as CorePriceTable,
};
use agentlens_core::query::{
    BreakdownRow as CoreBreakdownRow, CoverageStatus as CoreCoverageStatus,
    DetailCost as CoreDetailCost, DetailPage as CoreDetailPage, DetailRow as CoreDetailRow,
    SeriesBucket as CoreSeriesBucket, Summary as CoreSummary, TokenValues as CoreTokenValues,
};
use serde::{Deserialize, Serialize};

impl std::fmt::Display for IpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IpcError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum IpcErrorCode {
    InvalidInput,
    InvalidRange,
    InvalidTimezone,
    NotFound,
    Conflict,
    Database,
    Pricing,
    Refresh,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

impl IpcError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn invalid_input(field: &str, message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::InvalidInput, message).with_field("field", field)
    }

    pub fn not_found(entity: &str, identifier: &str) -> Self {
        Self::new(
            IpcErrorCode::NotFound,
            format!("{entity} {identifier:?} does not exist"),
        )
        .with_field("entity", entity)
        .with_field("identifier", identifier)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    Monday,
    Sunday,
}

impl From<WeekStart> for agentlens_core::query::WeekStart {
    fn from(value: WeekStart) -> Self {
        match value {
            WeekStart::Monday => Self::Monday,
            WeekStart::Sunday => Self::Sunday,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DateRange {
    pub start_date: String,
    pub end_date_exclusive: String,
    pub week_start: WeekStart,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    Day,
    Week,
    Month,
}

impl From<Granularity> for agentlens_core::query::Granularity {
    fn from(value: Granularity) -> Self {
        match value {
            Granularity::Hour => Self::Hour,
            Granularity::Day => Self::Day,
            Granularity::Week => Self::Week,
            Granularity::Month => Self::Month,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AggregateFilters {
    pub host_id: Option<String>,
    pub source: Option<String>,
    pub agent_key: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

impl From<AggregateFilters> for agentlens_core::query::AggregateFilters {
    fn from(value: AggregateFilters) -> Self {
        Self {
            host_id: value.host_id,
            source: value.source,
            agent_key: value.agent_key,
            provider_id: value.provider_id,
            model_id: value.model_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct BreakdownDimensions {
    pub timezone: String,
    pub filters: AggregateFilters,
    pub expand_variant: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DetailFilters {
    pub host_id: Option<String>,
    pub source: Option<String>,
    pub agent_key: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub is_incomplete: Option<bool>,
}

impl From<DetailFilters> for agentlens_core::query::DetailFilters {
    fn from(value: DetailFilters) -> Self {
        Self {
            host_id: value.host_id,
            source: value.source,
            agent_key: value.agent_key,
            provider_id: value.provider_id,
            model_id: value.model_id,
            is_incomplete: value.is_incomplete,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MessageFilters {
    pub range: DateRange,
    pub timezone: String,
    #[serde(flatten)]
    #[cfg_attr(feature = "ts-export", ts(flatten))]
    pub detail: DetailFilters,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TokenValues {
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub tok_input: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub tok_output: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub tok_reasoning: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub tok_cache_read: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub tok_cache_write: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub total_input: u64,
}

impl From<CoreTokenValues> for TokenValues {
    fn from(value: CoreTokenValues) -> Self {
        Self {
            tok_input: value.tok_input,
            tok_output: value.tok_output,
            tok_reasoning: value.tok_reasoning,
            tok_cache_read: value.tok_cache_read,
            tok_cache_write: value.tok_cache_write,
            total_input: value.total_input,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CostTotals {
    pub actual_sum: f64,
    pub estimated_sum: f64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub unavailable_count: u64,
}

impl From<CoreCostTotals> for CostTotals {
    fn from(value: CoreCostTotals) -> Self {
        Self {
            actual_sum: value.actual_sum,
            estimated_sum: value.estimated_sum,
            unavailable_count: value.unavailable_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum CoverageStatus {
    Full,
    Partial,
    None,
}

impl From<CoreCoverageStatus> for CoverageStatus {
    fn from(value: CoreCoverageStatus) -> Self {
        match value {
            CoreCoverageStatus::Full => Self::Full,
            CoreCoverageStatus::Partial => Self::Partial,
            CoreCoverageStatus::None => Self::None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TimeBucket {
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub start_utc_ms: i64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub end_utc_ms: i64,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    pub bucket: TimeBucket,
    pub coverage: CoverageStatus,
    pub tokens: Option<TokenValues>,
    pub cost: Option<CostTotals>,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub message_count: Option<u64>,
}

impl From<CoreSeriesBucket> for SeriesPoint {
    fn from(value: CoreSeriesBucket) -> Self {
        Self {
            bucket: TimeBucket {
                start_utc_ms: value.bucket.start_utc_ms,
                end_utc_ms: value.bucket.end_utc_ms,
                label: value.bucket.label,
            },
            coverage: value.coverage.into(),
            tokens: value.tokens.map(Into::into),
            cost: value.cost.map(Into::into),
            message_count: value.message_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub tokens: TokenValues,
    pub cost: CostTotals,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub message_count: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub active_session_count: u64,
}

impl From<CoreSummary> for Summary {
    fn from(value: CoreSummary) -> Self {
        Self {
            tokens: value.tokens.into(),
            cost: value.cost.into(),
            message_count: value.message_count,
            active_session_count: value.active_session_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub source: String,
    pub agent_key: String,
    pub agent_raw: String,
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
    pub tokens: TokenValues,
    pub cost: CostTotals,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub message_count: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub active_session_count: u64,
}

impl From<CoreBreakdownRow> for BreakdownRow {
    fn from(value: CoreBreakdownRow) -> Self {
        Self {
            source: value.source,
            agent_key: value.agent_key,
            agent_raw: value.agent_raw,
            provider_id: value.provider_id,
            model_id: value.model_id,
            variant: value.variant,
            tokens: value.tokens.into(),
            cost: value.cost.into(),
            message_count: value.message_count,
            active_session_count: value.active_session_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DetailCost {
    pub actual: Option<f64>,
    pub estimated: Option<f64>,
    pub unavailable: bool,
}

impl From<CoreDetailCost> for DetailCost {
    fn from(value: CoreDetailCost) -> Self {
        Self {
            actual: value.actual,
            estimated: value.estimated,
            unavailable: value.unavailable,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub host_id: String,
    pub source: String,
    pub message_id: String,
    pub session_id: String,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub time_created_utc: i64,
    pub agent_raw: String,
    pub agent_key: String,
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
    pub tokens: TokenValues,
    pub cost: DetailCost,
    pub is_incomplete: bool,
    pub project_dir: String,
}

impl From<CoreDetailRow> for MessageRow {
    fn from(value: CoreDetailRow) -> Self {
        Self {
            host_id: value.host_id,
            source: value.source,
            message_id: value.message_id,
            session_id: value.session_id,
            time_created_utc: value.time_created_utc,
            agent_raw: value.agent_raw,
            agent_key: value.agent_key,
            provider_id: value.provider_id,
            model_id: value.model_id,
            variant: value.variant,
            tokens: value.tokens.into(),
            cost: value.cost.into(),
            is_incomplete: value.is_incomplete,
            project_dir: value.project_dir,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub rows: Vec<MessageRow>,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub total_count: u64,
    pub limit: u32,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub offset: u64,
}

impl From<CoreDetailPage> for MessagePage {
    fn from(value: CoreDetailPage) -> Self {
        Self {
            rows: value.rows.into_iter().map(Into::into).collect(),
            total_count: value.total_count,
            limit: value.limit,
            offset: value.offset,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    Local,
    Ssh,
}

impl From<CoreHostKind> for HostKind {
    fn from(value: CoreHostKind) -> Self {
        match value {
            CoreHostKind::Local => Self::Local,
            CoreHostKind::Ssh => Self::Ssh,
        }
    }
}

impl From<HostKind> for CoreHostKind {
    fn from(value: HostKind) -> Self {
        match value {
            HostKind::Local => Self::Local,
            HostKind::Ssh => Self::Ssh,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub host_id: String,
    pub machine_id_hash: String,
    pub display_name: String,
    pub kind: HostKind,
    pub ssh_target: Option<String>,
    pub remote_data_dir: Option<String>,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub last_success_utc: Option<i64>,
}

impl From<HostRecord> for Host {
    fn from(value: HostRecord) -> Self {
        Self {
            host_id: value.host_id().to_owned(),
            machine_id_hash: value.machine_id_hash().to_owned(),
            display_name: value.display_name,
            kind: value.kind.into(),
            ssh_target: value.ssh_target,
            remote_data_dir: value.remote_data_dir,
            last_success_utc: value.last_success_utc,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostCreateInput {
    pub display_name: String,
    pub kind: HostKind,
    pub machine_id_hash: String,
    pub ssh_target: Option<String>,
    pub remote_data_dir: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct HostUpdateInput {
    pub host_id: String,
    pub display_name: String,
    pub kind: HostKind,
    pub ssh_target: Option<String>,
    pub remote_data_dir: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Auto,
    Manual,
}

impl From<CoreTriggerMode> for TriggerMode {
    fn from(value: CoreTriggerMode) -> Self {
        match value {
            CoreTriggerMode::Auto => Self::Auto,
            CoreTriggerMode::Manual => Self::Manual,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SourceState {
    Idle,
    Running,
    Error {
        last_error: String,
        #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
        last_success: Option<i64>,
    },
}

impl From<CoreSourceState> for SourceState {
    fn from(value: CoreSourceState) -> Self {
        match value {
            CoreSourceState::Idle => Self::Idle,
            CoreSourceState::Running => Self::Running,
            CoreSourceState::Error {
                last_error,
                last_success,
            } => Self::Error {
                last_error,
                last_success,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub host_id: String,
    pub display_name: String,
    pub kind: HostKind,
    pub state: SourceState,
    pub trigger: TriggerMode,
    pub last_error: Option<String>,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub last_success_utc: Option<i64>,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub last_completed_utc: Option<i64>,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub last_duration_ms: Option<u64>,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub interval_ms: u64,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub next_due_utc: Option<i64>,
    pub interrupted: bool,
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub cursor_time_updated: Option<i64>,
}

impl From<CoreSourceStatus> for SourceStatus {
    fn from(value: CoreSourceStatus) -> Self {
        Self {
            host_id: value.host_id,
            display_name: value.display_name,
            kind: value.kind.into(),
            state: value.state.into(),
            trigger: value.trigger.into(),
            last_error: value.last_error,
            last_success_utc: value.last_success_utc,
            last_completed_utc: value.last_completed_utc,
            last_duration_ms: value.last_duration_ms,
            interval_ms: value.interval_ms,
            next_due_utc: value.next_due_utc,
            interrupted: value.interrupted,
            cursor_time_updated: value.cursor_time_updated,
        }
    }
}

/// Ordered progress emitted by one manual `trigger_refresh` invocation.
///
/// `Finished` carries an optional status because deleting a host while its round is running is
/// allowed. In that race the terminal event still closes the stream and tells the frontend to
/// remove the no-longer-registered source instead of leaving it stuck in `running`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
pub enum RefreshEvent {
    Started {
        status: SourceStatus,
    },
    Finished {
        host_id: String,
        status: Option<SourceStatus>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum TriggerRefreshResult {
    Started {
        host_id: String,
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        started_at_utc: i64,
    },
    AlreadyRunning {
        host_id: String,
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        started_at_utc: i64,
    },
}

impl TryFrom<CoreTriggerOutcome> for TriggerRefreshResult {
    type Error = IpcError;

    fn try_from(value: CoreTriggerOutcome) -> Result<Self, Self::Error> {
        match value {
            CoreTriggerOutcome::Started(action) => Ok(Self::Started {
                host_id: action.host_id,
                started_at_utc: action.started_at_utc,
            }),
            CoreTriggerOutcome::AlreadyRunning {
                host_id,
                started_at_utc,
            } => Ok(Self::AlreadyRunning {
                host_id,
                started_at_utc,
            }),
            CoreTriggerOutcome::UnknownHost { host_id } => {
                Err(IpcError::not_found("host", &host_id))
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub values: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PriceEntry {
    pub provider_id: String,
    pub model_id: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl From<CorePriceEntry> for PriceEntry {
    fn from(value: CorePriceEntry) -> Self {
        Self {
            provider_id: value.provider_id,
            model_id: value.model_id,
            input_per_mtok: value.input_per_mtok,
            output_per_mtok: value.output_per_mtok,
            cache_read_per_mtok: value.cache_read_per_mtok,
            cache_write_per_mtok: value.cache_write_per_mtok,
            extra: value.extra,
        }
    }
}

impl From<PriceEntry> for CorePriceEntry {
    fn from(value: PriceEntry) -> Self {
        Self {
            provider_id: value.provider_id,
            model_id: value.model_id,
            input_per_mtok: value.input_per_mtok,
            output_per_mtok: value.output_per_mtok,
            cache_read_per_mtok: value.cache_read_per_mtok,
            cache_write_per_mtok: value.cache_write_per_mtok,
            extra: value.extra,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PriceTable {
    pub schema_version: u32,
    pub entries: Vec<PriceEntry>,
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl From<CorePriceTable> for PriceTable {
    fn from(value: CorePriceTable) -> Self {
        Self {
            schema_version: value.schema_version,
            entries: value.entries.into_iter().map(Into::into).collect(),
            extra: value.extra,
        }
    }
}

impl From<PriceTable> for CorePriceTable {
    fn from(value: PriceTable) -> Self {
        Self {
            schema_version: value.schema_version,
            entries: value.entries.into_iter().map(Into::into).collect(),
            extra: value.extra,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PriceMatchKind {
    Exact,
    Normalized,
    Family,
    Unknown,
}

impl From<CorePriceMatchKind> for PriceMatchKind {
    fn from(value: CorePriceMatchKind) -> Self {
        match value {
            CorePriceMatchKind::Exact => Self::Exact,
            CorePriceMatchKind::Normalized => Self::Normalized,
            CorePriceMatchKind::Family => Self::Family,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ObservedModelPrice {
    pub provider_id: String,
    pub model_id: String,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub usage_count: u64,
    pub match_kind: PriceMatchKind,
    pub matched_price: Option<PriceEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PriceCatalog {
    pub schema_version: u32,
    pub catalog_version: String,
    pub updated_at: String,
    pub currency: String,
    pub entries: Vec<PriceEntry>,
    pub observed_models: Vec<ObservedModelPrice>,
}

impl PriceCatalog {
    pub(crate) fn from_core(
        catalog: &CorePriceCatalog,
        observed_models: Vec<ObservedModelPrice>,
    ) -> Self {
        Self {
            schema_version: catalog.schema_version,
            catalog_version: catalog.catalog_version.clone(),
            updated_at: catalog.updated_at.clone(),
            currency: catalog.currency.clone(),
            entries: catalog.entries.iter().cloned().map(Into::into).collect(),
            observed_models,
        }
    }
}

#[cfg(test)]
mod tests {
    use agentlens_core::hostsource::{RefreshAction, TriggerReason};
    use serde_json::json;

    use super::*;

    #[test]
    fn uncovered_series_values_serialize_as_null_and_covered_zero_stays_zero() {
        let uncovered = SeriesPoint {
            bucket: TimeBucket {
                start_utc_ms: 0,
                end_utc_ms: 1,
                label: "empty".to_owned(),
            },
            coverage: CoverageStatus::None,
            tokens: None,
            cost: None,
            message_count: None,
        };
        assert_eq!(
            serde_json::to_value(uncovered).expect("serialize uncovered point"),
            json!({
                "bucket": { "startUtcMs": 0, "endUtcMs": 1, "label": "empty" },
                "coverage": "none",
                "tokens": null,
                "cost": null,
                "messageCount": null
            })
        );

        let covered = SeriesPoint {
            bucket: TimeBucket {
                start_utc_ms: 0,
                end_utc_ms: 1,
                label: "covered".to_owned(),
            },
            coverage: CoverageStatus::Full,
            tokens: Some(TokenValues::default()),
            cost: Some(CostTotals::default()),
            message_count: Some(0),
        };
        let encoded = serde_json::to_value(covered).expect("serialize covered point");
        assert_eq!(encoded["messageCount"], 0);
        assert_eq!(encoded["tokens"]["totalInput"], 0);
        assert_eq!(
            encoded["cost"],
            json!({ "actualSum": 0.0, "estimatedSum": 0.0, "unavailableCount": 0 })
        );
    }

    #[test]
    fn ipc_error_is_a_serializable_object() {
        let error = IpcError::invalid_input("range.startDate", "invalid date");
        let encoded = serde_json::to_value(error).expect("serialize IPC error");
        assert_eq!(encoded["code"], "invalidInput");
        assert_eq!(encoded["message"], "invalid date");
        assert_eq!(encoded["fields"]["field"], "range.startDate");
    }

    fn core_tokens() -> CoreTokenValues {
        CoreTokenValues {
            tok_input: 11,
            tok_output: 12,
            tok_reasoning: 13,
            tok_cache_read: 14,
            tok_cache_write: 15,
            total_input: 40,
        }
    }

    fn core_cost() -> CoreCostTotals {
        CoreCostTotals {
            actual_sum: 1.25,
            estimated_sum: 2.5,
            unavailable_count: 3,
        }
    }

    #[test]
    fn enum_and_error_contracts_keep_every_wire_discriminator_stable() {
        assert_eq!(
            agentlens_core::query::WeekStart::from(WeekStart::Monday),
            agentlens_core::query::WeekStart::Monday
        );
        assert_eq!(
            agentlens_core::query::WeekStart::from(WeekStart::Sunday),
            agentlens_core::query::WeekStart::Sunday
        );

        for (dto, core) in [
            (Granularity::Hour, agentlens_core::query::Granularity::Hour),
            (Granularity::Day, agentlens_core::query::Granularity::Day),
            (Granularity::Week, agentlens_core::query::Granularity::Week),
            (
                Granularity::Month,
                agentlens_core::query::Granularity::Month,
            ),
        ] {
            assert_eq!(agentlens_core::query::Granularity::from(dto), core);
        }

        for (dto, core) in [
            (HostKind::Local, CoreHostKind::Local),
            (HostKind::Ssh, CoreHostKind::Ssh),
        ] {
            assert_eq!(CoreHostKind::from(dto), core);
            assert_eq!(HostKind::from(core), dto);
        }

        for (core, dto) in [
            (CoreCoverageStatus::Full, CoverageStatus::Full),
            (CoreCoverageStatus::Partial, CoverageStatus::Partial),
            (CoreCoverageStatus::None, CoverageStatus::None),
        ] {
            assert_eq!(CoverageStatus::from(core), dto);
        }
        for (core, dto) in [
            (CoreTriggerMode::Auto, TriggerMode::Auto),
            (CoreTriggerMode::Manual, TriggerMode::Manual),
        ] {
            assert_eq!(TriggerMode::from(core), dto);
        }

        let error = IpcError::not_found("host", "missing-host");
        assert_eq!(error.to_string(), "host \"missing-host\" does not exist");
        assert_eq!(error.fields.get("entity").map(String::as_str), Some("host"));
        assert_eq!(
            error.fields.get("identifier").map(String::as_str),
            Some("missing-host")
        );
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn aggregate_and_detail_conversions_preserve_every_numeric_and_identity_field() {
        let tokens = TokenValues::from(core_tokens());
        assert_eq!(tokens.tok_input, 11);
        assert_eq!(tokens.tok_output, 12);
        assert_eq!(tokens.tok_reasoning, 13);
        assert_eq!(tokens.tok_cache_read, 14);
        assert_eq!(tokens.tok_cache_write, 15);
        assert_eq!(tokens.total_input, 40);

        let cost = CostTotals::from(core_cost());
        assert_eq!(cost.actual_sum, 1.25);
        assert_eq!(cost.estimated_sum, 2.5);
        assert_eq!(cost.unavailable_count, 3);

        let summary = Summary::from(CoreSummary {
            tokens: core_tokens(),
            cost: core_cost(),
            message_count: 21,
            active_session_count: 8,
        });
        assert_eq!(summary.tokens, tokens);
        assert_eq!(summary.cost, cost);
        assert_eq!(summary.message_count, 21);
        assert_eq!(summary.active_session_count, 8);

        let series = SeriesPoint::from(CoreSeriesBucket {
            bucket: agentlens_core::query::TimeBucket {
                start_utc_ms: 100,
                end_utc_ms: 200,
                label: "fixture-hour".to_owned(),
            },
            coverage: CoreCoverageStatus::Full,
            tokens: Some(core_tokens()),
            cost: Some(core_cost()),
            message_count: Some(5),
        });
        assert_eq!(series.bucket.start_utc_ms, 100);
        assert_eq!(series.bucket.end_utc_ms, 200);
        assert_eq!(series.bucket.label, "fixture-hour");
        assert_eq!(series.coverage, CoverageStatus::Full);
        assert_eq!(series.tokens, Some(tokens));
        assert_eq!(series.cost, Some(cost));
        assert_eq!(series.message_count, Some(5));

        let breakdown = BreakdownRow::from(CoreBreakdownRow {
            source: "opencode".to_owned(),
            agent_key: "build".to_owned(),
            agent_raw: "Build Agent".to_owned(),
            provider_id: "provider".to_owned(),
            model_id: "model".to_owned(),
            variant: Some("xhigh".to_owned()),
            tokens: core_tokens(),
            cost: core_cost(),
            message_count: 34,
            active_session_count: 9,
        });
        assert_eq!(breakdown.source, "opencode");
        assert_eq!(breakdown.agent_key, "build");
        assert_eq!(breakdown.agent_raw, "Build Agent");
        assert_eq!(breakdown.provider_id, "provider");
        assert_eq!(breakdown.model_id, "model");
        assert_eq!(breakdown.variant.as_deref(), Some("xhigh"));
        assert_eq!(breakdown.tokens, tokens);
        assert_eq!(breakdown.cost, cost);
        assert_eq!(breakdown.message_count, 34);
        assert_eq!(breakdown.active_session_count, 9);

        let detail_cost = DetailCost::from(CoreDetailCost {
            actual: Some(0.75),
            estimated: Some(1.5),
            unavailable: false,
        });
        assert_eq!(detail_cost.actual, Some(0.75));
        assert_eq!(detail_cost.estimated, Some(1.5));
        assert!(!detail_cost.unavailable);

        let page = MessagePage::from(CoreDetailPage {
            rows: vec![CoreDetailRow {
                host_id: "host-1".to_owned(),
                source: "opencode".to_owned(),
                message_id: "message-1".to_owned(),
                session_id: "session-1".to_owned(),
                time_created_utc: 1_234,
                agent_raw: "Build Agent".to_owned(),
                agent_key: "build".to_owned(),
                provider_id: "provider".to_owned(),
                model_id: "model".to_owned(),
                variant: Some("high".to_owned()),
                tokens: core_tokens(),
                cost: CoreDetailCost {
                    actual: Some(0.75),
                    estimated: None,
                    unavailable: false,
                },
                is_incomplete: true,
                project_dir: "/workspace/project".to_owned(),
            }],
            total_count: 17,
            limit: 25,
            offset: 50,
        });
        assert_eq!(page.total_count, 17);
        assert_eq!(page.limit, 25);
        assert_eq!(page.offset, 50);
        assert_eq!(page.rows.len(), 1);
        let row = &page.rows[0];
        assert_eq!(row.host_id, "host-1");
        assert_eq!(row.source, "opencode");
        assert_eq!(row.message_id, "message-1");
        assert_eq!(row.session_id, "session-1");
        assert_eq!(row.time_created_utc, 1_234);
        assert_eq!(row.agent_raw, "Build Agent");
        assert_eq!(row.agent_key, "build");
        assert_eq!(row.provider_id, "provider");
        assert_eq!(row.model_id, "model");
        assert_eq!(row.variant.as_deref(), Some("high"));
        assert_eq!(row.tokens, tokens);
        assert_eq!(row.cost.actual, Some(0.75));
        assert!(row.is_incomplete);
        assert_eq!(row.project_dir, "/workspace/project");
    }

    #[test]
    fn scheduler_state_and_trigger_conversions_preserve_lifecycle_context() {
        assert_eq!(SourceState::from(CoreSourceState::Idle), SourceState::Idle);
        assert_eq!(
            SourceState::from(CoreSourceState::Running),
            SourceState::Running
        );
        assert_eq!(
            SourceState::from(CoreSourceState::Error {
                last_error: "network failed".to_owned(),
                last_success: Some(99),
            }),
            SourceState::Error {
                last_error: "network failed".to_owned(),
                last_success: Some(99),
            }
        );

        let status = SourceStatus::from(CoreSourceStatus {
            host_id: "host-a".to_owned(),
            display_name: "Host A".to_owned(),
            kind: CoreHostKind::Ssh,
            state: CoreSourceState::Error {
                last_error: "timeout".to_owned(),
                last_success: Some(10),
            },
            trigger: CoreTriggerMode::Manual,
            last_error: Some("timeout".to_owned()),
            last_success_utc: Some(10),
            last_completed_utc: Some(20),
            last_duration_ms: Some(30),
            interval_ms: 900_000,
            next_due_utc: None,
            interrupted: false,
            cursor_time_updated: Some(40),
        });
        assert_eq!(status.host_id, "host-a");
        assert_eq!(status.display_name, "Host A");
        assert_eq!(status.kind, HostKind::Ssh);
        assert!(matches!(status.state, SourceState::Error { .. }));
        assert_eq!(status.trigger, TriggerMode::Manual);
        assert_eq!(status.last_error.as_deref(), Some("timeout"));
        assert_eq!(status.last_success_utc, Some(10));
        assert_eq!(status.last_completed_utc, Some(20));
        assert_eq!(status.last_duration_ms, Some(30));
        assert_eq!(status.interval_ms, 900_000);
        assert_eq!(status.next_due_utc, None);
        assert!(!status.interrupted);
        assert_eq!(status.cursor_time_updated, Some(40));

        let started = TriggerRefreshResult::try_from(CoreTriggerOutcome::Started(RefreshAction {
            host_id: "host-a".to_owned(),
            kind: CoreHostKind::Local,
            reason: TriggerReason::Manual,
            started_at_utc: 100,
        }))
        .expect("started outcome");
        assert_eq!(
            started,
            TriggerRefreshResult::Started {
                host_id: "host-a".to_owned(),
                started_at_utc: 100,
            }
        );

        let already_running = TriggerRefreshResult::try_from(CoreTriggerOutcome::AlreadyRunning {
            host_id: "host-a".to_owned(),
            started_at_utc: 100,
        })
        .expect("already-running outcome");
        assert_eq!(
            already_running,
            TriggerRefreshResult::AlreadyRunning {
                host_id: "host-a".to_owned(),
                started_at_utc: 100,
            }
        );

        let unknown = TriggerRefreshResult::try_from(CoreTriggerOutcome::UnknownHost {
            host_id: "missing-host".to_owned(),
        })
        .expect_err("unknown host must become an IPC error");
        assert_eq!(unknown.code, IpcErrorCode::NotFound);
        assert_eq!(
            unknown.fields.get("identifier").map(String::as_str),
            Some("missing-host")
        );
    }
}
