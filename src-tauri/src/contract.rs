use std::collections::BTreeMap;

use agentlens_core::host::{HostKind as CoreHostKind, HostRecord};
use agentlens_core::hostsource::{
    SourceState as CoreSourceState, SourceStatus as CoreSourceStatus,
    TriggerMode as CoreTriggerMode, TriggerOutcome as CoreTriggerOutcome,
};
use agentlens_core::pricing::{
    CostTotals as CoreCostTotals, PriceEntry as CorePriceEntry, PriceTable as CorePriceTable,
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

#[cfg(test)]
mod tests {
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
}
