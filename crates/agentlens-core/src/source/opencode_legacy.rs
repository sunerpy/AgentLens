//! legacy JSON 回填、备份库导入与覆盖区间（todo 7）。
//!
//! 本模块遍历 `storage/message/*/*.json`（lossy 解析，agent 为 slug、无 variant），
//! 归一化后走同一 `crate::ingest` 路径（优先级 legacy=1），绝不覆盖 live 行、
//! 也不推进 live watermark；可选导入 `opencode.db.bak.*`（同 schema、同解析器，优先级 bak=2）。
//!
//! 覆盖区间语义写入 `coverage_interval(host_id, source, origin, interval_start, interval_end)`：
//! live 每次成功扫描把 `interval_end` 延伸到本次扫描快照的 cutoff 时刻（即使本轮 0 行也延伸）；
//! `interval_start` 首次扫描有行时取最早 observed record time，首次扫描 0 eligible 行时取 cutoff
//! （零长度区间，不虚构历史覆盖）；legacy / bak 的区间为该来源数据的实际跨度。
//!
//! 三态运算：先把每个所选 `(host, source)` 的全部 origin 区间合并为并集；
//! `full` = 每个所选 `(host, source)` 的并集都完整覆盖整个桶；
//! `none` = 没有任何所选 `(host, source)` 的并集与桶相交；
//! `partial` = 其余一切情形，包括单主机区间只覆盖桶的一部分。

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde_json::Value;
use thiserror::Error;

use crate::archive::{Archive, NormalizedUsageRecord, Origin};
use crate::ingest::{IngestError, IngestRound, IngestStats, OPENCODE_SOURCE};
use crate::query::{AggregateFilters, CoverageLookup, CoverageStatus, TimeBucket};
use crate::source::opencode::{
    parse_message, scan_database, OpenCodeError, ParseContext, ScanRequest, ScanResult, SinkError,
    SkipReason, SkippedBreakdown, SourceMessageRow, DEFAULT_BATCH_SIZE,
};

/// Default batch size shared with the live OpenCode scanner.
pub const DEFAULT_LEGACY_BATCH_SIZE: usize = DEFAULT_BATCH_SIZE;

/// Result type returned by legacy, backup, live-coverage, and coverage-lookup operations.
pub type Result<T> = std::result::Result<T, LegacyError>;

/// Failures that prevent a trustworthy import or coverage update.
#[derive(Debug, Error)]
pub enum LegacyError {
    /// The configured batch size cannot deliver any records.
    #[error("legacy backfill batch_size must be greater than zero")]
    InvalidBatchSize,
    /// A half-open interval has its end before its start.
    #[error("invalid coverage interval [{start}, {end}): end must not precede start")]
    InvalidInterval {
        /// Inclusive interval start.
        start: i64,
        /// Exclusive interval end.
        end: i64,
    },
    /// A live snapshot cutoff cannot precede a record visible in that snapshot.
    #[error("live coverage cutoff {cutoff} precedes earliest observed record {earliest}")]
    CutoffBeforeRecord {
        /// Earliest eligible record observed by the scan.
        earliest: i64,
        /// Caller-captured snapshot cutoff.
        cutoff: i64,
    },
    /// A required legacy tree directory could not be enumerated.
    #[error("cannot read legacy JSON directory {path}: {source}")]
    ReadDirectory {
        /// Directory that could not be read.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// A scanner sink rejected a normalized batch.
    #[error("archive ingest rejected a scanner batch: {0}")]
    Sink(String),
    /// The shared archive ingest contract rejected a record or transaction.
    #[error(transparent)]
    Ingest(#[from] IngestError),
    /// The shared read-only OpenCode database scanner failed.
    #[error(transparent)]
    OpenCode(#[from] OpenCodeError),
    /// SQLite rejected a coverage read or write.
    #[error("coverage interval database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Validated half-open UTC epoch-millisecond coverage interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverageInterval {
    /// Inclusive UTC epoch-millisecond boundary.
    pub start: i64,
    /// Exclusive UTC epoch-millisecond boundary.
    pub end: i64,
}

impl CoverageInterval {
    /// Creates an interval, accepting zero length while rejecting reversed boundaries.
    pub fn new(start: i64, end: i64) -> Result<Self> {
        if end < start {
            return Err(LegacyError::InvalidInterval { start, end });
        }
        Ok(Self { start, end })
    }

    fn intersects(self, bucket: &TimeBucket) -> bool {
        self.start < bucket.end_utc_ms && self.end > bucket.start_utc_ms
    }

    fn covers(self, bucket: &TimeBucket) -> bool {
        self.start <= bucket.start_utc_ms && self.end >= bucket.end_utc_ms
    }
}

/// Immutable, error-free coverage snapshot consumed directly by `query_series`.
///
/// Loading validates stored intervals and unions every origin for each `(host_id, source)` pair.
/// Query-time status resolution is therefore infallible, matching [`CoverageLookup`]'s signature.
#[derive(Clone, Debug, Default)]
pub struct CoverageStore {
    pairs: BTreeMap<(String, String), Vec<CoverageInterval>>,
}

impl CoverageStore {
    /// Loads and unions all stored origin intervals from one archive connection.
    pub fn load(connection: &Connection) -> Result<Self> {
        let mut statement = connection.prepare(
            "SELECT host_id, source, interval_start, interval_end
             FROM coverage_interval
             ORDER BY host_id, source, interval_start, interval_end",
        )?;
        let mut rows = statement.query([])?;
        let mut pairs = BTreeMap::<(String, String), Vec<CoverageInterval>>::new();
        while let Some(row) = rows.next()? {
            let key = (row.get(0)?, row.get(1)?);
            let interval = CoverageInterval::new(row.get(2)?, row.get(3)?)?;
            pairs.entry(key).or_default().push(interval);
        }
        for intervals in pairs.values_mut() {
            *intervals = union_intervals(std::mem::take(intervals));
        }
        Ok(Self { pairs })
    }

    /// Returns the already-unioned intervals for diagnostics and IPC adapters.
    pub fn intervals_for(&self, host_id: &str, source: &str) -> &[CoverageInterval] {
        self.pairs
            .get(&(host_id.to_string(), source.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

impl CoverageLookup for CoverageStore {
    fn status(&self, bucket: &TimeBucket, filters: &AggregateFilters) -> CoverageStatus {
        if bucket.end_utc_ms <= bucket.start_utc_ms {
            return CoverageStatus::None;
        }

        let selected = self.pairs.iter().filter(|((host_id, source), _)| {
            filters
                .host_id
                .as_deref()
                .is_none_or(|selected_host| selected_host == host_id)
                && filters
                    .source
                    .as_deref()
                    .is_none_or(|selected_source| selected_source == source)
        });
        let mut selected_count = 0_u64;
        let mut every_pair_covers = true;
        let mut any_pair_intersects = false;
        for (_, intervals) in selected {
            selected_count += 1;
            every_pair_covers &= intervals.iter().any(|interval| interval.covers(bucket));
            any_pair_intersects |= intervals.iter().any(|interval| interval.intersects(bucket));
        }

        if selected_count == 0 || !any_pair_intersects {
            CoverageStatus::None
        } else if every_pair_covers {
            CoverageStatus::Full
        } else {
            CoverageStatus::Partial
        }
    }
}

/// Replaces all rows for one `(host, source, origin)` with at most one canonical interval.
///
/// Legacy and backup imports use this replace-per-origin strategy so idempotent reruns cannot grow
/// `coverage_interval` without bound. Passing `None` removes stale coverage for an empty source.
pub fn replace_origin_coverage(
    connection: &mut Connection,
    host_id: &str,
    source: &str,
    origin: Origin,
    interval: Option<CoverageInterval>,
) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "DELETE FROM coverage_interval
         WHERE host_id = ?1 AND source = ?2 AND origin = ?3",
        params![host_id, source, origin.as_str()],
    )?;
    if let Some(interval) = interval {
        transaction.execute(
            "INSERT INTO coverage_interval (
                host_id, source, origin, interval_start, interval_end
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                host_id,
                source,
                origin.as_str(),
                interval.start,
                interval.end
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Records one successful live scan, preserving its first start and monotonically extending end.
///
/// A first scan with no eligible rows writes `[cutoff, cutoff)`. Later scans never move start,
/// including when overlap-window records older than that first snapshot are observed.
pub fn extend_live_coverage(
    connection: &mut Connection,
    host_id: &str,
    source: &str,
    earliest_observed: Option<i64>,
    cutoff: i64,
) -> Result<CoverageInterval> {
    if let Some(earliest) = earliest_observed {
        if earliest > cutoff {
            return Err(LegacyError::CutoffBeforeRecord { earliest, cutoff });
        }
    }

    let transaction = connection.transaction()?;
    let existing = transaction.query_row(
        "SELECT min(interval_start), max(interval_end)
         FROM coverage_interval
         WHERE host_id = ?1 AND source = ?2 AND origin = 'live'",
        params![host_id, source],
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    let start = existing
        .0
        .unwrap_or_else(|| earliest_observed.unwrap_or(cutoff));
    let end = existing.1.map_or(cutoff, |current| current.max(cutoff));
    let interval = CoverageInterval::new(start, end)?;
    transaction.execute(
        "DELETE FROM coverage_interval
         WHERE host_id = ?1 AND source = ?2 AND origin = 'live'",
        params![host_id, source],
    )?;
    transaction.execute(
        "INSERT INTO coverage_interval (
            host_id, source, origin, interval_start, interval_end
         ) VALUES (?1, ?2, 'live', ?3, ?4)",
        params![host_id, source, interval.start, interval.end],
    )?;
    transaction.commit()?;
    Ok(interval)
}

/// Composite result from one live scan, atomic ingest, and successful-cutoff coverage update.
#[derive(Clone, Debug)]
pub struct LiveImportStats {
    /// Shared scanner counters and EOF state.
    pub scan: ScanResult,
    /// Shared conditional-upsert and cursor counters.
    pub ingest: IngestStats,
    /// Updated live interval; absent when the scan did not reach EOF.
    pub interval: Option<CoverageInterval>,
}

/// Runs the real live scanner through [`IngestRound`] and extends coverage to `cutoff` after EOF.
pub fn ingest_live_database(
    archive: &mut Archive,
    database_path: impl AsRef<Path>,
    host_id: impl Into<String>,
    watermark: Option<i64>,
    cutoff: i64,
) -> Result<LiveImportStats> {
    let host_id = host_id.into();
    let request = ScanRequest::live(host_id.clone(), watermark);
    let mut earliest_observed = None::<i64>;
    let mut sink_failure = None::<String>;
    let mut round = IngestRound::begin(archive.connection_mut(), &host_id, Origin::Live)?;
    let scan = scan_database(database_path, &request, |batch| {
        observe_span(batch, &mut earliest_observed, &mut None);
        if let Err(error) = round.ingest_batch(batch) {
            let message = error.to_string();
            sink_failure = Some(message.clone());
            return Err(SinkError::new(message));
        }
        Ok(())
    })?;
    if let Some(error) = sink_failure {
        drop(round);
        return Err(LegacyError::Sink(error));
    }
    if scan.reached_eof && earliest_observed.is_some_and(|earliest| earliest > cutoff) {
        drop(round);
        return Err(LegacyError::CutoffBeforeRecord {
            earliest: earliest_observed.expect("checked Some earliest"),
            cutoff,
        });
    }
    let ingest = round.finish(&scan)?;
    let interval = if ingest.committed && scan.reached_eof {
        Some(extend_live_coverage(
            archive.connection_mut(),
            &host_id,
            OPENCODE_SOURCE,
            earliest_observed,
            cutoff,
        )?)
    } else {
        None
    };
    Ok(LiveImportStats {
        scan,
        ingest,
        interval,
    })
}

/// Controls memory usage and optional bounded read-only QA scans of a legacy tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyBackfillOptions {
    /// Number of normalized records passed to each ingest call.
    pub batch_size: usize,
    /// Optional deterministic cap after sorting candidate paths.
    pub max_files: Option<usize>,
}

impl Default for LegacyBackfillOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_LEGACY_BATCH_SIZE,
            max_files: None,
        }
    }
}

/// Lossy warning counters from one legacy tree walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyWarningCounts {
    /// Individual files that disappeared or became unreadable during the walk.
    pub read_errors: u64,
    /// Truncated or otherwise invalid JSON documents.
    pub malformed_json: u64,
    /// JSON rows whose `tokens` value was not an object.
    pub invalid_tokens: u64,
    /// Assistant rows with no `tokens` key.
    pub missing_tokens: u64,
    /// Rows that were not assistant messages.
    pub non_assistant: u64,
    /// Rows missing a usable creation time or message/session path component.
    pub invalid_records: u64,
    /// Directory entries that violated the exact `session/message.json` tree shape.
    pub invalid_paths: u64,
    /// Paths imported through a lossy filename conversion or an embedded JSON identifier.
    pub non_utf8_paths: u64,
}

impl LegacyWarningCounts {
    fn total(self) -> u64 {
        self.read_errors
            + self.malformed_json
            + self.invalid_tokens
            + self.missing_tokens
            + self.non_assistant
            + self.invalid_records
            + self.invalid_paths
            + self.non_utf8_paths
    }

    fn record_skip(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::NonAssistant => self.non_assistant += 1,
            SkipReason::MissingTokens => self.missing_tokens += 1,
            SkipReason::MalformedJson => self.malformed_json += 1,
            SkipReason::InvalidTokens => self.invalid_tokens += 1,
        }
    }
}

/// Observable counters and transaction outcome from one legacy backfill.
#[derive(Clone, Debug)]
pub struct LegacyBackfillStats {
    /// JSON-shaped entries found before applying a cap.
    pub files_seen: u64,
    /// Regular files actually read before completion or interruption.
    pub files_attempted: u64,
    /// Eligible normalized records parsed from attempted files.
    pub eligible_records: u64,
    /// Attempted records skipped lossily.
    pub skipped_records: u64,
    /// Total warning count across all categories.
    pub warning_count: u64,
    /// Warning category detail.
    pub warnings: LegacyWarningCounts,
    /// True when `max_files` intentionally bounded the candidate list.
    pub limited: bool,
    /// True when the caller's control callback interrupted the round.
    pub interrupted: bool,
    /// Shared conditional-upsert transaction outcome.
    pub ingest: IngestStats,
    /// Actual imported record span, absent for interruption or no eligible records.
    pub interval: Option<CoverageInterval>,
}

/// Backfills the complete `storage/message/*/*.json` tree below an OpenCode data directory.
pub fn backfill_legacy(
    archive: &mut Archive,
    data_dir: impl AsRef<Path>,
    host_id: impl Into<String>,
) -> Result<LegacyBackfillStats> {
    backfill_legacy_with_options(
        archive,
        data_dir,
        host_id,
        &LegacyBackfillOptions::default(),
    )
}

/// Backfills with a deterministic path cap, primarily for bounded real-surface QA.
pub fn backfill_legacy_with_options(
    archive: &mut Archive,
    data_dir: impl AsRef<Path>,
    host_id: impl Into<String>,
    options: &LegacyBackfillOptions,
) -> Result<LegacyBackfillStats> {
    backfill_legacy_with_control(archive, data_dir, host_id, options, |_, _| true)
}

/// Backfills with an injectable per-file continuation check for cancellation/rollback tests.
pub fn backfill_legacy_with_control<F>(
    archive: &mut Archive,
    data_dir: impl AsRef<Path>,
    host_id: impl Into<String>,
    options: &LegacyBackfillOptions,
    mut should_continue: F,
) -> Result<LegacyBackfillStats>
where
    F: FnMut(&Path, u64) -> bool,
{
    if options.batch_size == 0 {
        return Err(LegacyError::InvalidBatchSize);
    }
    let host_id = host_id.into();
    let root = data_dir.as_ref().join("storage").join("message");
    let (mut paths, mut warnings, files_seen) = collect_legacy_paths(&root)?;
    let limited = options.max_files.is_some_and(|limit| paths.len() > limit);
    if let Some(limit) = options.max_files {
        paths.truncate(limit);
    }

    let context = ParseContext::new(&host_id, Origin::Legacy);
    let mut round = IngestRound::begin(archive.connection_mut(), &host_id, Origin::Legacy)?;
    let mut batch = Vec::<NormalizedUsageRecord>::with_capacity(options.batch_size);
    let mut files_attempted = 0_u64;
    let mut eligible_records = 0_u64;
    let mut skipped_records = 0_u64;
    let mut earliest = None::<i64>;
    let mut latest = None::<i64>;
    let mut interrupted = false;

    for path in paths {
        if !should_continue(&path, files_attempted) {
            interrupted = true;
            break;
        }
        files_attempted += 1;
        let Some(record) = parse_legacy_path(&path, &context, &mut warnings) else {
            skipped_records += 1;
            continue;
        };
        earliest = Some(earliest.map_or(record.time_created_utc, |value| {
            value.min(record.time_created_utc)
        }));
        latest = Some(latest.map_or(record.time_created_utc, |value| {
            value.max(record.time_created_utc)
        }));
        eligible_records += 1;
        batch.push(record);
        if batch.len() == options.batch_size {
            round.ingest_batch(&batch)?;
            batch.clear();
        }
    }

    if !interrupted && !batch.is_empty() {
        round.ingest_batch(&batch)?;
    }
    let scan = ScanResult {
        delivered_records: if interrupted { 0 } else { eligible_records },
        delivered_batches: 0,
        eligible_count: eligible_records,
        skipped_count: skipped_records,
        skipped_breakdown: SkippedBreakdown {
            non_assistant: warnings.non_assistant,
            missing_tokens: warnings.missing_tokens,
            malformed_json: warnings.malformed_json,
            invalid_tokens: warnings.invalid_tokens,
        },
        observed_max_time_updated: None,
        reached_eof: !interrupted,
        busy_retry_count: 0,
        last_success_utc: None,
        skip_reason: None,
    };
    let ingest = round.finish(&scan)?;
    let interval = if ingest.committed {
        actual_span(earliest, latest)?
    } else {
        None
    };
    if ingest.committed {
        replace_origin_coverage(
            archive.connection_mut(),
            &host_id,
            OPENCODE_SOURCE,
            Origin::Legacy,
            interval,
        )?;
    }
    let warning_count = warnings.total();
    Ok(LegacyBackfillStats {
        files_seen,
        files_attempted,
        eligible_records,
        skipped_records,
        warning_count,
        warnings,
        limited,
        interrupted,
        ingest,
        interval,
    })
}

/// Observable counters from an explicit optional backup-database import.
#[derive(Clone, Debug)]
pub struct BackupImportStats {
    /// Caller-provided opt-in state.
    pub enabled: bool,
    /// Number of candidate paths supplied without touching them.
    pub databases_requested: u64,
    /// Number of candidates successfully opened by the shared scanner.
    pub databases_opened: u64,
    /// Eligible records observed across all opened databases.
    pub eligible_records: u64,
    /// Lossily skipped rows observed across all opened databases.
    pub skipped_records: u64,
    /// Shared transaction outcome; default/false when disabled.
    pub ingest: IngestStats,
    /// Actual imported record span.
    pub interval: Option<CoverageInterval>,
}

/// Imports same-schema OpenCode backup databases only when `enabled` is explicitly true.
///
/// The disabled branch returns before metadata checks or SQLite opens, so even an enormous or
/// inaccessible backup path is untouched.
pub fn import_backup_databases(
    archive: &mut Archive,
    host_id: impl Into<String>,
    database_paths: &[PathBuf],
    enabled: bool,
) -> Result<BackupImportStats> {
    let databases_requested = database_paths.len() as u64;
    if !enabled {
        return Ok(BackupImportStats {
            enabled: false,
            databases_requested,
            databases_opened: 0,
            eligible_records: 0,
            skipped_records: 0,
            ingest: IngestStats::default(),
            interval: None,
        });
    }

    let host_id = host_id.into();
    let request = ScanRequest {
        host_id: host_id.clone(),
        watermark: None,
        origin: Origin::Bak,
        last_success_utc: None,
        batch_size: DEFAULT_BATCH_SIZE,
    };
    let mut round = IngestRound::begin(archive.connection_mut(), &host_id, Origin::Bak)?;
    let mut databases_opened = 0_u64;
    let mut eligible_records = 0_u64;
    let mut skipped_records = 0_u64;
    let mut delivered_records = 0_u64;
    let mut delivered_batches = 0_u64;
    let mut observed_max_time_updated = None::<i64>;
    let mut earliest = None::<i64>;
    let mut latest = None::<i64>;
    let mut all_reached_eof = true;
    let mut sink_failure = None::<String>;

    for path in database_paths {
        let scan = scan_database(path, &request, |batch| {
            observe_span(batch, &mut earliest, &mut latest);
            if let Err(error) = round.ingest_batch(batch) {
                let message = error.to_string();
                sink_failure = Some(message.clone());
                return Err(SinkError::new(message));
            }
            Ok(())
        })?;
        databases_opened += 1;
        eligible_records += scan.eligible_count;
        skipped_records += scan.skipped_count;
        delivered_records += scan.delivered_records;
        delivered_batches += scan.delivered_batches;
        observed_max_time_updated =
            max_optional(observed_max_time_updated, scan.observed_max_time_updated);
        all_reached_eof &= scan.reached_eof;
        if !scan.reached_eof {
            break;
        }
    }
    if let Some(error) = sink_failure {
        drop(round);
        return Err(LegacyError::Sink(error));
    }
    let combined_scan = ScanResult {
        delivered_records,
        delivered_batches,
        eligible_count: eligible_records,
        skipped_count: skipped_records,
        skipped_breakdown: SkippedBreakdown::default(),
        observed_max_time_updated: all_reached_eof
            .then_some(observed_max_time_updated)
            .flatten(),
        reached_eof: all_reached_eof,
        busy_retry_count: 0,
        last_success_utc: None,
        skip_reason: None,
    };
    let ingest = round.finish(&combined_scan)?;
    let interval = if ingest.committed {
        actual_span(earliest, latest)?
    } else {
        None
    };
    if ingest.committed {
        replace_origin_coverage(
            archive.connection_mut(),
            &host_id,
            OPENCODE_SOURCE,
            Origin::Bak,
            interval,
        )?;
    }
    Ok(BackupImportStats {
        enabled: true,
        databases_requested,
        databases_opened,
        eligible_records,
        skipped_records,
        ingest,
        interval,
    })
}

fn union_intervals(mut intervals: Vec<CoverageInterval>) -> Vec<CoverageInterval> {
    intervals.sort_by_key(|interval| (interval.start, interval.end));
    let mut union = Vec::<CoverageInterval>::with_capacity(intervals.len());
    for interval in intervals {
        if let Some(previous) = union.last_mut() {
            if interval.start <= previous.end {
                previous.end = previous.end.max(interval.end);
                continue;
            }
        }
        union.push(interval);
    }
    union
}

fn actual_span(earliest: Option<i64>, latest: Option<i64>) -> Result<Option<CoverageInterval>> {
    match (earliest, latest) {
        (Some(start), Some(end)) => Ok(Some(CoverageInterval::new(start, end.saturating_add(1))?)),
        _ => Ok(None),
    }
}

fn observe_span(
    batch: &[NormalizedUsageRecord],
    earliest: &mut Option<i64>,
    latest: &mut Option<i64>,
) {
    for record in batch {
        *earliest = Some(earliest.map_or(record.time_created_utc, |value| {
            value.min(record.time_created_utc)
        }));
        *latest = Some(latest.map_or(record.time_created_utc, |value| {
            value.max(record.time_created_utc)
        }));
    }
}

fn max_optional(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn collect_legacy_paths(root: &Path) -> Result<(Vec<PathBuf>, LegacyWarningCounts, u64)> {
    let sessions = fs::read_dir(root).map_err(|source| LegacyError::ReadDirectory {
        path: root.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    let mut warnings = LegacyWarningCounts::default();
    let mut files_seen = 0_u64;
    for session in sessions {
        let Ok(session) = session else {
            warnings.invalid_paths += 1;
            continue;
        };
        let session_path = session.path();
        let Ok(file_type) = session.file_type() else {
            warnings.invalid_paths += 1;
            continue;
        };
        if !file_type.is_dir() {
            warnings.invalid_paths += 1;
            continue;
        }
        let Ok(messages) = fs::read_dir(&session_path) else {
            warnings.invalid_paths += 1;
            continue;
        };
        for message in messages {
            let Ok(message) = message else {
                warnings.invalid_paths += 1;
                continue;
            };
            let path = message.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            files_seen += 1;
            let Ok(file_type) = message.file_type() else {
                warnings.invalid_paths += 1;
                continue;
            };
            if !file_type.is_file() {
                warnings.invalid_paths += 1;
                continue;
            }
            if path.file_name().is_some_and(|name| name.to_str().is_none()) {
                warnings.non_utf8_paths += 1;
            }
            paths.push(path);
        }
    }
    paths.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    Ok((paths, warnings, files_seen))
}

fn parse_legacy_path(
    path: &Path,
    context: &ParseContext,
    warnings: &mut LegacyWarningCounts,
) -> Option<NormalizedUsageRecord> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            warnings.read_errors += 1;
            return None;
        }
    };
    let data: Value = match serde_json::from_slice(&bytes) {
        Ok(data) => data,
        Err(_) => {
            warnings.malformed_json += 1;
            return None;
        }
    };
    let Some(time_created) = lossy_i64(data.pointer("/time/created")) else {
        warnings.invalid_records += 1;
        return None;
    };
    let message_id = nonblank_string(data.get("id")).unwrap_or_else(|| path_stem_lossy(path));
    let session_id =
        nonblank_string(data.get("sessionID")).unwrap_or_else(|| parent_name_lossy(path));
    if message_id.trim().is_empty() || session_id.trim().is_empty() {
        warnings.invalid_records += 1;
        return None;
    }
    let time_updated = lossy_i64(data.pointer("/time/updated"))
        .or_else(|| lossy_i64(data.pointer("/time/completed")))
        .unwrap_or(time_created);
    let data = match serde_json::to_string(&data) {
        Ok(data) => data,
        Err(_) => {
            warnings.malformed_json += 1;
            return None;
        }
    };
    let row = SourceMessageRow {
        message_id,
        session_id,
        time_created,
        time_updated,
        data,
    };
    match parse_message(row, context) {
        Ok(mut record) => {
            record.variant = None;
            Some(record)
        }
        Err(reason) => {
            warnings.record_skip(reason);
            None
        }
    }
}

fn nonblank_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn path_stem_lossy(path: &Path) -> String {
    path.file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parent_name_lossy(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn lossy_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value.trunc() as i64)
        })
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
mod tests {
    // `target_os = "linux"`, not `cfg(unix)`: inside this module `fs` is only reached from
    // the Linux-gated fixtures below, so on macOS a `cfg(unix)` import would be unused.
    #[cfg(target_os = "linux")]
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    #[cfg(target_os = "linux")]
    use std::time::Instant;

    use chrono::{DateTime, NaiveDate, Utc};
    use rusqlite::Connection;
    // Only `assistant_json` builds JSON values, and that helper is Linux-gated.
    #[cfg(target_os = "linux")]
    use serde_json::json;

    use crate::archive::{Archive, Origin};
    use crate::fixture::{generate, Manifest};
    use crate::ingest::{read_cursor, OPENCODE_SOURCE};
    use crate::pricing::PriceTable;
    use crate::query::{
        query_series, AggregateFilters, CoverageLookup, CoverageStatus, Granularity,
        LocalDateRange, TimeBucket, WeekStart,
    };

    use super::*;

    const TEST_HOST: &str = "host-legacy-test";

    fn fixture_directory() -> (tempfile::TempDir, PathBuf, Manifest) {
        let temp = tempfile::tempdir().expect("create fixture parent");
        let directory = temp.path().join("fixture");
        let manifest = generate(&directory).expect("generate fixture");
        (temp, directory, manifest)
    }

    fn fixture_archive() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        PathBuf,
        Archive,
        Manifest,
    ) {
        let (fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("create archive parent");
        let mut archive =
            Archive::open_in_data_dir(archive_temp.path()).expect("open fixture archive");
        let live = ingest_live_database(
            &mut archive,
            fixture_directory.join("opencode.db"),
            TEST_HOST,
            None,
            manifest.coverage.live_cutoff,
        )
        .expect("ingest fixture live database");
        assert!(live.scan.reached_eof);
        assert!(live.ingest.committed);
        let legacy = backfill_legacy(&mut archive, &fixture_directory, TEST_HOST)
            .expect("backfill fixture legacy tree");
        assert!(legacy.ingest.committed);
        (
            fixture_temp,
            archive_temp,
            fixture_directory,
            archive,
            manifest,
        )
    }

    fn count_rows(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT count(*) FROM usage_record", [], |row| row.get(0))
            .expect("count usage records")
    }

    fn count_coverage_rows(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT count(*) FROM coverage_interval", [], |row| {
                row.get(0)
            })
            .expect("count coverage intervals")
    }

    fn bucket(start: i64, end: i64) -> TimeBucket {
        TimeBucket {
            start_utc_ms: start,
            end_utc_ms: end,
            label: format!("{start}..{end}"),
        }
    }

    fn utc_date(epoch_ms: i64) -> NaiveDate {
        DateTime::<Utc>::from_timestamp_millis(epoch_ms)
            .expect("fixture timestamp is valid")
            .date_naive()
    }

    fn empty_source_database(path: &Path) {
        let connection = Connection::open(path).expect("create empty source database");
        connection
            .execute_batch(
                "CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                );",
            )
            .expect("create empty message table");
    }

    // Gated exactly like its only caller, the Linux-only malformed-input fixture.
    #[cfg(target_os = "linux")]
    fn assistant_json(id: &str, session_id: &str, created: i64, agent: Option<&str>) -> Vec<u8> {
        let mut value = json!({
            "id": id,
            "sessionID": session_id,
            "role": "assistant",
            "time": {"created": created, "completed": created + 10},
            "modelID": "legacy-model",
            "providerID": "legacy-provider",
            "path": {"cwd": "/fixture/adversarial"},
            "cost": 0,
            "tokens": {
                "input": 11,
                "output": 7,
                "reasoning": 3,
                "cache": {"read": 2, "write": 1}
            }
        });
        if let Some(agent) = agent {
            value["agent"] = json!(agent);
        }
        serde_json::to_vec(&value).expect("serialize assistant JSON")
    }

    #[test]
    fn legacy_overlap_keeps_live_values_once_and_does_not_backflow_cursor() {
        let (_fixture_temp, _archive_temp, _fixture_directory, archive, manifest) =
            fixture_archive();
        let overlap = &manifest.legacy_overlap;
        let stored: (i64, String, String, String) = archive
            .connection()
            .query_row(
                "SELECT tok_input, origin, provider_id, model_id
                 FROM usage_record
                 WHERE host_id = ?1 AND source = ?2 AND message_id = ?3",
                (TEST_HOST, OPENCODE_SOURCE, overlap.message_id.as_str()),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("query overlap winner");
        let expected_unique = manifest
            .combined_unique_message_count
            .saturating_sub(manifest.skipped_count);

        assert_eq!(count_rows(archive.connection()) as u64, expected_unique);
        assert_eq!(
            stored,
            (
                overlap.database.tokens.input as i64,
                "live".to_string(),
                overlap.database.provider_id.clone(),
                overlap.database.model_id.clone(),
            )
        );
        assert_eq!(
            read_cursor(archive.connection(), TEST_HOST).expect("read live cursor"),
            manifest
                .special_rows
                .values()
                .map(|row| row.time_updated)
                .chain(std::iter::once(manifest.same_timestamp_bucket.time_updated))
                .max()
        );
    }

    #[test]
    fn legacy_manifest_coverage_gap_is_exactly_declared_half_open_interval() {
        let (_fixture_temp, _archive_temp, _fixture_directory, archive, manifest) =
            fixture_archive();
        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");
        let filters = AggregateFilters {
            host_id: Some(TEST_HOST.to_string()),
            source: Some(OPENCODE_SOURCE.to_string()),
            ..AggregateFilters::default()
        };
        let gap = manifest.coverage.expected_gap;

        assert_eq!(
            coverage.status(&bucket(gap.start, gap.end), &filters),
            CoverageStatus::None
        );
        assert_eq!(
            coverage.status(&bucket(gap.start - 1, gap.start), &filters),
            CoverageStatus::Full
        );
        assert_eq!(
            coverage.status(&bucket(gap.end, gap.end + 1), &filters),
            CoverageStatus::Full
        );
    }

    #[test]
    fn legacy_covered_zero_usage_window_is_full_and_query_returns_zero() {
        let (_fixture_temp, _archive_temp, _fixture_directory, archive, manifest) =
            fixture_archive();
        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");
        let zero = manifest.coverage.covered_zero_usage;
        let range = LocalDateRange::new(
            utc_date(zero.start),
            utc_date(zero.end),
            chrono_tz::UTC,
            WeekStart::Monday,
        )
        .expect("create zero-usage range");
        let filters = AggregateFilters {
            host_id: Some(TEST_HOST.to_string()),
            source: Some(OPENCODE_SOURCE.to_string()),
            ..AggregateFilters::default()
        };
        let series = query_series(
            &archive,
            &range,
            Granularity::Day,
            &filters,
            &PriceTable::new(),
            &coverage,
        )
        .expect("query covered zero-usage series");

        assert_eq!(series.len(), 7);
        assert!(series.iter().all(|item| {
            item.coverage == CoverageStatus::Full
                && item.message_count == Some(0)
                && item.tokens == Some(Default::default())
        }));
    }

    #[test]
    fn legacy_live_trailing_idle_before_cutoff_is_full() {
        let (_fixture_temp, _archive_temp, _fixture_directory, archive, manifest) =
            fixture_archive();
        let latest: i64 = archive
            .connection()
            .query_row(
                "SELECT max(time_created_utc) FROM usage_record WHERE origin = 'live'",
                [],
                |row| row.get(0),
            )
            .expect("query latest live record");
        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");
        let filters = AggregateFilters {
            host_id: Some(TEST_HOST.to_string()),
            source: Some(OPENCODE_SOURCE.to_string()),
            ..AggregateFilters::default()
        };

        assert!(latest < manifest.coverage.live_cutoff);
        assert_eq!(
            coverage.status(
                &bucket(latest.saturating_add(1), manifest.coverage.live_cutoff),
                &filters,
            ),
            CoverageStatus::Full
        );
    }

    #[test]
    fn legacy_two_selected_hosts_with_only_one_covering_bucket_is_partial() {
        let temp = tempfile::tempdir().expect("coverage tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        replace_origin_coverage(
            archive.connection_mut(),
            "host-covered",
            OPENCODE_SOURCE,
            Origin::Live,
            Some(CoverageInterval::new(0, 100).expect("valid interval")),
        )
        .expect("write covered host");
        replace_origin_coverage(
            archive.connection_mut(),
            "host-gap",
            OPENCODE_SOURCE,
            Origin::Live,
            Some(CoverageInterval::new(200, 300).expect("valid interval")),
        )
        .expect("write gap host");
        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");

        assert_eq!(
            coverage.status(&bucket(0, 100), &AggregateFilters::default()),
            CoverageStatus::Partial
        );
    }

    #[test]
    fn legacy_single_host_covering_only_half_bucket_is_partial() {
        let temp = tempfile::tempdir().expect("coverage tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        replace_origin_coverage(
            archive.connection_mut(),
            TEST_HOST,
            OPENCODE_SOURCE,
            Origin::Legacy,
            Some(CoverageInterval::new(0, 50).expect("valid interval")),
        )
        .expect("write half interval");
        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");

        assert_eq!(
            coverage.status(&bucket(0, 100), &AggregateFilters::default()),
            CoverageStatus::Partial
        );
    }

    #[test]
    fn legacy_empty_live_scan_starts_zero_length_then_extends_end() {
        let source_temp = tempfile::tempdir().expect("source tempdir");
        let database = source_temp.path().join("opencode.db");
        empty_source_database(&database);
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let first_cutoff = 1_800_000_000_000_i64;
        let first = ingest_live_database(&mut archive, &database, TEST_HOST, None, first_cutoff)
            .expect("run first empty scan");
        assert_eq!(first.scan.eligible_count, 0);
        assert_eq!(
            first.interval,
            CoverageInterval::new(first_cutoff, first_cutoff).ok()
        );

        let stored: (i64, i64) = archive
            .connection()
            .query_row(
                "SELECT interval_start, interval_end FROM coverage_interval
                 WHERE host_id = ?1 AND source = ?2 AND origin = 'live'",
                (TEST_HOST, OPENCODE_SOURCE),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read first zero-length interval");
        assert_eq!(stored, (first_cutoff, first_cutoff));

        let second_cutoff = first_cutoff + 60_000;
        ingest_live_database(&mut archive, &database, TEST_HOST, None, second_cutoff)
            .expect("run second empty scan");
        let stored: (i64, i64) = archive
            .connection()
            .query_row(
                "SELECT interval_start, interval_end FROM coverage_interval
                 WHERE host_id = ?1 AND source = ?2 AND origin = 'live'",
                (TEST_HOST, OPENCODE_SOURCE),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read extended interval");
        assert_eq!(stored, (first_cutoff, second_cutoff));
        assert_eq!(count_rows(archive.connection()), 0);
    }

    #[test]
    fn legacy_backup_import_disabled_never_opens_candidate_path() {
        let temp = tempfile::tempdir().expect("backup-off tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let poison = temp.path().join("missing-and-must-not-be-opened.db");
        let stats = import_backup_databases(
            &mut archive,
            TEST_HOST,
            std::slice::from_ref(&poison),
            false,
        )
        .expect("disabled backup import is a no-op");

        assert!(!poison.exists());
        assert!(!stats.enabled);
        assert_eq!(stats.databases_requested, 1);
        assert_eq!(stats.databases_opened, 0);
        assert_eq!(count_rows(archive.connection()), 0);
    }

    #[test]
    fn legacy_backup_import_reuses_parser_and_bak_beats_legacy_without_cursor() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        backfill_legacy(&mut archive, &fixture_directory, TEST_HOST).expect("legacy backfill");
        let stats = import_backup_databases(
            &mut archive,
            TEST_HOST,
            &[fixture_directory.join("opencode.db")],
            true,
        )
        .expect("backup database import");
        let stored: (String, i64) = archive
            .connection()
            .query_row(
                "SELECT origin, tok_input FROM usage_record WHERE message_id = ?1",
                [&manifest.legacy_overlap.message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query backup overlap winner");

        assert!(stats.enabled);
        assert_eq!(stats.databases_opened, 1);
        assert_eq!(stored.0, "bak");
        assert_eq!(
            stored.1,
            manifest.legacy_overlap.database.tokens.input as i64
        );
        assert_eq!(read_cursor(archive.connection(), TEST_HOST).unwrap(), None);
    }

    // Linux-only, NOT `cfg(unix)`: the fixture writes a filename containing a raw 0xff
    // byte, which only a filesystem that accepts arbitrary byte sequences allows. APFS
    // (macOS) enforces UTF-8 and rejects it with `Os { code: 92, "Illegal byte sequence" }`,
    // so the test failed for a harness reason, not a product reason. The scanner's
    // non-UTF-8-name handling is Linux-specific behaviour and is asserted here only.
    #[cfg(target_os = "linux")]
    #[test]
    fn legacy_malformed_inputs_are_lossy_warned_and_survivors_continue() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let source_temp = tempfile::tempdir().expect("legacy tree tempdir");
        let session = source_temp.path().join("storage/message/ses_adversarial");
        fs::create_dir_all(&session).expect("create adversarial session");
        fs::write(
            session.join("msg_missing_agent.json"),
            assistant_json("msg_missing_agent", "ses_adversarial", 1_000, None),
        )
        .expect("write missing-agent row");
        fs::write(
            session.join("msg_truncated.json"),
            b"{\"role\":\"assistant\"",
        )
        .expect("write truncated row");
        fs::write(
            session.join("msg_wrong_tokens.json"),
            br#"{"role":"assistant","time":{"created":1500},"tokens":"wrong"}"#,
        )
        .expect("write wrong-token row");
        fs::create_dir(session.join("msg_directory.json")).expect("create directory-as-file");
        let non_utf8_name = OsString::from_vec(vec![
            b'm', b's', b'g', b'_', 0xff, b'.', b'j', b's', b'o', b'n',
        ]);
        fs::write(
            session.join(non_utf8_name),
            assistant_json("msg_non_utf8", "ses_adversarial", 2_000, Some("librarian")),
        )
        .expect("write non-UTF8-path row");

        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let stats = backfill_legacy(&mut archive, source_temp.path(), TEST_HOST)
            .expect("lossy adversarial backfill");
        let missing_agent: (String, String) = archive
            .connection()
            .query_row(
                "SELECT agent_raw, agent_key FROM usage_record WHERE message_id = 'msg_missing_agent'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query missing-agent fallback");

        assert_eq!(count_rows(archive.connection()), 2);
        assert_eq!(
            missing_agent,
            ("unknown".to_string(), "unknown".to_string())
        );
        assert_eq!(stats.warnings.malformed_json, 1);
        assert_eq!(stats.warnings.invalid_tokens, 1);
        assert_eq!(stats.warnings.invalid_paths, 1);
        assert_eq!(stats.warnings.non_utf8_paths, 1);
        assert_eq!(stats.warning_count, 4);
        println!(
            "corrupt_json warning_count={} malformed={} invalid_tokens={} invalid_paths={} non_utf8={} surviving_rows={}",
            stats.warning_count,
            stats.warnings.malformed_json,
            stats.warnings.invalid_tokens,
            stats.warnings.invalid_paths,
            stats.warnings.non_utf8_paths,
            count_rows(archive.connection())
        );
    }

    #[test]
    fn legacy_repeated_backfill_is_idempotent_and_replaces_origin_interval() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        for _ in 0..2 {
            let stats = backfill_legacy(&mut archive, &fixture_directory, TEST_HOST)
                .expect("repeat legacy backfill");
            assert!(stats.ingest.committed);
        }

        assert_eq!(
            count_rows(archive.connection()) as u64,
            manifest.legacy_message_rows
        );
        assert_eq!(count_coverage_rows(archive.connection()), 1);
        let duplicate_groups: i64 = archive
            .connection()
            .query_row(
                "SELECT count(*) FROM (
                    SELECT host_id, source, message_id
                    FROM usage_record GROUP BY 1,2,3 HAVING count(*) > 1
                 )",
                [],
                |row| row.get(0),
            )
            .expect("count duplicate groups");
        assert_eq!(duplicate_groups, 0);
    }

    #[test]
    fn legacy_interrupted_round_rolls_back_and_rerun_completes_consistently() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let interrupted = backfill_legacy_with_control(
            &mut archive,
            &fixture_directory,
            TEST_HOST,
            &LegacyBackfillOptions::default(),
            |_, attempted| attempted < 2,
        )
        .expect("interrupt legacy backfill");
        assert!(interrupted.interrupted);
        assert!(!interrupted.ingest.committed);
        assert_eq!(count_rows(archive.connection()), 0);
        assert_eq!(count_coverage_rows(archive.connection()), 0);

        let completed = backfill_legacy(&mut archive, &fixture_directory, TEST_HOST)
            .expect("rerun complete legacy backfill");
        assert!(completed.ingest.committed);
        assert_eq!(
            count_rows(archive.connection()) as u64,
            manifest.legacy_message_rows
        );
        assert_eq!(count_coverage_rows(archive.connection()), 1);
    }

    #[test]
    fn legacy_coverage_store_unions_origins_filters_pairs_and_rejects_corrupt_intervals() {
        let temp = tempfile::tempdir().expect("coverage tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        for (origin, start, end) in [
            (Origin::Live, 0, 50),
            (Origin::Bak, 40, 75),
            (Origin::Legacy, 75, 100),
        ] {
            replace_origin_coverage(
                archive.connection_mut(),
                TEST_HOST,
                OPENCODE_SOURCE,
                origin,
                Some(CoverageInterval::new(start, end).expect("valid interval")),
            )
            .expect("store origin interval");
        }
        archive
            .connection()
            .execute(
                "INSERT INTO coverage_interval (
                    host_id, source, origin, interval_start, interval_end
                 ) VALUES (?1, ?2, 'bak', 150, 200)",
                (TEST_HOST, OPENCODE_SOURCE),
            )
            .expect("seed a disjoint interval for the same pair");
        let store = CoverageStore::load(archive.connection()).expect("load unioned coverage");
        assert_eq!(
            store.intervals_for(TEST_HOST, OPENCODE_SOURCE),
            &[
                CoverageInterval { start: 0, end: 100 },
                CoverageInterval {
                    start: 150,
                    end: 200,
                },
            ]
        );
        assert!(store
            .intervals_for("missing-host", OPENCODE_SOURCE)
            .is_empty());
        assert_eq!(
            store.status(&bucket(0, 0), &AggregateFilters::default()),
            CoverageStatus::None
        );
        assert_eq!(
            store.status(
                &bucket(0, 100),
                &AggregateFilters {
                    host_id: Some("missing-host".to_string()),
                    ..AggregateFilters::default()
                },
            ),
            CoverageStatus::None
        );
        assert!(matches!(
            CoverageInterval::new(2, 1),
            Err(LegacyError::InvalidInterval { start: 2, end: 1 })
        ));

        let corrupt = Connection::open_in_memory().expect("open corrupt interval fixture");
        corrupt
            .execute_batch(
                "CREATE TABLE coverage_interval (
                    host_id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    interval_start INTEGER NOT NULL,
                    interval_end INTEGER NOT NULL
                 );
                 INSERT INTO coverage_interval VALUES ('host', 'opencode', 9, 3);",
            )
            .expect("seed reversed interval");
        assert!(matches!(
            CoverageStore::load(&corrupt),
            Err(LegacyError::InvalidInterval { start: 9, end: 3 })
        ));
    }

    #[test]
    fn legacy_origin_removal_and_live_cutoff_validation_leave_no_stale_coverage() {
        let temp = tempfile::tempdir().expect("coverage tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        replace_origin_coverage(
            archive.connection_mut(),
            TEST_HOST,
            OPENCODE_SOURCE,
            Origin::Legacy,
            Some(CoverageInterval::new(10, 20).expect("valid interval")),
        )
        .expect("store legacy coverage");
        replace_origin_coverage(
            archive.connection_mut(),
            TEST_HOST,
            OPENCODE_SOURCE,
            Origin::Legacy,
            None,
        )
        .expect("remove empty legacy coverage");
        assert_eq!(count_coverage_rows(archive.connection()), 0);

        let error = extend_live_coverage(
            archive.connection_mut(),
            TEST_HOST,
            OPENCODE_SOURCE,
            Some(101),
            100,
        )
        .expect_err("cutoff cannot precede an observed record");
        assert!(matches!(
            error,
            LegacyError::CutoffBeforeRecord {
                earliest: 101,
                cutoff: 100
            }
        ));
        assert_eq!(count_coverage_rows(archive.connection()), 0);
    }

    #[test]
    fn legacy_bounded_backfill_uses_sorted_cap_and_one_record_batches() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let options = LegacyBackfillOptions {
            batch_size: 1,
            max_files: Some(1),
        };
        let stats =
            backfill_legacy_with_options(&mut archive, &fixture_directory, TEST_HOST, &options)
                .expect("run bounded backfill");

        assert!(stats.limited);
        assert_eq!(stats.files_seen, manifest.legacy_message_rows);
        assert_eq!(stats.files_attempted, 1);
        assert_eq!(stats.eligible_records, 1);
        assert_eq!(stats.ingest.received_records, 1);
        assert!(stats.ingest.committed);
        assert!(stats.interval.is_some());
        assert_eq!(count_rows(archive.connection()), 1);

        let invalid = LegacyBackfillOptions {
            batch_size: 0,
            max_files: None,
        };
        assert!(matches!(
            backfill_legacy_with_options(&mut archive, &fixture_directory, TEST_HOST, &invalid,),
            Err(LegacyError::InvalidBatchSize)
        ));
    }

    #[test]
    fn legacy_path_parser_falls_back_to_path_identity_and_coerces_timestamp_variants() {
        let temp = tempfile::tempdir().expect("legacy parser tempdir");
        let session = temp.path().join("session-from-path");
        std::fs::create_dir(&session).expect("create session directory");
        let path = session.join("message-from-path.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "id": "   ",
                "sessionID": "",
                "role": "assistant",
                "agent": "Legacy Agent",
                "time": {"created": "1000", "updated": 1005.9},
                "providerID": "legacy-provider",
                "modelID": "legacy-model",
                "variant": "must-be-cleared",
                "cost": 0.5,
                "tokens": {
                    "input": 11,
                    "output": 7,
                    "reasoning": 3,
                    "cache": {"read": 2, "write": 1}
                }
            }))
            .expect("serialize legacy row"),
        )
        .expect("write legacy row");
        let mut warnings = LegacyWarningCounts::default();
        let record = parse_legacy_path(
            &path,
            &ParseContext::new(TEST_HOST, Origin::Legacy),
            &mut warnings,
        )
        .expect("path fallbacks keep a valid assistant row");
        assert_eq!(record.message_id, "message-from-path");
        assert_eq!(record.session_id, "session-from-path");
        assert_eq!(record.time_created_utc, 1_000);
        assert_eq!(record.source_time_updated, 1_005);
        assert_eq!(record.variant, None);
        assert_eq!(record.cost, Some(0.5));
        assert_eq!(warnings.total(), 0);

        let invalid = session.join("invalid-time.json");
        std::fs::write(&invalid, br#"{"role":"assistant","time":{},"tokens":{}}"#)
            .expect("write invalid-time row");
        assert!(parse_legacy_path(
            &invalid,
            &ParseContext::new(TEST_HOST, Origin::Legacy),
            &mut warnings,
        )
        .is_none());
        assert_eq!(warnings.invalid_records, 1);
    }

    #[test]
    fn legacy_live_import_maps_archive_sink_failure_and_rolls_back_future_cutoff_rows() {
        let (_fixture_temp, fixture_directory, _manifest) = fixture_directory();
        let database = fixture_directory.join("opencode.db");

        let sink_temp = tempfile::tempdir().expect("sink archive tempdir");
        let mut sink_archive =
            Archive::open_in_data_dir(sink_temp.path()).expect("open sink archive");
        sink_archive
            .connection()
            .execute("DROP TABLE usage_record", [])
            .expect("remove sink table");
        let sink_error =
            ingest_live_database(&mut sink_archive, &database, TEST_HOST, None, i64::MAX)
                .expect_err("archive sink failure must be surfaced");
        assert!(matches!(sink_error, LegacyError::Sink(_)));

        let cutoff_temp = tempfile::tempdir().expect("cutoff archive tempdir");
        let mut cutoff_archive =
            Archive::open_in_data_dir(cutoff_temp.path()).expect("open cutoff archive");
        let cutoff_error = ingest_live_database(&mut cutoff_archive, &database, TEST_HOST, None, 0)
            .expect_err("records after the snapshot cutoff must reject the round");
        assert!(matches!(
            cutoff_error,
            LegacyError::CutoffBeforeRecord { cutoff: 0, .. }
        ));
        assert_eq!(count_rows(cutoff_archive.connection()), 0);
        assert_eq!(count_coverage_rows(cutoff_archive.connection()), 0);
        assert_eq!(
            read_cursor(cutoff_archive.connection(), TEST_HOST).expect("read rolled-back cursor"),
            None
        );
    }

    #[test]
    #[ignore = "manual QA invokes external sqlite3 and prints manifest coverage states"]
    fn legacy_manual_qa_fixture_external_sqlite3_and_tri_state_dump() {
        let (fixture_temp, archive_temp, _fixture_directory, archive, manifest) = fixture_archive();
        let archive_path = archive.path().to_path_buf();
        let counts = external_sqlite(
            &archive_path,
            "SELECT origin, count(*) FROM usage_record GROUP BY origin ORDER BY origin;",
        );
        let intervals = external_sqlite(
            &archive_path,
            "SELECT * FROM coverage_interval ORDER BY host_id, source, origin;",
        );
        let duplicates = external_sqlite(
            &archive_path,
            "SELECT host_id, source, message_id, count(*) c FROM usage_record GROUP BY 1,2,3 HAVING c>1;",
        );
        assert!(duplicates.is_empty());
        println!("external_origin_counts=\n{counts}");
        println!("external_coverage_intervals=\n{intervals}");
        println!("external_duplicate_check={duplicates:?}");

        let coverage = CoverageStore::load(archive.connection()).expect("load coverage snapshot");
        let filters = AggregateFilters {
            host_id: Some(TEST_HOST.to_string()),
            source: Some(OPENCODE_SOURCE.to_string()),
            ..AggregateFilters::default()
        };
        let gap = manifest.coverage.expected_gap;
        let zero = manifest.coverage.covered_zero_usage;
        println!(
            "tri_state gap=[{}, {}) status={:?}",
            gap.start,
            gap.end,
            coverage.status(&bucket(gap.start, gap.end), &filters)
        );
        println!(
            "tri_state covered_zero=[{}, {}) status={:?} value=0",
            zero.start,
            zero.end,
            coverage.status(&bucket(zero.start, zero.end), &filters)
        );
        drop(archive);
        let archive_directory = archive_temp.path().to_path_buf();
        archive_temp.close().expect("remove QA archive");
        fixture_temp.close().expect("remove QA fixture");
        assert!(!archive_directory.exists());
        println!("cleanup_receipt=removed {}", archive_directory.display());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "manual QA reads a bounded real legacy subset and checks the real backup stays unopened"]
    fn legacy_manual_qa_real_tree_bounded_and_backup_off() {
        let data_dir = PathBuf::from("/config/.local/share/opencode");
        let tree = data_dir.join("storage/message");
        let backup = data_dir.join("opencode.db.bak.20260408");
        let external_count_output = Command::new("find")
            .arg(&tree)
            .args(["-name", "*.json"])
            .output()
            .expect("run real-tree find");
        assert!(external_count_output.status.success());
        let external_file_count = String::from_utf8_lossy(&external_count_output.stdout)
            .lines()
            .count();

        let archive_temp = tempfile::tempdir().expect("real archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let bak_handles_before = process_handles_for(&backup);
        let bak_stats = import_backup_databases(
            &mut archive,
            TEST_HOST,
            std::slice::from_ref(&backup),
            false,
        )
        .expect("disabled real backup import");
        let bak_handles_after = process_handles_for(&backup);
        assert_eq!(bak_handles_before, bak_handles_after);
        assert_eq!(bak_stats.databases_opened, 0);

        let started = Instant::now();
        let stats = backfill_legacy_with_options(
            &mut archive,
            &data_dir,
            TEST_HOST,
            &LegacyBackfillOptions {
                batch_size: DEFAULT_LEGACY_BATCH_SIZE,
                max_files: Some(1_000),
            },
        )
        .expect("bounded real legacy backfill");
        let elapsed = started.elapsed();
        let archived = external_sqlite(archive.path(), "SELECT count(*) FROM usage_record;");
        println!(
            "real_tree external_file_count={} cap=1000 attempted={} eligible={} skipped={} warnings={} archived={} elapsed_ms={}",
            external_file_count,
            stats.files_attempted,
            stats.eligible_records,
            stats.skipped_records,
            stats.warning_count,
            archived,
            elapsed.as_millis()
        );
        println!(
            "real_bak_off path={} exists={} opened={} handles_before={:?} handles_after={:?}",
            backup.display(),
            backup.exists(),
            bak_stats.databases_opened,
            bak_handles_before,
            bak_handles_after
        );
    }

    fn external_sqlite(database: &Path, sql: &str) -> String {
        let output = Command::new("sqlite3")
            .arg(database)
            .arg(sql)
            .output()
            .expect("run external sqlite3");
        assert!(
            output.status.success(),
            "external sqlite3 failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("external sqlite3 output is UTF-8")
            .trim()
            .to_string()
    }

    #[cfg(target_os = "linux")]
    fn process_handles_for(path: &Path) -> Vec<PathBuf> {
        let mut handles = fs::read_dir("/proc/self/fd")
            .expect("read process descriptors")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .filter(|target| target == path)
            .collect::<Vec<_>>();
        handles.sort();
        handles
    }
}
