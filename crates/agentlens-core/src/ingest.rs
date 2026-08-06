//! 归档 upsert、去重与 watermark 推进（todo 6）。
//!
//! 本模块将实现把归一化记录写入 `usage_record` 的 origin 优先级条件 upsert：
//! live / bak / legacy 三种来源共用同一个 `source='opencode'`，来源层级另存
//! `origin` + `origin_priority`（live=3、bak=2、legacy=1），使
//! `(host_id, source, message_id)` 唯一键能够跨来源冲突；冲突时仅当
//! `excluded.origin_priority` 更高，或优先级相同且 `source_time_updated` 不更旧时才覆盖。
//!
//! 同时负责 `source_cursor`（主键 `(host_id, source)`）的 watermark 语义：
//! 仅在整轮扫描完整到达 EOF 后原子写入本轮 `max(time_updated)`；
//! bak / legacy 回填不推进 live watermark；崩溃或中断后重跑保持幂等。

use rusqlite::{params, Connection, OptionalExtension as _, Transaction};
use thiserror::Error;

use crate::archive::{NormalizedUsageRecord, Origin};
use crate::source::opencode::ScanResult;

/// Canonical source key shared by live, backup, and legacy OpenCode records.
pub const OPENCODE_SOURCE: &str = "opencode";

/// Conditional upsert used for every normalized OpenCode record.
///
/// The conflict predicate is intentionally literal: higher-priority origins win, while equal
/// priorities update only when the incoming source timestamp is at least as recent.
pub const USAGE_UPSERT_SQL: &str = "INSERT INTO usage_record (
    host_id, source, message_id, session_id,
    time_created_utc, time_completed_utc, source_time_updated,
    origin, origin_priority, agent_raw, agent_key,
    provider_id, model_id, variant,
    tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
    cost, cost_source, is_incomplete, project_dir
) VALUES (
    ?1, 'opencode', ?2, ?3,
    ?4, ?5, ?6,
    ?7, ?8, ?9, ?10,
    ?11, ?12, ?13,
    ?14, ?15, ?16, ?17, ?18,
    ?19, ?20, ?21, ?22
)
ON CONFLICT(host_id, source, message_id) DO UPDATE SET
    session_id = excluded.session_id,
    time_created_utc = excluded.time_created_utc,
    time_completed_utc = excluded.time_completed_utc,
    source_time_updated = excluded.source_time_updated,
    origin = excluded.origin,
    origin_priority = excluded.origin_priority,
    agent_raw = excluded.agent_raw,
    agent_key = excluded.agent_key,
    provider_id = excluded.provider_id,
    model_id = excluded.model_id,
    variant = excluded.variant,
    tok_input = excluded.tok_input,
    tok_output = excluded.tok_output,
    tok_reasoning = excluded.tok_reasoning,
    tok_cache_read = excluded.tok_cache_read,
    tok_cache_write = excluded.tok_cache_write,
    cost = excluded.cost,
    cost_source = excluded.cost_source,
    is_incomplete = excluded.is_incomplete,
    project_dir = excluded.project_dir
WHERE excluded.origin_priority > usage_record.origin_priority
   OR (excluded.origin_priority = usage_record.origin_priority AND excluded.source_time_updated >= usage_record.source_time_updated)";

const CURSOR_UPSERT_SQL: &str = "INSERT INTO source_cursor (
    host_id, source, cursor_time_updated
) VALUES (?1, 'opencode', ?2)
ON CONFLICT(host_id, source) DO UPDATE SET
    cursor_time_updated = excluded.cursor_time_updated";

/// Result type returned by archive ingest operations.
pub type Result<T> = std::result::Result<T, IngestError>;

/// Errors that reject malformed records or prevent an atomic ingest transaction from completing.
#[derive(Debug, Error)]
pub enum IngestError {
    /// The round host is blank and cannot form a stable deduplication key.
    #[error("archive ingest host_id is empty")]
    EmptyHostId,
    /// Empty message identifiers would collapse unrelated malformed rows onto one unique key.
    #[error("archive ingest message_id is empty for host {host_id}")]
    EmptyMessageId {
        /// Host carrying the malformed record.
        host_id: String,
    },
    /// A batch attempted to mix records from another host into this round.
    #[error(
        "archive ingest record host_id {record_host:?} does not match round host {round_host:?}"
    )]
    HostMismatch {
        /// Host fixed when the round began.
        round_host: String,
        /// Host encoded in the rejected record.
        record_host: String,
    },
    /// All OpenCode origins must share the same source key so the unique constraint can deduplicate.
    #[error("archive ingest source {found:?} is invalid; expected constant \"opencode\"")]
    InvalidSource {
        /// Rejected source value.
        found: String,
    },
    /// A batch attempted to mix provenance tiers inside a round.
    #[error(
        "archive ingest record origin {record_origin} does not match round origin {round_origin}"
    )]
    OriginMismatch {
        /// Origin fixed when the round began.
        round_origin: &'static str,
        /// Origin encoded in the rejected record.
        record_origin: &'static str,
    },
    /// The explicit wire priority disagrees with the canonical priority of its origin.
    #[error(
        "archive ingest origin_priority {found} does not match origin {origin} priority {expected}"
    )]
    OriginPriorityMismatch {
        /// Encoded origin.
        origin: &'static str,
        /// Canonical priority returned by `Origin::priority()`.
        expected: i32,
        /// Rejected explicit priority.
        found: i32,
    },
    /// SQLite INTEGER is signed and cannot represent this token count without corruption.
    #[error("archive ingest {field} exceeds SQLite INTEGER range: {value}")]
    TokenOutOfRange {
        /// Token field whose conversion failed.
        field: &'static str,
        /// Rejected unsigned value.
        value: u64,
    },
    /// SQLite must never receive NaN or infinity as a usage cost.
    #[error("archive ingest cost must be finite when present, got {value}")]
    NonFiniteCost {
        /// Rejected floating-point value.
        value: f64,
    },
    /// SQLite failed while applying, rolling back, or committing the round.
    #[error("archive ingest database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Observable counters from one scan-round transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IngestStats {
    /// Records accepted from scanner batches, including idempotent replays.
    pub received_records: u64,
    /// Statements that inserted or updated a row before commit; zero-result stale conflicts exclude
    /// themselves. When `committed` is false these changes were rolled back.
    pub changed_records: u64,
    /// True only when the transaction reached the commit point after EOF.
    pub committed: bool,
    /// Cursor written in the same transaction, or `None` for interruption, empty scans, and
    /// backup/legacy backfills.
    pub cursor_time_updated: Option<i64>,
}

/// One atomic archive transaction spanning every batch delivered by a scanner round.
///
/// Create the round before calling `scan_database` / `scan_connection`, pass each sink batch to
/// [`Self::ingest_batch`], then pass the returned [`ScanResult`] to [`Self::finish`]. Dropping this
/// value at any earlier point rolls back the whole round. This chooses all-or-nothing semantics for
/// deterministic interruption handling: no applied prefix survives, and the cursor never advances.
pub struct IngestRound<'connection> {
    transaction: Option<Transaction<'connection>>,
    host_id: String,
    origin: Origin,
    stats: IngestStats,
}

impl<'connection> IngestRound<'connection> {
    /// Begins one all-or-nothing OpenCode scan round on an archive connection.
    pub fn begin(
        connection: &'connection mut Connection,
        host_id: impl Into<String>,
        origin: Origin,
    ) -> Result<Self> {
        let host_id = host_id.into();
        if host_id.trim().is_empty() {
            return Err(IngestError::EmptyHostId);
        }
        let transaction = connection.transaction()?;
        Ok(Self {
            transaction: Some(transaction),
            host_id,
            origin,
            stats: IngestStats::default(),
        })
    }

    /// Validates and conditionally upserts one scanner-delivered batch inside the round transaction.
    pub fn ingest_batch(&mut self, records: &[NormalizedUsageRecord]) -> Result<()> {
        let transaction = self
            .transaction
            .as_ref()
            .expect("an unfinished ingest round always owns its transaction");
        let mut statement = transaction.prepare_cached(USAGE_UPSERT_SQL)?;

        for record in records {
            let values = ValidatedRecord::new(record, &self.host_id, self.origin)?;
            let changed = statement.execute(params![
                record.host_id,
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
                values.tok_input,
                values.tok_output,
                values.tok_reasoning,
                values.tok_cache_read,
                values.tok_cache_write,
                record.cost,
                record.cost_source.as_str(),
                record.is_incomplete,
                record.project_dir,
            ])?;
            self.stats.received_records += 1;
            self.stats.changed_records += changed as u64;
        }
        Ok(())
    }

    /// Commits only an EOF-complete scan and writes a live watermark in the same transaction.
    ///
    /// An interrupted result (`reached_eof == false`) explicitly rolls back all delivered batches.
    /// Backup and legacy rounds commit their records after EOF but never write `source_cursor`.
    pub fn finish(mut self, scan_result: &ScanResult) -> Result<IngestStats> {
        let transaction = self
            .transaction
            .take()
            .expect("an unfinished ingest round always owns its transaction");

        if !scan_result.reached_eof {
            transaction.rollback()?;
            return Ok(self.stats);
        }

        if self.origin == Origin::Live {
            if let Some(observed_max) = scan_result.observed_max_time_updated {
                transaction.execute(CURSOR_UPSERT_SQL, params![self.host_id, observed_max])?;
                self.stats.cursor_time_updated = Some(observed_max);
            }
        }
        transaction.commit()?;
        self.stats.committed = true;
        Ok(self.stats)
    }
}

/// Reads the committed OpenCode watermark for one host.
pub fn read_cursor(connection: &Connection, host_id: &str) -> Result<Option<i64>> {
    read_source_cursor(connection, host_id, OPENCODE_SOURCE)
}

/// Reads a committed watermark by its full `(host_id, source)` primary key.
pub fn read_source_cursor(
    connection: &Connection,
    host_id: &str,
    source: &str,
) -> Result<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT cursor_time_updated FROM source_cursor WHERE host_id = ?1 AND source = ?2",
            params![host_id, source],
            |row| row.get(0),
        )
        .optional()?)
}

struct ValidatedRecord {
    tok_input: i64,
    tok_output: i64,
    tok_reasoning: i64,
    tok_cache_read: i64,
    tok_cache_write: i64,
}

impl ValidatedRecord {
    fn new(record: &NormalizedUsageRecord, round_host: &str, round_origin: Origin) -> Result<Self> {
        if record.message_id.trim().is_empty() {
            return Err(IngestError::EmptyMessageId {
                host_id: record.host_id.clone(),
            });
        }
        if record.host_id != round_host {
            return Err(IngestError::HostMismatch {
                round_host: round_host.to_owned(),
                record_host: record.host_id.clone(),
            });
        }
        if record.source != OPENCODE_SOURCE {
            return Err(IngestError::InvalidSource {
                found: record.source.clone(),
            });
        }
        if record.origin != round_origin {
            return Err(IngestError::OriginMismatch {
                round_origin: round_origin.as_str(),
                record_origin: record.origin.as_str(),
            });
        }
        let expected_priority = record.origin.priority();
        if record.origin_priority != expected_priority {
            return Err(IngestError::OriginPriorityMismatch {
                origin: record.origin.as_str(),
                expected: expected_priority,
                found: record.origin_priority,
            });
        }
        if record.cost.is_some_and(|cost| !cost.is_finite()) {
            return Err(IngestError::NonFiniteCost {
                value: record.cost.expect("checked Some cost"),
            });
        }

        Ok(Self {
            tok_input: sqlite_integer("tok_input", record.tok_input)?,
            tok_output: sqlite_integer("tok_output", record.tok_output)?,
            tok_reasoning: sqlite_integer("tok_reasoning", record.tok_reasoning)?,
            tok_cache_read: sqlite_integer("tok_cache_read", record.tok_cache_read)?,
            tok_cache_write: sqlite_integer("tok_cache_write", record.tok_cache_write)?,
        })
    }
}

fn sqlite_integer(field: &'static str, value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| IngestError::TokenOutOfRange { field, value })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use rusqlite::Connection;
    #[cfg(unix)]
    use rusqlite::OpenFlags;
    use serde_json::json;

    use crate::archive::{Archive, CostSource, NormalizedUsageRecord, Origin};
    use crate::fixture::{generate, FixtureGuard, Manifest};
    use crate::source::opencode::{
        scan_database, ScanRequest, ScanResult, SinkError, SkippedBreakdown, DEFAULT_BATCH_SIZE,
        OVERLAP_WINDOW_MS,
    };

    use super::*;

    const TEST_HOST: &str = "host-ingest-test";

    fn fixture_directory() -> (tempfile::TempDir, PathBuf, Manifest) {
        let temp = tempfile::tempdir().expect("create fixture parent");
        let directory = temp.path().join("fixture");
        let manifest = generate(&directory).expect("generate fixture");
        (temp, directory, manifest)
    }

    fn record(
        message_id: impl Into<String>,
        origin: Origin,
        source_time_updated: i64,
        tok_input: u64,
    ) -> NormalizedUsageRecord {
        NormalizedUsageRecord {
            host_id: TEST_HOST.to_string(),
            source: OPENCODE_SOURCE.to_string(),
            message_id: message_id.into(),
            session_id: "session-ingest-test".to_string(),
            time_created_utc: source_time_updated.saturating_sub(1_000),
            time_completed_utc: Some(source_time_updated.saturating_sub(100)),
            source_time_updated,
            origin,
            origin_priority: origin.priority(),
            agent_raw: format!("{origin:?} Agent"),
            agent_key: format!("{origin:?}-agent").to_lowercase(),
            provider_id: format!("{origin:?}-provider").to_lowercase(),
            model_id: format!("{origin:?}-model").to_lowercase(),
            variant: Some(format!("{origin:?}").to_lowercase()),
            tok_input,
            tok_output: tok_input.saturating_add(1),
            tok_reasoning: tok_input.saturating_add(2),
            tok_cache_read: tok_input.saturating_add(3),
            tok_cache_write: tok_input.saturating_add(4),
            cost: None,
            cost_source: CostSource::Unavailable,
            is_incomplete: false,
            project_dir: "/fixture/ingest".to_string(),
        }
    }

    fn scan_result(reached_eof: bool, observed_max_time_updated: Option<i64>) -> ScanResult {
        ScanResult {
            delivered_records: 0,
            delivered_batches: 0,
            eligible_count: 0,
            skipped_count: 0,
            skipped_breakdown: SkippedBreakdown::default(),
            observed_max_time_updated,
            reached_eof,
            busy_retry_count: 0,
            last_success_utc: None,
            skip_reason: None,
        }
    }

    fn ingest_direct_round(
        archive: &mut Archive,
        origin: Origin,
        batches: &[Vec<NormalizedUsageRecord>],
        result: &ScanResult,
    ) -> IngestStats {
        let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, origin)
            .expect("begin ingest round");
        for batch in batches {
            round.ingest_batch(batch).expect("ingest batch");
        }
        round.finish(result).expect("finish ingest round")
    }

    fn query_count(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT count(*) FROM usage_record", [], |row| row.get(0))
            .expect("count usage records")
    }

    fn query_tokens(connection: &Connection, message_id: &str) -> u64 {
        connection
            .query_row(
                "SELECT tok_input FROM usage_record WHERE host_id = ?1 AND source = ?2 AND message_id = ?3",
                (TEST_HOST, OPENCODE_SOURCE, message_id),
                |row| row.get::<_, i64>(0),
            )
            .expect("query token value") as u64
    }

    fn duplicate_group_count(connection: &Connection) -> i64 {
        connection
            .query_row(
                "SELECT count(*) FROM (
                    SELECT host_id, source, message_id
                    FROM usage_record
                    GROUP BY host_id, source, message_id
                    HAVING count(*) > 1
                )",
                [],
                |row| row.get(0),
            )
            .expect("count duplicate groups")
    }

    fn scan_fixture_into_archive(
        archive: &mut Archive,
        database: &Path,
        request: &ScanRequest,
    ) -> (ScanResult, IngestStats) {
        let mut round = IngestRound::begin(
            archive.connection_mut(),
            request.host_id.clone(),
            request.origin,
        )
        .expect("begin fixture scan round");
        let result = scan_database(database, request, |batch| {
            round
                .ingest_batch(batch)
                .map_err(|error| SinkError::new(error.to_string()))
        })
        .expect("scan fixture database");
        let stats = round.finish(&result).expect("finish fixture scan round");
        (result, stats)
    }

    #[test]
    fn ingest_update_after_cursor_is_captured() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let guard = FixtureGuard::new(&fixture_directory).expect("hold fixture WAL writer");
        let lagged = &manifest.lagged_update;
        guard
            .writer_connection()
            .execute("DELETE FROM message WHERE id != ?1", [&lagged.message_id])
            .expect("isolate lagged fixture row");
        guard
            .writer_connection()
            .execute(
                "UPDATE message SET time_updated = ?1,
                 data = json_remove(
                    json_set(data,
                        '$.tokens.input', 0,
                        '$.tokens.output', 0,
                        '$.tokens.reasoning', 0,
                        '$.tokens.cache.read', 0,
                        '$.tokens.cache.write', 0
                    ),
                    '$.time.completed'
                 )
                 WHERE id = ?2",
                rusqlite::params![lagged.pre_update_time_updated, lagged.message_id],
            )
            .expect("restore stale lagged fixture state");

        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let first_request = ScanRequest::live(TEST_HOST, None);
        let (first_result, first) =
            scan_fixture_into_archive(&mut archive, guard.db_path(), &first_request);
        assert_eq!(
            first.cursor_time_updated,
            Some(lagged.pre_update_time_updated)
        );
        assert_eq!(first_result.eligible_count, 1);
        assert_eq!(query_tokens(archive.connection(), &lagged.message_id), 0);

        guard
            .writer_connection()
            .execute(
                "UPDATE message SET time_updated = ?1,
                 data = json_set(data,
                    '$.tokens.input', ?2,
                    '$.tokens.output', ?3,
                    '$.tokens.reasoning', ?4,
                    '$.tokens.cache.read', ?5,
                    '$.tokens.cache.write', ?6,
                    '$.time.completed', ?7
                 )
                 WHERE id = ?8",
                rusqlite::params![
                    lagged.post_update_time_updated,
                    lagged.final_tokens.input as i64,
                    lagged.final_tokens.output as i64,
                    lagged.final_tokens.reasoning as i64,
                    lagged.final_tokens.cache_read as i64,
                    lagged.final_tokens.cache_write as i64,
                    lagged.post_update_time_updated - 1_000,
                    lagged.message_id,
                ],
            )
            .expect("commit final lagged fixture state");
        let second_request = ScanRequest::live(TEST_HOST, first.cursor_time_updated);
        let (second_result, second) =
            scan_fixture_into_archive(&mut archive, guard.db_path(), &second_request);
        assert_eq!(
            second.cursor_time_updated,
            Some(lagged.post_update_time_updated)
        );
        assert_eq!(second_result.eligible_count, 1);
        assert_eq!(
            query_tokens(archive.connection(), &lagged.message_id),
            1_234
        );
    }

    #[test]
    fn ingest_three_backfills_and_two_incremental_rounds_are_idempotent() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let legacy = vec![
            record("idempotent-one", Origin::Legacy, 100, 1),
            record("idempotent-two", Origin::Legacy, 101, 2),
        ];
        for _ in 0..3 {
            let stats = ingest_direct_round(
                &mut archive,
                Origin::Legacy,
                std::slice::from_ref(&legacy),
                &scan_result(true, Some(101)),
            );
            assert_eq!(stats.cursor_time_updated, None);
            assert_eq!(
                read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
                None
            );
        }

        let live = vec![
            record("idempotent-one", Origin::Live, 200, 11),
            record("idempotent-two", Origin::Live, 201, 22),
        ];
        for _ in 0..2 {
            ingest_direct_round(
                &mut archive,
                Origin::Live,
                std::slice::from_ref(&live),
                &scan_result(true, Some(201)),
            );
        }

        assert_eq!(query_count(archive.connection()), 2);
        assert_eq!(duplicate_group_count(archive.connection()), 0);
        assert_eq!(
            read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
            Some(201)
        );
    }

    #[test]
    fn ingest_fixture_scanner_lands_all_1001_same_timestamp_rows_and_manifest_specials() {
        let (_fixture_temp, fixture_directory, manifest) = fixture_directory();
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let request = ScanRequest::live(TEST_HOST, None);
        let (result, stats) = scan_fixture_into_archive(
            &mut archive,
            &fixture_directory.join("opencode.db"),
            &request,
        );

        assert!(result.reached_eof);
        assert!(stats.committed);
        assert_eq!(result.eligible_count, manifest.eligible_assistant_count);
        assert_eq!(
            query_count(archive.connection()) as u64,
            manifest.eligible_assistant_count
        );
        let bucket_count: i64 = archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE source_time_updated = ?1",
                [manifest.same_timestamp_bucket.time_updated],
                |row| row.get(0),
            )
            .expect("count same-timestamp bucket");
        assert_eq!(bucket_count as u64, manifest.same_timestamp_bucket.count);

        for (label, expected) in &manifest.special_rows {
            let count: i64 = archive
                .connection()
                .query_row(
                    "SELECT count(*) FROM usage_record WHERE host_id = ?1 AND source = ?2 AND message_id = ?3",
                    (TEST_HOST, OPENCODE_SOURCE, expected.message_id.as_str()),
                    |row| row.get(0),
                )
                .expect("find special manifest row");
            assert_eq!(count, 1, "missing manifest special row {label}");
        }
        assert_eq!(
            read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
            result.observed_max_time_updated
        );
    }

    #[test]
    fn ingest_boundary_late_row_is_recovered_by_24h_overlap_window() {
        let source_temp = tempfile::tempdir().expect("source tempdir");
        let database = source_temp.path().join("opencode.db");
        let watermark = 1_800_000_000_000_i64;
        let boundary = watermark - OVERLAP_WINDOW_MS;
        create_source_database(&database, "boundary-late", boundary);

        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(archive_temp.path()).expect("open archive");
        let request = ScanRequest::live(TEST_HOST, Some(watermark));
        let (result, _) = scan_fixture_into_archive(&mut archive, &database, &request);

        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, 1);
        assert_eq!(query_count(archive.connection()), 1);
        assert_eq!(query_tokens(archive.connection(), "boundary-late"), 77);
    }

    #[test]
    fn ingest_all_six_origin_permutations_end_with_one_live_value() {
        let permutations = [
            [Origin::Live, Origin::Bak, Origin::Legacy],
            [Origin::Live, Origin::Legacy, Origin::Bak],
            [Origin::Bak, Origin::Live, Origin::Legacy],
            [Origin::Bak, Origin::Legacy, Origin::Live],
            [Origin::Legacy, Origin::Live, Origin::Bak],
            [Origin::Legacy, Origin::Bak, Origin::Live],
        ];

        for origins in permutations {
            let temp = tempfile::tempdir().expect("archive tempdir");
            let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
            for origin in origins {
                let tokens = match origin {
                    Origin::Live => 333,
                    Origin::Bak => 222,
                    Origin::Legacy => 111,
                };
                ingest_direct_round(
                    &mut archive,
                    origin,
                    &[vec![record("origin-conflict", origin, 1_000, tokens)]],
                    &scan_result(true, Some(1_000)),
                );
            }

            let stored: (i64, String, String) = archive
                .connection()
                .query_row(
                    "SELECT tok_input, origin, provider_id FROM usage_record WHERE message_id = 'origin-conflict'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("read origin winner");
            assert_eq!(query_count(archive.connection()), 1, "order {origins:?}");
            assert_eq!(
                stored,
                (333, "live".to_string(), "live-provider".to_string())
            );
        }
    }

    #[test]
    fn ingest_interrupted_round_keeps_cursor_and_rerun_loses_nothing() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![record("before-interrupt", Origin::Live, 10, 1)]],
            &scan_result(true, Some(10)),
        );

        let first_batch = (0..DEFAULT_BATCH_SIZE)
            .map(|index| record(format!("interrupt-{index}"), Origin::Live, 20, 2))
            .collect::<Vec<_>>();
        let second_batch = vec![record("interrupt-tail", Origin::Live, 21, 3)];
        for _ in 0..2 {
            let stats = ingest_direct_round(
                &mut archive,
                Origin::Live,
                &[first_batch.clone(), second_batch.clone()],
                &scan_result(false, None),
            );
            assert!(!stats.committed);
            assert_eq!(query_count(archive.connection()), 1);
            assert_eq!(
                read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
                Some(10)
            );
        }

        let stats = ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[first_batch, second_batch],
            &scan_result(true, Some(21)),
        );
        assert!(stats.committed);
        assert_eq!(query_count(archive.connection()), 1_002);
        assert_eq!(duplicate_group_count(archive.connection()), 0);
        assert_eq!(
            read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
            Some(21)
        );
    }

    #[test]
    fn ingest_repeated_interruptions_and_batch_error_roll_back_the_whole_round() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![record("stable", Origin::Live, 50, 5)]],
            &scan_result(true, Some(50)),
        );

        for attempt in 0..2 {
            let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, Origin::Live)
                .expect("begin failing round");
            round
                .ingest_batch(&[record(format!("prefix-{attempt}"), Origin::Live, 60, 6)])
                .expect("apply valid prefix");
            let mut invalid = record("invalid-empty-id", Origin::Live, 61, 7);
            invalid.message_id.clear();
            let error = round
                .ingest_batch(&[invalid])
                .expect_err("empty message id must interrupt the round");
            assert!(error.to_string().contains("message_id is empty"));
            drop(round);

            assert_eq!(query_count(archive.connection()), 1);
            assert_eq!(
                read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
                Some(50)
            );
        }
    }

    #[test]
    fn ingest_stale_state_is_stable_and_removed_source_rows_are_not_pruned() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let row = record("authoritative-archive-row", Origin::Live, 70, 7);
        for _ in 0..2 {
            ingest_direct_round(
                &mut archive,
                Origin::Live,
                &[vec![row.clone()]],
                &scan_result(true, Some(70)),
            );
        }
        let count_before_empty_scan = query_count(archive.connection());
        let cursor_before_empty_scan =
            read_cursor(archive.connection(), TEST_HOST).expect("read cursor");

        ingest_direct_round(&mut archive, Origin::Live, &[], &scan_result(true, None));

        assert_eq!(query_count(archive.connection()), count_before_empty_scan);
        assert_eq!(
            read_cursor(archive.connection(), TEST_HOST).expect("read cursor"),
            cursor_before_empty_scan
        );
        assert_eq!(
            query_tokens(archive.connection(), "authoritative-archive-row"),
            7
        );
    }

    #[test]
    fn ingest_malformed_input_contract_is_explicit_and_readable() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");

        let mut accepted = record("long-values", Origin::Live, 80, 8);
        accepted.project_dir = "x".repeat(128 * 1_024);
        accepted.cost = None;
        accepted.cost_source = CostSource::Unavailable;
        ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![accepted]],
            &scan_result(true, Some(80)),
        );
        assert_eq!(query_count(archive.connection()), 1);

        for (invalid, expected) in [
            (
                record("oversized-token", Origin::Live, 81, u64::MAX),
                "tok_input exceeds SQLite INTEGER range",
            ),
            (
                {
                    let mut value = record("nan-cost", Origin::Live, 82, 9);
                    value.cost = Some(f64::NAN);
                    value
                },
                "cost must be finite",
            ),
            (
                {
                    let mut value = record("", Origin::Live, 83, 10);
                    value.message_id = "   ".to_string();
                    value
                },
                "message_id is empty",
            ),
        ] {
            let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, Origin::Live)
                .expect("begin malformed round");
            let error = round
                .ingest_batch(std::slice::from_ref(&invalid))
                .expect_err("invalid record must be rejected");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
            drop(round);
        }
        assert_eq!(query_count(archive.connection()), 1);
    }

    #[test]
    fn ingest_round_rejects_identity_and_provenance_mismatches() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");

        let error = match IngestRound::begin(archive.connection_mut(), "  ", Origin::Live) {
            Ok(_) => panic!("blank round host must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, IngestError::EmptyHostId));

        let cases = [
            (
                {
                    let mut value = record("host-mismatch", Origin::Live, 100, 1);
                    value.host_id = "another-host".into();
                    value
                },
                "does not match round host",
            ),
            (
                {
                    let mut value = record("source-mismatch", Origin::Live, 101, 1);
                    value.source = "codex".into();
                    value
                },
                "invalid; expected constant",
            ),
            (
                record("origin-mismatch", Origin::Bak, 102, 1),
                "does not match round origin",
            ),
            (
                {
                    let mut value = record("priority-mismatch", Origin::Live, 103, 1);
                    value.origin_priority = Origin::Legacy.priority();
                    value
                },
                "does not match origin live priority",
            ),
        ];

        for (invalid, expected) in cases {
            let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, Origin::Live)
                .expect("begin validation round");
            let error = round
                .ingest_batch(&[invalid])
                .expect_err("identity mismatch must fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
            drop(round);
        }
        assert_eq!(query_count(archive.connection()), 0);
    }

    #[test]
    fn ingest_rejects_every_out_of_range_token_bucket_and_infinite_cost() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");

        for field in [
            "tok_input",
            "tok_output",
            "tok_reasoning",
            "tok_cache_read",
            "tok_cache_write",
        ] {
            let mut invalid = record(format!("oversized-{field}"), Origin::Live, 110, 1);
            match field {
                "tok_input" => invalid.tok_input = u64::MAX,
                "tok_output" => invalid.tok_output = u64::MAX,
                "tok_reasoning" => invalid.tok_reasoning = u64::MAX,
                "tok_cache_read" => invalid.tok_cache_read = u64::MAX,
                "tok_cache_write" => invalid.tok_cache_write = u64::MAX,
                _ => unreachable!(),
            }
            let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, Origin::Live)
                .expect("begin oversized-token round");
            let error = round
                .ingest_batch(&[invalid])
                .expect_err("out-of-range token must fail");
            assert!(
                matches!(error, IngestError::TokenOutOfRange { field: actual, value } if actual == field && value == u64::MAX),
                "unexpected token error for {field}: {error}"
            );
            drop(round);
        }

        for cost in [f64::INFINITY, f64::NEG_INFINITY] {
            let mut invalid = record("infinite-cost", Origin::Live, 111, 1);
            invalid.cost = Some(cost);
            let mut round = IngestRound::begin(archive.connection_mut(), TEST_HOST, Origin::Live)
                .expect("begin non-finite-cost round");
            let error = round
                .ingest_batch(&[invalid])
                .expect_err("infinite cost must fail");
            assert!(matches!(error, IngestError::NonFiniteCost { value } if value == cost));
            drop(round);
        }
        assert_eq!(query_count(archive.connection()), 0);
    }

    #[test]
    fn ingest_stats_exclude_stale_conflicts_and_custom_cursor_reads_full_key() {
        let temp = tempfile::tempdir().expect("archive tempdir");
        let mut archive = Archive::open_in_data_dir(temp.path()).expect("open archive");

        let first = ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![record("same-priority", Origin::Live, 200, 20)]],
            &scan_result(true, Some(200)),
        );
        assert_eq!(first.received_records, 1);
        assert_eq!(first.changed_records, 1);
        assert!(first.committed);

        let stale = ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![record("same-priority", Origin::Live, 199, 99)]],
            &scan_result(true, Some(200)),
        );
        assert_eq!(stale.received_records, 1);
        assert_eq!(stale.changed_records, 0);
        assert_eq!(query_tokens(archive.connection(), "same-priority"), 20);

        archive
            .connection()
            .execute(
                "INSERT INTO source_cursor (host_id, source, cursor_time_updated) VALUES (?1, ?2, ?3)",
                (TEST_HOST, "another-source", 321_i64),
            )
            .expect("insert custom source cursor");
        assert_eq!(
            read_source_cursor(archive.connection(), TEST_HOST, "another-source")
                .expect("read custom cursor"),
            Some(321)
        );
        assert_eq!(
            read_source_cursor(archive.connection(), TEST_HOST, "missing-source")
                .expect("read absent cursor"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn ingest_read_only_archive_returns_readable_error_without_panicking() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("archive tempdir");
        let archive = Archive::open_in_data_dir(temp.path()).expect("open archive");
        let path = archive.path().to_path_buf();
        drop(archive);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
            .expect("chmod archive read-only");

        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open archive read-only");
        let mut connection = connection;
        let mut round = IngestRound::begin(&mut connection, TEST_HOST, Origin::Live)
            .expect("deferred read-only transaction begins");
        let error = round
            .ingest_batch(&[record("read-only", Origin::Live, 90, 9)])
            .expect_err("read-only archive must reject ingest");
        let error_text = error.to_string();
        drop(round);
        drop(connection);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("restore archive permissions");

        assert!(error_text.contains("archive ingest database operation failed"));
        assert!(
            error_text.contains("readonly"),
            "unexpected error: {error_text}"
        );
        println!("read_only_archive_error={error_text}");
    }

    #[test]
    #[ignore = "manual QA invokes the external sqlite3 binary"]
    fn ingest_manual_qa_external_sqlite3_fixture_round() {
        let (fixture_temp, fixture_directory, manifest) = fixture_directory();
        let archive_temp = tempfile::tempdir().expect("archive tempdir");
        let archive_directory = archive_temp.path().to_path_buf();
        let mut archive = Archive::open_in_data_dir(&archive_directory).expect("open archive");
        let archive_path = archive.path().to_path_buf();
        let request = ScanRequest::live(TEST_HOST, None);
        let (result, stats) = scan_fixture_into_archive(
            &mut archive,
            &fixture_directory.join("opencode.db"),
            &request,
        );
        assert!(stats.committed);
        assert_eq!(result.eligible_count, manifest.eligible_assistant_count);

        let counts = external_sqlite(
            &archive_path,
            "SELECT count(*) FROM usage_record; SELECT count(*) FROM source_cursor; SELECT origin, count(*) FROM usage_record GROUP BY origin;",
        );
        let duplicates = external_sqlite(
            &archive_path,
            "SELECT host_id, source, message_id, count(*) c FROM usage_record GROUP BY 1,2,3 HAVING c > 1;",
        );
        let cursor_before = external_sqlite(&archive_path, "SELECT * FROM source_cursor;");
        assert!(duplicates.is_empty());
        assert!(cursor_before.contains(
            &result
                .observed_max_time_updated
                .expect("fixture scan observed max")
                .to_string()
        ));
        println!("external_fixture_counts=\n{counts}");
        println!("external_fixture_duplicate_check={duplicates:?}");
        println!("external_fixture_cursor_before_interrupt=\n{cursor_before}");
        println!(
            "fixture_scan eligible={} observed_max={:?}",
            result.eligible_count, result.observed_max_time_updated
        );

        ingest_direct_round(
            &mut archive,
            Origin::Live,
            &[vec![record(
                "rolled-back-manual-row",
                Origin::Live,
                i64::MAX,
                1,
            )]],
            &scan_result(false, None),
        );
        let cursor_after = external_sqlite(&archive_path, "SELECT * FROM source_cursor;");
        assert_eq!(cursor_after, cursor_before);
        println!("external_fixture_cursor_after_interrupt=\n{cursor_after}");
        drop(archive);
        drop(fixture_temp);
        archive_temp.close().expect("remove fixture QA archive");
        assert!(!archive_directory.exists());
        println!("cleanup_receipt=removed {}", archive_directory.display());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "manual QA scans the live OpenCode database read-only"]
    fn ingest_manual_qa_real_database_scanner_count_matches_external_archive() {
        let database = PathBuf::from("/config/.local/share/opencode/opencode.db");
        let watermark = chrono::Utc::now().timestamp_millis() - 10 * 60 * 1_000;
        let archive_temp = tempfile::tempdir().expect("real QA archive tempdir");
        let archive_directory = archive_temp.path().to_path_buf();
        let mut archive =
            Archive::open_in_data_dir(&archive_directory).expect("open real QA archive");
        let archive_path = archive.path().to_path_buf();
        let request = ScanRequest::live(TEST_HOST, Some(watermark));
        let (result, stats) = scan_fixture_into_archive(&mut archive, &database, &request);
        assert!(result.reached_eof);
        assert!(stats.committed);

        let count_output = external_sqlite(&archive_path, "SELECT count(*) FROM usage_record;");
        let external_count: u64 = count_output
            .trim()
            .parse()
            .expect("external archive count is integer");
        assert_eq!(external_count, result.eligible_count);
        let cursor_output = external_sqlite(&archive_path, "SELECT * FROM source_cursor;");
        println!(
            "real_db_round watermark={watermark} window_start={} eligible={} skipped={} reached_eof={} observed_max={:?}",
            request.window_start(),
            result.eligible_count,
            result.skipped_count,
            result.reached_eof,
            result.observed_max_time_updated
        );
        println!("real_db_external_archive_count={count_output}");
        println!("real_db_external_cursor=\n{cursor_output}");
        drop(archive);
        archive_temp.close().expect("remove real QA archive");
        assert!(!archive_directory.exists());
        println!("cleanup_receipt=removed {}", archive_directory.display());
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
            .expect("external sqlite3 output UTF-8")
            .trim()
            .to_string()
    }

    fn create_source_database(path: &Path, message_id: &str, time_updated: i64) {
        let connection = Connection::open(path).expect("create source database");
        let mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .expect("set source WAL mode");
        assert_eq!(mode, "wal");
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
            .expect("create source message table");
        let data = json!({
            "role": "assistant",
            "agent": "Boundary Agent",
            "path": {"cwd": "/fixture/boundary"},
            "cost": 0,
            "tokens": {
                "input": 77,
                "output": 7,
                "reasoning": 3,
                "cache": {"read": 2, "write": 1}
            },
            "modelID": "boundary-model",
            "providerID": "boundary-provider",
            "time": {"created": time_updated - 1_000, "completed": time_updated - 100}
        });
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data)
                 VALUES (?1, 'boundary-session', ?2, ?3, ?4)",
                (
                    message_id,
                    time_updated - 1_000,
                    time_updated,
                    data.to_string(),
                ),
            )
            .expect("insert boundary source row");
    }
}
