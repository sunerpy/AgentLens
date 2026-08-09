//! 聚合查询引擎：报表时区分桶与双维度下钻（todo 8）。
//!
//! 本模块将定义 `LocalDateRange { start_date, end_date_exclusive, tz, week_start }`，
//! 支持 hour / day / week / month 粒度与自定义半开区间 `[start, end)`。
//!
//! DST 算法对全部 IANA 时区都是全函数：日界线由报表时区本地 00:00 的 `LocalResult`
//! 三态显式处理（`Single` 取该时刻、`Ambiguous` 取 `earliest()`、`None` 取缺失区间结束时刻）；
//! hour 桶自日界 UTC 时刻起按 60 分钟递增，每桶为 `[start, min(start + 1h, 次日日界))`，
//! 末桶截断到日界，因此天然覆盖 23/25 桶以及 30 分钟 DST 产生的尾桶，
//! 且 hour 桶并集恒等于 day 桶；fall-back 重复的本地小时标签携带 UTC 偏移。
//! 桶边界在报表时区计算后转 UTC epoch ms 再交给 SQL 聚合。
//!
//! 输出五个原子值 `tok_input` / `tok_output` / `tok_reasoning` / `tok_cache_read` /
//! `tok_cache_write` 与派生 `total_input = input + cache_read + cache_write`；
//! 每桶携带覆盖状态 full / partial / none（见 todo 7），`none` 返回 `None`、
//! `full` 且无记录返回 0。维度顺序为 source → agent_key → (provider_id, model_id[, variant])。
//!
//! 另提供汇总卡查询与明细分页查询（limit/offset + total_count + host/source/agent_key/
//! model/is_incomplete 过滤器）；`is_incomplete` 记录排除出聚合但在明细中可见。

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::archive::{Archive, CostSource};
use crate::pricing::{CostTotals, PriceTable, ResolvedCost, TokenCounts};

/// Earliest local calendar year accepted by report queries.
pub const MIN_REPORT_YEAR: i32 = 1970;
/// Latest local calendar year accepted by report queries.
pub const MAX_REPORT_YEAR: i32 = 2100;
/// Largest accepted local-date range, preventing accidental multi-century bucket expansion.
pub const MAX_REPORT_RANGE_DAYS: i64 = 3_660;
/// Hard server-side page-size cap shared with the IPC layer.
pub const MAX_DETAIL_LIMIT: u32 = 200;

const SUMMARY_SQL: &str = "SELECT
        coalesce(sum(tok_input), 0),
        coalesce(sum(tok_output), 0),
        coalesce(sum(tok_reasoning), 0),
        coalesce(sum(tok_cache_read), 0),
        coalesce(sum(tok_cache_write), 0),
        coalesce(sum(CASE WHEN granularity = 'message' THEN 1 ELSE 0 END), 0),
        coalesce(sum(CASE WHEN granularity = 'session' THEN 1 ELSE 0 END), 0),
        count(DISTINCT session_id)
    FROM usage_record
    WHERE is_incomplete = 0
      AND time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)";

const COST_ROWS_SQL: &str = "SELECT
        provider_id, model_id, cost_source,
        CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN 1 ELSE 0 END,
        coalesce(sum(tok_input), 0),
        coalesce(sum(tok_output), 0),
        coalesce(sum(tok_reasoning), 0),
        coalesce(sum(tok_cache_read), 0),
        coalesce(sum(tok_cache_write), 0),
        sum(CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN cost END),
        sum(CASE
            WHEN (cost IS NOT NULL AND abs(cost) < 1e999)
              OR tok_input > 0 OR tok_output > 0 OR tok_cache_read > 0 OR tok_cache_write > 0
            THEN 1 ELSE 0
        END),
        min(tok_input), min(tok_output), min(tok_reasoning), min(tok_cache_read), min(tok_cache_write)
    FROM usage_record
    WHERE is_incomplete = 0
      AND time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
    GROUP BY provider_id, model_id, cost_source,
        CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN 1 ELSE 0 END";

const BREAKDOWN_WITH_VARIANT_SQL: &str = "SELECT
        source, agent_key, max(agent_raw), provider_id, model_id, variant,
        coalesce(sum(tok_input), 0),
        coalesce(sum(tok_output), 0),
        coalesce(sum(tok_reasoning), 0),
        coalesce(sum(tok_cache_read), 0),
        coalesce(sum(tok_cache_write), 0),
        coalesce(sum(CASE WHEN granularity = 'message' THEN 1 ELSE 0 END), 0),
        coalesce(sum(CASE WHEN granularity = 'session' THEN 1 ELSE 0 END), 0),
        count(DISTINCT session_id)
    FROM usage_record
    WHERE is_incomplete = 0
      AND time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
    GROUP BY source, agent_key, provider_id, model_id, variant
    ORDER BY source, agent_key, provider_id, model_id, variant";

const BREAKDOWN_COLLAPSED_SQL: &str = "SELECT
        source, agent_key, max(agent_raw), provider_id, model_id, NULL,
        coalesce(sum(tok_input), 0),
        coalesce(sum(tok_output), 0),
        coalesce(sum(tok_reasoning), 0),
        coalesce(sum(tok_cache_read), 0),
        coalesce(sum(tok_cache_write), 0),
        coalesce(sum(CASE WHEN granularity = 'message' THEN 1 ELSE 0 END), 0),
        coalesce(sum(CASE WHEN granularity = 'session' THEN 1 ELSE 0 END), 0),
        count(DISTINCT session_id)
    FROM usage_record
    WHERE is_incomplete = 0
      AND time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
    GROUP BY source, agent_key, provider_id, model_id
    ORDER BY source, agent_key, provider_id, model_id";

const BREAKDOWN_COST_SQL: &str = "SELECT
        source, agent_key, provider_id, model_id, variant, cost_source,
        CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN 1 ELSE 0 END,
        coalesce(sum(tok_input), 0),
        coalesce(sum(tok_output), 0),
        coalesce(sum(tok_reasoning), 0),
        coalesce(sum(tok_cache_read), 0),
        coalesce(sum(tok_cache_write), 0),
        sum(CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN cost END),
        sum(CASE
            WHEN tok_input > 0 OR tok_output > 0 OR tok_cache_read > 0 OR tok_cache_write > 0
            THEN 1 ELSE 0
        END),
        min(tok_input), min(tok_output), min(tok_reasoning), min(tok_cache_read), min(tok_cache_write)
    FROM usage_record
    WHERE is_incomplete = 0
      AND time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
    GROUP BY source, agent_key, provider_id, model_id, variant, cost_source,
        CASE WHEN cost IS NOT NULL AND abs(cost) < 1e999 THEN 1 ELSE 0 END";

const SERIES_GROUPED_SQL_SUFFIX: &str = ")
    SELECT
        report_bucket.bucket_index,
        usage_record.source,
        usage_record.agent_key,
        max(usage_record.agent_raw),
        usage_record.provider_id,
        usage_record.model_id,
        usage_record.cost_source,
        CASE
            WHEN usage_record.cost IS NOT NULL AND abs(usage_record.cost) < 1e999 THEN 1
            ELSE 0
        END AS has_trusted_cost,
        coalesce(sum(usage_record.tok_input), 0),
        coalesce(sum(usage_record.tok_output), 0),
        coalesce(sum(usage_record.tok_reasoning), 0),
        coalesce(sum(usage_record.tok_cache_read), 0),
        coalesce(sum(usage_record.tok_cache_write), 0),
        coalesce(sum(CASE WHEN usage_record.granularity = 'message' THEN 1 ELSE 0 END), 0),
        coalesce(sum(CASE WHEN usage_record.granularity = 'session' THEN 1 ELSE 0 END), 0),
        sum(CASE
            WHEN usage_record.cost IS NOT NULL AND abs(usage_record.cost) < 1e999
            THEN usage_record.cost
        END),
        sum(CASE
            WHEN usage_record.tok_input > 0
              OR usage_record.tok_output > 0
              OR usage_record.tok_cache_read > 0
              OR usage_record.tok_cache_write > 0
            THEN 1 ELSE 0
        END),
        min(usage_record.tok_input),
        min(usage_record.tok_output),
        min(usage_record.tok_reasoning),
        min(usage_record.tok_cache_read),
        min(usage_record.tok_cache_write)
    FROM report_bucket
    JOIN usage_record
      ON usage_record.time_created_utc >= report_bucket.start_utc_ms
     AND usage_record.time_created_utc < report_bucket.end_utc_ms
    WHERE usage_record.is_incomplete = 0
      AND (?1 IS NULL OR usage_record.host_id = ?1)
      AND (?2 IS NULL OR usage_record.source = ?2)
      AND (?3 IS NULL OR usage_record.agent_key = ?3)
      AND (?4 IS NULL OR usage_record.provider_id = ?4)
      AND (?5 IS NULL OR usage_record.model_id = ?5)
    GROUP BY
        report_bucket.bucket_index,
        usage_record.source,
        usage_record.agent_key,
        usage_record.provider_id,
        usage_record.model_id,
        usage_record.cost_source,
        has_trusted_cost
    ORDER BY
        report_bucket.bucket_index,
        usage_record.source,
        usage_record.agent_key,
        usage_record.provider_id,
        usage_record.model_id,
        usage_record.cost_source,
        has_trusted_cost";

const DETAIL_COUNT_SQL: &str = "SELECT count(*)
    FROM usage_record
    WHERE time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
      AND (?8 IS NULL OR is_incomplete = ?8)";

const DETAIL_ROWS_SQL: &str = "SELECT
        host_id, source, message_id, session_id, time_created_utc,
        agent_raw, agent_key, provider_id, model_id, variant,
        tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
        cost, cost_source, is_incomplete, project_dir
    FROM usage_record
    WHERE time_created_utc >= ?1
      AND time_created_utc < ?2
      AND (?3 IS NULL OR host_id = ?3)
      AND (?4 IS NULL OR source = ?4)
      AND (?5 IS NULL OR agent_key = ?5)
      AND (?6 IS NULL OR provider_id = ?6)
      AND (?7 IS NULL OR model_id = ?7)
      AND (?8 IS NULL OR is_incomplete = ?8)
    ORDER BY time_created_utc DESC, message_id ASC
    LIMIT ?9 OFFSET ?10";

/// Result type returned by report queries.
pub type Result<T> = std::result::Result<T, QueryError>;

/// Structured validation, calendar-resolution, archive, and stored-data failures.
#[derive(Debug, Error)]
pub enum QueryError {
    /// An IANA timezone identifier could not be parsed.
    #[error("invalid IANA timezone '{0}'; choose a name such as UTC or Asia/Shanghai")]
    InvalidTimezone(String),
    /// A half-open local date range is empty or reversed.
    #[error(
        "invalid local date range [{start_date}, {end_date_exclusive}): end_date_exclusive must be after start_date"
    )]
    InvalidDateRange {
        /// Inclusive local start date.
        start_date: NaiveDate,
        /// Exclusive local end date.
        end_date_exclusive: NaiveDate,
    },
    /// A report date is outside the deliberately bounded product range.
    #[error(
        "unsupported report years in [{start_date}, {end_date_exclusive}); supported report years are {min_year} through {max_year}"
    )]
    UnsupportedYear {
        /// Inclusive local start date.
        start_date: NaiveDate,
        /// Exclusive local end date.
        end_date_exclusive: NaiveDate,
        /// Earliest supported year.
        min_year: i32,
        /// Latest supported year.
        max_year: i32,
    },
    /// The range would create an excessive number of calendar buckets.
    #[error("report range spans {days} days; maximum report range is {max_days} days")]
    RangeTooLarge {
        /// Requested local-day count.
        days: i64,
        /// Maximum accepted local-day count.
        max_days: i64,
    },
    /// Checked calendar arithmetic reached chrono's representable boundary.
    #[error("calendar arithmetic overflow near {0}")]
    DateOverflow(NaiveDate),
    /// A midnight gap could not be resolved within the maximum IANA transition gap.
    #[error("could not resolve local day boundary {local_midnight} in timezone {timezone}")]
    DayBoundaryResolution {
        /// Missing local midnight.
        local_midnight: String,
        /// Report timezone.
        timezone: String,
    },
    /// An epoch-millisecond value is outside chrono's supported range.
    #[error("UTC epoch millisecond timestamp is out of range: {0}")]
    InvalidTimestamp(i64),
    /// Detail pages cannot be empty by construction.
    #[error("detail limit must be greater than zero")]
    InvalidLimit,
    /// SQL OFFSET is intentionally exposed as signed input so negative IPC values can be rejected.
    #[error("detail offset must not be negative: {0}")]
    NegativeOffset(i64),
    /// A non-negative archive counter was corrupt or too large for the public unsigned contract.
    #[error("archive column {column} contains invalid non-negative integer value {value}")]
    InvalidStoredInteger {
        /// Archive column name.
        column: &'static str,
        /// Invalid signed SQLite value.
        value: i64,
    },
    /// A cost provenance string bypassed the archive CHECK constraint.
    #[error("archive contains unsupported cost_source '{0}'")]
    InvalidCostSource(String),
    /// SQLite rejected a read-only archive query.
    #[error("archive query failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// SQL 返回了超出 Rust 桶表范围的序号。
    #[error("archive query returned bucket index {index} for {bucket_count} generated buckets")]
    InvalidBucketIndex {
        /// SQL 返回的非法桶序号。
        index: i64,
        /// Rust 生成的报表桶数量。
        bucket_count: usize,
    },
}

/// Configurable first day of a report week.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    /// ISO-style Monday start.
    Monday,
    /// Sunday start using chrono's `%U` week numbering.
    Sunday,
}

/// Calendar granularity used to produce report-timezone buckets.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// Consecutive UTC hours clipped to each local day boundary.
    Hour,
    /// Local calendar day.
    Day,
    /// Local calendar week using [`LocalDateRange::week_start`].
    Week,
    /// Local calendar month.
    Month,
}

/// Validated custom half-open local-date range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDateRange {
    /// Inclusive local report date.
    pub start_date: NaiveDate,
    /// Exclusive local report date.
    pub end_date_exclusive: NaiveDate,
    /// IANA report timezone used only for calendar interpretation.
    pub tz: Tz,
    /// Week-label and week-boundary convention.
    pub week_start: WeekStart,
}

impl LocalDateRange {
    /// Validates a typed IANA timezone range.
    pub fn new(
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
        tz: Tz,
        week_start: WeekStart,
    ) -> Result<Self> {
        let range = Self {
            start_date,
            end_date_exclusive,
            tz,
            week_start,
        };
        range.validate()?;
        Ok(range)
    }

    /// Revalidates public fields after deserialization or direct struct construction.
    pub fn validate(&self) -> Result<()> {
        let start_date = self.start_date;
        let end_date_exclusive = self.end_date_exclusive;
        if end_date_exclusive <= start_date {
            return Err(QueryError::InvalidDateRange {
                start_date,
                end_date_exclusive,
            });
        }
        if start_date.year() < MIN_REPORT_YEAR
            || start_date.year() > MAX_REPORT_YEAR
            || end_date_exclusive.year() < MIN_REPORT_YEAR
            || end_date_exclusive.year() > MAX_REPORT_YEAR + 1
            || (end_date_exclusive.year() == MAX_REPORT_YEAR + 1
                && (end_date_exclusive.month(), end_date_exclusive.day()) != (1, 1))
        {
            return Err(QueryError::UnsupportedYear {
                start_date,
                end_date_exclusive,
                min_year: MIN_REPORT_YEAR,
                max_year: MAX_REPORT_YEAR,
            });
        }
        let days = end_date_exclusive
            .signed_duration_since(start_date)
            .num_days();
        if days > MAX_REPORT_RANGE_DAYS {
            return Err(QueryError::RangeTooLarge {
                days,
                max_days: MAX_REPORT_RANGE_DAYS,
            });
        }
        Ok(())
    }

    /// Parses an IANA timezone name, then validates the date range.
    pub fn from_timezone_name(
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
        timezone: &str,
        week_start: WeekStart,
    ) -> Result<Self> {
        let tz = timezone
            .parse::<Tz>()
            .map_err(|_| QueryError::InvalidTimezone(timezone.to_string()))?;
        Self::new(start_date, end_date_exclusive, tz, week_start)
    }

    /// Converts both resolved local day boundaries to UTC epoch milliseconds.
    pub fn utc_bounds(&self) -> Result<(i64, i64)> {
        self.validate()?;
        Ok((
            resolve_day_boundary(self.start_date, self.tz)?.timestamp_millis(),
            resolve_day_boundary(self.end_date_exclusive, self.tz)?.timestamp_millis(),
        ))
    }
}

/// One half-open UTC bucket with a report-timezone label.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TimeBucket {
    /// Inclusive UTC epoch-millisecond boundary passed to SQL.
    pub start_utc_ms: i64,
    /// Exclusive UTC epoch-millisecond boundary passed to SQL.
    pub end_utc_ms: i64,
    /// Human-facing local label; repeated hours include their numeric UTC offset.
    pub label: String,
}

impl TimeBucket {
    /// Returns whether an epoch millisecond belongs to this half-open bucket.
    pub fn contains(&self, epoch_ms: i64) -> bool {
        self.start_utc_ms <= epoch_ms && epoch_ms < self.end_utc_ms
    }
}

/// Coverage state supplied by todo 7 for one selected host/source bucket.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageStatus {
    /// Every selected host/source covers the whole bucket.
    Full,
    /// At least one interval overlaps, but complete selected coverage is absent.
    Partial,
    /// No selected host/source coverage overlaps the bucket.
    None,
}

/// One selected `(host_id, source)` pair that keeps a bucket from being [`CoverageStatus::Full`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CoverageShortfall {
    /// Host whose archived intervals fall short of the bucket.
    pub host_id: String,
    /// Open source name within that host.
    pub source: String,
    /// `true` when the pair archived part of the bucket, `false` when it archived none of it.
    pub partial: bool,
}

/// Why one bucket is not fully covered, keyed by the label its series point already carries.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CoverageNote {
    /// Matches [`TimeBucket::label`].
    pub label: String,
    /// Never empty for an emitted note.
    pub shortfalls: Vec<CoverageShortfall>,
}

/// Injection seam between todo 7's interval-union implementation and series aggregation.
pub trait CoverageLookup {
    /// Computes coverage for one already-resolved UTC bucket and aggregate filter set.
    fn status(&self, bucket: &TimeBucket, filters: &AggregateFilters) -> CoverageStatus;

    /// Names the pairs behind a non-`Full` [`Self::status`], so the UI can say *why*.
    ///
    /// Defaulted to empty rather than required: a lookup is allowed to decline to diagnose, and
    /// the series layer treats "no shortfalls" as "no explanation available" instead of as
    /// "fully covered" — `status` remains the only authority on the tri-state.
    fn shortfalls(
        &self,
        _bucket: &TimeBucket,
        _filters: &AggregateFilters,
    ) -> Vec<CoverageShortfall> {
        Vec::new()
    }
}

/// Five atomic token values plus the one permitted derived input value.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct TokenValues {
    /// Cache-miss input tokens.
    pub tok_input: u64,
    /// Output tokens.
    pub tok_output: u64,
    /// Reasoning tokens, not merged into output and not priced.
    pub tok_reasoning: u64,
    /// Cache-read input tokens.
    pub tok_cache_read: u64,
    /// Cache-write input tokens.
    pub tok_cache_write: u64,
    /// `tok_input + tok_cache_read + tok_cache_write`.
    pub total_input: u64,
}

impl TokenValues {
    fn new(
        tok_input: u64,
        tok_output: u64,
        tok_reasoning: u64,
        tok_cache_read: u64,
        tok_cache_write: u64,
    ) -> Self {
        Self {
            tok_input,
            tok_output,
            tok_reasoning,
            tok_cache_read,
            tok_cache_write,
            total_input: tok_input
                .saturating_add(tok_cache_read)
                .saturating_add(tok_cache_write),
        }
    }

    /// Adds another aggregate while preserving atomic buckets and recomputing `total_input`.
    pub fn add_assign(&mut self, other: Self) {
        self.tok_input = self.tok_input.saturating_add(other.tok_input);
        self.tok_output = self.tok_output.saturating_add(other.tok_output);
        self.tok_reasoning = self.tok_reasoning.saturating_add(other.tok_reasoning);
        self.tok_cache_read = self.tok_cache_read.saturating_add(other.tok_cache_read);
        self.tok_cache_write = self.tok_cache_write.saturating_add(other.tok_cache_write);
        self.total_input = self
            .tok_input
            .saturating_add(self.tok_cache_read)
            .saturating_add(self.tok_cache_write);
    }
}

/// Record and billable-token coverage for one mutually exclusive cost layer.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct CostCoverageLayer {
    /// Records whose cost resolved into this layer.
    pub record_count: u64,
    /// Input, output, cache-read, and cache-write tokens represented by this layer.
    pub billable_tokens: u64,
}

/// Coverage quantities kept separate for actual, estimated, and unavailable costs.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct CostCoverage {
    /// Coverage behind the source-reported actual amount.
    pub actual: CostCoverageLayer,
    /// Coverage behind the query-time estimated amount.
    pub estimated: CostCoverageLayer,
    /// Coverage excluded from both amounts because no price was available.
    pub unavailable: CostCoverageLayer,
}

/// Optional equality filters applied to aggregate queries.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AggregateFilters {
    /// Restrict to one host.
    pub host_id: Option<String>,
    /// Restrict to one open source name.
    pub source: Option<String>,
    /// Restrict to one normalized agent key.
    pub agent_key: Option<String>,
    /// Restrict to one provider.
    pub provider_id: Option<String>,
    /// Restrict to one model within the provider filter, when supplied.
    pub model_id: Option<String>,
}

/// Coverage-aware trend point.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SeriesBucket {
    /// UTC boundaries and local label.
    pub bucket: TimeBucket,
    /// Coverage state from the injected todo 7 lookup.
    pub coverage: CoverageStatus,
    /// Atomic tokens, or `None` only when coverage is [`CoverageStatus::None`].
    pub tokens: Option<TokenValues>,
    /// Layered cost, or `None` only when coverage is [`CoverageStatus::None`].
    pub cost: Option<CostTotals>,
    /// Complete message count, or `None` only when coverage is [`CoverageStatus::None`].
    pub message_count: Option<u64>,
    /// Complete session-level record count, or `None` only when coverage is absent.
    pub session_record_count: Option<u64>,
}

/// 一条预聚合趋势线代表的分组维度。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeriesGroupDimension {
    /// `opencode`、`claude-code` 等数据源适配器。
    Source,
    /// 规范化 agent key。
    Agent,
    /// 不区分模型的 provider。
    Provider,
    /// provider 与 model 组合键。
    Model,
}

/// 一个维度值及其完整、带覆盖状态的桶序列。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SeriesGroup {
    /// 用于解释稳定 id 的维度。
    pub dimension: SeriesGroupDimension,
    /// 维度内的稳定标识。
    pub id: String,
    /// 面向用户的图例标签。
    pub label: String,
    /// 与总趋势共用覆盖掩码的稠密序列。
    pub series: Vec<SeriesBucket>,
}

/// 一次归档扫描得到的总趋势及全部 source、agent、provider、model 趋势。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SeriesQueryResult {
    /// 默认图表与覆盖带使用的不分组总趋势。
    pub total: Vec<SeriesBucket>,
    /// 用户本地切换分组时使用的预聚合维度趋势。
    pub groups: Vec<SeriesGroup>,
    /// 非 `Full` 桶的成因，只收录能给出说明的桶，因此通常远短于 `total`。
    ///
    /// 挂在结果上而不是逐点挂在 `SeriesBucket` 上：覆盖状态是**桶**的属性，total 与全部分组
    /// 共用同一份掩码（见 `materialize_series`），逐点携带会把同样的主机与源字符串在四个维度
    /// 的几十条分组线上重复几十遍，却不多给一点信息。也不另开 IPC 命令：那需要第二处按报表
    /// 时区推导桶边界，而本仓库刻意只保留一份时区实现。
    pub coverage_notes: Vec<CoverageNote>,
}

/// Summary-card values for a whole custom range.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Summary {
    /// Five atomic token values and derived total input.
    pub tokens: TokenValues,
    /// Actual, estimated, and unavailable costs kept separate.
    pub cost: CostTotals,
    /// Record and billable-token coverage behind each mutually exclusive cost layer.
    pub cost_coverage: CostCoverage,
    /// Complete message count.
    pub message_count: u64,
    /// Complete session-level record count.
    pub session_record_count: u64,
    /// Distinct session count among complete messages.
    pub active_session_count: u64,
}

/// Controls whether model variants become separate breakdown rows.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct BreakdownOptions {
    /// `true` groups by variant; `false` collapses variants at model level.
    pub expand_variant: bool,
}

/// Flat source → agent → model dimension row suitable for hierarchical UI grouping.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BreakdownRow {
    /// First-level source key.
    pub source: String,
    /// Second-level normalized agent key.
    pub agent_key: String,
    /// Representative raw display name.
    pub agent_raw: String,
    /// Third-level provider key.
    pub provider_id: String,
    /// Third-level model key.
    pub model_id: String,
    /// Optional expanded variant; always `None` when variants are collapsed.
    pub variant: Option<String>,
    /// Five atomic token values and derived total input.
    pub tokens: TokenValues,
    /// Layered cost totals for this exact group.
    pub cost: CostTotals,
    /// Complete message count.
    pub message_count: u64,
    /// Complete session-level record count.
    pub session_record_count: u64,
    /// Distinct active session count.
    pub active_session_count: u64,
}

/// Detail-only filters; incomplete rows are intentionally selectable here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct DetailFilters {
    /// Restrict to one host.
    pub host_id: Option<String>,
    /// Restrict to one source.
    pub source: Option<String>,
    /// Restrict to one normalized agent key.
    pub agent_key: Option<String>,
    /// Restrict to one provider.
    pub provider_id: Option<String>,
    /// Restrict to one model.
    pub model_id: Option<String>,
    /// Restrict to complete or incomplete rows; `None` returns both.
    pub is_incomplete: Option<bool>,
}

/// Serializable cost resolution for one detail row.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DetailCost {
    /// Trustworthy source cost when present.
    pub actual: Option<f64>,
    /// Query-time price-table estimate when present.
    pub estimated: Option<f64>,
    /// True only when neither actual nor estimated cost exists.
    pub unavailable: bool,
}

/// One server-paged archive detail row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DetailRow {
    /// Stable host key.
    pub host_id: String,
    /// Open source name.
    pub source: String,
    /// Source message identifier.
    pub message_id: String,
    /// Source session identifier.
    pub session_id: String,
    /// UTC epoch-millisecond creation time.
    pub time_created_utc: i64,
    /// Raw agent display name.
    pub agent_raw: String,
    /// Normalized agent key.
    pub agent_key: String,
    /// Provider identifier.
    pub provider_id: String,
    /// Model identifier.
    pub model_id: String,
    /// Optional model variant.
    pub variant: Option<String>,
    /// Five atomic token values and derived total input.
    pub tokens: TokenValues,
    /// Actual, estimated, or unavailable cost resolution.
    pub cost: DetailCost,
    /// Incomplete rows remain visible in this API.
    pub is_incomplete: bool,
    /// Source project directory.
    pub project_dir: String,
}

/// Server-side page plus the filtered total count.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DetailPage {
    /// At most `limit` rows.
    pub rows: Vec<DetailRow>,
    /// Count before LIMIT/OFFSET.
    pub total_count: u64,
    /// Effective limit after applying [`MAX_DETAIL_LIMIT`].
    pub limit: u32,
    /// Validated non-negative offset.
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BreakdownKey {
    source: String,
    agent_key: String,
    provider_id: String,
    model_id: String,
    variant: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct RawAggregate {
    tok_input: i64,
    tok_output: i64,
    tok_reasoning: i64,
    tok_cache_read: i64,
    tok_cache_write: i64,
    message_count: i64,
    session_record_count: i64,
    active_session_count: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct BucketAggregate {
    tokens: TokenValues,
    cost: CostTotals,
    message_count: u64,
    session_record_count: u64,
}

impl BucketAggregate {
    fn add_assign(&mut self, other: Self) {
        self.tokens.add_assign(other.tokens);
        self.cost.actual_sum += other.cost.actual_sum;
        self.cost.estimated_sum += other.cost.estimated_sum;
        self.cost.unavailable_count = self
            .cost
            .unavailable_count
            .saturating_add(other.cost.unavailable_count);
        self.message_count = self.message_count.saturating_add(other.message_count);
        self.session_record_count = self
            .session_record_count
            .saturating_add(other.session_record_count);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesGroupKey {
    dimension: SeriesGroupDimension,
    id: String,
}

#[derive(Clone, Debug)]
struct GroupAggregate {
    label: String,
    buckets: Vec<BucketAggregate>,
}

/// Labels one fixed UTC instant using report-timezone calendar semantics.
pub fn label_for_epoch_ms(
    epoch_ms: i64,
    tz: Tz,
    granularity: Granularity,
    week_start: WeekStart,
) -> Result<String> {
    let utc = DateTime::<Utc>::from_timestamp_millis(epoch_ms)
        .ok_or(QueryError::InvalidTimestamp(epoch_ms))?;
    let local = utc.with_timezone(&tz);
    Ok(match granularity {
        Granularity::Hour => local.format("%Y-%m-%dT%H:%M%:z").to_string(),
        Granularity::Day => local.format("%Y-%m-%d").to_string(),
        Granularity::Week => week_label(local.date_naive(), week_start),
        Granularity::Month => local.format("%Y-%m").to_string(),
    })
}

/// Generates report-timezone calendar buckets with UTC epoch-millisecond SQL boundaries.
pub fn generate_buckets(
    range: &LocalDateRange,
    granularity: Granularity,
) -> Result<Vec<TimeBucket>> {
    range.validate()?;
    match granularity {
        Granularity::Hour => generate_hour_buckets(range),
        Granularity::Day => generate_day_buckets(range),
        Granularity::Week => generate_week_buckets(range),
        Granularity::Month => generate_month_buckets(range),
    }
}

/// 复用单次扫描实现，只返回不分组总趋势。
pub fn query_series<C: CoverageLookup + ?Sized>(
    archive: &Archive,
    range: &LocalDateRange,
    granularity: Granularity,
    filters: &AggregateFilters,
    prices: &PriceTable,
    coverage: &C,
) -> Result<Vec<SeriesBucket>> {
    query_series_bundle(archive, range, granularity, filters, prices, coverage)
        .map(|result| result.total)
}

/// 一次查询返回总趋势及全部分组趋势，未覆盖桶仍为 `None`，绝不伪造为零。
///
/// Rust 先按报表时区生成精确桶边界，再以内联 `VALUES` CTE 交给 SQLite 做区间 JOIN；
/// 这样既保留 DST 与后端预格式化 label，也把“逐桶 × 逐分组”收敛为一次归档扫描。
/// 251737 行实测中，按模型六组从 6123ms 降至 514ms，因此这里刻意不再逐桶查询。
pub fn query_series_bundle<C: CoverageLookup + ?Sized>(
    archive: &Archive,
    range: &LocalDateRange,
    granularity: Granularity,
    filters: &AggregateFilters,
    prices: &PriceTable,
    coverage: &C,
) -> Result<SeriesQueryResult> {
    let buckets = generate_buckets(range, granularity)?;
    if buckets.is_empty() {
        return Ok(SeriesQueryResult {
            total: Vec::new(),
            groups: Vec::new(),
            coverage_notes: Vec::new(),
        });
    }

    let coverage_statuses = buckets
        .iter()
        .map(|bucket| coverage.status(bucket, filters))
        .collect::<Vec<_>>();
    let coverage_notes = collect_coverage_notes(&buckets, &coverage_statuses, coverage, filters);
    let mut totals = vec![BucketAggregate::default(); buckets.len()];
    let mut groups = BTreeMap::<SeriesGroupKey, GroupAggregate>::new();
    let sql = series_grouped_sql(&buckets);
    let mut statement = archive.connection().prepare(&sql)?;
    let mut rows = statement.query(params![
        filters.host_id.as_deref(),
        filters.source.as_deref(),
        filters.agent_key.as_deref(),
        filters.provider_id.as_deref(),
        filters.model_id.as_deref(),
    ])?;

    while let Some(row) = rows.next()? {
        let bucket_index_raw: i64 = row.get(0)?;
        let bucket_index =
            usize::try_from(bucket_index_raw).map_err(|_| QueryError::InvalidBucketIndex {
                index: bucket_index_raw,
                bucket_count: buckets.len(),
            })?;
        if bucket_index >= buckets.len() {
            return Err(QueryError::InvalidBucketIndex {
                index: bucket_index_raw,
                bucket_count: buckets.len(),
            });
        }

        let source: String = row.get(1)?;
        let agent_key: String = row.get(2)?;
        let agent_raw: String = row.get(3)?;
        let provider_id: String = row.get(4)?;
        let model_id: String = row.get(5)?;
        let cost_source_text: String = row.get(6)?;
        let tokens = grouped_tokens(row, 8, 17)?;
        let row_count = nonnegative("cost_row_count", row.get(16)?)?;
        let aggregate = BucketAggregate {
            tokens,
            cost: aggregated_cost(
                prices,
                &provider_id,
                &model_id,
                tokens,
                row.get(15)?,
                parse_cost_source(&cost_source_text)?,
                row_count,
            ),
            message_count: nonnegative("message_count", row.get(13)?)?,
            session_record_count: nonnegative("session_record_count", row.get(14)?)?,
        };
        totals[bucket_index].add_assign(aggregate);

        add_group_aggregate(
            &mut groups,
            SeriesGroupDimension::Source,
            source.clone(),
            source,
            buckets.len(),
            bucket_index,
            aggregate,
        );
        add_group_aggregate(
            &mut groups,
            SeriesGroupDimension::Agent,
            agent_key.clone(),
            agent_raw,
            buckets.len(),
            bucket_index,
            aggregate,
        );
        add_group_aggregate(
            &mut groups,
            SeriesGroupDimension::Provider,
            provider_id.clone(),
            provider_id.clone(),
            buckets.len(),
            bucket_index,
            aggregate,
        );
        add_group_aggregate(
            &mut groups,
            SeriesGroupDimension::Model,
            format!("{provider_id}\0{model_id}"),
            format!("{provider_id} / {model_id}"),
            buckets.len(),
            bucket_index,
            aggregate,
        );
    }

    Ok(SeriesQueryResult {
        total: materialize_series(&buckets, &coverage_statuses, &totals),
        groups: groups
            .into_iter()
            .map(|(key, group)| SeriesGroup {
                dimension: key.dimension,
                id: key.id,
                label: group.label,
                series: materialize_series(&buckets, &coverage_statuses, &group.buckets),
            })
            .collect(),
        coverage_notes,
    })
}

/// Asks the lookup to explain every bucket `status` did not call `Full`.
fn collect_coverage_notes<C: CoverageLookup + ?Sized>(
    buckets: &[TimeBucket],
    statuses: &[CoverageStatus],
    coverage: &C,
    filters: &AggregateFilters,
) -> Vec<CoverageNote> {
    buckets
        .iter()
        .zip(statuses)
        .filter(|(_, status)| **status != CoverageStatus::Full)
        .filter_map(|(bucket, _)| {
            let shortfalls = coverage.shortfalls(bucket, filters);
            (!shortfalls.is_empty()).then(|| CoverageNote {
                label: bucket.label.clone(),
                shortfalls,
            })
        })
        .collect()
}

/// Queries summary-card metrics for the full half-open local range.
pub fn query_summary(
    archive: &Archive,
    range: &LocalDateRange,
    filters: &AggregateFilters,
    prices: &PriceTable,
) -> Result<Summary> {
    let (start, end) = range.utc_bounds()?;
    let aggregate = query_raw_aggregate(archive.connection(), start, end, filters)?;
    let cost = query_cost_totals(archive.connection(), start, end, filters, prices)?;
    Ok(Summary {
        tokens: tokens_from_raw(aggregate)?,
        cost: cost.totals,
        cost_coverage: cost.coverage,
        message_count: nonnegative("message_count", aggregate.message_count)?,
        session_record_count: nonnegative("session_record_count", aggregate.session_record_count)?,
        active_session_count: nonnegative("active_session_count", aggregate.active_session_count)?,
    })
}

/// Queries source → agent → model breakdown rows, optionally expanding model variants.
pub fn query_breakdown(
    archive: &Archive,
    range: &LocalDateRange,
    filters: &AggregateFilters,
    options: BreakdownOptions,
    prices: &PriceTable,
) -> Result<Vec<BreakdownRow>> {
    let (start, end) = range.utc_bounds()?;
    let sql = if options.expand_variant {
        BREAKDOWN_WITH_VARIANT_SQL
    } else {
        BREAKDOWN_COLLAPSED_SQL
    };
    let mut statement = archive.connection().prepare(sql)?;
    let mut rows = statement.query(params![
        start,
        end,
        filters.host_id.as_deref(),
        filters.source.as_deref(),
        filters.agent_key.as_deref(),
        filters.provider_id.as_deref(),
        filters.model_id.as_deref(),
    ])?;
    let mut groups = BTreeMap::<BreakdownKey, BreakdownRow>::new();
    while let Some(row) = rows.next()? {
        let key = BreakdownKey {
            source: row.get(0)?,
            agent_key: row.get(1)?,
            provider_id: row.get(3)?,
            model_id: row.get(4)?,
            variant: row.get(5)?,
        };
        let raw = RawAggregate {
            tok_input: row.get(6)?,
            tok_output: row.get(7)?,
            tok_reasoning: row.get(8)?,
            tok_cache_read: row.get(9)?,
            tok_cache_write: row.get(10)?,
            message_count: row.get(11)?,
            session_record_count: row.get(12)?,
            active_session_count: row.get(13)?,
        };
        groups.insert(
            key.clone(),
            BreakdownRow {
                source: key.source.clone(),
                agent_key: key.agent_key.clone(),
                agent_raw: row.get(2)?,
                provider_id: key.provider_id.clone(),
                model_id: key.model_id.clone(),
                variant: key.variant.clone(),
                tokens: tokens_from_raw(raw)?,
                cost: CostTotals::default(),
                message_count: nonnegative("message_count", raw.message_count)?,
                session_record_count: nonnegative(
                    "session_record_count",
                    raw.session_record_count,
                )?,
                active_session_count: nonnegative(
                    "active_session_count",
                    raw.active_session_count,
                )?,
            },
        );
    }
    drop(rows);
    drop(statement);

    add_breakdown_costs(
        archive.connection(),
        start,
        end,
        filters,
        options,
        prices,
        &mut groups,
    )?;
    Ok(groups.into_values().collect())
}

/// Queries one validated, capped server-side detail page and its independent total count.
pub fn query_details(
    archive: &Archive,
    range: &LocalDateRange,
    filters: &DetailFilters,
    limit: u32,
    offset: i64,
    prices: &PriceTable,
) -> Result<DetailPage> {
    if limit == 0 {
        return Err(QueryError::InvalidLimit);
    }
    let offset_sql = offset;
    let offset = u64::try_from(offset).map_err(|_| QueryError::NegativeOffset(offset))?;
    let effective_limit = limit.min(MAX_DETAIL_LIMIT);
    let (start, end) = range.utc_bounds()?;
    let total_count_raw: i64 = archive.connection().query_row(
        DETAIL_COUNT_SQL,
        params![
            start,
            end,
            filters.host_id.as_deref(),
            filters.source.as_deref(),
            filters.agent_key.as_deref(),
            filters.provider_id.as_deref(),
            filters.model_id.as_deref(),
            filters.is_incomplete,
        ],
        |row| row.get(0),
    )?;
    let total_count = nonnegative("total_count", total_count_raw)?;

    let mut statement = archive.connection().prepare(DETAIL_ROWS_SQL)?;
    let mut rows = statement.query(params![
        start,
        end,
        filters.host_id.as_deref(),
        filters.source.as_deref(),
        filters.agent_key.as_deref(),
        filters.provider_id.as_deref(),
        filters.model_id.as_deref(),
        filters.is_incomplete,
        effective_limit,
        offset_sql,
    ])?;
    let mut detail_rows = Vec::with_capacity(effective_limit as usize);
    while let Some(row) = rows.next()? {
        let tokens = TokenValues::new(
            nonnegative("tok_input", row.get(10)?)?,
            nonnegative("tok_output", row.get(11)?)?,
            nonnegative("tok_reasoning", row.get(12)?)?,
            nonnegative("tok_cache_read", row.get(13)?)?,
            nonnegative("tok_cache_write", row.get(14)?)?,
        );
        let provider_id: String = row.get(7)?;
        let model_id: String = row.get(8)?;
        let cost_source_text: String = row.get(16)?;
        let resolved = prices.resolve_cost(
            &provider_id,
            &model_id,
            TokenCounts {
                input: tokens.tok_input,
                output: tokens.tok_output,
                cache_read: tokens.tok_cache_read,
                cache_write: tokens.tok_cache_write,
            },
            row.get(15)?,
            parse_cost_source(&cost_source_text)?,
        );
        detail_rows.push(DetailRow {
            host_id: row.get(0)?,
            source: row.get(1)?,
            message_id: row.get(2)?,
            session_id: row.get(3)?,
            time_created_utc: row.get(4)?,
            agent_raw: row.get(5)?,
            agent_key: row.get(6)?,
            provider_id,
            model_id,
            variant: row.get(9)?,
            tokens,
            cost: detail_cost(resolved),
            is_incomplete: row.get(17)?,
            project_dir: row.get(18)?,
        });
    }

    Ok(DetailPage {
        rows: detail_rows,
        total_count,
        limit: effective_limit,
        offset,
    })
}

fn resolve_day_boundary(date: NaiveDate, tz: Tz) -> Result<DateTime<Tz>> {
    let local_midnight = date
        .and_hms_opt(0, 0, 0)
        .ok_or(QueryError::DateOverflow(date))?;
    match tz.from_local_datetime(&local_midnight) {
        LocalResult::Single(value) => Ok(value),
        LocalResult::Ambiguous(earliest, _) => Ok(earliest),
        LocalResult::None => {
            let mut candidate = local_midnight;
            for _ in 0..=172_800 {
                candidate = candidate
                    .checked_add_signed(Duration::seconds(1))
                    .ok_or(QueryError::DateOverflow(date))?;
                match tz.from_local_datetime(&candidate) {
                    LocalResult::Single(value) => return Ok(value),
                    LocalResult::Ambiguous(earliest, _) => return Ok(earliest),
                    LocalResult::None => {}
                }
            }
            Err(QueryError::DayBoundaryResolution {
                local_midnight: local_midnight.to_string(),
                timezone: tz.name().to_string(),
            })
        }
    }
}

fn generate_hour_buckets(range: &LocalDateRange) -> Result<Vec<TimeBucket>> {
    let mut buckets = Vec::new();
    let mut date = range.start_date;
    while date < range.end_date_exclusive {
        let next_date = date.succ_opt().ok_or(QueryError::DateOverflow(date))?;
        let day_start = resolve_day_boundary(date, range.tz)?.with_timezone(&Utc);
        let day_end = resolve_day_boundary(next_date, range.tz)?.with_timezone(&Utc);
        let mut start = day_start;
        while start < day_end {
            let nominal_end = start
                .checked_add_signed(Duration::hours(1))
                .ok_or(QueryError::InvalidTimestamp(start.timestamp_millis()))?;
            let end = nominal_end.min(day_end);
            buckets.push(TimeBucket {
                start_utc_ms: start.timestamp_millis(),
                end_utc_ms: end.timestamp_millis(),
                label: start
                    .with_timezone(&range.tz)
                    .format("%Y-%m-%dT%H:%M%:z")
                    .to_string(),
            });
            start = end;
        }
        date = next_date;
    }
    Ok(buckets)
}

fn generate_day_buckets(range: &LocalDateRange) -> Result<Vec<TimeBucket>> {
    let mut buckets = Vec::new();
    let mut date = range.start_date;
    while date < range.end_date_exclusive {
        let next_date = date.succ_opt().ok_or(QueryError::DateOverflow(date))?;
        push_calendar_bucket(&mut buckets, date, next_date, date.to_string(), range)?;
        date = next_date;
    }
    Ok(buckets)
}

fn generate_week_buckets(range: &LocalDateRange) -> Result<Vec<TimeBucket>> {
    let mut buckets = Vec::new();
    let mut current = range.start_date;
    while current < range.end_date_exclusive {
        let weekday_offset = match range.week_start {
            WeekStart::Monday => current.weekday().num_days_from_monday() as i64,
            WeekStart::Sunday => current.weekday().num_days_from_sunday() as i64,
        };
        let calendar_start = current
            .checked_sub_signed(Duration::days(weekday_offset))
            .ok_or(QueryError::DateOverflow(current))?;
        let next_calendar_start = calendar_start
            .checked_add_signed(Duration::days(7))
            .ok_or(QueryError::DateOverflow(calendar_start))?;
        let end = next_calendar_start.min(range.end_date_exclusive);
        push_calendar_bucket(
            &mut buckets,
            current,
            end,
            week_label(calendar_start, range.week_start),
            range,
        )?;
        current = end;
    }
    Ok(buckets)
}

fn generate_month_buckets(range: &LocalDateRange) -> Result<Vec<TimeBucket>> {
    let mut buckets = Vec::new();
    let mut current = range.start_date;
    while current < range.end_date_exclusive {
        let month_start = NaiveDate::from_ymd_opt(current.year(), current.month(), 1)
            .ok_or(QueryError::DateOverflow(current))?;
        let next_month_start = if current.month() == 12 {
            NaiveDate::from_ymd_opt(current.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(current.year(), current.month() + 1, 1)
        }
        .ok_or(QueryError::DateOverflow(current))?;
        let end = next_month_start.min(range.end_date_exclusive);
        push_calendar_bucket(
            &mut buckets,
            current,
            end,
            month_start.format("%Y-%m").to_string(),
            range,
        )?;
        current = end;
    }
    Ok(buckets)
}

fn push_calendar_bucket(
    buckets: &mut Vec<TimeBucket>,
    start_date: NaiveDate,
    end_date: NaiveDate,
    label: String,
    range: &LocalDateRange,
) -> Result<()> {
    let start = resolve_day_boundary(start_date, range.tz)?.timestamp_millis();
    let end = resolve_day_boundary(end_date, range.tz)?.timestamp_millis();
    if start < end {
        buckets.push(TimeBucket {
            start_utc_ms: start,
            end_utc_ms: end,
            label,
        });
    }
    Ok(())
}

fn week_label(date: NaiveDate, week_start: WeekStart) -> String {
    match week_start {
        WeekStart::Monday => {
            let week = date.iso_week();
            format!("{:04}-W{:02}", week.year(), week.week())
        }
        WeekStart::Sunday => date.format("%Y-W%U").to_string(),
    }
}

fn query_raw_aggregate(
    connection: &Connection,
    start: i64,
    end: i64,
    filters: &AggregateFilters,
) -> Result<RawAggregate> {
    Ok(connection.query_row(
        SUMMARY_SQL,
        params![
            start,
            end,
            filters.host_id.as_deref(),
            filters.source.as_deref(),
            filters.agent_key.as_deref(),
            filters.provider_id.as_deref(),
            filters.model_id.as_deref(),
        ],
        |row| {
            Ok(RawAggregate {
                tok_input: row.get(0)?,
                tok_output: row.get(1)?,
                tok_reasoning: row.get(2)?,
                tok_cache_read: row.get(3)?,
                tok_cache_write: row.get(4)?,
                message_count: row.get(5)?,
                session_record_count: row.get(6)?,
                active_session_count: row.get(7)?,
            })
        },
    )?)
}

fn tokens_from_raw(raw: RawAggregate) -> Result<TokenValues> {
    Ok(TokenValues::new(
        nonnegative("tok_input", raw.tok_input)?,
        nonnegative("tok_output", raw.tok_output)?,
        nonnegative("tok_reasoning", raw.tok_reasoning)?,
        nonnegative("tok_cache_read", raw.tok_cache_read)?,
        nonnegative("tok_cache_write", raw.tok_cache_write)?,
    ))
}

#[derive(Clone, Copy, Debug, Default)]
struct CostQueryResult {
    totals: CostTotals,
    coverage: CostCoverage,
}

fn query_cost_totals(
    connection: &Connection,
    start: i64,
    end: i64,
    filters: &AggregateFilters,
    prices: &PriceTable,
) -> Result<CostQueryResult> {
    let mut statement = connection.prepare(COST_ROWS_SQL)?;
    let mut rows = statement.query(params![
        start,
        end,
        filters.host_id.as_deref(),
        filters.source.as_deref(),
        filters.agent_key.as_deref(),
        filters.provider_id.as_deref(),
        filters.model_id.as_deref(),
    ])?;
    let mut result = CostQueryResult::default();
    while let Some(row) = rows.next()? {
        let provider_id: String = row.get(0)?;
        let model_id: String = row.get(1)?;
        let source_text: String = row.get(2)?;
        let tokens = grouped_tokens(row, 4, 11)?;
        let grouped = aggregated_cost_with_coverage(
            prices,
            &provider_id,
            &model_id,
            tokens,
            row.get(9)?,
            parse_cost_source(&source_text)?,
            nonnegative("cost_row_count", row.get(10)?)?,
        );
        add_cost_totals(&mut result.totals, grouped.totals);
        add_cost_coverage(&mut result.coverage, grouped.coverage);
    }
    Ok(result)
}

fn add_breakdown_costs(
    connection: &Connection,
    start: i64,
    end: i64,
    filters: &AggregateFilters,
    options: BreakdownOptions,
    prices: &PriceTable,
    groups: &mut BTreeMap<BreakdownKey, BreakdownRow>,
) -> Result<()> {
    let mut statement = connection.prepare(BREAKDOWN_COST_SQL)?;
    let mut rows = statement.query(params![
        start,
        end,
        filters.host_id.as_deref(),
        filters.source.as_deref(),
        filters.agent_key.as_deref(),
        filters.provider_id.as_deref(),
        filters.model_id.as_deref(),
    ])?;
    while let Some(row) = rows.next()? {
        let provider_id: String = row.get(2)?;
        let model_id: String = row.get(3)?;
        let key = BreakdownKey {
            source: row.get(0)?,
            agent_key: row.get(1)?,
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
            variant: if options.expand_variant {
                row.get(4)?
            } else {
                None
            },
        };
        let source_text: String = row.get(5)?;
        if let Some(group) = groups.get_mut(&key) {
            let tokens = grouped_tokens(row, 7, 14)?;
            let grouped = aggregated_cost(
                prices,
                &provider_id,
                &model_id,
                tokens,
                row.get(12)?,
                parse_cost_source(&source_text)?,
                nonnegative("cost_row_count", row.get(13)?)?,
            );
            add_cost_totals(&mut group.cost, grouped);
        }
    }
    Ok(())
}

fn series_grouped_sql(buckets: &[TimeBucket]) -> String {
    let values = buckets
        .iter()
        .enumerate()
        .map(|(index, bucket)| format!("({index}, {}, {})", bucket.start_utc_ms, bucket.end_utc_ms))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "WITH report_bucket(bucket_index, start_utc_ms, end_utc_ms) AS (VALUES {values}{SERIES_GROUPED_SQL_SUFFIX}"
    )
}

fn grouped_tokens(
    row: &rusqlite::Row<'_>,
    sum_start: usize,
    min_start: usize,
) -> Result<TokenValues> {
    for (offset, column) in [
        "tok_input",
        "tok_output",
        "tok_reasoning",
        "tok_cache_read",
        "tok_cache_write",
    ]
    .into_iter()
    .enumerate()
    {
        nonnegative(column, row.get(min_start + offset)?)?;
    }
    Ok(TokenValues::new(
        nonnegative("tok_input", row.get(sum_start)?)?,
        nonnegative("tok_output", row.get(sum_start + 1)?)?,
        nonnegative("tok_reasoning", row.get(sum_start + 2)?)?,
        nonnegative("tok_cache_read", row.get(sum_start + 3)?)?,
        nonnegative("tok_cache_write", row.get(sum_start + 4)?)?,
    ))
}

fn aggregated_cost(
    prices: &PriceTable,
    provider_id: &str,
    model_id: &str,
    tokens: TokenValues,
    trusted_cost: Option<f64>,
    cost_source: CostSource,
    row_count: u64,
) -> CostTotals {
    aggregated_cost_with_coverage(
        prices,
        provider_id,
        model_id,
        tokens,
        trusted_cost,
        cost_source,
        row_count,
    )
    .totals
}

fn aggregated_cost_with_coverage(
    prices: &PriceTable,
    provider_id: &str,
    model_id: &str,
    tokens: TokenValues,
    trusted_cost: Option<f64>,
    cost_source: CostSource,
    row_count: u64,
) -> CostQueryResult {
    let mut result = CostQueryResult::default();
    let billable_tokens = tokens
        .tok_input
        .saturating_add(tokens.tok_output)
        .saturating_add(tokens.tok_cache_read)
        .saturating_add(tokens.tok_cache_write);
    // Pricing consumes exactly four buckets. `tok_reasoning` is an output subset and is not
    // independently billable, so a row with no input/output/cache usage has an exact zero cost
    // regardless of whether the provider/model has a catalog entry.
    if billable_tokens == 0 && trusted_cost.is_none() {
        return result;
    }
    let resolved = match (trusted_cost, cost_source) {
        (Some(value), CostSource::Actual) => ResolvedCost::Actual(value),
        (Some(value), CostSource::Estimated) => ResolvedCost::Estimated(value),
        _ => prices.resolve_cost(
            provider_id,
            model_id,
            TokenCounts {
                input: tokens.tok_input,
                output: tokens.tok_output,
                cache_read: tokens.tok_cache_read,
                cache_write: tokens.tok_cache_write,
            },
            None,
            cost_source,
        ),
    };
    let coverage = CostCoverageLayer {
        record_count: row_count,
        billable_tokens,
    };
    match resolved {
        ResolvedCost::Actual(value) => {
            result.totals.actual_sum = value;
            result.coverage.actual = coverage;
        }
        ResolvedCost::Estimated(value) => {
            result.totals.estimated_sum = value;
            result.coverage.estimated = coverage;
        }
        ResolvedCost::Unavailable => {
            result.totals.unavailable_count = row_count;
            result.coverage.unavailable = coverage;
        }
    }
    result
}

fn add_cost_totals(target: &mut CostTotals, value: CostTotals) {
    target.actual_sum += value.actual_sum;
    target.estimated_sum += value.estimated_sum;
    target.unavailable_count = target
        .unavailable_count
        .saturating_add(value.unavailable_count);
}

fn add_cost_coverage(target: &mut CostCoverage, value: CostCoverage) {
    for (target, value) in [
        (&mut target.actual, value.actual),
        (&mut target.estimated, value.estimated),
        (&mut target.unavailable, value.unavailable),
    ] {
        target.record_count = target.record_count.saturating_add(value.record_count);
        target.billable_tokens = target.billable_tokens.saturating_add(value.billable_tokens);
    }
}

fn add_group_aggregate(
    groups: &mut BTreeMap<SeriesGroupKey, GroupAggregate>,
    dimension: SeriesGroupDimension,
    id: String,
    label: String,
    bucket_count: usize,
    bucket_index: usize,
    aggregate: BucketAggregate,
) {
    let group = groups
        .entry(SeriesGroupKey { dimension, id })
        .or_insert_with(|| GroupAggregate {
            label: label.clone(),
            buckets: vec![BucketAggregate::default(); bucket_count],
        });
    if label > group.label {
        group.label = label;
    }
    group.buckets[bucket_index].add_assign(aggregate);
}

fn materialize_series(
    buckets: &[TimeBucket],
    coverage: &[CoverageStatus],
    aggregates: &[BucketAggregate],
) -> Vec<SeriesBucket> {
    buckets
        .iter()
        .zip(coverage)
        .zip(aggregates)
        .map(|((bucket, coverage), aggregate)| {
            let covered = *coverage != CoverageStatus::None;
            SeriesBucket {
                bucket: bucket.clone(),
                coverage: *coverage,
                tokens: covered.then_some(aggregate.tokens),
                cost: covered.then_some(aggregate.cost),
                message_count: covered.then_some(aggregate.message_count),
                session_record_count: covered.then_some(aggregate.session_record_count),
            }
        })
        .collect()
}

fn parse_cost_source(value: &str) -> Result<CostSource> {
    match value {
        "actual" => Ok(CostSource::Actual),
        "estimated" => Ok(CostSource::Estimated),
        "unavailable" => Ok(CostSource::Unavailable),
        _ => Err(QueryError::InvalidCostSource(value.to_string())),
    }
}

fn detail_cost(resolved: ResolvedCost) -> DetailCost {
    match resolved {
        ResolvedCost::Actual(value) => DetailCost {
            actual: Some(value),
            estimated: None,
            unavailable: false,
        },
        ResolvedCost::Estimated(value) => DetailCost {
            actual: None,
            estimated: Some(value),
            unavailable: false,
        },
        ResolvedCost::Unavailable => DetailCost {
            actual: None,
            estimated: None,
            unavailable: true,
        },
    }
}

fn nonnegative(column: &'static str, value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| QueryError::InvalidStoredInteger { column, value })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Instant;

    use chrono::{DateTime, NaiveDate, Utc};
    use rusqlite::params;

    use crate::archive::{Archive, CostSource, NormalizedUsageRecord, Origin};
    use crate::fixture::{generate, Manifest};
    use crate::pricing::{PriceEntry, PriceTable};
    use crate::source::opencode::{scan_database, ScanRequest};

    use super::*;

    struct AllCoverage;

    impl CoverageLookup for AllCoverage {
        fn status(&self, _bucket: &TimeBucket, _filters: &AggregateFilters) -> CoverageStatus {
            CoverageStatus::Full
        }
    }

    struct CoverageByStart {
        statuses: BTreeMap<i64, CoverageStatus>,
    }

    impl CoverageLookup for CoverageByStart {
        fn status(&self, bucket: &TimeBucket, _filters: &AggregateFilters) -> CoverageStatus {
            self.statuses
                .get(&bucket.start_utc_ms)
                .copied()
                .unwrap_or(CoverageStatus::Partial)
        }
    }

    /// Mirrors a real store: it reports a tri-state *and* can explain a non-`Full` one.
    struct DiagnosingCoverage {
        statuses: BTreeMap<i64, CoverageStatus>,
    }

    impl CoverageLookup for DiagnosingCoverage {
        fn status(&self, bucket: &TimeBucket, _filters: &AggregateFilters) -> CoverageStatus {
            self.statuses
                .get(&bucket.start_utc_ms)
                .copied()
                .unwrap_or(CoverageStatus::Full)
        }

        fn shortfalls(
            &self,
            bucket: &TimeBucket,
            _filters: &AggregateFilters,
        ) -> Vec<CoverageShortfall> {
            match self.status(bucket, &AggregateFilters::default()) {
                CoverageStatus::Full => Vec::new(),
                status => vec![CoverageShortfall {
                    host_id: "host-a".to_string(),
                    source: "codex".to_string(),
                    partial: status == CoverageStatus::Partial,
                }],
            }
        }
    }

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("valid test date")
    }

    fn report_range(start: &str, end: &str, timezone: &str) -> LocalDateRange {
        LocalDateRange::from_timezone_name(date(start), date(end), timezone, WeekStart::Monday)
            .expect("valid report range")
    }

    fn empty_archive() -> (tempfile::TempDir, Archive) {
        let temp = tempfile::tempdir().expect("create archive tempdir");
        let archive = Archive::open_in_data_dir(temp.path()).expect("open test archive");
        (temp, archive)
    }

    fn fixed_record(message_id: &str, timestamp: i64, tok_input: u64) -> NormalizedUsageRecord {
        NormalizedUsageRecord {
            host_id: "host-query-test".to_string(),
            source: "opencode".to_string(),
            granularity: crate::archive::UsageGranularity::Message,
            message_id: message_id.to_string(),
            session_id: format!("session-{message_id}"),
            time_created_utc: timestamp,
            time_completed_utc: Some(timestamp + 500),
            source_time_updated: timestamp + 1_000,
            origin: Origin::Live,
            origin_priority: Origin::Live.priority(),
            agent_raw: "Atlas - Plan Executor".to_string(),
            agent_key: "atlas-plan-executor".to_string(),
            provider_id: "query-provider".to_string(),
            model_id: "query-model".to_string(),
            variant: Some("xhigh".to_string()),
            tok_input,
            tok_output: tok_input + 1,
            tok_reasoning: tok_input + 2,
            tok_cache_read: tok_input + 3,
            tok_cache_write: tok_input + 4,
            cost: None,
            cost_source: CostSource::Unavailable,
            is_incomplete: false,
            project_dir: "/fixture/query".to_string(),
        }
    }

    fn insert_record(archive: &Archive, record: &NormalizedUsageRecord) {
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
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
                )",
                params![
                    record.host_id,
                    record.source,
                    record.message_id,
                    record.session_id,
                    record.time_created_utc,
                    record.time_completed_utc,
                    record.source_time_updated,
                    record.origin.as_str(),
                    record.origin_priority,
                    record.agent_raw,
                    record.agent_key,
                    record.provider_id,
                    record.model_id,
                    record.variant,
                    i64::try_from(record.tok_input).expect("tok_input fits SQLite INTEGER"),
                    i64::try_from(record.tok_output).expect("tok_output fits SQLite INTEGER"),
                    i64::try_from(record.tok_reasoning).expect("tok_reasoning fits SQLite INTEGER"),
                    i64::try_from(record.tok_cache_read)
                        .expect("tok_cache_read fits SQLite INTEGER"),
                    i64::try_from(record.tok_cache_write)
                        .expect("tok_cache_write fits SQLite INTEGER"),
                    record.cost,
                    record.cost_source.as_str(),
                    record.is_incomplete,
                    record.project_dir,
                ],
            )
            .expect("insert normalized query record");
    }

    fn fixture_archive() -> (tempfile::TempDir, Archive, Manifest) {
        let temp = tempfile::tempdir().expect("create fixture tempdir");
        let fixture_directory = temp.path().join("fixture");
        let manifest = generate(&fixture_directory).expect("generate fixture");
        let archive =
            Archive::open_in_data_dir(temp.path().join("archive-data")).expect("open archive");
        let request = ScanRequest::live("host-fixture", None);
        let result = scan_database(fixture_directory.join("opencode.db"), &request, |batch| {
            for record in batch {
                insert_record(&archive, record);
            }
            Ok(())
        })
        .expect("scan fixture into archive");
        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, manifest.eligible_assistant_count);
        (temp, archive, manifest)
    }

    fn utc_date_for(epoch_ms: i64) -> String {
        DateTime::<Utc>::from_timestamp_millis(epoch_ms)
            .expect("valid fixture timestamp")
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    }

    fn next_date(value: &str) -> String {
        date(value)
            .succ_opt()
            .expect("test date has successor")
            .format("%Y-%m-%d")
            .to_string()
    }

    fn sum_series_tokens(series: &[SeriesBucket]) -> TokenValues {
        let mut total = TokenValues::default();
        for tokens in series.iter().filter_map(|bucket| bucket.tokens) {
            total.add_assign(tokens);
        }
        total
    }

    #[test]
    fn zero_billable_tokens_do_not_create_an_unavailable_cost() {
        let prices = PriceTable::new();
        let zero_billable = TokenValues::new(0, 0, 99, 0, 0);

        let totals = aggregated_cost(
            &prices,
            "kiro-auth",
            "auto",
            zero_billable,
            None,
            CostSource::Unavailable,
            1,
        );

        assert_eq!(totals, CostTotals::default());
    }

    #[test]
    fn unavailable_count_excludes_zero_billable_rows_in_a_mixed_model_group() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("mixed-cost test bounds");
        let mut zero = fixed_record("zero-billable", start + 1_000, 0);
        zero.tok_output = 0;
        zero.tok_reasoning = 99;
        zero.tok_cache_read = 0;
        zero.tok_cache_write = 0;
        insert_record(&archive, &zero);
        insert_record(&archive, &fixed_record("billable", start + 2_000, 1));

        let summary = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("query mixed-cost summary");

        assert_eq!(summary.cost.unavailable_count, 1);
    }

    #[test]
    fn summary_reports_record_and_billable_token_coverage_for_each_cost_layer() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("cost coverage test bounds");

        let mut actual = fixed_record("actual", start + 1_000, 10);
        actual.cost = Some(1.25);
        actual.cost_source = CostSource::Actual;
        insert_record(&archive, &actual);

        let estimated = fixed_record("estimated", start + 2_000, 20);
        insert_record(&archive, &estimated);

        let mut unavailable = fixed_record("unavailable", start + 3_000, 30);
        unavailable.model_id = "unpriced-model".to_owned();
        insert_record(&archive, &unavailable);

        let prices = PriceTable::from_entries(vec![PriceEntry::new(
            "query-provider",
            "query-model",
            1.0,
            2.0,
            0.5,
            0.75,
        )]);
        let summary = query_summary(&archive, &range, &AggregateFilters::default(), &prices)
            .expect("query layered cost coverage");

        assert_eq!(summary.cost_coverage.actual.record_count, 1);
        assert_eq!(summary.cost_coverage.actual.billable_tokens, 48);
        assert_eq!(summary.cost_coverage.estimated.record_count, 1);
        assert_eq!(summary.cost_coverage.estimated.billable_tokens, 88);
        assert_eq!(summary.cost_coverage.unavailable.record_count, 1);
        assert_eq!(summary.cost_coverage.unavailable.billable_tokens, 128);
        assert_eq!(summary.cost.actual_sum, 1.25);
        assert!(summary.cost.estimated_sum > 0.0);
        assert_eq!(summary.cost.unavailable_count, 1);
    }

    fn query_cost_totals_per_row_legacy(
        connection: &Connection,
        start: i64,
        end: i64,
        filters: &AggregateFilters,
        prices: &PriceTable,
    ) -> Result<CostTotals> {
        let mut statement = connection.prepare(
            "SELECT
                provider_id, model_id,
                tok_input, tok_output, tok_cache_read, tok_cache_write,
                cost, cost_source
             FROM usage_record
             WHERE is_incomplete = 0
               AND time_created_utc >= ?1
               AND time_created_utc < ?2
               AND (?3 IS NULL OR host_id = ?3)
               AND (?4 IS NULL OR source = ?4)
               AND (?5 IS NULL OR agent_key = ?5)
               AND (?6 IS NULL OR provider_id = ?6)
               AND (?7 IS NULL OR model_id = ?7)",
        )?;
        let mut rows = statement.query(params![
            start,
            end,
            filters.host_id.as_deref(),
            filters.source.as_deref(),
            filters.agent_key.as_deref(),
            filters.provider_id.as_deref(),
            filters.model_id.as_deref(),
        ])?;
        let mut totals = CostTotals::default();
        while let Some(row) = rows.next()? {
            let provider_id: String = row.get(0)?;
            let model_id: String = row.get(1)?;
            let source_text: String = row.get(7)?;
            totals.add(prices.resolve_cost(
                &provider_id,
                &model_id,
                TokenCounts {
                    input: nonnegative("tok_input", row.get(2)?)?,
                    output: nonnegative("tok_output", row.get(3)?)?,
                    cache_read: nonnegative("tok_cache_read", row.get(4)?)?,
                    cache_write: nonnegative("tok_cache_write", row.get(5)?)?,
                },
                row.get(6)?,
                parse_cost_source(&source_text)?,
            ));
        }
        Ok(totals)
    }

    /// 保留优化前的逐桶查询形状，只用于证明单次聚合与旧语义逐字段等价。
    fn query_series_per_bucket_legacy<C: CoverageLookup + ?Sized>(
        archive: &Archive,
        range: &LocalDateRange,
        granularity: Granularity,
        filters: &AggregateFilters,
        prices: &PriceTable,
        coverage: &C,
    ) -> Result<Vec<SeriesBucket>> {
        let mut series = Vec::new();
        for bucket in generate_buckets(range, granularity)? {
            let coverage_status = coverage.status(&bucket, filters);
            if coverage_status == CoverageStatus::None {
                series.push(SeriesBucket {
                    bucket,
                    coverage: coverage_status,
                    tokens: None,
                    cost: None,
                    message_count: None,
                    session_record_count: None,
                });
                continue;
            }

            let aggregate = query_raw_aggregate(
                archive.connection(),
                bucket.start_utc_ms,
                bucket.end_utc_ms,
                filters,
            )?;
            let cost = query_cost_totals_per_row_legacy(
                archive.connection(),
                bucket.start_utc_ms,
                bucket.end_utc_ms,
                filters,
                prices,
            )?;
            series.push(SeriesBucket {
                bucket,
                coverage: coverage_status,
                tokens: Some(tokens_from_raw(aggregate)?),
                cost: Some(cost),
                message_count: Some(nonnegative("message_count", aggregate.message_count)?),
                session_record_count: Some(nonnegative(
                    "session_record_count",
                    aggregate.session_record_count,
                )?),
            });
        }
        Ok(series)
    }

    #[test]
    fn query_literal_epoch_labels_are_report_timezone_calendar_values() {
        let epoch_ms = 1_785_468_844_419;
        let shanghai: chrono_tz::Tz = "Asia/Shanghai".parse().expect("Shanghai timezone");
        let utc: chrono_tz::Tz = "UTC".parse().expect("UTC timezone");

        let cases = [
            (shanghai, Granularity::Day, WeekStart::Monday, "2026-07-31"),
            (shanghai, Granularity::Week, WeekStart::Monday, "2026-W31"),
            (shanghai, Granularity::Month, WeekStart::Monday, "2026-07"),
            (utc, Granularity::Day, WeekStart::Monday, "2026-07-31"),
        ];
        for (timezone, granularity, week_start, expected) in cases {
            assert_eq!(
                label_for_epoch_ms(epoch_ms, timezone, granularity, week_start)
                    .expect("label fixed epoch"),
                expected
            );
        }
    }

    #[test]
    fn query_cross_timezone_utc_1630_lands_on_next_shanghai_day() {
        let epoch_ms = 1_785_515_400_000;
        let shanghai: chrono_tz::Tz = "Asia/Shanghai".parse().expect("Shanghai timezone");
        let utc: chrono_tz::Tz = "UTC".parse().expect("UTC timezone");

        let utc_label = label_for_epoch_ms(epoch_ms, utc, Granularity::Day, WeekStart::Monday)
            .expect("UTC label");
        let shanghai_label =
            label_for_epoch_ms(epoch_ms, shanghai, Granularity::Day, WeekStart::Monday)
                .expect("Shanghai label");
        assert_eq!(utc_label, "2026-07-31");
        assert_eq!(shanghai_label, "2026-08-01");
    }

    #[test]
    fn query_week_start_changes_week_label() {
        let epoch_ms = 1_785_468_844_419;
        let timezone: chrono_tz::Tz = "Asia/Shanghai".parse().expect("Shanghai timezone");
        let monday = label_for_epoch_ms(epoch_ms, timezone, Granularity::Week, WeekStart::Monday)
            .expect("Monday week label");
        let sunday = label_for_epoch_ms(epoch_ms, timezone, Granularity::Week, WeekStart::Sunday)
            .expect("Sunday week label");

        assert_eq!(monday, "2026-W31");
        assert_eq!(sunday, "2026-W30");
        assert_ne!(monday, sunday);
    }

    #[test]
    fn query_invalid_timezone_and_adversarial_ranges_return_readable_errors() {
        let invalid_timezone = LocalDateRange::from_timezone_name(
            date("2026-07-31"),
            date("2026-08-01"),
            "Mars/Olympus_Mons",
            WeekStart::Monday,
        )
        .expect_err("invalid timezone must fail");
        assert!(invalid_timezone.to_string().contains("Mars/Olympus_Mons"));
        assert!(invalid_timezone.to_string().contains("IANA timezone"));

        let utc: chrono_tz::Tz = "UTC".parse().expect("UTC timezone");
        for (start, end) in [("2026-07-31", "2026-07-31"), ("2026-08-01", "2026-07-31")] {
            let error = LocalDateRange::new(date(start), date(end), utc, WeekStart::Monday)
                .expect_err("non-positive range must fail");
            assert!(error.to_string().contains("end_date_exclusive"));
        }
        for (start, end) in [("1900-01-01", "1900-01-02"), ("3000-01-01", "3000-01-02")] {
            let error = LocalDateRange::new(date(start), date(end), utc, WeekStart::Monday)
                .expect_err("unsupported report year must fail");
            assert!(error.to_string().contains("supported report years"));
        }
        let error = LocalDateRange::new(
            date("2020-01-01"),
            date("2035-01-01"),
            utc,
            WeekStart::Monday,
        )
        .expect_err("oversized range must fail");
        assert!(error.to_string().contains("maximum report range"));
    }

    #[test]
    fn query_half_open_range_excludes_record_at_exact_end() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, end) = range.utc_bounds().expect("UTC report bounds");
        insert_record(&archive, &fixed_record("at-start", start, 10));
        insert_record(&archive, &fixed_record("at-end", end, 100));

        let summary = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("query half-open summary");
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.tokens.tok_input, 10);
        assert_eq!(summary.tokens.total_input, 10 + 13 + 14);
    }

    #[test]
    fn query_separates_record_granularity_counts_but_sums_all_tokens() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("granularity test bounds");
        insert_record(
            &archive,
            &fixed_record("message-granularity", start + 1_000, 10),
        );
        insert_record(
            &archive,
            &fixed_record("session-granularity", start + 2_000, 20),
        );
        archive
            .connection()
            .execute(
                "UPDATE usage_record SET granularity = 'session' WHERE message_id = 'session-granularity'",
                [],
            )
            .expect("mark one fixture row as session-level");

        let summary = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("query mixed-granularity summary");
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.session_record_count, 1);
        assert_eq!(summary.active_session_count, 2);
        assert_eq!(summary.tokens.tok_input, 30);
        assert_eq!(summary.tokens.tok_output, 32);
        assert_eq!(summary.tokens.tok_reasoning, 34);
        assert_eq!(summary.tokens.tok_cache_read, 36);
        assert_eq!(summary.tokens.tok_cache_write, 38);

        let series = query_series(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &PriceTable::new(),
            &AllCoverage,
        )
        .expect("query mixed-granularity series");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].message_count, Some(1));
        assert_eq!(series[0].session_record_count, Some(1));
        assert_eq!(
            series[0].tokens.expect("covered bucket has tokens"),
            summary.tokens
        );

        let breakdown = query_breakdown(
            &archive,
            &range,
            &AggregateFilters::default(),
            BreakdownOptions {
                expand_variant: false,
            },
            &PriceTable::new(),
        )
        .expect("query mixed-granularity breakdown");
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].message_count, 1);
        assert_eq!(breakdown[0].session_record_count, 1);
        assert_eq!(breakdown[0].tokens, summary.tokens);
    }

    #[test]
    fn query_dst_bucket_counts_tail_lengths_labels_and_conservation() {
        let cases = [
            ("America/New_York", "2026-03-08", 23usize, 60_i64),
            ("America/New_York", "2026-11-01", 25usize, 60_i64),
            ("Australia/Lord_Howe", "2026-10-04", 24usize, 30_i64),
            ("Australia/Lord_Howe", "2026-04-05", 25usize, 30_i64),
        ];
        let (_temp, archive) = empty_archive();

        for (index, (timezone, day, expected_count, expected_tail_minutes)) in
            cases.into_iter().enumerate()
        {
            let end = next_date(day);
            let range = report_range(day, &end, timezone);
            let hour_boundaries =
                generate_buckets(&range, Granularity::Hour).expect("generate DST hour buckets");
            assert_eq!(hour_boundaries.len(), expected_count, "{timezone} {day}");
            let tail = hour_boundaries.last().expect("DST day has hour buckets");
            assert_eq!(
                tail.end_utc_ms - tail.start_utc_ms,
                expected_tail_minutes * 60 * 1_000,
                "{timezone} {day} tail"
            );
            assert_eq!(
                hour_boundaries.first().expect("first hour").start_utc_ms,
                range.utc_bounds().expect("day bounds").0
            );
            assert_eq!(tail.end_utc_ms, range.utc_bounds().expect("day bounds").1);

            let timestamp = hour_boundaries[hour_boundaries.len() / 2].start_utc_ms + 1_000;
            insert_record(
                &archive,
                &fixed_record(
                    &format!("dst-conservation-{index}"),
                    timestamp,
                    10 + index as u64,
                ),
            );
            let hourly = query_series(
                &archive,
                &range,
                Granularity::Hour,
                &AggregateFilters::default(),
                &PriceTable::new(),
                &AllCoverage,
            )
            .expect("query DST hours");
            let daily = query_series(
                &archive,
                &range,
                Granularity::Day,
                &AggregateFilters::default(),
                &PriceTable::new(),
                &AllCoverage,
            )
            .expect("query DST day");
            assert_eq!(sum_series_tokens(&hourly), sum_series_tokens(&daily));
        }

        let fall = report_range("2026-11-01", "2026-11-02", "America/New_York");
        let labels = generate_buckets(&fall, Granularity::Hour)
            .expect("generate New York fall buckets")
            .into_iter()
            .map(|bucket| bucket.label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&"2026-11-01T01:00-04:00".to_string()));
        assert!(labels.contains(&"2026-11-01T01:00-05:00".to_string()));
    }

    #[test]
    fn query_regular_day_has_twenty_four_hour_buckets() {
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let buckets = generate_buckets(&range, Granularity::Hour).expect("normal UTC hours");
        assert_eq!(buckets.len(), 24);
        assert!(buckets
            .iter()
            .all(|bucket| bucket.end_utc_ms - bucket.start_utc_ms == 3_600_000));
    }

    #[test]
    fn query_fixture_manifest_cost_tokens_breakdown_and_dst_samples_match() {
        let (_temp, archive, manifest) = fixture_archive();
        let cost_row = &manifest.special_rows["cost_nonzero"];
        let day = utc_date_for(cost_row.time_created);
        let end = next_date(&day);
        let range = report_range(&day, &end, "UTC");
        let expected = &manifest.daily_expectations["UTC"][&day];

        let summary = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("query fixture summary");
        assert_eq!(summary.cost.actual_sum, 0.0102);
        assert_eq!(summary.cost.unavailable_count, 1);
        assert_eq!(summary.cost.estimated_sum, 0.0);
        assert_eq!(summary.cost.actual_sum, expected.cost.actual_sum);
        assert_eq!(
            summary.cost.unavailable_count,
            expected.cost.unavailable_count
        );
        assert_eq!(summary.tokens.tok_input, expected.tokens.input);
        assert_eq!(summary.tokens.tok_output, expected.tokens.output);
        assert_eq!(summary.tokens.tok_reasoning, expected.tokens.reasoning);
        assert_eq!(summary.tokens.tok_cache_read, expected.tokens.cache_read);
        assert_eq!(summary.tokens.tok_cache_write, expected.tokens.cache_write);
        assert_eq!(summary.tokens.total_input, expected.tokens.total_input);
        assert_eq!(summary.message_count, expected.message_count);
        assert_eq!(summary.active_session_count, expected.active_session_count);

        let literal_day = report_range("2026-07-31", "2026-08-01", "UTC");
        let breakdown = query_breakdown(
            &archive,
            &literal_day,
            &AggregateFilters::default(),
            BreakdownOptions {
                expand_variant: true,
            },
            &PriceTable::new(),
        )
        .expect("query fixture breakdown");
        assert!(breakdown.iter().any(|row| {
            row.source == "opencode"
                && row.agent_key == "atlas-plan-executor"
                && row.provider_id == "myopenai"
                && row.model_id == "us.anthropic.claude-fable-5"
                && row.variant.as_deref() == Some("xhigh")
        }));

        for sample in &manifest.dst_samples {
            let local_day = sample.local_time[..10].to_string();
            let sample_range = report_range(&local_day, &next_date(&local_day), &sample.timezone);
            let matching_bucket = generate_buckets(&sample_range, Granularity::Hour)
                .expect("generate manifest DST buckets")
                .into_iter()
                .find(|bucket| bucket.contains(sample.epoch_ms))
                .expect("manifest DST sample belongs to one hour bucket");
            if sample.label.starts_with("fall_") {
                assert!(
                    matching_bucket.label.ends_with("-04:00")
                        || matching_bucket.label.ends_with("-05:00")
                );
            }
        }
    }

    #[test]
    fn query_coverage_none_is_null_and_full_idle_is_zero() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-02", "UTC");
        let buckets = generate_buckets(&range, Granularity::Day).expect("two day buckets");
        let coverage = CoverageByStart {
            statuses: BTreeMap::from([
                (buckets[0].start_utc_ms, CoverageStatus::None),
                (buckets[1].start_utc_ms, CoverageStatus::Full),
            ]),
        };

        let series = query_series(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &PriceTable::new(),
            &coverage,
        )
        .expect("query coverage-aware series");
        assert_eq!(series[0].coverage, CoverageStatus::None);
        assert_eq!(series[0].tokens, None);
        assert_eq!(series[0].cost, None);
        assert_eq!(series[0].message_count, None);
        assert_eq!(series[1].coverage, CoverageStatus::Full);
        assert_eq!(series[1].tokens, Some(TokenValues::default()));
        assert_eq!(series[1].cost, Some(Default::default()));
        assert_eq!(series[1].message_count, Some(0));
    }

    /// The reason a bucket is not `Full` must reach the caller keyed by the bucket label the
    /// series already carries, and only for the buckets that need explaining — a note per bucket
    /// would put an unactionable row under every fully covered day.
    #[test]
    fn query_bundle_reports_coverage_notes_only_for_explainable_non_full_buckets() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-30", "2026-08-02", "UTC");
        let buckets = generate_buckets(&range, Granularity::Day).expect("three day buckets");
        let coverage = DiagnosingCoverage {
            statuses: BTreeMap::from([
                (buckets[0].start_utc_ms, CoverageStatus::Full),
                (buckets[1].start_utc_ms, CoverageStatus::Partial),
                (buckets[2].start_utc_ms, CoverageStatus::None),
            ]),
        };

        let bundle = query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &PriceTable::new(),
            &coverage,
        )
        .expect("query series bundle with diagnosable coverage");

        assert_eq!(
            bundle.coverage_notes,
            vec![
                CoverageNote {
                    label: buckets[1].label.clone(),
                    shortfalls: vec![CoverageShortfall {
                        host_id: "host-a".to_string(),
                        source: "codex".to_string(),
                        partial: true,
                    }],
                },
                CoverageNote {
                    label: buckets[2].label.clone(),
                    shortfalls: vec![CoverageShortfall {
                        host_id: "host-a".to_string(),
                        source: "codex".to_string(),
                        partial: false,
                    }],
                },
            ]
        );
    }

    /// A lookup that does not override the defaulted `shortfalls` must produce no notes at all,
    /// rather than notes with empty shortfall lists that the UI would render as blank rows.
    #[test]
    fn query_bundle_omits_coverage_notes_when_the_lookup_cannot_diagnose() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-02", "UTC");
        let buckets = generate_buckets(&range, Granularity::Day).expect("two day buckets");
        let coverage = CoverageByStart {
            statuses: BTreeMap::from([
                (buckets[0].start_utc_ms, CoverageStatus::Partial),
                (buckets[1].start_utc_ms, CoverageStatus::None),
            ]),
        };

        let bundle = query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &PriceTable::new(),
            &coverage,
        )
        .expect("query series bundle with undiagnosable coverage");

        assert_eq!(bundle.total[0].coverage, CoverageStatus::Partial);
        assert!(bundle.coverage_notes.is_empty());
    }

    #[test]
    fn query_single_pass_series_is_field_for_field_equivalent_to_per_bucket_path() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-10-30", "2026-11-04", "America/New_York");
        let buckets = generate_buckets(&range, Granularity::Day).expect("five local-day buckets");
        assert_eq!(buckets.len(), 5);

        let mut actual_one = fixed_record("actual-one", buckets[0].start_utc_ms + 1_000, 10);
        actual_one.cost = Some(0.125);
        actual_one.cost_source = CostSource::Actual;
        let mut actual_two = fixed_record("actual-two", buckets[0].start_utc_ms + 2_000, 20);
        actual_two.cost = Some(0.25);
        actual_two.cost_source = CostSource::Actual;

        let mut stored_estimate =
            fixed_record("stored-estimate", buckets[1].start_utc_ms + 1_000, 30);
        stored_estimate.cost = Some(0.5);
        stored_estimate.cost_source = CostSource::Estimated;

        let dynamic_one = fixed_record("dynamic-one", buckets[2].start_utc_ms + 1_000, 40);
        let dynamic_two = fixed_record("dynamic-two", buckets[2].start_utc_ms + 2_000, 50);

        let mut unavailable_one =
            fixed_record("unavailable-one", buckets[3].start_utc_ms + 1_000, 60);
        unavailable_one.provider_id = "unpriced-provider".to_string();
        unavailable_one.model_id = "unpriced-model".to_string();
        let mut unavailable_two =
            fixed_record("unavailable-two", buckets[3].start_utc_ms + 2_000, 70);
        unavailable_two.provider_id = "unpriced-provider".to_string();
        unavailable_two.model_id = "unpriced-model".to_string();

        let mut session_record =
            fixed_record("session-record", buckets[4].start_utc_ms + 1_000, 80);
        session_record.granularity = crate::archive::UsageGranularity::Session;
        session_record.source = "claude-code".to_string();
        session_record.agent_raw = "Build".to_string();
        session_record.agent_key = "build".to_string();
        session_record.provider_id = "second-provider".to_string();
        session_record.model_id = "second-model".to_string();

        for record in [
            &actual_one,
            &actual_two,
            &stored_estimate,
            &dynamic_one,
            &dynamic_two,
            &unavailable_one,
            &unavailable_two,
            &session_record,
        ] {
            insert_record(&archive, record);
        }
        archive
            .connection()
            .execute(
                "UPDATE usage_record SET granularity = 'session' WHERE message_id = 'session-record'",
                [],
            )
            .expect("mark equivalence fixture session record");

        let prices = PriceTable::from_entries(vec![
            PriceEntry::new(
                "query-provider",
                "query-model",
                1_000_000.0,
                1_000_000.0,
                1_000_000.0,
                1_000_000.0,
            ),
            PriceEntry::new(
                "second-provider",
                "second-model",
                1_000_000.0,
                1_000_000.0,
                1_000_000.0,
                1_000_000.0,
            ),
        ]);
        let coverage = CoverageByStart {
            statuses: BTreeMap::from([
                (buckets[0].start_utc_ms, CoverageStatus::Full),
                (buckets[1].start_utc_ms, CoverageStatus::Partial),
                (buckets[2].start_utc_ms, CoverageStatus::None),
                (buckets[3].start_utc_ms, CoverageStatus::Full),
                (buckets[4].start_utc_ms, CoverageStatus::Full),
            ]),
        };

        let expected = query_series_per_bucket_legacy(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &prices,
            &coverage,
        )
        .expect("legacy per-bucket series");
        let actual = query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &AggregateFilters::default(),
            &prices,
            &coverage,
        )
        .expect("single-pass grouped series");

        assert_eq!(actual.total, expected);
        assert_eq!(actual.total[0].cost.expect("actual cost").actual_sum, 0.375);
        assert_eq!(
            actual.total[1]
                .cost
                .expect("stored estimated cost")
                .estimated_sum,
            0.5
        );
        assert_eq!(actual.total[2].coverage, CoverageStatus::None);
        assert_eq!(actual.total[2].tokens, None);
        assert_eq!(
            actual.total[3]
                .cost
                .expect("unavailable cost")
                .unavailable_count,
            2
        );
        assert_eq!(actual.total[4].message_count, Some(0));
        assert_eq!(actual.total[4].session_record_count, Some(1));
        assert!(actual.groups.iter().any(|group| {
            group.dimension == SeriesGroupDimension::Model
                && group.id == "query-provider\0query-model"
        }));
    }

    #[test]
    #[ignore = "manual 251737-row performance acceptance"]
    fn query_perf_251737_rows_reports_single_pass_and_index_cost() {
        const ROW_COUNT: usize = 251_737;
        const MODEL_COUNT: usize = 6;
        const INDEXES: &str = "
            CREATE INDEX usage_record_source_time_created_utc_idx
                ON usage_record(source, time_created_utc);
            CREATE INDEX usage_record_agent_key_time_created_utc_idx
                ON usage_record(agent_key, time_created_utc);
            CREATE INDEX usage_record_provider_id_time_created_utc_idx
                ON usage_record(provider_id, time_created_utc);
            CREATE INDEX usage_record_model_id_time_created_utc_idx
                ON usage_record(model_id, time_created_utc);";

        let (_temp, mut archive) = empty_archive();
        let range = report_range("2026-01-01", "2026-02-01", "UTC");
        let (start, end) = range.utc_bounds().expect("performance range bounds");
        let span = end - start;
        let transaction = archive
            .connection_mut()
            .transaction()
            .expect("start performance fixture transaction");
        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO usage_record (
                        host_id, source, message_id, session_id,
                        time_created_utc, time_completed_utc, source_time_updated,
                        origin, origin_priority, agent_raw, agent_key,
                        provider_id, model_id, variant,
                        tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
                        cost, cost_source, is_incomplete, project_dir
                    ) VALUES (
                        'host-perf', 'opencode', ?1, ?2, ?3, ?3, ?3,
                        'live', 3, 'Build', 'build',
                        'openai', ?4, NULL,
                        ?5, ?6, ?7, ?8, ?9,
                        ?10, ?11, 0, '/fixture/performance'
                    )",
                )
                .expect("prepare performance fixture insert");
            for index in 0..ROW_COUNT {
                let model_index = index % MODEL_COUNT;
                let timestamp = start + (index as i64 * span / ROW_COUNT as i64);
                let token = i64::try_from(index % 97 + 1).expect("small token fits i64");
                let (cost, cost_source) = match index % 3 {
                    0 => (Some(0.0), "actual"),
                    1 => (Some(0.0), "estimated"),
                    _ => (None, "unavailable"),
                };
                insert
                    .execute(params![
                        format!("perf-{index}"),
                        format!("session-{}", index % 4_096),
                        timestamp,
                        format!("model-{model_index}"),
                        token,
                        token + 1,
                        token + 2,
                        token + 3,
                        token + 4,
                        cost,
                        cost_source,
                    ])
                    .expect("insert performance fixture row");
            }
        }
        transaction
            .commit()
            .expect("commit performance fixture transaction");
        assert_eq!(
            archive
                .connection()
                .query_row("SELECT COUNT(*) FROM usage_record", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count performance fixture rows"),
            ROW_COUNT as i64
        );

        archive
            .connection()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")
            .expect("compact indexed performance archive");
        let indexed_bytes = fs::metadata(archive.path())
            .expect("stat indexed performance archive")
            .len();
        archive
            .connection()
            .execute_batch(
                "DROP INDEX usage_record_source_time_created_utc_idx;
                 DROP INDEX usage_record_agent_key_time_created_utc_idx;
                 DROP INDEX usage_record_provider_id_time_created_utc_idx;
                 DROP INDEX usage_record_model_id_time_created_utc_idx;
                 VACUUM;",
            )
            .expect("measure performance archive without dimension indexes");
        let unindexed_bytes = fs::metadata(archive.path())
            .expect("stat unindexed performance archive")
            .len();
        archive
            .connection()
            .execute_batch(INDEXES)
            .expect("restore performance dimension indexes");

        let filters = AggregateFilters::default();
        let prices = PriceTable::new();

        // 先预热 SQLite 页缓存，计时结果只比较查询形状而不是首次磁盘读取。
        query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &filters,
            &prices,
            &AllCoverage,
        )
        .expect("warm single-pass query");

        let started = Instant::now();
        let old_total = query_series_per_bucket_legacy(
            &archive,
            &range,
            Granularity::Day,
            &filters,
            &prices,
            &AllCoverage,
        )
        .expect("legacy ungrouped performance query");
        let old_ungrouped_ms = started.elapsed().as_millis();

        let started = Instant::now();
        let new_ungrouped = query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &filters,
            &prices,
            &AllCoverage,
        )
        .expect("single-pass ungrouped performance query");
        let new_ungrouped_ms = started.elapsed().as_millis();
        assert_eq!(new_ungrouped.total, old_total);

        let started = Instant::now();
        let mut old_model_series = BTreeMap::new();
        for model_index in 0..MODEL_COUNT {
            let model_id = format!("model-{model_index}");
            let model_filters = AggregateFilters {
                model_id: Some(model_id.clone()),
                ..AggregateFilters::default()
            };
            old_model_series.insert(
                format!("openai\0{model_id}"),
                query_series_per_bucket_legacy(
                    &archive,
                    &range,
                    Granularity::Day,
                    &model_filters,
                    &prices,
                    &AllCoverage,
                )
                .expect("legacy model performance query"),
            );
        }
        let old_six_groups_ms = started.elapsed().as_millis();

        let started = Instant::now();
        let new_grouped = query_series_bundle(
            &archive,
            &range,
            Granularity::Day,
            &filters,
            &prices,
            &AllCoverage,
        )
        .expect("single-pass grouped performance query");
        let new_six_groups_ms = started.elapsed().as_millis();
        for group in new_grouped
            .groups
            .iter()
            .filter(|group| group.dimension == SeriesGroupDimension::Model)
        {
            assert_eq!(
                old_model_series.get(&group.id),
                Some(&group.series),
                "model series {} differs from legacy query",
                group.id
            );
        }
        assert_eq!(
            new_grouped
                .groups
                .iter()
                .filter(|group| group.dimension == SeriesGroupDimension::Model)
                .count(),
            MODEL_COUNT
        );

        eprintln!(
            "PERF rows={ROW_COUNT} old_ungrouped_ms={old_ungrouped_ms} \
             new_ungrouped_ms={new_ungrouped_ms} old_six_groups_ms={old_six_groups_ms} \
             new_six_groups_ms={new_six_groups_ms} unindexed_bytes={unindexed_bytes} \
             indexed_bytes={indexed_bytes} index_delta_bytes={} ",
            indexed_bytes - unindexed_bytes
        );
    }

    #[test]
    fn query_detail_is_server_paged_with_correct_total_and_filters() {
        let (_temp, archive, manifest) = fixture_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, end) = range.utc_bounds().expect("detail UTC bounds");
        let independent_total_raw: i64 = archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE time_created_utc >= ?1 AND time_created_utc < ?2",
                params![start, end],
                |row| row.get(0),
            )
            .expect("independent detail count");
        let independent_total =
            u64::try_from(independent_total_raw).expect("count is non-negative");
        let page = query_details(
            &archive,
            &range,
            &DetailFilters::default(),
            50,
            0,
            &PriceTable::new(),
        )
        .expect("query first detail page");
        assert!(page.rows.len() <= 50);
        assert_eq!(page.limit, 50);
        assert_eq!(page.offset, 0);
        assert_eq!(page.total_count, independent_total);
        assert!(page.total_count >= manifest.same_timestamp_bucket.count);

        let capped = query_details(
            &archive,
            &range,
            &DetailFilters::default(),
            500,
            0,
            &PriceTable::new(),
        )
        .expect("query capped detail page");
        assert_eq!(capped.limit, MAX_DETAIL_LIMIT);
        assert!(capped.rows.len() <= MAX_DETAIL_LIMIT as usize);

        let incomplete = query_details(
            &archive,
            &range,
            &DetailFilters {
                is_incomplete: Some(true),
                ..DetailFilters::default()
            },
            50,
            0,
            &PriceTable::new(),
        )
        .expect("query incomplete detail rows");
        assert_eq!(incomplete.total_count, 1);
        assert!(incomplete.rows[0].is_incomplete);
    }

    #[test]
    fn query_detail_rejects_zero_limit_and_negative_offset_without_panicking() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let zero_limit = query_details(
            &archive,
            &range,
            &DetailFilters::default(),
            0,
            0,
            &PriceTable::new(),
        )
        .expect_err("zero detail limit must fail");
        assert!(zero_limit
            .to_string()
            .contains("limit must be greater than zero"));

        let negative_offset = query_details(
            &archive,
            &range,
            &DetailFilters::default(),
            50,
            -1,
            &PriceTable::new(),
        )
        .expect_err("negative offset must fail");
        assert!(negative_offset
            .to_string()
            .contains("offset must not be negative"));
    }

    #[test]
    fn query_repeated_reads_are_stable_and_new_rows_are_immediately_visible() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("stale-state bounds");
        insert_record(&archive, &fixed_record("stable-one", start + 1_000, 10));

        let first = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("first summary");
        let second = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("second summary");
        assert_eq!(first, second);

        insert_record(&archive, &fixed_record("stable-two", start + 2_000, 20));
        let after_insert = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("summary after insert");
        assert_eq!(after_insert.message_count, first.message_count + 1);
        assert_eq!(after_insert.tokens.tok_input, first.tokens.tok_input + 20);
    }

    #[test]
    fn query_week_month_hour_and_skipped_local_day_buckets_follow_calendar_boundaries() {
        let utc: chrono_tz::Tz = "UTC".parse().expect("UTC timezone");
        let final_supported_day = LocalDateRange::new(
            date("2100-12-31"),
            date("2101-01-01"),
            utc,
            WeekStart::Monday,
        )
        .expect("the final supported day is queryable");
        assert_eq!(
            generate_buckets(&final_supported_day, Granularity::Day)
                .expect("generate final supported day")
                .len(),
            1
        );
        assert!(matches!(
            LocalDateRange::new(
                date("2100-12-31"),
                date("2101-01-02"),
                utc,
                WeekStart::Monday,
            ),
            Err(QueryError::UnsupportedYear { .. })
        ));

        assert_eq!(
            label_for_epoch_ms(0, utc, Granularity::Hour, WeekStart::Monday)
                .expect("label Unix epoch hour"),
            "1970-01-01T00:00+00:00"
        );
        assert!(matches!(
            label_for_epoch_ms(i64::MAX, utc, Granularity::Day, WeekStart::Monday),
            Err(QueryError::InvalidTimestamp(i64::MAX))
        ));

        let weeks = generate_buckets(
            &report_range("2026-07-29", "2026-08-12", "UTC"),
            Granularity::Week,
        )
        .expect("generate clipped week buckets");
        assert_eq!(
            weeks
                .iter()
                .map(|bucket| bucket.label.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-W31", "2026-W32", "2026-W33"]
        );
        assert!(weeks
            .windows(2)
            .all(|pair| pair[0].end_utc_ms == pair[1].start_utc_ms));

        let months = generate_buckets(
            &report_range("2026-07-31", "2026-09-02", "UTC"),
            Granularity::Month,
        )
        .expect("generate clipped month buckets");
        assert_eq!(
            months
                .iter()
                .map(|bucket| bucket.label.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-07", "2026-08", "2026-09"]
        );
        assert_eq!(months[0].end_utc_ms - months[0].start_utc_ms, 86_400_000);

        let skipped_day = report_range("2011-12-30", "2011-12-31", "Pacific/Apia");
        let (start, end) = skipped_day.utc_bounds().expect("resolve skipped Apia day");
        assert_eq!(start, end);
        assert!(generate_buckets(&skipped_day, Granularity::Day)
            .expect("a skipped local date is an empty interval")
            .is_empty());
    }

    #[test]
    fn query_collapsed_breakdown_and_detail_rows_keep_all_four_cost_states_distinct() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("cost-state bounds");

        let mut actual = fixed_record("cost-actual", start + 1_000, 10);
        actual.variant = Some("high".to_string());
        actual.cost = Some(0.1);
        actual.cost_source = CostSource::Actual;
        let mut stored_estimate = fixed_record("cost-stored-estimate", start + 2_000, 20);
        stored_estimate.variant = Some("low".to_string());
        stored_estimate.cost = Some(0.2);
        stored_estimate.cost_source = CostSource::Estimated;
        let mut dynamic_estimate = fixed_record("cost-dynamic-estimate", start + 3_000, 30);
        dynamic_estimate.variant = None;
        let mut unavailable = fixed_record("cost-unavailable", start + 4_000, 40);
        unavailable.provider_id = "unpriced-provider".to_string();
        unavailable.model_id = "unpriced-model".to_string();
        for record in [&actual, &stored_estimate, &dynamic_estimate, &unavailable] {
            insert_record(&archive, record);
        }

        let prices = PriceTable::from_entries(vec![PriceEntry::new(
            "query-provider",
            "query-model",
            1.0,
            1.0,
            1.0,
            1.0,
        )]);
        let expected_dynamic = prices
            .estimate(
                "query-provider",
                "query-model",
                crate::pricing::TokenCounts {
                    input: dynamic_estimate.tok_input,
                    output: dynamic_estimate.tok_output,
                    cache_read: dynamic_estimate.tok_cache_read,
                    cache_write: dynamic_estimate.tok_cache_write,
                },
            )
            .expect("priced model has an estimate");
        let collapsed = query_breakdown(
            &archive,
            &range,
            &AggregateFilters::default(),
            BreakdownOptions {
                expand_variant: false,
            },
            &prices,
        )
        .expect("query collapsed cost breakdown");
        let priced_group = collapsed
            .iter()
            .find(|row| row.provider_id == "query-provider")
            .expect("priced collapsed group");
        assert_eq!(priced_group.variant, None);
        assert_eq!(priced_group.message_count, 3);
        assert_eq!(priced_group.cost.actual_sum, 0.1);
        assert_eq!(priced_group.cost.estimated_sum, 0.2 + expected_dynamic);
        assert_eq!(priced_group.cost.unavailable_count, 0);
        let unavailable_group = collapsed
            .iter()
            .find(|row| row.provider_id == "unpriced-provider")
            .expect("unpriced collapsed group");
        assert_eq!(unavailable_group.cost.unavailable_count, 1);

        let filters = DetailFilters {
            host_id: Some("host-query-test".to_string()),
            source: Some("opencode".to_string()),
            agent_key: Some("atlas-plan-executor".to_string()),
            provider_id: Some("query-provider".to_string()),
            model_id: Some("query-model".to_string()),
            is_incomplete: Some(false),
        };
        let details = query_details(&archive, &range, &filters, 50, 0, &prices)
            .expect("query all priced detail cost states");
        assert_eq!(details.total_count, 3);
        let by_id = details
            .rows
            .iter()
            .map(|row| (row.message_id.as_str(), row.cost))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            by_id["cost-actual"],
            DetailCost {
                actual: Some(0.1),
                estimated: None,
                unavailable: false,
            }
        );
        assert_eq!(
            by_id["cost-stored-estimate"],
            DetailCost {
                actual: None,
                estimated: Some(0.2),
                unavailable: false,
            }
        );
        assert_eq!(
            by_id["cost-dynamic-estimate"],
            DetailCost {
                actual: None,
                estimated: Some(expected_dynamic),
                unavailable: false,
            }
        );

        let unpriced_details = query_details(
            &archive,
            &range,
            &DetailFilters {
                provider_id: Some("unpriced-provider".to_string()),
                ..DetailFilters::default()
            },
            50,
            0,
            &prices,
        )
        .expect("query unavailable detail state");
        assert_eq!(unpriced_details.total_count, 1);
        assert_eq!(
            unpriced_details.rows[0].cost,
            DetailCost {
                actual: None,
                estimated: None,
                unavailable: true,
            }
        );
    }

    #[test]
    fn query_reports_corrupt_negative_tokens_and_unknown_cost_provenance_by_column() {
        let (_temp, archive) = empty_archive();
        let range = report_range("2026-07-31", "2026-08-01", "UTC");
        let (start, _) = range.utc_bounds().expect("corrupt-row bounds");
        insert_record(&archive, &fixed_record("corrupt-row", start + 1_000, 10));
        archive
            .connection()
            .execute(
                "UPDATE usage_record SET tok_input = -1 WHERE message_id = 'corrupt-row'",
                [],
            )
            .expect("inject a negative token count");
        let negative = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect_err("negative archive counters must be rejected");
        assert!(matches!(
            negative,
            QueryError::InvalidStoredInteger {
                column: "tok_input",
                value: -1
            }
        ));

        archive
            .connection()
            .execute(
                "UPDATE usage_record SET tok_input = 10 WHERE message_id = 'corrupt-row'",
                [],
            )
            .expect("restore token count");
        archive
            .connection()
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("allow corruption fixture");
        archive
            .connection()
            .execute(
                "UPDATE usage_record SET cost_source = 'mystery' WHERE message_id = 'corrupt-row'",
                [],
            )
            .expect("inject unknown cost provenance");
        let provenance = query_summary(
            &archive,
            &range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect_err("unknown cost provenance must be rejected");
        assert!(matches!(
            provenance,
            QueryError::InvalidCostSource(ref value) if value == "mystery"
        ));
    }

    #[test]
    #[ignore = "manual QA invokes the external sqlite3 binary and prints DST bucket lists"]
    fn query_manual_qa_external_sqlite3_dst_and_error_surface() {
        let (temp, archive) = empty_archive();
        let literal_range = report_range("2026-07-31", "2026-08-01", "Asia/Shanghai");
        insert_record(
            &archive,
            &fixed_record("manual-literal", 1_785_468_844_419, 7_322),
        );

        for granularity in [
            Granularity::Hour,
            Granularity::Day,
            Granularity::Week,
            Granularity::Month,
        ] {
            let series = query_series(
                &archive,
                &literal_range,
                granularity,
                &AggregateFilters::default(),
                &PriceTable::new(),
                &AllCoverage,
            )
            .expect("manual surface query");
            println!("manual_surface granularity={granularity:?} series={series:#?}");
        }

        let summary = query_summary(
            &archive,
            &literal_range,
            &AggregateFilters::default(),
            &PriceTable::new(),
        )
        .expect("manual summary");
        let (start, end) = literal_range.utc_bounds().expect("manual UTC bounds");
        let sql = format!(
            "SELECT coalesce(sum(tok_input), 0) FROM usage_record WHERE is_incomplete = 0 AND time_created_utc >= {start} AND time_created_utc < {end};"
        );
        let output = Command::new("sqlite3")
            .arg(archive.path())
            .arg(sql)
            .output()
            .expect("run external sqlite3");
        assert!(output.status.success());
        let external = String::from_utf8(output.stdout)
            .expect("sqlite3 UTF-8 output")
            .trim()
            .parse::<u64>()
            .expect("sqlite3 integer sum");
        assert_eq!(summary.tokens.tok_input, external);
        println!(
            "external_sqlite3_cross_check query={} sqlite3={external}",
            summary.tokens.tok_input
        );

        for (index, (timezone, day)) in [
            ("America/New_York", "2026-03-08"),
            ("America/New_York", "2026-11-01"),
            ("Australia/Lord_Howe", "2026-10-04"),
            ("Australia/Lord_Howe", "2026-04-05"),
        ]
        .into_iter()
        .enumerate()
        {
            let range = report_range(day, &next_date(day), timezone);
            let boundaries = generate_buckets(&range, Granularity::Hour).expect("manual DST hours");
            let timestamp = boundaries[boundaries.len() / 2].start_utc_ms + 1_000;
            insert_record(
                &archive,
                &fixed_record(
                    &format!("manual-dst-{index}"),
                    timestamp,
                    101 + index as u64,
                ),
            );
            let hourly = query_series(
                &archive,
                &range,
                Granularity::Hour,
                &AggregateFilters::default(),
                &PriceTable::new(),
                &AllCoverage,
            )
            .expect("manual DST hour query");
            let daily = query_series(
                &archive,
                &range,
                Granularity::Day,
                &AggregateFilters::default(),
                &PriceTable::new(),
                &AllCoverage,
            )
            .expect("manual DST day query");
            let hour_total = sum_series_tokens(&hourly).tok_input;
            let day_total = sum_series_tokens(&daily).tok_input;
            assert_eq!(hour_total, day_total);
            println!(
                "DST {timezone} {day} count={} conservation={hour_total}={day_total}",
                boundaries.len()
            );
            for bucket in boundaries {
                println!(
                    "  {} [{}..{}) duration_minutes={}",
                    bucket.label,
                    bucket.start_utc_ms,
                    bucket.end_utc_ms,
                    (bucket.end_utc_ms - bucket.start_utc_ms) / 60_000
                );
            }
        }

        let error = LocalDateRange::from_timezone_name(
            date("2026-07-31"),
            date("2026-08-01"),
            "Mars/Olympus_Mons",
            WeekStart::Monday,
        )
        .expect_err("manual invalid timezone");
        println!("invalid_timezone_error={error}");

        let path: PathBuf = archive.path().to_path_buf();
        drop(archive);
        temp.close().expect("remove manual QA directory");
        assert!(!path.exists());
        println!("cleanup_receipt removed={}", path.display());
    }
}
