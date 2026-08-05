//! 合成 fixture 数据集：生成 <5MB 的 WAL 模式 `opencode.db`（复刻 `message`/`session`/`part`
//! 三表与索引）、legacy `storage/message/` JSON 树，以及记录精确期望值的 `manifest.json`；
//! 另含测试侧持有 writer 连接的 `FixtureGuard`。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use rusqlite::config::DbConfig;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;
const FIXTURE_VERSION: u32 = 1;
const MAX_FIXTURE_BYTES: u64 = 5 * 1024 * 1024;
const DATABASE_FILE: &str = "opencode.db";
const MANIFEST_FILE: &str = "manifest.json";
const PRIMARY_SESSION_ID: &str = "ses_fixture_primary";
const BUCKET_SESSION_ID: &str = "ses_fixture_same_timestamp";
const DST_SESSION_ID: &str = "ses_fixture_dst";
const WAL_GUARD_MESSAGE_ID: &str = "msg_fixture_wal_uncheckpointed";

const LEGACY_START: i64 = 1_768_046_400_000;
const LEGACY_LAST_MILLISECOND: i64 = 1_769_903_999_999;
const LEGACY_END: i64 = 1_769_904_000_000;
const DATABASE_START: i64 = 1_772_323_200_000;
const DST_SPRING_BEFORE: i64 = 1_772_951_400_000;
const DST_SPRING_AFTER: i64 = 1_772_955_000_000;
const ZERO_USAGE_START: i64 = 1_777_593_600_000;
const ZERO_USAGE_END: i64 = 1_778_198_400_000;
const LAGGED_CREATED: i64 = 1_785_373_200_000;
const EIGHT_HOURS_MS: i64 = 8 * 60 * 60 * 1_000;
const LITERAL_TOKEN_ROW_CREATED: i64 = 1_785_468_844_419;
const COST_NONZERO_CREATED: i64 = 1_785_319_200_000;
const COST_ZERO_CREATED: i64 = 1_785_322_800_000;
const SAME_TIMESTAMP: i64 = 1_785_499_200_000;
const CROSS_TIMEZONE_CREATED: i64 = 1_785_511_800_000;
const DST_FALL_FIRST: i64 = 1_793_511_000_000;
const DST_FALL_SECOND: i64 = 1_793_514_600_000;
const COVERAGE_CUTOFF: i64 = 1_794_268_800_000;

const MESSAGE_DDL: &str = "CREATE TABLE `message` (\n  `id` text PRIMARY KEY,\n  `session_id` text NOT NULL,\n  `time_created` integer NOT NULL,\n  `time_updated` integer NOT NULL,\n  `data` text NOT NULL,\n  CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`)\n    REFERENCES `session`(`id`) ON DELETE CASCADE\n)";
const MESSAGE_INDEX_DDL: &str = "CREATE INDEX `message_session_time_created_id_idx` ON `message` (`session_id`,`time_created`,`id`)";
const SESSION_DDL: &str = "CREATE TABLE `session` (\n  `id` text PRIMARY KEY,\n  `project_id` text NOT NULL,\n  `parent_id` text,\n  `slug` text NOT NULL,\n  `directory` text NOT NULL,\n  `title` text NOT NULL,\n  `version` text NOT NULL,\n  `time_created` integer NOT NULL,\n  `time_updated` integer NOT NULL,\n  `time_archived` integer,\n  `workspace_id` text,\n  `agent` text,\n  `model` text,\n  `cost` real NOT NULL DEFAULT 0,\n  `tokens_input` integer NOT NULL DEFAULT 0,\n  `tokens_output` integer NOT NULL DEFAULT 0,\n  `tokens_reasoning` integer NOT NULL DEFAULT 0,\n  `tokens_cache_read` integer NOT NULL DEFAULT 0,\n  `tokens_cache_write` integer NOT NULL DEFAULT 0\n)";
const PART_DDL: &str = "CREATE TABLE `part` (\n  `id` text PRIMARY KEY,\n  `message_id` text NOT NULL,\n  `session_id` text NOT NULL,\n  `time_created` integer NOT NULL,\n  `time_updated` integer NOT NULL,\n  `data` text NOT NULL,\n  CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`)\n    REFERENCES `message`(`id`) ON DELETE CASCADE,\n  CONSTRAINT `fk_part_session_id_session_id_fk` FOREIGN KEY (`session_id`)\n    REFERENCES `session`(`id`) ON DELETE CASCADE\n)";

/// Result type used by the fixture generator API.
pub type Result<T> = std::result::Result<T, FixtureError>;

/// Errors returned while generating, installing, reading, or validating a fixture.
#[derive(Debug, Error)]
pub enum FixtureError {
    /// The requested output path exists but is not a directory.
    #[error("fixture output path exists and is not a directory: {0}")]
    OutputNotDirectory(PathBuf),
    /// The output path cannot be represented as a replaceable directory entry.
    #[error("fixture output path must have a final directory name: {0}")]
    InvalidOutputPath(PathBuf),
    /// A filesystem operation failed.
    #[error("fixture filesystem operation failed at {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Original I/O error.
        source: std::io::Error,
    },
    /// SQLite rejected a fixture operation.
    #[error("fixture SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON serialization or deserialization failed.
    #[error("fixture JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Generated artifacts disagree with the manifest.
    #[error("fixture validation failed: {0}")]
    Validation(String),
}

/// Five atomic token counters plus AgentLens's derived total-input value.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct TokenExpectation {
    /// Cache-miss input tokens from `tokens.input`.
    pub input: u64,
    /// Output tokens from `tokens.output`.
    pub output: u64,
    /// Reasoning tokens from `tokens.reasoning`.
    pub reasoning: u64,
    /// Cache-read input tokens from `tokens.cache.read`.
    pub cache_read: u64,
    /// Cache-write input tokens from `tokens.cache.write`.
    pub cache_write: u64,
    /// Derived as `input + cache_read + cache_write`; never read from `tokens.total`.
    pub total_input: u64,
}

impl TokenExpectation {
    fn new(input: u64, output: u64, reasoning: u64, cache_read: u64, cache_write: u64) -> Self {
        Self {
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
            total_input: input + cache_read + cache_write,
        }
    }

    fn all_zero(self) -> bool {
        self.input == 0
            && self.output == 0
            && self.reasoning == 0
            && self.cache_read == 0
            && self.cache_write == 0
    }

    fn add_assign(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total_input += other.total_input;
    }
}

/// Cost provenance expected after normalization.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedCostSource {
    /// A positive source cost is available.
    Actual,
    /// The source wrote zero, so cost is unavailable rather than actual zero.
    Unavailable,
}

/// Exact normalized parse result for one assistant message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ParseExpectation {
    /// Raw agent name or legacy slug.
    pub agent_raw: String,
    /// Expected normalized agent key.
    pub agent_key: String,
    /// Provider identifier.
    pub provider_id: String,
    /// Model identifier.
    pub model_id: String,
    /// Optional model variant.
    pub variant: Option<String>,
    /// Atomic and derived token expectations.
    pub tokens: TokenExpectation,
    /// Source `tokens.total`, retained only to prove its incompatible semantics.
    pub source_tokens_total: Option<u64>,
    /// Raw source cost before normalization.
    pub source_cost: f64,
    /// Normalized cost; zero source cost maps to `None`.
    pub cost: Option<f64>,
    /// Normalized cost provenance.
    pub cost_source: ExpectedCostSource,
    /// True only for all-zero tokens without `time.completed`.
    pub is_incomplete: bool,
}

/// One database row singled out for downstream parser assertions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SpecialRowExpectation {
    /// Stable database message identifier.
    pub message_id: String,
    /// Database session identifier.
    pub session_id: String,
    /// Source creation time in UTC epoch milliseconds.
    pub time_created: i64,
    /// Source update time in UTC epoch milliseconds.
    pub time_updated: i64,
    /// Optional completion time in UTC epoch milliseconds.
    pub time_completed: Option<i64>,
    /// Exact normalized result expected from this row.
    pub expected: ParseExpectation,
}

/// Exact population sharing one `time_updated` value.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SameTimestampBucket {
    /// Shared UTC epoch-millisecond update timestamp.
    pub time_updated: i64,
    /// Exact number of rows in the bucket.
    pub count: u64,
}

/// Two-phase state transition for the lagged update record.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LaggedUpdateExpectation {
    /// Stable message identifier updated in place.
    pub message_id: String,
    /// Original creation time.
    pub time_created: i64,
    /// Update timestamp on the stale, zero-token insert.
    pub pre_update_time_updated: i64,
    /// Update timestamp after the final write (`created + 8h`).
    pub post_update_time_updated: i64,
    /// Token values present in the first committed phase.
    pub stale_tokens: TokenExpectation,
    /// Token values present after the second committed phase.
    pub final_tokens: TokenExpectation,
}

/// Half-open UTC epoch-millisecond interval.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ExpectedInterval {
    /// Inclusive interval start.
    pub start: i64,
    /// Exclusive interval end.
    pub end: i64,
}

/// Coverage boundaries represented by the fixture.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CoverageExpectation {
    /// Exclusive end of the legacy source's covered range (D1).
    pub legacy_end: i64,
    /// Inclusive start of the live database range (D2).
    pub db_start: i64,
    /// Expected uncovered half-open interval `[D1, D2)`.
    pub expected_gap: ExpectedInterval,
    /// A fully covered seven-day range containing no eligible usage.
    pub covered_zero_usage: ExpectedInterval,
    /// Successful live-scan cutoff extending coverage beyond the final record.
    pub live_cutoff: i64,
}

/// Cost totals for one report-timezone day.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DailyCostExpectation {
    /// Sum of positive source costs.
    pub actual_sum: f64,
    /// Count of records whose source cost was zero/unavailable.
    pub unavailable_count: u64,
    /// Reserved estimated-cost sum; zero in this fixture.
    pub estimated_sum: f64,
}

/// Exact complete-usage aggregate for one local report day.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DailyExpectation {
    /// Local ISO date in the enclosing report timezone.
    pub date: String,
    /// Atomic and derived token totals. Incomplete rows are excluded.
    pub tokens: TokenExpectation,
    /// Layered cost totals. Incomplete rows are excluded.
    pub cost: DailyCostExpectation,
    /// Number of complete eligible messages.
    pub message_count: u64,
    /// Number of distinct active sessions.
    pub active_session_count: u64,
    /// Number of incomplete eligible messages deliberately excluded.
    pub incomplete_excluded_count: u64,
}

/// A message placed on a daylight-saving transition day.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct DstSampleExpectation {
    /// Stable sample label.
    pub label: String,
    /// Database message identifier.
    pub message_id: String,
    /// Exact UTC epoch-millisecond instant.
    pub epoch_ms: i64,
    /// IANA timezone used by the assertion.
    pub timezone: String,
    /// Human-readable local wall time and UTC offset.
    pub local_time: String,
    /// Why this point exists in the fixture.
    pub note: String,
}

/// The conflicting live and legacy representations of one message ID.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LegacyOverlapExpectation {
    /// Message identifier present in both stores.
    pub message_id: String,
    /// Relative legacy JSON path under the fixture root.
    pub legacy_relative_path: String,
    /// Live database value, which must win ingestion conflicts.
    pub database: ParseExpectation,
    /// Deliberately different lower-priority legacy value.
    pub legacy: ParseExpectation,
}

/// Versioned, deterministic source of truth for every downstream fixture assertion.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Manifest {
    /// Manifest schema version. Readers should reject unknown values.
    pub schema_version: u32,
    /// Dataset revision. A changed row set increments this value.
    pub fixture_version: u32,
    /// Eligible database rows (`assistant` with a `tokens` object).
    pub eligible_assistant_count: u64,
    /// Database rows a scanner must skip.
    pub skipped_count: u64,
    /// Stable skip counts keyed by `non_assistant` and `missing_tokens`.
    pub skipped_breakdown: BTreeMap<String, u64>,
    /// Total rows in the database `message` table.
    pub total_message_rows: u64,
    /// Total JSON files in the legacy tree, including the overlap.
    pub legacy_message_rows: u64,
    /// Unique message IDs across the database and legacy tree.
    pub combined_unique_message_count: u64,
    /// Parser assertions keyed by stable scenario label.
    pub special_rows: BTreeMap<String, SpecialRowExpectation>,
    /// The pagination-adversarial 1001-row update-time bucket.
    pub same_timestamp_bucket: SameTimestampBucket,
    /// Two committed states of the eight-hour lagged update row.
    pub lagged_update: LaggedUpdateExpectation,
    /// Legacy/live gap and fully covered zero-usage interval.
    pub coverage: CoverageExpectation,
    /// Final live-wins-over-legacy aggregates keyed by timezone then ISO date.
    pub daily_expectations: BTreeMap<String, BTreeMap<String, DailyExpectation>>,
    /// Spring-forward and fall-back instants for America/New_York.
    pub dst_samples: Vec<DstSampleExpectation>,
    /// Exact conflicting values for the cross-source duplicate.
    pub legacy_overlap: LegacyOverlapExpectation,
}

impl Manifest {
    /// Reads `manifest.json` from a generated fixture directory.
    pub fn read_from(out_dir: impl AsRef<Path>) -> Result<Self> {
        let path = out_dir.as_ref().join(MANIFEST_FILE);
        let bytes = read_file(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Cross-checks all manifest sections against the generated database and JSON tree.
    ///
    /// This validator is intentionally fixture-specific. It is not the production OpenCode scanner;
    /// its purpose is to prove that the generated test oracle is not decorative or stale.
    pub fn validate(&self, out_dir: impl AsRef<Path>) -> Result<()> {
        let out_dir = out_dir.as_ref();
        validate_version("schema_version", SCHEMA_VERSION, self.schema_version)?;
        validate_version("fixture_version", FIXTURE_VERSION, self.fixture_version)?;

        let connection = open_read_only(&out_dir.join(DATABASE_FILE))?;
        let total_rows = query_count(&connection, "SELECT count(*) FROM message", [])?;
        validate_value("total_message_rows", self.total_message_rows, total_rows)?;

        let eligible_rows = query_count(
            &connection,
            "SELECT count(*) FROM message WHERE json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.tokens') IS NOT NULL",
            [],
        )?;
        validate_value(
            "eligible_assistant_count",
            self.eligible_assistant_count,
            eligible_rows,
        )?;

        let non_assistant = query_count(
            &connection,
            "SELECT count(*) FROM message WHERE coalesce(json_extract(data, '$.role'), '') != 'assistant'",
            [],
        )?;
        let missing_tokens = query_count(
            &connection,
            "SELECT count(*) FROM message WHERE json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.tokens') IS NULL",
            [],
        )?;
        let actual_breakdown = BTreeMap::from([
            ("missing_tokens".to_string(), missing_tokens),
            ("non_assistant".to_string(), non_assistant),
        ]);
        validate_value(
            "skipped_breakdown",
            &self.skipped_breakdown,
            &actual_breakdown,
        )?;
        validate_value(
            "skipped_count",
            self.skipped_count,
            non_assistant + missing_tokens,
        )?;

        let same_timestamp_count = query_count(
            &connection,
            "SELECT count(*) FROM message WHERE time_updated = ?1",
            [self.same_timestamp_bucket.time_updated],
        )?;
        validate_value(
            "same_timestamp_bucket.count",
            self.same_timestamp_bucket.count,
            same_timestamp_count,
        )?;

        for (label, expected) in &self.special_rows {
            validate_special_row(&connection, label, expected)?;
        }
        validate_lagged_update(self)?;
        validate_coverage(&connection, out_dir, self)?;
        validate_legacy_overlap(out_dir, self)?;

        let artifacts = collect_artifact_records(&connection, out_dir)?;
        validate_value(
            "legacy_message_rows",
            self.legacy_message_rows,
            artifacts.legacy_count,
        )?;
        validate_value(
            "combined_unique_message_count",
            self.combined_unique_message_count,
            artifacts.all_message_ids.len() as u64,
        )?;
        let actual_daily = daily_expectations(artifacts.eligible_by_id.values().cloned())?;
        validate_value(
            "daily_expectations",
            &self.daily_expectations,
            &actual_daily,
        )?;

        let size = tree_size(out_dir)?;
        if size >= MAX_FIXTURE_BYTES {
            return Err(FixtureError::Validation(format!(
                "payload_size expected < {MAX_FIXTURE_BYTES}, actual {size}"
            )));
        }
        Ok(())
    }
}

/// Keeps a writer connection alive after committing one row into the fixture WAL.
///
/// Create this guard after [`generate`], keep it in scope while opening a separate `mode=ro`
/// reader, and query [`FixtureGuard::message_id`]. The writer disables auto-checkpoint and
/// checkpoint-on-close; while the guard is alive, `opencode.db-wal` contains committed frames and
/// the read-only connection must observe the inserted row. Dropping the guard ends the invariant,
/// so callers must complete the read before drop.
pub struct FixtureGuard {
    db_path: PathBuf,
    writer: Connection,
}

impl FixtureGuard {
    /// Opens the generated database for writing and commits a dedicated message into its WAL.
    pub fn new(out_dir: impl AsRef<Path>) -> Result<Self> {
        let db_path = out_dir.as_ref().join(DATABASE_FILE);
        let writer = Connection::open(&db_path)?;
        writer.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
        writer.pragma_update(None, "wal_autocheckpoint", 0)?;

        let data = json!({
            "parentID": "msg_fixture_guard_parent",
            "role": "assistant",
            "mode": "fixture-wal-guard",
            "agent": "fixture-wal-guard",
            "path": {"cwd": "/fixture/wal", "root": "/fixture/wal"},
            "cost": 0,
            "tokens": {
                "input": 17,
                "output": 3,
                "reasoning": 1,
                "cache": {"read": 5, "write": 2},
                "total": 20
            },
            "modelID": "fixture-wal-model",
            "providerID": "fixture-wal-provider",
            "time": {"created": COVERAGE_CUTOFF + 1_000, "completed": COVERAGE_CUTOFF + 1_500},
            "finish": "stop"
        });
        writer.execute(
            "INSERT OR REPLACE INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                WAL_GUARD_MESSAGE_ID,
                PRIMARY_SESSION_ID,
                COVERAGE_CUTOFF + 1_000,
                COVERAGE_CUTOFF + 2_000,
                serde_json::to_string(&data)?
            ],
        )?;

        Ok(Self { db_path, writer })
    }

    /// Returns the generated database path used by the live writer.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Returns the stable ID of the row committed only by this guard.
    pub fn message_id(&self) -> &'static str {
        WAL_GUARD_MESSAGE_ID
    }

    /// Exposes the live writer for tests that need to assert connection-level WAL settings.
    pub fn writer_connection(&self) -> &Connection {
        &self.writer
    }
}

/// Generates and atomically installs a complete deterministic fixture directory.
///
/// Existing directories are replaced as a unit after generation succeeds. Existing files are
/// rejected without modification. No clock, process identifier, random generator, or unordered
/// map contributes to artifact contents.
pub fn generate(out_dir: impl AsRef<Path>) -> Result<Manifest> {
    let out_dir = out_dir.as_ref();
    let output_name = out_dir
        .file_name()
        .ok_or_else(|| FixtureError::InvalidOutputPath(out_dir.to_path_buf()))?;
    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;

    if let Ok(metadata) = fs::symlink_metadata(out_dir) {
        if !metadata.is_dir() {
            return Err(FixtureError::OutputNotDirectory(out_dir.to_path_buf()));
        }
    }

    let output_name = output_name.to_string_lossy();
    let stage = parent.join(format!(".{output_name}.agentlens-stage"));
    let backup = parent.join(format!(".{output_name}.agentlens-backup"));
    remove_path_if_exists(&stage)?;
    remove_path_if_exists(&backup)?;
    create_dir(&stage)?;

    let generation = generate_staged(&stage);
    let manifest = match generation {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    if let Err(error) = manifest.validate(&stage) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }

    install_staged(out_dir, &stage, &backup)?;
    Ok(manifest)
}

#[derive(Clone)]
enum ModelShape {
    Flat,
    Nested,
}

#[derive(Clone)]
struct RecordSpec {
    id: String,
    session_id: String,
    time_created: i64,
    time_updated: i64,
    time_completed: Option<i64>,
    agent: String,
    provider_id: String,
    model_id: String,
    variant: Option<String>,
    tokens: TokenExpectation,
    source_cost: f64,
    include_tokens_total: bool,
    model_shape: ModelShape,
}

impl RecordSpec {
    fn expected(&self) -> ParseExpectation {
        ParseExpectation {
            agent_raw: self.agent.clone(),
            agent_key: normalize_agent_key(&self.agent),
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
            variant: self.variant.clone(),
            tokens: self.tokens,
            source_tokens_total: self
                .include_tokens_total
                .then_some(self.tokens.input + self.tokens.output),
            source_cost: self.source_cost,
            cost: (self.source_cost > 0.0).then_some(self.source_cost),
            cost_source: if self.source_cost > 0.0 {
                ExpectedCostSource::Actual
            } else {
                ExpectedCostSource::Unavailable
            },
            is_incomplete: self.tokens.all_zero() && self.time_completed.is_none(),
        }
    }

    fn special(&self) -> SpecialRowExpectation {
        SpecialRowExpectation {
            message_id: self.id.clone(),
            session_id: self.session_id.clone(),
            time_created: self.time_created,
            time_updated: self.time_updated,
            time_completed: self.time_completed,
            expected: self.expected(),
        }
    }

    fn data(&self) -> Value {
        assistant_data(self)
    }
}

#[derive(Clone)]
struct AggregateRecord {
    session_id: String,
    time_created: i64,
    tokens: TokenExpectation,
    source_cost: f64,
    is_incomplete: bool,
}

impl From<&RecordSpec> for AggregateRecord {
    fn from(record: &RecordSpec) -> Self {
        Self {
            session_id: record.session_id.clone(),
            time_created: record.time_created,
            tokens: record.tokens,
            source_cost: record.source_cost,
            is_incomplete: record.tokens.all_zero() && record.time_completed.is_none(),
        }
    }
}

struct ArtifactRecords {
    all_message_ids: BTreeSet<String>,
    eligible_by_id: BTreeMap<String, AggregateRecord>,
    legacy_count: u64,
}

fn generate_staged(out_dir: &Path) -> Result<Manifest> {
    let (special_specs, mut database_specs, lagged_stale) = database_records();
    let legacy_specs = legacy_records();

    let mut special_rows = BTreeMap::new();
    for (label, record) in &special_specs {
        special_rows.insert((*label).to_string(), record.special());
    }

    let lagged_final = special_specs
        .iter()
        .find_map(|(label, record)| (*label == "lagged_update").then_some(record))
        .expect("lagged fixture record is defined");
    let database_overlap = special_specs
        .iter()
        .find_map(|(label, record)| (*label == "legacy_overlap").then_some(record))
        .expect("database overlap fixture record is defined");
    let legacy_overlap = legacy_specs
        .iter()
        .find(|record| record.id == database_overlap.id)
        .expect("legacy overlap fixture record is defined");

    database_specs.extend(same_timestamp_records());
    let total_message_rows = database_specs.len() as u64 + 2;
    let eligible_assistant_count = database_specs.len() as u64;

    let mut merged = BTreeMap::<String, AggregateRecord>::new();
    for record in &legacy_specs {
        merged.insert(record.id.clone(), AggregateRecord::from(record));
    }
    for record in &database_specs {
        merged.insert(record.id.clone(), AggregateRecord::from(record));
    }

    let daily_expectations = daily_expectations(merged.values().cloned())?;
    let legacy_relative_path = legacy_path(legacy_overlap)
        .to_string_lossy()
        .replace('\\', "/");
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        fixture_version: FIXTURE_VERSION,
        eligible_assistant_count,
        skipped_count: 2,
        skipped_breakdown: BTreeMap::from([
            ("missing_tokens".to_string(), 1),
            ("non_assistant".to_string(), 1),
        ]),
        total_message_rows,
        legacy_message_rows: legacy_specs.len() as u64,
        combined_unique_message_count: total_message_rows + legacy_specs.len() as u64 - 1,
        special_rows,
        same_timestamp_bucket: SameTimestampBucket {
            time_updated: SAME_TIMESTAMP,
            count: 1_001,
        },
        lagged_update: LaggedUpdateExpectation {
            message_id: lagged_final.id.clone(),
            time_created: lagged_final.time_created,
            pre_update_time_updated: lagged_stale.time_updated,
            post_update_time_updated: lagged_final.time_updated,
            stale_tokens: lagged_stale.tokens,
            final_tokens: lagged_final.tokens,
        },
        coverage: CoverageExpectation {
            legacy_end: LEGACY_END,
            db_start: DATABASE_START,
            expected_gap: ExpectedInterval {
                start: LEGACY_END,
                end: DATABASE_START,
            },
            covered_zero_usage: ExpectedInterval {
                start: ZERO_USAGE_START,
                end: ZERO_USAGE_END,
            },
            live_cutoff: COVERAGE_CUTOFF,
        },
        daily_expectations,
        dst_samples: dst_samples(),
        legacy_overlap: LegacyOverlapExpectation {
            message_id: database_overlap.id.clone(),
            legacy_relative_path,
            database: database_overlap.expected(),
            legacy: legacy_overlap.expected(),
        },
    };

    write_database(out_dir, &database_specs, &lagged_stale)?;
    write_legacy_tree(out_dir, &legacy_specs)?;
    write_manifest(out_dir, &manifest)?;

    let size = tree_size(out_dir)?;
    if size >= MAX_FIXTURE_BYTES {
        return Err(FixtureError::Validation(format!(
            "payload_size expected < {MAX_FIXTURE_BYTES}, actual {size}"
        )));
    }
    Ok(manifest)
}

fn database_records() -> (Vec<(&'static str, RecordSpec)>, Vec<RecordSpec>, RecordSpec) {
    let coverage_start = record(
        "msg_fixture_db_coverage_start",
        PRIMARY_SESSION_ID,
        DATABASE_START,
        TokenExpectation::new(11, 2, 1, 3, 0),
    );
    let spring_before = record(
        "msg_fixture_dst_spring_before",
        DST_SESSION_ID,
        DST_SPRING_BEFORE,
        TokenExpectation::new(13, 3, 1, 2, 0),
    );
    let spring_after = record(
        "msg_fixture_dst_spring_after",
        DST_SESSION_ID,
        DST_SPRING_AFTER,
        TokenExpectation::new(17, 4, 2, 3, 1),
    );

    let lagged_final = RecordSpec {
        id: "msg_fixture_lagged_update".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LAGGED_CREATED,
        time_updated: LAGGED_CREATED + EIGHT_HOURS_MS,
        time_completed: Some(LAGGED_CREATED + EIGHT_HOURS_MS - 1_000),
        agent: "Atlas - Plan Executor".to_string(),
        provider_id: "kiro-auth".to_string(),
        model_id: "gpt-5.6-sol-xhigh".to_string(),
        variant: Some("xhigh".to_string()),
        tokens: TokenExpectation::new(1_234, 56, 7, 100, 20),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let mut lagged_stale = lagged_final.clone();
    lagged_stale.time_updated = lagged_stale.time_created;
    lagged_stale.time_completed = None;
    lagged_stale.tokens = TokenExpectation::default();

    let flat_with_variant = RecordSpec {
        id: "msg_fixture_flat_xhigh_literal".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 44_204,
        time_completed: Some(LITERAL_TOKEN_ROW_CREATED + 44_204),
        agent: "Atlas - Plan Executor".to_string(),
        provider_id: "myopenai".to_string(),
        model_id: "us.anthropic.claude-fable-5".to_string(),
        variant: Some("xhigh".to_string()),
        tokens: TokenExpectation::new(7_322, 227, 91, 46_543, 0),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let nested_assistant = RecordSpec {
        id: "msg_fixture_nested_assistant".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED + 600_000,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 601_000,
        time_completed: Some(LITERAL_TOKEN_ROW_CREATED + 600_900),
        agent: "Explore".to_string(),
        provider_id: "defensive-provider".to_string(),
        model_id: "defensive-nested-model".to_string(),
        variant: Some("high".to_string()),
        tokens: TokenExpectation::new(101, 19, 5, 23, 7),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Nested,
    };
    let no_variant = RecordSpec {
        id: "msg_fixture_no_variant".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED + 1_200_000,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 1_201_000,
        time_completed: Some(LITERAL_TOKEN_ROW_CREATED + 1_200_900),
        agent: "Librarian".to_string(),
        provider_id: "kiro-auth".to_string(),
        model_id: "gpt-5.6-sol".to_string(),
        variant: None,
        tokens: TokenExpectation::new(211, 29, 3, 31, 11),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let interrupted = RecordSpec {
        id: "msg_fixture_interrupted_zero_token".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED + 1_800_000,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 1_801_000,
        time_completed: None,
        agent: "Atlas - Plan Executor".to_string(),
        provider_id: "kiro-auth".to_string(),
        model_id: "gpt-5.6-sol-xhigh".to_string(),
        variant: Some("xhigh".to_string()),
        tokens: TokenExpectation::default(),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let cost_nonzero = RecordSpec {
        id: "msg_fixture_cost_nonzero".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: COST_NONZERO_CREATED,
        time_updated: COST_NONZERO_CREATED + 1_000,
        time_completed: Some(COST_NONZERO_CREATED + 900),
        agent: "Build".to_string(),
        provider_id: "priced-provider".to_string(),
        model_id: "priced-model".to_string(),
        variant: Some("medium".to_string()),
        tokens: TokenExpectation::new(307, 41, 9, 43, 13),
        source_cost: 0.0102,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let cost_zero = RecordSpec {
        id: "msg_fixture_cost_zero_same_day".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: COST_ZERO_CREATED,
        time_updated: COST_ZERO_CREATED + 1_000,
        time_completed: Some(COST_ZERO_CREATED + 900),
        agent: "Build".to_string(),
        provider_id: "subscription-provider".to_string(),
        model_id: "subscription-model".to_string(),
        variant: Some("low".to_string()),
        tokens: TokenExpectation::new(401, 43, 11, 47, 17),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let missing_total = RecordSpec {
        id: "msg_fixture_missing_tokens_total".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED + 8_800_000,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 8_801_000,
        time_completed: Some(LITERAL_TOKEN_ROW_CREATED + 8_800_900),
        agent: "Build".to_string(),
        provider_id: "kiro-auth".to_string(),
        model_id: "gpt-5.6-sol-xhigh".to_string(),
        variant: Some("max".to_string()),
        tokens: TokenExpectation::new(503, 47, 13, 53, 19),
        source_cost: 0.0,
        include_tokens_total: false,
        model_shape: ModelShape::Flat,
    };
    let overlap = RecordSpec {
        id: "msg_fixture_live_legacy_overlap".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: LITERAL_TOKEN_ROW_CREATED + 12_400_000,
        time_updated: LITERAL_TOKEN_ROW_CREATED + 12_401_000,
        time_completed: Some(LITERAL_TOKEN_ROW_CREATED + 12_400_900),
        agent: "Atlas - Plan Executor".to_string(),
        provider_id: "live-provider".to_string(),
        model_id: "live-model".to_string(),
        variant: Some("xhigh".to_string()),
        tokens: TokenExpectation::new(607, 59, 17, 61, 23),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let cross_timezone = RecordSpec {
        id: "msg_fixture_cross_timezone_utc_1530".to_string(),
        session_id: PRIMARY_SESSION_ID.to_string(),
        time_created: CROSS_TIMEZONE_CREATED,
        time_updated: CROSS_TIMEZONE_CREATED + 1_000,
        time_completed: Some(CROSS_TIMEZONE_CREATED + 900),
        agent: "Explore".to_string(),
        provider_id: "timezone-provider".to_string(),
        model_id: "timezone-model".to_string(),
        variant: None,
        tokens: TokenExpectation::new(701, 61, 19, 67, 29),
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    };
    let fall_first = record(
        "msg_fixture_dst_fall_first_0130",
        DST_SESSION_ID,
        DST_FALL_FIRST,
        TokenExpectation::new(23, 5, 2, 4, 1),
    );
    let fall_second = record(
        "msg_fixture_dst_fall_second_0130",
        DST_SESSION_ID,
        DST_FALL_SECOND,
        TokenExpectation::new(29, 7, 3, 5, 2),
    );

    let special = vec![
        ("db_coverage_start", coverage_start.clone()),
        ("dst_spring_before", spring_before.clone()),
        ("dst_spring_after", spring_after.clone()),
        ("lagged_update", lagged_final.clone()),
        ("flat_with_variant", flat_with_variant.clone()),
        ("nested_assistant", nested_assistant.clone()),
        ("no_variant", no_variant.clone()),
        ("interrupted_zero_token", interrupted.clone()),
        ("cost_nonzero", cost_nonzero.clone()),
        ("cost_zero_same_day", cost_zero.clone()),
        ("missing_tokens_total", missing_total.clone()),
        ("legacy_overlap", overlap.clone()),
        ("cross_timezone", cross_timezone.clone()),
        ("dst_fall_first", fall_first.clone()),
        ("dst_fall_second", fall_second.clone()),
    ];
    let database = vec![
        coverage_start,
        spring_before,
        spring_after,
        lagged_final,
        flat_with_variant,
        nested_assistant,
        no_variant,
        interrupted,
        cost_nonzero,
        cost_zero,
        missing_total,
        overlap,
        cross_timezone,
        fall_first,
        fall_second,
    ];
    (special, database, lagged_stale)
}

fn record(id: &str, session_id: &str, created: i64, tokens: TokenExpectation) -> RecordSpec {
    RecordSpec {
        id: id.to_string(),
        session_id: session_id.to_string(),
        time_created: created,
        time_updated: created + 1_000,
        time_completed: Some(created + 900),
        agent: "Atlas - Plan Executor".to_string(),
        provider_id: "kiro-auth".to_string(),
        model_id: "gpt-5.6-sol-xhigh".to_string(),
        variant: Some("xhigh".to_string()),
        tokens,
        source_cost: 0.0,
        include_tokens_total: true,
        model_shape: ModelShape::Flat,
    }
}

fn same_timestamp_records() -> Vec<RecordSpec> {
    (0..1_001)
        .map(|index| {
            let input = (index % 7 + 1) as u64;
            let output = (index % 5 + 1) as u64;
            let reasoning = (index % 3) as u64;
            let cache_read = (index % 4) as u64;
            let cache_write = (index % 2) as u64;
            let created = SAME_TIMESTAMP - 20_000 + index as i64;
            RecordSpec {
                id: format!("msg_fixture_same_timestamp_{index:04}"),
                session_id: BUCKET_SESSION_ID.to_string(),
                time_created: created,
                time_updated: SAME_TIMESTAMP,
                time_completed: Some(created + 500),
                agent: "Bucket Worker".to_string(),
                provider_id: "pagination-provider".to_string(),
                model_id: "pagination-model".to_string(),
                variant: Some(if index % 2 == 0 { "high" } else { "medium" }.to_string()),
                tokens: TokenExpectation::new(input, output, reasoning, cache_read, cache_write),
                source_cost: 0.0,
                include_tokens_total: index != 1_000,
                model_shape: ModelShape::Flat,
            }
        })
        .collect()
}

fn legacy_records() -> Vec<RecordSpec> {
    vec![
        RecordSpec {
            id: "msg_fixture_legacy_start".to_string(),
            session_id: "ses_fixture_legacy_history".to_string(),
            time_created: LEGACY_START,
            time_updated: LEGACY_START + 1_000,
            time_completed: Some(LEGACY_START + 900),
            agent: "librarian".to_string(),
            provider_id: "legacy-provider".to_string(),
            model_id: "legacy-model".to_string(),
            variant: None,
            tokens: TokenExpectation::new(809, 71, 23, 73, 31),
            source_cost: 0.0,
            include_tokens_total: true,
            model_shape: ModelShape::Flat,
        },
        RecordSpec {
            id: "msg_fixture_legacy_end".to_string(),
            session_id: "ses_fixture_legacy_history".to_string(),
            time_created: LEGACY_LAST_MILLISECOND,
            time_updated: LEGACY_LAST_MILLISECOND,
            time_completed: Some(LEGACY_LAST_MILLISECOND),
            agent: "librarian".to_string(),
            provider_id: "legacy-provider".to_string(),
            model_id: "legacy-model".to_string(),
            variant: None,
            tokens: TokenExpectation::new(907, 79, 29, 83, 37),
            source_cost: 0.0,
            include_tokens_total: true,
            model_shape: ModelShape::Flat,
        },
        RecordSpec {
            id: "msg_fixture_live_legacy_overlap".to_string(),
            session_id: "ses_fixture_legacy_history".to_string(),
            time_created: LEGACY_START + 86_400_000,
            time_updated: LEGACY_START + 86_401_000,
            time_completed: Some(LEGACY_START + 86_400_900),
            agent: "librarian".to_string(),
            provider_id: "legacy-conflict-provider".to_string(),
            model_id: "legacy-conflict-model".to_string(),
            variant: None,
            tokens: TokenExpectation::new(9_999, 999, 99, 999, 99),
            source_cost: 0.0204,
            include_tokens_total: true,
            model_shape: ModelShape::Flat,
        },
    ]
}

fn dst_samples() -> Vec<DstSampleExpectation> {
    vec![
        DstSampleExpectation {
            label: "spring_before_jump".to_string(),
            message_id: "msg_fixture_dst_spring_before".to_string(),
            epoch_ms: DST_SPRING_BEFORE,
            timezone: "America/New_York".to_string(),
            local_time: "2026-03-08T01:30:00-05:00".to_string(),
            note: "last represented 01:30 hour before the 02:00 spring-forward gap".to_string(),
        },
        DstSampleExpectation {
            label: "spring_after_jump".to_string(),
            message_id: "msg_fixture_dst_spring_after".to_string(),
            epoch_ms: DST_SPRING_AFTER,
            timezone: "America/New_York".to_string(),
            local_time: "2026-03-08T03:30:00-04:00".to_string(),
            note: "first represented 03:30 hour after the spring-forward gap".to_string(),
        },
        DstSampleExpectation {
            label: "fall_first_0130".to_string(),
            message_id: "msg_fixture_dst_fall_first_0130".to_string(),
            epoch_ms: DST_FALL_FIRST,
            timezone: "America/New_York".to_string(),
            local_time: "2026-11-01T01:30:00-04:00".to_string(),
            note: "first 01:30 during fall-back, still on EDT".to_string(),
        },
        DstSampleExpectation {
            label: "fall_second_0130".to_string(),
            message_id: "msg_fixture_dst_fall_second_0130".to_string(),
            epoch_ms: DST_FALL_SECOND,
            timezone: "America/New_York".to_string(),
            local_time: "2026-11-01T01:30:00-05:00".to_string(),
            note: "second 01:30 during fall-back, now on EST".to_string(),
        },
    ]
}

fn assistant_data(record: &RecordSpec) -> Value {
    let mut tokens = Map::new();
    tokens.insert("input".to_string(), json!(record.tokens.input));
    tokens.insert("output".to_string(), json!(record.tokens.output));
    tokens.insert("reasoning".to_string(), json!(record.tokens.reasoning));
    tokens.insert(
        "cache".to_string(),
        json!({"read": record.tokens.cache_read, "write": record.tokens.cache_write}),
    );
    if record.include_tokens_total {
        tokens.insert(
            "total".to_string(),
            json!(record.tokens.input + record.tokens.output),
        );
    }

    let mut time = Map::new();
    time.insert("created".to_string(), json!(record.time_created));
    if let Some(completed) = record.time_completed {
        time.insert("completed".to_string(), json!(completed));
    }

    let mut data = Map::new();
    data.insert("parentID".to_string(), json!("msg_fixture_parent"));
    data.insert("role".to_string(), json!("assistant"));
    data.insert("mode".to_string(), json!(record.agent));
    data.insert("agent".to_string(), json!(record.agent));
    data.insert(
        "path".to_string(),
        json!({"cwd": "/fixture/project", "root": "/fixture/project"}),
    );
    data.insert("cost".to_string(), json!(record.source_cost));
    data.insert("tokens".to_string(), Value::Object(tokens));
    match record.model_shape {
        ModelShape::Flat => {
            data.insert("modelID".to_string(), json!(record.model_id));
            data.insert("providerID".to_string(), json!(record.provider_id));
        }
        ModelShape::Nested => {
            data.insert(
                "model".to_string(),
                json!({"providerID": record.provider_id, "modelID": record.model_id}),
            );
        }
    }
    if let Some(variant) = &record.variant {
        data.insert("variant".to_string(), json!(variant));
    }
    data.insert("time".to_string(), Value::Object(time));
    if record.time_completed.is_some() {
        data.insert("finish".to_string(), json!("stop"));
    }
    Value::Object(data)
}

fn write_database(out_dir: &Path, records: &[RecordSpec], lagged_stale: &RecordSpec) -> Result<()> {
    let db_path = out_dir.join(DATABASE_FILE);
    let mut connection = Connection::open(&db_path)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if mode != "wal" {
        return Err(FixtureError::Validation(format!(
            "journal_mode expected wal, actual {mode}"
        )));
    }
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute_batch(SESSION_DDL)?;
    connection.execute_batch(MESSAGE_DDL)?;
    connection.execute_batch(MESSAGE_INDEX_DDL)?;
    connection.execute_batch(PART_DDL)?;

    let first_phase = connection.transaction()?;
    for session_id in [PRIMARY_SESSION_ID, BUCKET_SESSION_ID, DST_SESSION_ID] {
        first_phase.execute(
            "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated, time_archived, workspace_id, agent, model, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, 9876.5, 999999999, 888888888, 777777777, 666666666, 555555555)",
            params![
                session_id,
                "project_fixture",
                session_id,
                "/fixture/project",
                "Fixture session",
                "1.0.0-fixture",
                DATABASE_START,
                COVERAGE_CUTOFF,
                "workspace_fixture",
                "session-preaggregate-must-not-be-read",
                "session-model-must-not-be-read"
            ],
        )?;
    }
    for record in records {
        if record.id == lagged_stale.id {
            insert_message(&first_phase, lagged_stale)?;
        } else {
            insert_message(&first_phase, record)?;
        }
    }
    insert_skipped_rows(&first_phase)?;
    first_phase.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            "prt_fixture_literal",
            "msg_fixture_flat_xhigh_literal",
            PRIMARY_SESSION_ID,
            LITERAL_TOKEN_ROW_CREATED,
            LITERAL_TOKEN_ROW_CREATED + 44_204,
            serde_json::to_string(&json!({"type": "step-finish", "reason": "stop"}))?
        ],
    )?;
    first_phase.commit()?;

    let second_phase = connection.transaction()?;
    second_phase.execute(
        "UPDATE message SET time_updated = ?1, data = ?2 WHERE id = ?3",
        params![
            lagged_stale.time_created + EIGHT_HOURS_MS,
            serde_json::to_string(
                &records
                    .iter()
                    .find(|record| record.id == lagged_stale.id)
                    .expect("lagged final record is present")
                    .data()
            )?,
            lagged_stale.id
        ],
    )?;
    second_phase.commit()?;
    Ok(())
}

fn insert_message(transaction: &rusqlite::Transaction<'_>, record: &RecordSpec) -> Result<()> {
    transaction.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            record.id,
            record.session_id,
            record.time_created,
            record.time_updated,
            serde_json::to_string(&record.data())?
        ],
    )?;
    Ok(())
}

fn insert_skipped_rows(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let user_created = COST_NONZERO_CREATED - 3_600_000;
    let user_data = json!({
        "role": "user",
        "agent": "Atlas - Plan Executor",
        "model": {"providerID": "user-provider", "modelID": "user-model"},
        "time": {"created": user_created},
        "summary": {"title": "fixture user row"}
    });
    transaction.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            "msg_fixture_user_nested_skip",
            PRIMARY_SESSION_ID,
            user_created,
            user_created + 1_000,
            serde_json::to_string(&user_data)?
        ],
    )?;

    let missing_created = COST_NONZERO_CREATED - 1_800_000;
    let missing_tokens = json!({
        "parentID": "msg_fixture_parent",
        "role": "assistant",
        "mode": "Build",
        "agent": "Build",
        "path": {"cwd": "/fixture/project", "root": "/fixture/project"},
        "cost": 0,
        "modelID": "missing-token-model",
        "providerID": "missing-token-provider",
        "time": {"created": missing_created, "completed": missing_created + 900},
        "finish": "stop"
    });
    transaction.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            "msg_fixture_assistant_missing_tokens_skip",
            PRIMARY_SESSION_ID,
            missing_created,
            missing_created + 1_000,
            serde_json::to_string(&missing_tokens)?
        ],
    )?;
    Ok(())
}

fn write_legacy_tree(out_dir: &Path, records: &[RecordSpec]) -> Result<()> {
    for record in records {
        let path = out_dir.join(legacy_path(record));
        let parent = path
            .parent()
            .ok_or_else(|| FixtureError::InvalidOutputPath(path.clone()))?;
        create_dir_all(parent)?;
        let mut bytes = serde_json::to_vec_pretty(&record.data())?;
        bytes.push(b'\n');
        write_file(&path, &bytes)?;
    }
    Ok(())
}

fn legacy_path(record: &RecordSpec) -> PathBuf {
    PathBuf::from("storage")
        .join("message")
        .join(&record.session_id)
        .join(format!("{}.json", record.id))
}

fn write_manifest(out_dir: &Path, manifest: &Manifest) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    write_file(&out_dir.join(MANIFEST_FILE), &bytes)
}

fn daily_expectations(
    records: impl IntoIterator<Item = AggregateRecord>,
) -> Result<BTreeMap<String, BTreeMap<String, DailyExpectation>>> {
    let records = records.into_iter().collect::<Vec<_>>();
    let mut result = BTreeMap::new();
    for timezone_name in ["UTC", "Asia/Shanghai"] {
        let timezone = timezone_name.parse::<chrono_tz::Tz>().map_err(|error| {
            FixtureError::Validation(format!("invalid fixture timezone {timezone_name}: {error}"))
        })?;
        let mut days = BTreeMap::<String, DailyExpectation>::new();
        let mut sessions = BTreeMap::<String, BTreeSet<String>>::new();
        for record in &records {
            let instant = Utc
                .timestamp_millis_opt(record.time_created)
                .single()
                .ok_or_else(|| {
                    FixtureError::Validation(format!(
                        "invalid fixture epoch milliseconds: {}",
                        record.time_created
                    ))
                })?;
            let date = instant
                .with_timezone(&timezone)
                .format("%Y-%m-%d")
                .to_string();
            let day = days
                .entry(date.clone())
                .or_insert_with(|| DailyExpectation {
                    date: date.clone(),
                    ..DailyExpectation::default()
                });
            if record.is_incomplete {
                day.incomplete_excluded_count += 1;
                continue;
            }
            day.tokens.add_assign(record.tokens);
            day.message_count += 1;
            if record.source_cost > 0.0 {
                day.cost.actual_sum += record.source_cost;
            } else {
                day.cost.unavailable_count += 1;
            }
            sessions
                .entry(date)
                .or_default()
                .insert(record.session_id.clone());
        }
        for (date, active_sessions) in sessions {
            days.get_mut(&date)
                .expect("daily fixture bucket exists")
                .active_session_count = active_sessions.len() as u64;
        }
        result.insert(timezone_name.to_string(), days);
    }
    Ok(result)
}

fn validate_special_row(
    connection: &Connection,
    label: &str,
    expected: &SpecialRowExpectation,
) -> Result<()> {
    let actual = connection.query_row(
        "SELECT session_id, time_created, time_updated, data FROM message WHERE id = ?1",
        [&expected.message_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    validate_value(
        &format!("special_rows.{label}.session_id"),
        &expected.session_id,
        &actual.0,
    )?;
    validate_value(
        &format!("special_rows.{label}.time_created"),
        expected.time_created,
        actual.1,
    )?;
    validate_value(
        &format!("special_rows.{label}.time_updated"),
        expected.time_updated,
        actual.2,
    )?;
    let data: Value = serde_json::from_str(&actual.3)?;
    let parsed = parse_fixture_assistant(&data)?;
    validate_value(
        &format!("special_rows.{label}.expected"),
        &expected.expected,
        &parsed,
    )?;
    let completed = data.pointer("/time/completed").and_then(Value::as_i64);
    validate_value(
        &format!("special_rows.{label}.time_completed"),
        expected.time_completed,
        completed,
    )
}

fn validate_lagged_update(manifest: &Manifest) -> Result<()> {
    let lagged = manifest
        .special_rows
        .get("lagged_update")
        .ok_or_else(|| FixtureError::Validation("special_rows.lagged_update is missing".into()))?;
    validate_value(
        "lagged_update.message_id",
        &manifest.lagged_update.message_id,
        &lagged.message_id,
    )?;
    validate_value(
        "lagged_update.time_created",
        manifest.lagged_update.time_created,
        lagged.time_created,
    )?;
    validate_value(
        "lagged_update.pre_update_time_updated",
        manifest.lagged_update.time_created,
        manifest.lagged_update.pre_update_time_updated,
    )?;
    validate_value(
        "lagged_update.post_update_time_updated",
        manifest.lagged_update.time_created + EIGHT_HOURS_MS,
        manifest.lagged_update.post_update_time_updated,
    )?;
    validate_value(
        "lagged_update.final_tokens",
        manifest.lagged_update.final_tokens,
        lagged.expected.tokens,
    )?;
    if !manifest.lagged_update.stale_tokens.all_zero() {
        return Err(FixtureError::Validation(
            "lagged_update.stale_tokens expected all zero".to_string(),
        ));
    }
    Ok(())
}

fn validate_coverage(connection: &Connection, out_dir: &Path, manifest: &Manifest) -> Result<()> {
    let db_start: i64 = connection.query_row(
        "SELECT min(time_created) FROM message WHERE json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.tokens') IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    validate_value("coverage.db_start", manifest.coverage.db_start, db_start)?;
    validate_value(
        "coverage.expected_gap.start",
        manifest.coverage.legacy_end,
        manifest.coverage.expected_gap.start,
    )?;
    validate_value(
        "coverage.expected_gap.end",
        manifest.coverage.db_start,
        manifest.coverage.expected_gap.end,
    )?;
    let zero_rows = query_count(
        connection,
        "SELECT count(*) FROM message WHERE time_created >= ?1 AND time_created < ?2 AND json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.tokens') IS NOT NULL",
        params![
            manifest.coverage.covered_zero_usage.start,
            manifest.coverage.covered_zero_usage.end
        ],
    )?;
    validate_value("coverage.covered_zero_usage.row_count", 0, zero_rows)?;

    let legacy_paths = collect_json_files(&out_dir.join("storage/message"))?;
    let mut max_created = i64::MIN;
    for path in legacy_paths {
        let data: Value = serde_json::from_slice(&read_file(&path)?)?;
        let created = required_i64(&data, "/time/created")?;
        max_created = max_created.max(created);
    }
    validate_value(
        "coverage.legacy_end",
        manifest.coverage.legacy_end,
        max_created + 1,
    )?;
    if manifest.coverage.covered_zero_usage.end - manifest.coverage.covered_zero_usage.start
        != 7 * 24 * 60 * 60 * 1_000
    {
        return Err(FixtureError::Validation(
            "coverage.covered_zero_usage must span exactly seven days".to_string(),
        ));
    }
    if manifest.coverage.live_cutoff <= DST_FALL_SECOND {
        return Err(FixtureError::Validation(
            "coverage.live_cutoff must extend beyond the final live record".to_string(),
        ));
    }
    Ok(())
}

fn validate_legacy_overlap(out_dir: &Path, manifest: &Manifest) -> Result<()> {
    let path = out_dir.join(&manifest.legacy_overlap.legacy_relative_path);
    let data: Value = serde_json::from_slice(&read_file(&path)?)?;
    let parsed = parse_fixture_assistant(&data)?;
    validate_value(
        "legacy_overlap.legacy",
        &manifest.legacy_overlap.legacy,
        &parsed,
    )?;
    let database = manifest
        .special_rows
        .get("legacy_overlap")
        .ok_or_else(|| FixtureError::Validation("special_rows.legacy_overlap is missing".into()))?;
    validate_value(
        "legacy_overlap.message_id",
        &manifest.legacy_overlap.message_id,
        &database.message_id,
    )?;
    validate_value(
        "legacy_overlap.database",
        &manifest.legacy_overlap.database,
        &database.expected,
    )?;
    if manifest.legacy_overlap.database == manifest.legacy_overlap.legacy {
        return Err(FixtureError::Validation(
            "legacy_overlap values must deliberately differ".to_string(),
        ));
    }
    Ok(())
}

fn collect_artifact_records(connection: &Connection, out_dir: &Path) -> Result<ArtifactRecords> {
    let mut all_message_ids = BTreeSet::new();
    let mut eligible_by_id = BTreeMap::new();
    let legacy_paths = collect_json_files(&out_dir.join("storage/message"))?;
    for path in &legacy_paths {
        let message_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FixtureError::Validation(format!("invalid legacy path: {path:?}")))?
            .to_string();
        let session_id = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| FixtureError::Validation(format!("invalid legacy path: {path:?}")))?
            .to_string();
        let data: Value = serde_json::from_slice(&read_file(path)?)?;
        let parsed = parse_fixture_assistant(&data)?;
        let time_created = required_i64(&data, "/time/created")?;
        all_message_ids.insert(message_id.clone());
        eligible_by_id.insert(
            message_id.clone(),
            AggregateRecord {
                session_id,
                time_created,
                tokens: parsed.tokens,
                source_cost: parsed.source_cost,
                is_incomplete: parsed.is_incomplete,
            },
        );
    }

    let mut statement =
        connection.prepare("SELECT id, session_id, time_created, data FROM message ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let message_id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let time_created: i64 = row.get(2)?;
        let data_text: String = row.get(3)?;
        let data: Value = serde_json::from_str(&data_text)?;
        all_message_ids.insert(message_id.clone());
        let eligible = data.get("role").and_then(Value::as_str) == Some("assistant")
            && data.get("tokens").is_some_and(Value::is_object);
        if eligible {
            let parsed = parse_fixture_assistant(&data)?;
            eligible_by_id.insert(
                message_id.clone(),
                AggregateRecord {
                    session_id,
                    time_created,
                    tokens: parsed.tokens,
                    source_cost: parsed.source_cost,
                    is_incomplete: parsed.is_incomplete,
                },
            );
        }
    }
    Ok(ArtifactRecords {
        all_message_ids,
        eligible_by_id,
        legacy_count: legacy_paths.len() as u64,
    })
}

fn parse_fixture_assistant(data: &Value) -> Result<ParseExpectation> {
    let agent_raw = required_str(data, "/agent")?.to_string();
    let provider_id = data
        .get("providerID")
        .and_then(Value::as_str)
        .or_else(|| data.pointer("/model/providerID").and_then(Value::as_str))
        .ok_or_else(|| FixtureError::Validation("missing providerID in fixture row".into()))?
        .to_string();
    let model_id = data
        .get("modelID")
        .and_then(Value::as_str)
        .or_else(|| data.pointer("/model/modelID").and_then(Value::as_str))
        .ok_or_else(|| FixtureError::Validation("missing modelID in fixture row".into()))?
        .to_string();
    let variant = data
        .get("variant")
        .and_then(Value::as_str)
        .map(str::to_string);
    let tokens = TokenExpectation::new(
        required_u64(data, "/tokens/input")?,
        required_u64(data, "/tokens/output")?,
        required_u64(data, "/tokens/reasoning")?,
        required_u64(data, "/tokens/cache/read")?,
        required_u64(data, "/tokens/cache/write")?,
    );
    let source_cost = data
        .get("cost")
        .and_then(Value::as_f64)
        .ok_or_else(|| FixtureError::Validation("missing numeric cost in fixture row".into()))?;
    let completed = data.pointer("/time/completed").and_then(Value::as_i64);
    Ok(ParseExpectation {
        agent_key: normalize_agent_key(&agent_raw),
        agent_raw,
        provider_id,
        model_id,
        variant,
        tokens,
        source_tokens_total: data.pointer("/tokens/total").and_then(Value::as_u64),
        source_cost,
        cost: (source_cost > 0.0).then_some(source_cost),
        cost_source: if source_cost > 0.0 {
            ExpectedCostSource::Actual
        } else {
            ExpectedCostSource::Unavailable
        },
        is_incomplete: tokens.all_zero() && completed.is_none(),
    })
}

fn normalize_agent_key(raw: &str) -> String {
    let mut normalized = String::new();
    let mut pending_separator = false;
    for character in raw.trim().chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if pending_separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character);
            pending_separator = false;
        } else {
            pending_separator = !normalized.is_empty();
        }
    }
    normalized
}

fn required_str<'a>(data: &'a Value, pointer: &str) -> Result<&'a str> {
    data.pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| FixtureError::Validation(format!("missing string at {pointer}")))
}

fn required_i64(data: &Value, pointer: &str) -> Result<i64> {
    data.pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or_else(|| FixtureError::Validation(format!("missing integer at {pointer}")))
}

fn required_u64(data: &Value, pointer: &str) -> Result<u64> {
    data.pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| FixtureError::Validation(format!("missing unsigned integer at {pointer}")))
}

fn query_count<P>(connection: &Connection, sql: &str, params: P) -> Result<u64>
where
    P: rusqlite::Params,
{
    let count: i64 = connection.query_row(sql, params, |row| row.get(0))?;
    u64::try_from(count).map_err(|_| {
        FixtureError::Validation(format!("negative count returned by fixture query: {count}"))
    })
}

fn validate_version(field: &str, expected: u32, actual: u32) -> Result<()> {
    validate_value(field, expected, actual)
}

fn validate_value<T>(field: &str, expected: T, actual: T) -> Result<()>
where
    T: std::fmt::Debug + PartialEq,
{
    if expected != actual {
        return Err(FixtureError::Validation(format!(
            "{field} mismatch: expected {expected:?}, actual {actual:?}"
        )));
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.display());
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn install_staged(out_dir: &Path, stage: &Path, backup: &Path) -> Result<()> {
    if out_dir.exists() {
        rename(out_dir, backup)?;
        if let Err(error) = rename(stage, out_dir) {
            let _ = fs::rename(backup, out_dir);
            return Err(error);
        }
        remove_path_if_exists(backup)?;
    } else {
        rename(stage, out_dir)?;
    }
    Ok(())
}

fn collect_json_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in read_dir(path)? {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let entry_path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|source| io_error(&entry_path, source))?;
        if metadata.is_dir() {
            files.extend(collect_json_files(&entry_path)?);
        } else if entry_path.extension().and_then(|value| value.to_str()) == Some("json") {
            files.push(entry_path);
        }
    }
    files.sort();
    Ok(files)
}

fn tree_size(path: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in read_dir(path)? {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let entry_path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|source| io_error(&entry_path, source))?;
        if metadata.is_dir() {
            size += tree_size(&entry_path)?;
        } else {
            size += metadata.len();
        }
    }
    Ok(size)
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|source| io_error(path, source))
        }
        Ok(_) => fs::remove_file(path).map_err(|source| io_error(path, source)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|source| io_error(path, source))
}

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))
}

fn read_dir(path: &Path) -> Result<fs::ReadDir> {
    fs::read_dir(path).map_err(|source| io_error(path, source))
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|source| io_error(path, source))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|source| io_error(path, source))
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|source| io_error(from, source))
}

fn io_error(path: &Path, source: std::io::Error) -> FixtureError {
    FixtureError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use rusqlite::{Connection, OpenFlags};

    use super::{generate, FixtureGuard, Manifest};

    const MAX_FIXTURE_BYTES: u64 = 5 * 1024 * 1024;
    const EXPECTED_MESSAGE_DDL: &str = "CREATE TABLE `message` (\n  `id` text PRIMARY KEY,\n  `session_id` text NOT NULL,\n  `time_created` integer NOT NULL,\n  `time_updated` integer NOT NULL,\n  `data` text NOT NULL,\n  CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`)\n    REFERENCES `session`(`id`) ON DELETE CASCADE\n)";
    const EXPECTED_MESSAGE_INDEX_DDL: &str = "CREATE INDEX `message_session_time_created_id_idx` ON `message` (`session_id`,`time_created`,`id`)";

    #[test]
    fn fixture_gen_artifacts_self_validate() {
        let root = tempfile::tempdir().expect("create fixture tempdir");
        let out_dir = root.path().join("fixture");

        let manifest = generate(&out_dir).expect("generate fixture");
        manifest.validate(&out_dir).expect("self-validate fixture");

        assert!(out_dir.join("opencode.db").is_file());
        assert!(out_dir.join("opencode.db-wal").is_file());
        assert!(out_dir.join("manifest.json").is_file());
        assert!(out_dir.join("storage/message").is_dir());
        assert!(tree_size(&out_dir) < MAX_FIXTURE_BYTES);

        let persisted = Manifest::read_from(&out_dir).expect("read persisted manifest");
        assert_eq!(manifest, persisted);

        let connection = Connection::open(out_dir.join("opencode.db")).expect("open fixture db");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal mode");
        assert_eq!(journal_mode, "wal");

        let message_ddl: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'message'",
                [],
                |row| row.get(0),
            )
            .expect("read message DDL");
        assert_eq!(message_ddl, EXPECTED_MESSAGE_DDL);

        let index_ddl: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'message_session_time_created_id_idx'",
                [],
                |row| row.get(0),
            )
            .expect("read message index DDL");
        assert_eq!(index_ddl, EXPECTED_MESSAGE_INDEX_DDL);

        let total_rows: i64 = connection
            .query_row("SELECT count(*) FROM message", [], |row| row.get(0))
            .expect("count messages");
        assert_eq!(total_rows as u64, manifest.total_message_rows);

        let eligible_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM message WHERE json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.tokens') IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count eligible messages");
        assert_eq!(eligible_rows as u64, manifest.eligible_assistant_count);

        let same_timestamp_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM message WHERE time_updated = ?1",
                [manifest.same_timestamp_bucket.time_updated],
                |row| row.get(0),
            )
            .expect("count same-timestamp rows");
        assert_eq!(same_timestamp_rows, 1_001);
        assert_eq!(
            same_timestamp_rows as u64,
            manifest.same_timestamp_bucket.count
        );

        for label in [
            "flat_with_variant",
            "nested_assistant",
            "no_variant",
            "interrupted_zero_token",
            "lagged_update",
            "cost_nonzero",
            "cost_zero_same_day",
            "missing_tokens_total",
            "legacy_overlap",
        ] {
            assert!(
                manifest.special_rows.contains_key(label),
                "manifest is missing special row {label}"
            );
        }

        let literal = &manifest.special_rows["flat_with_variant"].expected;
        assert_eq!(literal.tokens.total_input, 53_865);
        assert_eq!(literal.tokens.output, 227);
        assert_eq!(literal.tokens.reasoning, 91);
        assert_eq!(literal.source_tokens_total, Some(7_549));
        assert_ne!(
            literal.source_tokens_total,
            Some(literal.tokens.total_input)
        );

        for timezone in ["UTC", "Asia/Shanghai"] {
            let mixed_cost_day = &manifest.daily_expectations[timezone]["2026-07-29"];
            assert_eq!(mixed_cost_day.message_count, 2);
            assert_eq!(mixed_cost_day.cost.actual_sum, 0.0102);
            assert_eq!(mixed_cost_day.cost.unavailable_count, 1);
        }
    }

    #[test]
    fn fixture_gen_is_deterministic() {
        let root = tempfile::tempdir().expect("create fixture tempdir");
        let first = root.path().join("first");
        let second = root.path().join("second");

        generate(&first).expect("generate first fixture");
        generate(&second).expect("generate second fixture");

        assert_eq!(
            fs::read(first.join("manifest.json")).expect("read first manifest"),
            fs::read(second.join("manifest.json")).expect("read second manifest")
        );
        assert_eq!(database_rows(&first), database_rows(&second));
        assert_eq!(legacy_files(&first), legacy_files(&second));
    }

    #[test]
    fn fixture_gen_guard_keeps_uncheckpointed_wal_visible() {
        let root = tempfile::tempdir().expect("create fixture tempdir");
        let out_dir = root.path().join("fixture");
        generate(&out_dir).expect("generate fixture");

        let guard = FixtureGuard::new(&out_dir).expect("open fixture guard");
        let wal = fs::read(out_dir.join("opencode.db-wal")).expect("read live WAL");
        assert!(wal.len() > 32, "WAL must contain at least one frame");
        assert!(
            matches!(
                &wal[..4],
                [0x37, 0x7f, 0x06, 0x82] | [0x37, 0x7f, 0x06, 0x83]
            ),
            "WAL header has an unexpected magic number"
        );

        let uri = format!("file:{}?mode=ro", guard.db_path().display());
        let reader = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open mode=ro reader");
        let visible: i64 = reader
            .query_row(
                "SELECT count(*) FROM message WHERE id = ?1",
                [guard.message_id()],
                |row| row.get(0),
            )
            .expect("query uncheckpointed row");
        assert_eq!(visible, 1);
    }

    #[test]
    fn fixture_gen_cleanly_replaces_stale_directory() {
        let root = tempfile::tempdir().expect("create fixture tempdir");
        let out_dir = root.path().join("fixture");
        fs::create_dir_all(out_dir.join("storage/message/stale")).expect("create stale tree");
        fs::write(out_dir.join("storage/message/stale/poison.json"), b"poison")
            .expect("write stale file");
        fs::write(out_dir.join("manifest.json"), b"not-json").expect("write stale manifest");

        let manifest = generate(&out_dir).expect("replace stale fixture");

        assert!(!out_dir.join("storage/message/stale/poison.json").exists());
        assert_eq!(
            Manifest::read_from(&out_dir).expect("read replacement manifest"),
            manifest
        );
        manifest
            .validate(&out_dir)
            .expect("validate replacement fixture");
    }

    #[test]
    fn fixture_gen_validation_rejects_manifest_mismatch() {
        let root = tempfile::tempdir().expect("create fixture tempdir");
        let out_dir = root.path().join("fixture");
        let mut manifest = generate(&out_dir).expect("generate fixture");
        manifest.eligible_assistant_count += 1;

        let error = manifest
            .validate(&out_dir)
            .expect_err("mismatched manifest must fail validation");
        assert!(
            error.to_string().contains("eligible_assistant_count"),
            "validation error must name the mismatched field: {error}"
        );
    }

    fn tree_size(path: &Path) -> u64 {
        fs::read_dir(path)
            .expect("read fixture directory")
            .map(|entry| {
                let entry = entry.expect("read fixture entry");
                let metadata = entry.metadata().expect("read fixture metadata");
                if metadata.is_dir() {
                    tree_size(&entry.path())
                } else {
                    metadata.len()
                }
            })
            .sum()
    }

    fn database_rows(out_dir: &Path) -> Vec<(String, String, i64, i64, String)> {
        let connection = Connection::open(out_dir.join("opencode.db")).expect("open fixture db");
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, time_created, time_updated, data FROM message ORDER BY id",
            )
            .expect("prepare deterministic row query");
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .expect("query deterministic rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect deterministic rows")
    }

    fn legacy_files(out_dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let root = out_dir.join("storage/message");
        let mut files = collect_files(&root)
            .into_iter()
            .map(|path| {
                let relative = path
                    .strip_prefix(out_dir)
                    .expect("strip fixture prefix")
                    .to_path_buf();
                let bytes = fs::read(&path).expect("read legacy fixture");
                (relative, bytes)
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    fn collect_files(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in fs::read_dir(path).expect("read legacy directory") {
            let entry = entry.expect("read legacy entry");
            let metadata = entry.metadata().expect("read legacy metadata");
            if metadata.is_dir() {
                files.extend(collect_files(&entry.path()));
            } else {
                files.push(entry.path());
            }
        }
        files
    }
}
