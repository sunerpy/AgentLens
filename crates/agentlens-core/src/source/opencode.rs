//! OpenCode SQLite 解析与增量扫描（todo 5）。
//!
//! 本模块以 `mode=ro` + `PRAGMA query_only=ON` + `busy_timeout=5000` 打开源库
//! （本地与远端 collector 统一此方式，SQLite 自行管理的 `-shm` / `-wal` 是唯一允许的副作用）。
//!
//! 扫描算法：真实库在 `time_updated` 上无索引，禁止在 SQL 端 ORDER BY / LIMIT 分页；
//! 每轮为单次流式全扫
//! `SELECT id, session_id, time_created, time_updated, data FROM message WHERE time_updated >= ?`
//! （watermark 减 24h 重叠窗口），应用层每 1000 行攒批交给 `crate::ingest`，
//! 重叠窗口重读的行由 upsert 幂等吸收；watermark 仅在完整扫到 EOF 后推进为本轮观察到的
//! `max(time_updated)`，中断 / 取消 / 出错则不推进。
//!
//! WAL / SHM 因权限不可读不是可降级场景，返回独立的 `WalUnreadable` 错误与权限 remediation，
//! 不尝试 snapshot。
//!
//! `data` JSON 解析：仅统计 assistant 行，且 model 字段兼容扁平
//! `modelID` / `providerID` / `variant` 与嵌套 `model:{}` 两种形态（防御式）；
//! `role != "assistant"` 或缺 `tokens` 键的行计入 skipped；派生 `is_incomplete`
//! （tokens 全零且缺 `time.completed`）；`cost > 0` → actual，否则 unavailable；
//! lossy 解析（未知键忽略、数值容忍、缺 agent 回退 `"unknown"`）。
//! `SQLITE_BUSY` / 库缺失 / VACUUM 中经指数退避 3 次后本轮 skip，并返回带 `last_success` 的状态。
//!
//! 数据目录发现顺序：`OPENCODE_DATA_DIR` → `$XDG_DATA_HOME/opencode` → `~/.local/share/opencode`。

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::ffi::ErrorCode;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use thiserror::Error;

use crate::archive::{normalize_agent_key, CostSource, NormalizedUsageRecord, Origin};

/// Inclusive overlap applied before every stored watermark.
pub const OVERLAP_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
/// Number of normalized records delivered to a sink at once.
pub const DEFAULT_BATCH_SIZE: usize = 1_000;

const SOURCE_NAME: &str = "opencode";
const DATABASE_FILE: &str = "opencode.db";
const BUSY_TIMEOUT_MS: u64 = 5_000;
const MAX_BUSY_RETRIES: u8 = 3;
const INITIAL_BACKOFF_MS: u64 = 100;
const SCAN_SQL: &str =
    "SELECT id, session_id, time_created, time_updated, data FROM message WHERE time_updated >= ?1";

/// Result type returned by OpenCode discovery, opening, parsing, and scanning operations.
pub type Result<T> = std::result::Result<T, OpenCodeError>;

/// Errors that prevent a read-only OpenCode scan from producing a trustworthy result.
#[derive(Debug, Error)]
pub enum OpenCodeError {
    /// No candidate database exists in the configured discovery chain.
    #[error(
        "OpenCode database was not found; probed paths: {}",
        display_paths(.probed_paths)
    )]
    DatabaseNotFound {
        /// Exact database paths checked in precedence order.
        probed_paths: Vec<PathBuf>,
    },
    /// The database exists, but a WAL-mode read cannot safely access its sidecars.
    #[error(
        "OpenCode WAL/SHM is unreadable for {database_path}; affected paths: {sidecars:?}. Grant read access with chmod or add the AgentLens user to the owning group; snapshot/VACUUM is not a fallback for unreadable WAL/SHM"
    )]
    WalUnreadable {
        /// Source database whose WAL state cannot be read safely.
        database_path: PathBuf,
        /// Missing or permission-denied sidecars relevant to remediation.
        sidecars: Vec<PathBuf>,
    },
    /// SQLite could not open the source with read-only URI flags.
    #[error("cannot open OpenCode database read-only at {path}: {source}")]
    Open {
        /// Source path passed to SQLite.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// Required connection hardening could not be applied.
    #[error("cannot configure read-only OpenCode connection at {path}: {source}")]
    Configure {
        /// Source database being configured.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// A connection reported that `PRAGMA query_only` is disabled.
    #[error("OpenCode source connection is not query-only; refusing to scan")]
    QueryOnlyDisabled,
    /// A caller supplied an impossible batch size.
    #[error("OpenCode scan batch_size must be greater than zero")]
    InvalidBatchSize,
    /// A non-retryable source failure stopped a scan before EOF.
    #[error("OpenCode scan stopped before EOF: {source}")]
    ScanFailed {
        /// Partial counters for diagnostics; its watermark is always absent.
        partial: Box<ScanResult>,
        /// Source streaming failure.
        source: StreamError,
    },
}

struct DisplayPaths<'a>(&'a [PathBuf]);

impl fmt::Display for DisplayPaths<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, path) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{}", path.display())?;
        }
        formatter.write_str("]")
    }
}

fn display_paths(paths: &[PathBuf]) -> DisplayPaths<'_> {
    DisplayPaths(paths)
}

/// One row read from the source `message` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMessageRow {
    /// `message.id` table column.
    pub message_id: String,
    /// `message.session_id` table column.
    pub session_id: String,
    /// `message.time_created` UTC epoch milliseconds.
    pub time_created: i64,
    /// `message.time_updated` UTC epoch milliseconds.
    pub time_updated: i64,
    /// Raw `message.data` JSON text.
    pub data: String,
}

/// Failure emitted while a [`SourceConnection`] streams rows.
#[derive(Debug, Error)]
pub enum StreamError {
    /// SQLite failed while preparing, stepping, or decoding the source query.
    #[error("SQLite source error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The caller-supplied sink requested an orderly interruption.
    #[error("scan interrupted by sink: {0}")]
    Interrupted(String),
}

impl StreamError {
    fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                )
        )
    }
}

/// Injectable source seam used by production SQLite and deterministic failure fakes.
pub trait SourceConnection {
    /// Returns whether SQLite currently enforces `PRAGMA query_only`.
    fn query_only(&self) -> rusqlite::Result<bool>;

    /// Streams one unpaginated query and invokes `visitor` once per source row.
    fn stream_messages(
        &mut self,
        window_start: i64,
        visitor: &mut dyn FnMut(SourceMessageRow) -> std::result::Result<(), StreamError>,
    ) -> std::result::Result<(), StreamError>;
}

/// Production [`SourceConnection`] backed by a hardened `rusqlite::Connection`.
#[derive(Debug)]
pub struct SqliteSourceConnection {
    path: PathBuf,
    connection: Connection,
}

impl SqliteSourceConnection {
    /// Opens a source with `mode=ro`, disables checkpoint-on-close, sets a five-second busy timeout,
    /// and enables `PRAGMA query_only` before any scan statement is prepared.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            return Err(OpenCodeError::DatabaseNotFound {
                probed_paths: vec![path],
            });
        }
        validate_wal_access(&path)?;

        let uri = format!("file:{}?mode=ro", path.display());
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|source| OpenCodeError::Open {
            path: path.clone(),
            source,
        })?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(|source| OpenCodeError::Configure {
                path: path.clone(),
                source,
            })?;
        connection
            .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(|source| OpenCodeError::Configure {
                path: path.clone(),
                source,
            })?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(|source| OpenCodeError::Configure {
                path: path.clone(),
                source,
            })?;
        let query_only = connection
            .pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0))
            .map_err(|source| OpenCodeError::Configure {
                path: path.clone(),
                source,
            })?;
        if !query_only {
            return Err(OpenCodeError::QueryOnlyDisabled);
        }
        connection
            .query_row("SELECT count(*) >= 0 FROM sqlite_schema", [], |row| {
                row.get::<_, bool>(0)
            })
            .map_err(|source| map_source_open_error(&path, source))?;

        Ok(Self { path, connection })
    }

    /// Returns the source database path represented by this connection.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SourceConnection for SqliteSourceConnection {
    fn query_only(&self) -> rusqlite::Result<bool> {
        self.connection
            .pragma_query_value(None, "query_only", |row| row.get(0))
    }

    fn stream_messages(
        &mut self,
        window_start: i64,
        visitor: &mut dyn FnMut(SourceMessageRow) -> std::result::Result<(), StreamError>,
    ) -> std::result::Result<(), StreamError> {
        let mut statement = self.connection.prepare(SCAN_SQL)?;
        let mut rows = statement.query([window_start])?;
        while let Some(row) = rows.next()? {
            visitor(SourceMessageRow {
                message_id: row.get(0)?,
                session_id: row.get(1)?,
                time_created: row.get(2)?,
                time_updated: row.get(3)?,
                data: row.get(4)?,
            })?;
        }
        Ok(())
    }
}

/// Stable parse context supplied by the host/source orchestrator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseContext {
    /// Stable machine-derived host identifier.
    pub host_id: String,
    /// Live, backup, or legacy provenance for conflict priority.
    pub origin: Origin,
}

impl ParseContext {
    /// Creates a parser context for one host and source origin.
    pub fn new(host_id: impl Into<String>, origin: Origin) -> Self {
        Self {
            host_id: host_id.into(),
            origin,
        }
    }
}

/// Reason one source row did not produce a normalized assistant usage record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The JSON role is absent or is not `assistant`.
    NonAssistant,
    /// An assistant row has no `tokens` key.
    MissingTokens,
    /// `message.data` is not valid JSON.
    MalformedJson,
    /// The `tokens` key exists but is not an object.
    InvalidTokens,
}

/// Lossy parse result: eligible rows are returned by value, while skipped rows carry their reason.
pub type ParseOutcome = std::result::Result<NormalizedUsageRecord, SkipReason>;

/// Counts each supported skip category without stopping the scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkippedBreakdown {
    /// Rows whose role is not `assistant`.
    pub non_assistant: u64,
    /// Assistant rows with no `tokens` key.
    pub missing_tokens: u64,
    /// Rows whose `data` text is invalid JSON.
    pub malformed_json: u64,
    /// Assistant rows whose `tokens` value is not an object.
    pub invalid_tokens: u64,
}

impl SkippedBreakdown {
    fn increment(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::NonAssistant => self.non_assistant += 1,
            SkipReason::MissingTokens => self.missing_tokens += 1,
            SkipReason::MalformedJson => self.malformed_json += 1,
            SkipReason::InvalidTokens => self.invalid_tokens += 1,
        }
    }
}

/// Immutable inputs for one overlap-window scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRequest {
    /// Stable host identifier copied into normalized records.
    pub host_id: String,
    /// Last successfully committed cursor, or `None` for a complete first scan.
    pub watermark: Option<i64>,
    /// Record provenance; normal live scans use [`Origin::Live`].
    pub origin: Origin,
    /// Previous successful refresh timestamp retained when this round is skipped.
    pub last_success_utc: Option<i64>,
    /// Rust-side delivery batch size. SQL pagination is never used.
    pub batch_size: usize,
}

impl ScanRequest {
    /// Creates a normal live-source request using the fixed 1000-record batch size.
    pub fn live(host_id: impl Into<String>, watermark: Option<i64>) -> Self {
        Self {
            host_id: host_id.into(),
            watermark,
            origin: Origin::Live,
            last_success_utc: None,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    /// Returns the inclusive query boundary after applying the 24-hour overlap.
    pub fn window_start(&self) -> i64 {
        self.watermark
            .map_or(i64::MIN, |value| value.saturating_sub(OVERLAP_WINDOW_MS))
    }
}

/// Why a scan returned without reaching EOF but did not raise a fatal source error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanSkipReason {
    /// SQLite remained busy or locked after three exponential backoffs.
    Busy,
    /// The sink cancelled or rejected a delivered batch.
    Interrupted(String),
}

/// Observable result of one scan round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// Number of records actually handed to the sink, including replayed retry attempts.
    pub delivered_records: u64,
    /// Number of sink calls, including replayed retry attempts.
    pub delivered_batches: u64,
    /// Eligible rows observed in the final scan attempt.
    pub eligible_count: u64,
    /// Skipped rows observed in the final scan attempt.
    pub skipped_count: u64,
    /// Final-attempt skip categories.
    pub skipped_breakdown: SkippedBreakdown,
    /// Maximum source update time, present only after the query reaches EOF.
    pub observed_max_time_updated: Option<i64>,
    /// The sole signal that todo 6 may use to advance its committed watermark.
    pub reached_eof: bool,
    /// Number of exponential backoffs performed for SQLite busy/locked errors.
    pub busy_retry_count: u8,
    /// Previous successful refresh retained for skipped/error status reporting.
    pub last_success_utc: Option<i64>,
    /// Recoverable reason this round did not reach EOF.
    pub skip_reason: Option<ScanSkipReason>,
}

impl ScanResult {
    fn empty(last_success_utc: Option<i64>) -> Self {
        Self {
            delivered_records: 0,
            delivered_batches: 0,
            eligible_count: 0,
            skipped_count: 0,
            skipped_breakdown: SkippedBreakdown::default(),
            observed_max_time_updated: None,
            reached_eof: false,
            busy_retry_count: 0,
            last_success_utc,
            skip_reason: None,
        }
    }
}

/// Error returned by a caller-supplied record sink to cancel a scan safely.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct SinkError {
    message: String,
}

impl SinkError {
    /// Creates a sink error whose message is retained in [`ScanSkipReason::Interrupted`].
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Parses one source row into the shared archive record contract.
pub fn parse_message(row: SourceMessageRow, context: &ParseContext) -> ParseOutcome {
    let data: Value = match serde_json::from_str(&row.data) {
        Ok(value) => value,
        Err(_) => return Err(SkipReason::MalformedJson),
    };
    if data.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(SkipReason::NonAssistant);
    }
    let Some(tokens) = data.get("tokens") else {
        return Err(SkipReason::MissingTokens);
    };
    if !tokens.is_object() {
        return Err(SkipReason::InvalidTokens);
    }

    let tok_input = lossy_u64(tokens.get("input"));
    let tok_output = lossy_u64(tokens.get("output"));
    let tok_reasoning = lossy_u64(tokens.get("reasoning"));
    let tok_cache_read = lossy_u64(tokens.pointer("/cache/read"));
    let tok_cache_write = lossy_u64(tokens.pointer("/cache/write"));
    let time_completed_utc = lossy_i64(data.pointer("/time/completed"));
    let agent_raw = data
        .get("agent")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    let provider_id = string_at(&data, "providerID", "/model/providerID")
        .unwrap_or_else(|| "unknown".to_string());
    let model_id =
        string_at(&data, "modelID", "/model/modelID").unwrap_or_else(|| "unknown".to_string());
    let variant = data
        .get("variant")
        .and_then(value_to_string)
        .or_else(|| data.pointer("/model/variant").and_then(value_to_string));
    let source_cost = lossy_f64(data.get("cost")).filter(|value| *value > 0.0);
    let project_dir = data
        .pointer("/path/cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let is_incomplete = tok_input == 0
        && tok_output == 0
        && tok_reasoning == 0
        && tok_cache_read == 0
        && tok_cache_write == 0
        && time_completed_utc.is_none();

    Ok(NormalizedUsageRecord {
        host_id: context.host_id.clone(),
        source: SOURCE_NAME.to_string(),
        message_id: row.message_id,
        session_id: row.session_id,
        time_created_utc: row.time_created,
        time_completed_utc,
        source_time_updated: row.time_updated,
        origin: context.origin,
        origin_priority: context.origin.priority(),
        agent_key: normalize_agent_key(&agent_raw),
        agent_raw,
        provider_id,
        model_id,
        variant,
        tok_input,
        tok_output,
        tok_reasoning,
        tok_cache_read,
        tok_cache_write,
        cost: source_cost,
        cost_source: if source_cost.is_some() {
            CostSource::Actual
        } else {
            CostSource::Unavailable
        },
        is_incomplete,
        project_dir,
    })
}

/// Opens an explicit database read-only and streams normalized records to `sink`.
pub fn scan_database<F>(
    path: impl AsRef<Path>,
    request: &ScanRequest,
    sink: F,
) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    let mut connection = SqliteSourceConnection::open(path)?;
    scan_connection(&mut connection, request, sink)
}

/// Scans an injected source, allowing BUSY and mid-stream failure behavior to be tested reliably.
pub fn scan_connection<C, F>(
    connection: &mut C,
    request: &ScanRequest,
    sink: F,
) -> Result<ScanResult>
where
    C: SourceConnection,
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    scan_connection_with_backoff(connection, request, sink, |delay_ms| {
        thread::sleep(Duration::from_millis(delay_ms));
    })
}

fn scan_connection_with_backoff<C, F, B>(
    connection: &mut C,
    request: &ScanRequest,
    mut sink: F,
    mut backoff: B,
) -> Result<ScanResult>
where
    C: SourceConnection,
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
    B: FnMut(u64),
{
    if request.batch_size == 0 {
        return Err(OpenCodeError::InvalidBatchSize);
    }
    if !connection
        .query_only()
        .map_err(|source| OpenCodeError::ScanFailed {
            partial: Box::new(ScanResult::empty(request.last_success_utc)),
            source: StreamError::Sqlite(source),
        })?
    {
        return Err(OpenCodeError::QueryOnlyDisabled);
    }

    let context = ParseContext::new(&request.host_id, request.origin);
    let mut cumulative_records = 0_u64;
    let mut cumulative_batches = 0_u64;
    let mut busy_retry_count = 0_u8;

    loop {
        let mut result = ScanResult::empty(request.last_success_utc);
        let mut batch = Vec::with_capacity(request.batch_size);
        let stream_result = connection.stream_messages(request.window_start(), &mut |row| {
            result.observed_max_time_updated = Some(
                result
                    .observed_max_time_updated
                    .map_or(row.time_updated, |current| current.max(row.time_updated)),
            );
            match parse_message(row, &context) {
                Ok(record) => {
                    result.eligible_count += 1;
                    batch.push(record);
                    if batch.len() == request.batch_size {
                        sink(&batch)
                            .map_err(|error| StreamError::Interrupted(error.to_string()))?;
                        cumulative_records += batch.len() as u64;
                        cumulative_batches += 1;
                        batch.clear();
                    }
                }
                Err(reason) => {
                    result.skipped_count += 1;
                    result.skipped_breakdown.increment(reason);
                }
            }
            Ok(())
        });

        match stream_result {
            Ok(()) => {
                if !batch.is_empty() {
                    if let Err(error) = sink(&batch) {
                        result.observed_max_time_updated = None;
                        result.skip_reason = Some(ScanSkipReason::Interrupted(error.to_string()));
                        result.delivered_records = cumulative_records;
                        result.delivered_batches = cumulative_batches;
                        result.busy_retry_count = busy_retry_count;
                        return Ok(result);
                    }
                    cumulative_records += batch.len() as u64;
                    cumulative_batches += 1;
                }
                result.delivered_records = cumulative_records;
                result.delivered_batches = cumulative_batches;
                result.busy_retry_count = busy_retry_count;
                result.reached_eof = true;
                return Ok(result);
            }
            Err(StreamError::Interrupted(message)) => {
                result.observed_max_time_updated = None;
                result.delivered_records = cumulative_records;
                result.delivered_batches = cumulative_batches;
                result.busy_retry_count = busy_retry_count;
                result.skip_reason = Some(ScanSkipReason::Interrupted(message));
                return Ok(result);
            }
            Err(error) if error.is_busy() && busy_retry_count < MAX_BUSY_RETRIES => {
                let delay = INITIAL_BACKOFF_MS << busy_retry_count;
                backoff(delay);
                busy_retry_count += 1;
            }
            Err(error) if error.is_busy() => {
                result.observed_max_time_updated = None;
                result.delivered_records = cumulative_records;
                result.delivered_batches = cumulative_batches;
                result.busy_retry_count = busy_retry_count;
                result.skip_reason = Some(ScanSkipReason::Busy);
                return Ok(result);
            }
            Err(source) => {
                result.observed_max_time_updated = None;
                result.delivered_records = cumulative_records;
                result.delivered_batches = cumulative_batches;
                result.busy_retry_count = busy_retry_count;
                return Err(OpenCodeError::ScanFailed {
                    partial: Box::new(result),
                    source,
                });
            }
        }
    }
}

/// Resolves the first existing source database in the documented environment precedence order.
pub fn discover_database_path() -> Result<PathBuf> {
    let explicit = env::var_os("OPENCODE_DATA_DIR").map(PathBuf::from);
    let xdg = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let home = dirs::home_dir();
    discover_database_path_from(explicit.as_deref(), xdg.as_deref(), home.as_deref())
}

fn discover_database_path_from(
    explicit_data_dir: Option<&Path>,
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf> {
    let mut probed_paths = Vec::new();
    if let Some(directory) = explicit_data_dir {
        probed_paths.push(directory.join(DATABASE_FILE));
    }
    if let Some(directory) = xdg_data_home {
        probed_paths.push(directory.join("opencode").join(DATABASE_FILE));
    }
    if let Some(directory) = home {
        probed_paths.push(
            directory
                .join(".local")
                .join("share")
                .join("opencode")
                .join(DATABASE_FILE),
        );
    }
    if let Some(path) = probed_paths.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    Err(OpenCodeError::DatabaseNotFound { probed_paths })
}

fn validate_wal_access(database_path: &Path) -> Result<()> {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let wal = sidecar_path(database_path, "-wal");
    let shm = sidecar_path(database_path, "-shm");
    let mut affected = [&wal, &shm]
        .into_iter()
        .filter(|path| path.exists() && !has_read_bit(path))
        .cloned()
        .collect::<Vec<_>>();

    if !directory_has_write_bit(parent) {
        for sidecar in [&wal, &shm] {
            if !sidecar.is_file() && !affected.contains(sidecar) {
                affected.push(sidecar.clone());
            }
        }
    }
    if affected.is_empty() {
        Ok(())
    } else {
        Err(OpenCodeError::WalUnreadable {
            database_path: database_path.to_path_buf(),
            sidecars: affected,
        })
    }
}

fn map_source_open_error(path: &Path, source: rusqlite::Error) -> OpenCodeError {
    if matches!(
        source.sqlite_error_code(),
        Some(ErrorCode::PermissionDenied | ErrorCode::CannotOpen | ErrorCode::ReadOnly)
    ) {
        let sidecars = [sidecar_path(path, "-wal"), sidecar_path(path, "-shm")];
        if sidecars.iter().any(|sidecar| sidecar.exists()) {
            return OpenCodeError::WalUnreadable {
                database_path: path.to_path_buf(),
                sidecars: sidecars.to_vec(),
            };
        }
    }
    OpenCodeError::Open {
        path: path.to_path_buf(),
        source,
    }
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(unix)]
fn has_read_bit(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o444 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn has_read_bit(path: &Path) -> bool {
    fs::File::open(path).is_ok()
}

#[cfg(unix)]
fn directory_has_write_bit(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o222 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn directory_has_write_bit(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

fn string_at(data: &Value, flat_key: &str, nested_pointer: &str) -> Option<String> {
    data.get(flat_key)
        .and_then(value_to_string)
        .or_else(|| data.pointer(nested_pointer).and_then(value_to_string))
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn lossy_u64(value: Option<&Value>) -> u64 {
    let Some(value) = value else {
        return 0;
    };
    if let Some(value) = value.as_u64() {
        return value;
    }
    if let Some(value) = value.as_i64() {
        return value.max(0) as u64;
    }
    if let Some(value) = value.as_f64() {
        return finite_nonnegative_u64(value);
    }
    value
        .as_str()
        .and_then(|text| text.parse::<f64>().ok())
        .map_or(0, finite_nonnegative_u64)
}

fn finite_nonnegative_u64(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value.trunc() as u64
    } else {
        0
    }
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
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

fn lossy_f64(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    #[cfg(unix)]
    use std::fs;
    use std::path::{Path, PathBuf};
    #[cfg(target_os = "linux")]
    use std::process::Command;
    #[cfg(target_os = "linux")]
    use std::time::Instant;

    use rusqlite::ffi;
    use serde_json::json;

    use crate::archive::{CostSource, Origin};
    use crate::fixture::{generate, FixtureGuard, Manifest};

    use super::*;

    fn fixture_directory() -> (tempfile::TempDir, PathBuf, Manifest) {
        let temp = tempfile::tempdir().expect("create fixture parent");
        let directory = temp.path().join("fixture");
        let generated = generate(&directory).expect("generate fixture");
        let persisted = Manifest::read_from(&directory).expect("read fixture manifest");
        assert_eq!(generated, persisted);
        (temp, directory, generated)
    }

    fn request(watermark: Option<i64>) -> ScanRequest {
        ScanRequest {
            host_id: "host-opencode-test".to_string(),
            watermark,
            origin: Origin::Live,
            last_success_utc: Some(1_785_000_000_000),
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    fn scan_fixture(
        database: &Path,
        watermark: Option<i64>,
    ) -> (
        ScanResult,
        Vec<crate::archive::NormalizedUsageRecord>,
        Vec<usize>,
    ) {
        let mut records = Vec::new();
        let mut batch_sizes = Vec::new();
        let result = scan_database(database, &request(watermark), |batch| {
            batch_sizes.push(batch.len());
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("scan fixture");
        (result, records, batch_sizes)
    }

    fn find_record<'a>(
        records: &'a [crate::archive::NormalizedUsageRecord],
        message_id: &str,
    ) -> &'a crate::archive::NormalizedUsageRecord {
        records
            .iter()
            .find(|record| record.message_id == message_id)
            .unwrap_or_else(|| panic!("missing normalized record {message_id}"))
    }

    #[test]
    fn opencode_fixture_manifest_tokens_models_counts_and_bucket_are_exact() {
        let (_temp, directory, manifest) = fixture_directory();
        let (result, records, batch_sizes) = scan_fixture(&directory.join("opencode.db"), None);

        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, manifest.eligible_assistant_count);
        assert_eq!(result.skipped_count, manifest.skipped_count);
        assert_eq!(
            result.skipped_breakdown.non_assistant,
            manifest.skipped_breakdown["non_assistant"]
        );
        assert_eq!(
            result.skipped_breakdown.missing_tokens,
            manifest.skipped_breakdown["missing_tokens"]
        );
        assert_eq!(result.delivered_records, records.len() as u64);
        assert_eq!(result.delivered_batches, batch_sizes.len() as u64);
        assert!(batch_sizes.iter().all(|size| *size <= DEFAULT_BATCH_SIZE));
        assert!(batch_sizes.contains(&DEFAULT_BATCH_SIZE));

        let flat_expected = &manifest.special_rows["flat_with_variant"];
        let flat = find_record(&records, &flat_expected.message_id);
        assert_eq!(flat.provider_id, "myopenai");
        assert_eq!(flat.model_id, "us.anthropic.claude-fable-5");
        assert_eq!(flat.variant.as_deref(), Some("xhigh"));
        assert_eq!(
            flat.tok_input + flat.tok_cache_read + flat.tok_cache_write,
            53_865
        );
        assert_eq!(flat.tok_input, 7_322);
        assert_eq!(flat.tok_cache_read, 46_543);
        assert_eq!(flat.tok_cache_write, 0);
        assert_eq!(flat.tok_output, 227);
        assert_eq!(flat.tok_reasoning, 91);

        let nested_expected = &manifest.special_rows["nested_assistant"];
        let nested = find_record(&records, &nested_expected.message_id);
        assert_eq!(nested.provider_id, nested_expected.expected.provider_id);
        assert_eq!(nested.model_id, nested_expected.expected.model_id);
        assert_eq!(nested.variant, nested_expected.expected.variant);

        let no_variant_expected = &manifest.special_rows["no_variant"];
        assert_eq!(
            find_record(&records, &no_variant_expected.message_id).variant,
            None
        );
        let interrupted_expected = &manifest.special_rows["interrupted_zero_token"];
        assert!(find_record(&records, &interrupted_expected.message_id).is_incomplete);

        let bucket_count = records
            .iter()
            .filter(|record| {
                record.source_time_updated == manifest.same_timestamp_bucket.time_updated
            })
            .count() as u64;
        assert_eq!(bucket_count, manifest.same_timestamp_bucket.count);
        println!(
            "manifest_vs_scanner eligible={}={} skipped={}={} batches={batch_sizes:?} same_timestamp={}={bucket_count}",
            manifest.eligible_assistant_count,
            result.eligible_count,
            manifest.skipped_count,
            result.skipped_count,
            manifest.same_timestamp_bucket.count
        );
    }

    #[test]
    fn opencode_read_only_connection_sets_query_only_and_sees_uncheckpointed_wal() {
        let (_temp, directory, _manifest) = fixture_directory();
        let guard = FixtureGuard::new(&directory).expect("create live fixture writer");
        let mut connection = SqliteSourceConnection::open(guard.db_path()).expect("open read-only");
        assert!(connection.query_only().expect("read query_only"));

        let mut found = false;
        connection
            .stream_messages(i64::MIN, &mut |row| {
                found |= row.message_id == guard.message_id();
                Ok(())
            })
            .expect("stream WAL-backed rows");
        assert!(found, "read-only scan must observe committed WAL frames");
        println!("query_only=1 wal_message_visible={found}");
    }

    #[derive(Debug)]
    struct BusyAtBatchConnection {
        calls: usize,
        rows_before_busy: usize,
    }

    impl SourceConnection for BusyAtBatchConnection {
        fn query_only(&self) -> rusqlite::Result<bool> {
            Ok(true)
        }

        fn stream_messages(
            &mut self,
            _window_start: i64,
            visitor: &mut dyn FnMut(SourceMessageRow) -> std::result::Result<(), StreamError>,
        ) -> std::result::Result<(), StreamError> {
            self.calls += 1;
            for index in 0..self.rows_before_busy {
                visitor(valid_source_row(index, index as i64))?;
            }
            Err(StreamError::Sqlite(rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_BUSY),
                Some("injected busy at batch boundary".to_string()),
            )))
        }
    }

    #[test]
    fn opencode_injected_busy_retries_three_times_and_never_reports_watermark() {
        let mut connection = BusyAtBatchConnection {
            calls: 0,
            rows_before_busy: DEFAULT_BATCH_SIZE,
        };
        let mut backoffs = Vec::new();
        let mut delivered_batches = 0_u64;
        let result = scan_connection_with_backoff(
            &mut connection,
            &request(Some(50_000)),
            |_| {
                delivered_batches += 1;
                Ok(())
            },
            |delay| backoffs.push(delay),
        )
        .expect("busy exhaustion is a skipped scan, not a panic");

        assert_eq!(connection.calls, 4);
        assert_eq!(backoffs, vec![100, 200, 400]);
        assert_eq!(result.busy_retry_count, 3);
        assert_eq!(delivered_batches, 4);
        assert!(!result.reached_eof);
        assert_eq!(result.observed_max_time_updated, None);
        assert_eq!(result.last_success_utc, Some(1_785_000_000_000));
        assert_eq!(result.skip_reason, Some(ScanSkipReason::Busy));
        println!(
            "busy_injection attempts={} backoffs_ms={backoffs:?} reached_eof={} observed_max={:?} last_success={:?}",
            connection.calls,
            result.reached_eof,
            result.observed_max_time_updated,
            result.last_success_utc
        );
    }

    #[test]
    fn opencode_cancelled_sink_reports_partial_without_watermark() {
        let (_temp, directory, _manifest) = fixture_directory();
        let mut connection = SqliteSourceConnection::open(directory.join("opencode.db"))
            .expect("open fixture read-only");
        let mut batches = 0_u64;
        let result = scan_connection(&mut connection, &request(None), |_| {
            batches += 1;
            Err(SinkError::new("injected cancellation"))
        })
        .expect("sink cancellation returns an interrupted result");

        assert_eq!(batches, 1);
        assert!(!result.reached_eof);
        assert_eq!(result.observed_max_time_updated, None);
        assert!(matches!(
            result.skip_reason,
            Some(ScanSkipReason::Interrupted(ref message)) if message == "injected cancellation"
        ));
        println!(
            "cancel_resume batches_attempted={batches} reached_eof={} observed_max={:?}",
            result.reached_eof, result.observed_max_time_updated
        );
    }

    #[test]
    fn opencode_same_watermark_is_stable_and_overlap_rereads_lagged_update() {
        let (_temp, directory, manifest) = fixture_directory();
        let database = directory.join("opencode.db");
        let watermark = manifest.lagged_update.post_update_time_updated;
        let (first, first_records, _) = scan_fixture(&database, Some(watermark));
        let (second, second_records, _) = scan_fixture(&database, Some(watermark));
        assert_eq!(first.eligible_count, second.eligible_count);
        assert_eq!(first.skipped_count, second.skipped_count);
        assert_eq!(
            first_records
                .iter()
                .map(|record| &record.message_id)
                .collect::<BTreeSet<_>>(),
            second_records
                .iter()
                .map(|record| &record.message_id)
                .collect::<BTreeSet<_>>()
        );

        let guard = FixtureGuard::new(&directory).expect("open fixture writer");
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
                rusqlite::params![
                    manifest.lagged_update.pre_update_time_updated,
                    manifest.lagged_update.message_id
                ],
            )
            .expect("restore stale lagged state");
        let (_, stale_records, _) = scan_fixture(&database, Some(watermark));
        let stale = find_record(&stale_records, &manifest.lagged_update.message_id);
        assert!(stale.is_incomplete);

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
                    manifest.lagged_update.post_update_time_updated,
                    manifest.lagged_update.final_tokens.input as i64,
                    manifest.lagged_update.final_tokens.output as i64,
                    manifest.lagged_update.final_tokens.reasoning as i64,
                    manifest.lagged_update.final_tokens.cache_read as i64,
                    manifest.lagged_update.final_tokens.cache_write as i64,
                    manifest.lagged_update.post_update_time_updated - 1_000,
                    manifest.lagged_update.message_id
                ],
            )
            .expect("commit final lagged state");
        let (_, final_records, _) = scan_fixture(&database, Some(watermark));
        let final_record = find_record(&final_records, &manifest.lagged_update.message_id);
        assert_eq!(
            final_record.tok_input,
            manifest.lagged_update.final_tokens.input
        );
        assert!(!final_record.is_incomplete);
        println!(
            "stale_state first_eligible={} second_eligible={} stale_incomplete={} final_input={}",
            first.eligible_count,
            second.eligible_count,
            stale.is_incomplete,
            final_record.tok_input
        );
    }

    #[test]
    fn opencode_malformed_input_is_lossy_counted_and_missing_agent_is_unknown() {
        let context = ParseContext::new("host-malformed", Origin::Live);
        let cases = [
            ("{not-json", SkipReason::MalformedJson),
            (
                r#"{"role":"assistant","tokens":"bad"}"#,
                SkipReason::InvalidTokens,
            ),
        ];
        for (data, expected) in cases {
            assert_eq!(
                parse_message(valid_source_row_with_data(data), &context),
                Err(expected)
            );
        }

        let numeric = json!({
            "role": "assistant",
            "tokens": {
                "input": -5,
                "output": 4.9,
                "reasoning": "3",
                "cache": {"read": 2.8, "write": -1}
            },
            "providerID": "provider",
            "modelID": "model",
            "time": {"created": 1, "completed": 2},
            "cost": 0
        });
        let Ok(record) = parse_message(valid_source_row_with_data(&numeric.to_string()), &context)
        else {
            panic!("numeric coercion row must remain eligible");
        };
        assert_eq!(record.agent_raw, "unknown");
        assert_eq!(record.agent_key, "unknown");
        assert_eq!(record.tok_input, 0);
        assert_eq!(record.tok_output, 4);
        assert_eq!(record.tok_reasoning, 3);
        assert_eq!(record.tok_cache_read, 2);
        assert_eq!(record.tok_cache_write, 0);
        assert_eq!(record.cost_source, CostSource::Unavailable);
        println!(
            "malformed_input invalid_json=skipped tokens_string=skipped negative_to={} float_output_to={} missing_agent={}",
            record.tok_input, record.tok_output, record.agent_raw
        );
    }

    #[test]
    fn opencode_missing_database_error_lists_all_probed_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let explicit = temp.path().join("explicit");
        let xdg = temp.path().join("xdg");
        let home = temp.path().join("home");
        let error = discover_database_path_from(Some(&explicit), Some(&xdg), Some(&home))
            .expect_err("all discovery candidates are absent");
        let text = error.to_string();
        for expected in [
            explicit.join("opencode.db"),
            xdg.join("opencode").join("opencode.db"),
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        ] {
            assert!(
                text.contains(expected.to_string_lossy().as_ref()),
                "missing probed path {expected:?} in {text}"
            );
        }
        println!("missing_database_error={text}");
    }

    #[test]
    fn opencode_missing_database_error_displays_spaces_and_trailing_backslash_verbatim() {
        let path = PathBuf::from(r"C:\Agent Lens Data\");
        let text = OpenCodeError::DatabaseNotFound {
            probed_paths: vec![path.clone()],
        }
        .to_string();

        assert!(text.contains(path.to_string_lossy().as_ref()), "{text}");
        assert!(!text.contains(r"C:\\Agent Lens Data\\"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn opencode_read_only_directory_without_sidecars_returns_wal_remediation() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_temp, directory, _manifest) = fixture_directory();
        let database = directory.join("opencode.db");
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", database.display()));
            if sidecar.exists() {
                fs::remove_file(sidecar).expect("remove fixture sidecar");
            }
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o555))
            .expect("make source directory read-only");
        let error = SqliteSourceConnection::open(&database)
            .expect_err("missing sidecars in a read-only directory must be actionable");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .expect("restore source directory permissions");

        assert!(matches!(error, OpenCodeError::WalUnreadable { .. }));
        let text = error.to_string();
        assert!(text.contains("chmod"));
        assert!(text.contains("group"));
        assert!(text.contains("WAL/SHM"));
        println!("wal_unreadable_error={text}");
    }

    fn valid_source_row(index: usize, time_updated: i64) -> SourceMessageRow {
        let data = json!({
            "role": "assistant",
            "agent": "Build",
            "path": {"cwd": "/fixture/project"},
            "cost": 0,
            "tokens": {
                "input": 1,
                "output": 2,
                "reasoning": 3,
                "cache": {"read": 4, "write": 5}
            },
            "modelID": "model",
            "providerID": "provider",
            "time": {"created": time_updated, "completed": time_updated + 1}
        });
        SourceMessageRow {
            message_id: format!("message-{index}"),
            session_id: "session".to_string(),
            time_created: time_updated,
            time_updated,
            data: data.to_string(),
        }
    }

    fn valid_source_row_with_data(data: &str) -> SourceMessageRow {
        SourceMessageRow {
            message_id: "message-malformed".to_string(),
            session_id: "session-malformed".to_string(),
            time_created: 1,
            time_updated: 2,
            data: data.to_string(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "manual QA scans the live OpenCode database"]
    fn opencode_manual_qa_real_database_read_only_and_external_count_agree() {
        let database = PathBuf::from("/config/.local/share/opencode/opencode.db");
        let directory = database.parent().expect("live database parent");
        let before = directory_entries(directory);
        let watermark = chrono::Utc::now().timestamp_millis() - 10 * 60 * 1_000;
        let window_start = watermark - OVERLAP_WINDOW_MS;
        let started = Instant::now();
        let mut connection = SqliteSourceConnection::open(&database).expect("open live database");
        assert!(connection.query_only().expect("live query_only"));
        let handles = process_database_handles(&database);
        assert!(
            !handles.is_empty(),
            "live database must have an open process handle"
        );
        assert!(
            handles
                .iter()
                .any(|handle| handle.path == database && handle.read_only),
            "the main live database handle must be O_RDONLY: {handles:?}"
        );
        assert!(
            handles.iter().all(|handle| {
                handle.read_only
                    || handle.path == sidecar_path(&database, "-wal")
                    || handle.path == sidecar_path(&database, "-shm")
            }),
            "only SQLite WAL/SHM handles may be writable: {handles:?}"
        );
        let mut examples = Vec::new();
        let result = scan_connection(&mut connection, &request(Some(watermark)), |batch| {
            let needed = 2_usize.saturating_sub(examples.len());
            examples.extend(batch.iter().take(needed).cloned());
            Ok(())
        })
        .expect("bounded live scan");
        let elapsed = started.elapsed();
        println!(
            "real_scan watermark={watermark} window_start={window_start} eligible={} skipped={} reached_eof={} elapsed_ms={}",
            result.eligible_count,
            result.skipped_count,
            result.reached_eof,
            elapsed.as_millis()
        );
        for record in &examples {
            println!(
                "real_record={}",
                serde_json::to_string(record).expect("serialize record")
            );
        }

        let sql = format!(
            "SELECT count(*) FROM message WHERE time_updated >= {window_start} AND json_extract(data,'$.role')='assistant' AND json_extract(data,'$.tokens') IS NOT NULL;"
        );
        let external = Command::new("sqlite3")
            .arg(format!("file:{}?mode=ro", database.display()))
            .arg(sql)
            .output()
            .expect("run external sqlite3");
        assert!(external.status.success(), "external sqlite3 failed");
        let external_count: u64 = String::from_utf8(external.stdout)
            .expect("sqlite count UTF-8")
            .trim()
            .parse()
            .expect("sqlite count integer");
        println!(
            "external_sqlite eligible={external_count} scanner_eligible={}",
            result.eligible_count
        );
        assert_eq!(result.eligible_count, external_count);
        drop(connection);

        let after = directory_entries(directory);
        let unexpected = after
            .difference(&before)
            .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            unexpected.is_empty(),
            "unexpected source artifacts: {unexpected:?}"
        );
        println!("read_only_query_only=1 handles={handles:?} unexpected_files={unexpected:?}");
    }

    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    struct OpenHandle {
        path: PathBuf,
        read_only: bool,
    }

    #[cfg(target_os = "linux")]
    fn process_database_handles(database: &Path) -> Vec<OpenHandle> {
        let mut handles = Vec::new();
        for entry in fs::read_dir("/proc/self/fd").expect("read process fd directory") {
            let entry = entry.expect("read process fd entry");
            let target = match fs::read_link(entry.path()) {
                Ok(target) => target,
                Err(_) => continue,
            };
            let target_text = target.to_string_lossy();
            if !target_text.starts_with(database.to_string_lossy().as_ref()) {
                continue;
            }
            let fdinfo = fs::read_to_string(Path::new("/proc/self/fdinfo").join(entry.file_name()))
                .expect("read process fdinfo");
            let flags = fdinfo
                .lines()
                .find_map(|line| line.strip_prefix("flags:\t"))
                .map(str::trim)
                .and_then(|value| u32::from_str_radix(value, 8).ok())
                .expect("parse fd flags");
            handles.push(OpenHandle {
                path: target,
                read_only: flags & 0b11 == 0,
            });
        }
        handles
    }

    #[cfg(target_os = "linux")]
    fn directory_entries(directory: &Path) -> BTreeSet<String> {
        fs::read_dir(directory)
            .expect("read source directory")
            .map(|entry| {
                entry
                    .expect("read source directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}
