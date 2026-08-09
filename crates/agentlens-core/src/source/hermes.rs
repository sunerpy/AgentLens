//! Hermes 会话级用量的只读 SQLite 适配器。
//!
//! Hermes 的 `messages.token_count` 当前不承载可用数据，五桶真值只存在于 `sessions`。
//! 因而每个 session 归一化成一条 `granularity=session` 记录，并以 session id 同时作为
//! message id，让归档的同级 last-write-wins 在会话增长时覆盖旧累计值。活动会话和零 token
//! 会话仍是有效计量记录，`is_incomplete` 固定为 false；否则聚合层会永久隐藏用户正在查看的
//! 真实用量。游标取该会话 `max(messages.timestamp)`，无消息时才回退 `started_at`。

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

use crate::archive::{
    normalize_agent_key, CostSource, NormalizedUsageRecord, Origin, UsageGranularity,
};

/// Hermes 记录写入归档时使用的 source 键。
pub const HERMES_SOURCE: &str = "hermes";
/// Hermes 状态数据库文件名。
pub const STATE_DATABASE: &str = "state.db";
/// 每次水位线扫描向前重叠 24 小时。
pub const OVERLAP_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
/// 默认交付批大小。
pub const DEFAULT_BATCH_SIZE: usize = 1_000;

/// Hermes 发现与扫描操作的结果类型。
pub type Result<T> = std::result::Result<T, HermesError>;

/// 阻止只读 Hermes 扫描完成的错误。
#[derive(Debug, Error)]
pub enum HermesError {
    /// 没有候选 Hermes 数据目录。
    #[error("未找到 Hermes 状态数据库；已检查：{}", display_paths(.probed_paths))]
    DataDirectoryNotFound {
        /// 按优先级检查过的数据目录。
        probed_paths: Vec<PathBuf>,
    },
    /// 状态数据库无法只读打开。
    #[error("无法只读打开 Hermes 状态数据库 {path}：{source}")]
    Open {
        /// 无法打开的数据库路径。
        path: PathBuf,
        /// SQLite 原始错误。
        source: rusqlite::Error,
    },
    /// 状态数据库查询失败。
    #[error("无法查询 Hermes 状态数据库 {path}：{source}")]
    Query {
        /// 查询失败的数据库路径。
        path: PathBuf,
        /// SQLite 原始错误。
        source: rusqlite::Error,
    },
    /// 调用方给出了不可能的批大小。
    #[error("Hermes 扫描 batch_size 必须大于 0")]
    InvalidBatchSize,
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

/// 调用方 sink 用来安全中断扫描的错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct SinkError {
    message: String,
}

impl SinkError {
    /// 创建保留原始原因的 sink 错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// 一行 session 未产生归一化记录的原因计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkippedBreakdown {
    /// SQLite 列类型与 Hermes schema 不兼容的行数。
    pub invalid_row: u64,
    /// session id 缺失或为空的行数。
    pub missing_session_id: u64,
    /// started、ended 或 source update 时间不可换算的行数。
    pub unparsable_timestamp: u64,
    /// 任一 token 桶为负数的行数。
    pub invalid_tokens: u64,
}

impl SkippedBreakdown {
    /// 返回所有跳过类别之和。
    pub const fn total(self) -> u64 {
        self.invalid_row + self.missing_session_id + self.unparsable_timestamp + self.invalid_tokens
    }
}

/// 一次 Hermes 重叠窗口扫描的不可变输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRequest {
    /// 写入每条记录的稳定主机标识。
    pub host_id: String,
    /// 上次成功提交的游标；首次全扫为 `None`。
    pub watermark: Option<i64>,
    /// 记录来源层级；正常扫描使用 [`Origin::Live`]。
    pub origin: Origin,
    /// 本轮跳过时保留的上次成功时间。
    pub last_success_utc: Option<i64>,
    /// Rust 侧交付批大小。
    pub batch_size: usize,
}

impl ScanRequest {
    /// 创建默认批大小的活动源请求。
    pub fn live(host_id: impl Into<String>, watermark: Option<i64>) -> Self {
        Self {
            host_id: host_id.into(),
            watermark,
            origin: Origin::Live,
            last_success_utc: None,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    fn window_start(&self) -> i64 {
        self.watermark
            .map_or(i64::MIN, |value| value.saturating_sub(OVERLAP_WINDOW_MS))
    }
}

/// 扫描没有到达 EOF 的可恢复原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanSkipReason {
    /// sink 拒绝了一个批次。
    Interrupted(String),
}

/// 一轮 Hermes 扫描的可观测结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// 已交给 sink 的记录数。
    pub delivered_records: u64,
    /// sink 调用次数。
    pub delivered_batches: u64,
    /// 落在窗口内并产生记录的 session 数。
    pub eligible_count: u64,
    /// 所有无损跳过的 session 数。
    pub skipped_count: u64,
    /// 按稳定原因拆分的跳过计数。
    pub skipped_breakdown: SkippedBreakdown,
    /// 完整扫描时观察到的最大会话更新时间。
    pub observed_max_time_updated: Option<i64>,
    /// 只有全部 session 处理完且 sink 未中断时才为 true。
    pub reached_eof: bool,
    /// 调用方传入的上次成功时间。
    pub last_success_utc: Option<i64>,
    /// 未到达 EOF 时的可恢复原因。
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
            last_success_utc,
            skip_reason: None,
        }
    }
}

#[derive(Debug)]
struct SessionRow {
    id: Option<String>,
    source: String,
    model: Option<String>,
    started_at: f64,
    ended_at: Option<f64>,
    tokens: [i64; 5],
    billing_provider: Option<String>,
    billing_base_url: Option<String>,
    source_time_updated: f64,
}

/// 扫描自动发现的 Hermes 数据目录。
pub fn scan_default<F>(request: &ScanRequest, sink: F) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    let data_dir = discover_data_dir()?;
    scan_data_dir(data_dir, request, sink)
}

/// 扫描显式 Hermes 数据目录中的 `state.db`。
pub fn scan_data_dir<F>(
    data_dir: impl AsRef<Path>,
    request: &ScanRequest,
    mut sink: F,
) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    if request.batch_size == 0 {
        return Err(HermesError::InvalidBatchSize);
    }
    let data_dir = data_dir.as_ref();
    let database = data_dir.join(STATE_DATABASE);
    if !database.is_file() {
        return Err(HermesError::DataDirectoryNotFound {
            probed_paths: vec![data_dir.to_path_buf()],
        });
    }

    let connection = open_read_only(&database)?;
    let mut statement =
        connection
            .prepare(SESSION_ROWS_SQL)
            .map_err(|source| HermesError::Query {
                path: database.clone(),
                source,
            })?;
    let rows = statement
        .query_map([], map_session_row)
        .map_err(|source| HermesError::Query {
            path: database.clone(),
            source,
        })?;
    let mut result = ScanResult::empty(request.last_success_utc);
    let mut records = Vec::new();

    for row in rows {
        let row = match row {
            Ok(row) => row,
            Err(_) => {
                result.skipped_breakdown.invalid_row += 1;
                continue;
            }
        };
        let Some(record) = normalize_session(row, request, &mut result.skipped_breakdown) else {
            continue;
        };
        if record.source_time_updated < request.window_start() {
            continue;
        }
        result.observed_max_time_updated = Some(
            result
                .observed_max_time_updated
                .map_or(record.source_time_updated, |current| {
                    current.max(record.source_time_updated)
                }),
        );
        records.push(record);
    }

    result.eligible_count = records.len() as u64;
    result.skipped_count = result.skipped_breakdown.total();
    for batch in records.chunks(request.batch_size) {
        if let Err(error) = sink(batch) {
            result.skip_reason = Some(ScanSkipReason::Interrupted(error.to_string()));
            result.observed_max_time_updated = None;
            return Ok(result);
        }
        result.delivered_records += batch.len() as u64;
        result.delivered_batches += 1;
    }
    result.reached_eof = true;
    Ok(result)
}

const SESSION_ROWS_SQL: &str = "SELECT
        sessions.id,
        sessions.source,
        sessions.model,
        sessions.started_at,
        sessions.ended_at,
        sessions.input_tokens,
        sessions.output_tokens,
        sessions.cache_read_tokens,
        sessions.cache_write_tokens,
        sessions.reasoning_tokens,
        sessions.billing_provider,
        sessions.billing_base_url,
        coalesce(
            (SELECT max(messages.timestamp)
             FROM messages
             WHERE messages.session_id = sessions.id),
            sessions.started_at
        )
    FROM sessions
    ORDER BY sessions.id";

fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        source: row.get(1)?,
        model: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        tokens: [
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
        ],
        billing_provider: row.get(10)?,
        billing_base_url: row.get(11)?,
        source_time_updated: row.get(12)?,
    })
}

fn normalize_session(
    row: SessionRow,
    request: &ScanRequest,
    skipped: &mut SkippedBreakdown,
) -> Option<NormalizedUsageRecord> {
    let session_id = row
        .id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let Some(session_id) = session_id else {
        skipped.missing_session_id += 1;
        return None;
    };
    let Some(time_created_utc) = epoch_seconds_to_ms(row.started_at) else {
        skipped.unparsable_timestamp += 1;
        return None;
    };
    let time_completed_utc = match row.ended_at {
        Some(value) => match epoch_seconds_to_ms(value) {
            Some(value) => Some(value),
            None => {
                skipped.unparsable_timestamp += 1;
                return None;
            }
        },
        None => None,
    };
    let Some(source_time_updated) = epoch_seconds_to_ms(row.source_time_updated) else {
        skipped.unparsable_timestamp += 1;
        return None;
    };
    let [Ok(tok_input), Ok(tok_output), Ok(tok_cache_read), Ok(tok_cache_write), Ok(tok_reasoning)] =
        row.tokens.map(u64::try_from)
    else {
        skipped.invalid_tokens += 1;
        return None;
    };
    let agent_raw = row.source.trim();
    let agent_raw = if agent_raw.is_empty() {
        "unknown"
    } else {
        agent_raw
    };
    let (provider_id, model_id) = normalize_provider_and_model(
        row.billing_provider.as_deref(),
        row.billing_base_url.as_deref(),
        row.model.as_deref(),
    );

    Some(NormalizedUsageRecord {
        host_id: request.host_id.clone(),
        source: HERMES_SOURCE.to_owned(),
        granularity: UsageGranularity::Session,
        message_id: session_id.clone(),
        session_id,
        time_created_utc,
        time_completed_utc,
        source_time_updated,
        origin: request.origin,
        origin_priority: request.origin.priority(),
        agent_raw: agent_raw.to_owned(),
        agent_key: normalize_agent_key(agent_raw),
        provider_id,
        model_id,
        variant: None,
        tok_input,
        tok_output,
        tok_reasoning,
        tok_cache_read,
        tok_cache_write,
        cost: None,
        cost_source: CostSource::Unavailable,
        is_incomplete: false,
        project_dir: String::new(),
    })
}

/// Hermes 的 `billing_provider=custom` 同时覆盖云网关与本地 Ollama，不能直接拿来定价。
/// 云模型以 `global.<provider>.` 或 `<provider>.` 命名空间拆分；Ollama 则由 base URL、
/// provider、`custom:<ollama-space>:` 前缀或无云命名空间的 Ollama tag 识别，并固定写入
/// `provider_id=ollama`。内置目录没有该 provider，因此本地模型不会误命中云端价格。
fn normalize_provider_and_model(
    billing_provider: Option<&str>,
    billing_base_url: Option<&str>,
    model: Option<&str>,
) -> (String, String) {
    let model = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    if let Some((provider, model_id)) = cloud_model_namespace(model) {
        return (provider.to_owned(), model_id.to_owned());
    }

    let provider = billing_provider.unwrap_or_default().trim();
    let base_url = billing_base_url.unwrap_or_default().trim();
    if is_ollama_model(provider, base_url, model) {
        return ("ollama".to_owned(), strip_ollama_prefix(model).to_owned());
    }

    (
        if provider.is_empty() {
            HERMES_SOURCE.to_owned()
        } else {
            provider.to_owned()
        },
        model.to_owned(),
    )
}

fn cloud_model_namespace(model: &str) -> Option<(&str, &str)> {
    const PROVIDERS: [&str; 6] = ["anthropic", "openai", "google", "mistral", "cohere", "meta"];
    let model = model.strip_prefix("global.").unwrap_or(model);
    let (provider, model_id) = model.split_once('.')?;
    PROVIDERS
        .contains(&provider)
        .then_some((provider, model_id.trim()))
        .filter(|(_, model_id)| !model_id.is_empty())
}

fn is_ollama_model(provider: &str, base_url: &str, model: &str) -> bool {
    let provider = provider.to_ascii_lowercase();
    let base_url = base_url.to_ascii_lowercase();
    provider.contains("ollama")
        || base_url.contains("ollama")
        || base_url.contains(":11434")
        || custom_ollama_model_id(model).is_some()
        || (cloud_model_namespace(model).is_none()
            && model
                .rsplit_once(':')
                .is_some_and(|(name, tag)| !name.trim().is_empty() && !tag.trim().is_empty()))
}

fn strip_ollama_prefix(model: &str) -> &str {
    custom_ollama_model_id(model).unwrap_or(model)
}

fn custom_ollama_model_id(model: &str) -> Option<&str> {
    let (prefix, remainder) = model.split_once(':')?;
    if !prefix.eq_ignore_ascii_case("custom") {
        return None;
    }
    let (space, model_id) = remainder.split_once(':')?;
    (space.to_ascii_lowercase().contains("ollama") && !model_id.trim().is_empty())
        .then_some(model_id)
}

fn epoch_seconds_to_ms(seconds: f64) -> Option<i64> {
    let milliseconds = (seconds * 1_000.0).trunc();
    (seconds >= 0.0 && milliseconds.is_finite() && milliseconds <= i64::MAX as f64)
        .then_some(milliseconds as i64)
}

/// 按 `HERMES_HOME`、`~/.hermes` 顺序发现数据目录。
pub fn discover_data_dir() -> Result<PathBuf> {
    let explicit = env::var_os("HERMES_HOME").map(PathBuf::from);
    let home = dirs::home_dir();
    discover_data_dir_from(explicit.as_deref(), home.as_deref())
}

fn discover_data_dir_from(explicit: Option<&Path>, home: Option<&Path>) -> Result<PathBuf> {
    let mut probed_paths = Vec::new();
    if let Some(path) = explicit {
        probed_paths.push(path.to_path_buf());
    }
    if let Some(path) = home {
        probed_paths.push(path.join(".hermes"));
    }
    if let Some(path) = probed_paths
        .iter()
        .find(|path| path.join(STATE_DATABASE).is_file())
    {
        return Ok(path.clone());
    }
    Err(HermesError::DataDirectoryNotFound { probed_paths })
}

fn open_read_only(database: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| HermesError::Open {
            path: database.to_path_buf(),
            source,
        })?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .and_then(|()| connection.pragma_update(None, "query_only", true))
        .map_err(|source| HermesError::Open {
            path: database.to_path_buf(),
            source,
        })?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use rusqlite::types::Value;
    use rusqlite::{params, Connection};

    use crate::archive::{CostSource, Origin, UsageGranularity};
    use crate::pricing::PriceTable;

    use super::*;

    const CLOUD_SESSION: &str = "session-cloud";
    const LOCAL_SESSION: &str = "session-local";
    const ACTIVE_SESSION: &str = "session-active";

    fn fixture() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().expect("create synthetic Hermes directory");
        let database = temp.path().join(STATE_DATABASE);
        let connection =
            Connection::open(database).expect("create synthetic Hermes state database");
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .expect("enable WAL for synthetic Hermes database");
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    source TEXT NOT NULL,
                    model TEXT,
                    started_at REAL NOT NULL,
                    ended_at REAL,
                    input_tokens INTEGER DEFAULT 0,
                    output_tokens INTEGER DEFAULT 0,
                    cache_read_tokens INTEGER DEFAULT 0,
                    cache_write_tokens INTEGER DEFAULT 0,
                    reasoning_tokens INTEGER DEFAULT 0,
                    billing_provider TEXT,
                    billing_base_url TEXT
                );
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    timestamp REAL NOT NULL
                );",
            )
            .expect("create synthetic Hermes schema");
        (temp, connection)
    }

    struct FixtureSession<'a> {
        id: &'a str,
        source: &'a str,
        model: &'a str,
        started_at: &'a str,
        ended_at: Option<&'a str>,
        tokens: [i64; 5],
        billing_provider: Option<&'a str>,
        billing_base_url: Option<&'a str>,
    }

    fn insert_session(connection: &Connection, session: &FixtureSession<'_>) {
        connection
            .execute(
                "INSERT INTO sessions (
                    id, source, model, started_at, ended_at,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    reasoning_tokens, billing_provider, billing_base_url
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    session.id,
                    session.source,
                    session.model,
                    session.started_at,
                    session.ended_at,
                    session.tokens[0],
                    session.tokens[1],
                    session.tokens[2],
                    session.tokens[3],
                    session.tokens[4],
                    session.billing_provider,
                    session.billing_base_url,
                ],
            )
            .expect("insert synthetic Hermes session");
    }

    fn insert_raw_session(
        connection: &Connection,
        id: Option<&str>,
        started_at: Value,
        ended_at: Value,
        input_tokens: Value,
    ) {
        connection
            .execute(
                "INSERT INTO sessions (
                    id, source, model, started_at, ended_at,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    reasoning_tokens, billing_provider, billing_base_url
                ) VALUES (?1, 'cli', NULL, ?2, ?3, ?4, 2, 3, 4, 5, NULL, NULL)",
                params![id, started_at, ended_at, input_tokens],
            )
            .expect("insert raw Hermes session");
    }

    #[test]
    fn scan_maps_session_totals_float_timestamps_and_wal_visible_messages() {
        let (temp, connection) = fixture();
        insert_session(
            &connection,
            &FixtureSession {
                id: CLOUD_SESSION,
                source: "cli",
                model: "global.anthropic.claude-opus-4-7",
                started_at: "1778141335.6447966",
                ended_at: None,
                tokens: [100, 20, 30, 40, 50],
                billing_provider: Some("custom"),
                billing_base_url: Some("https://gateway.example.test/api/v1"),
            },
        );
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES (?1, ?2)",
                params![CLOUD_SESSION, "1778141352.7654321"],
            )
            .expect("insert uncheckpointed WAL message");
        insert_session(
            &connection,
            &FixtureSession {
                id: LOCAL_SESSION,
                source: "weixin",
                model: "custom:Ollama-fixture:synthetic-publisher/demo-8b:q4_k_m",
                started_at: "1778141400.1259",
                ended_at: Some("1778141500.5009"),
                tokens: [5, 6, 7, 8, 9],
                billing_provider: Some("custom"),
                billing_base_url: Some("http://ollama-deployments/v1"),
            },
        );
        insert_session(
            &connection,
            &FixtureSession {
                id: ACTIVE_SESSION,
                source: "cli",
                model: "demo-model:9b",
                started_at: "1778141600.9999",
                ended_at: None,
                tokens: [0; 5],
                billing_provider: None,
                billing_base_url: None,
            },
        );

        let mut records = Vec::new();
        let result = scan_data_dir(
            temp.path(),
            &ScanRequest::live("host-hermes-test", None),
            |batch| {
                records.extend_from_slice(batch);
                Ok(())
            },
        )
        .expect("scan synthetic Hermes database while WAL writer remains open");

        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, 3);
        assert_eq!(result.delivered_records, 3);
        assert_eq!(result.skipped_count, 0);
        assert_eq!(result.skipped_breakdown, SkippedBreakdown::default());
        assert_eq!(result.observed_max_time_updated, Some(1_778_141_600_999));

        let cloud = records
            .iter()
            .find(|record| record.session_id == CLOUD_SESSION)
            .expect("cloud session record");
        assert_eq!(cloud.message_id, CLOUD_SESSION);
        assert_eq!(cloud.source, HERMES_SOURCE);
        assert_eq!(cloud.granularity, UsageGranularity::Session);
        assert_eq!(cloud.time_created_utc, 1_778_141_335_644);
        assert_eq!(cloud.time_completed_utc, None);
        assert_eq!(cloud.source_time_updated, 1_778_141_352_765);
        assert_eq!(cloud.agent_raw, "cli");
        assert_eq!(cloud.agent_key, "cli");
        assert_eq!(cloud.provider_id, "anthropic");
        assert_eq!(cloud.model_id, "claude-opus-4-7");
        assert_eq!(cloud.tok_input, 100);
        assert_eq!(cloud.tok_output, 20);
        assert_eq!(cloud.tok_cache_read, 30);
        assert_eq!(cloud.tok_cache_write, 40);
        assert_eq!(cloud.tok_reasoning, 50);
        assert_eq!(cloud.cost, None);
        assert_eq!(cloud.cost_source, CostSource::Unavailable);
        assert_eq!(cloud.origin, Origin::Live);
        assert!(!cloud.is_incomplete);

        let local = records
            .iter()
            .find(|record| record.session_id == LOCAL_SESSION)
            .expect("local session record");
        assert_eq!(local.provider_id, "ollama");
        assert_eq!(local.model_id, "synthetic-publisher/demo-8b:q4_k_m");
        assert_eq!(local.time_created_utc, 1_778_141_400_125);
        assert_eq!(local.time_completed_utc, Some(1_778_141_500_500));
        assert_eq!(local.source_time_updated, local.time_created_utc);
        assert!(PriceTable::default()
            .resolve_record(local)
            .estimated()
            .is_none());

        let active = records
            .iter()
            .find(|record| record.session_id == ACTIVE_SESSION)
            .expect("zero-token active session record");
        assert_eq!(active.provider_id, "ollama");
        assert!(!active.is_incomplete, "会话级零 token 记录仍是有效会话计数");
    }

    #[test]
    fn scan_reemits_one_session_with_a_later_message_cursor_for_last_write_wins() {
        let (temp, connection) = fixture();
        insert_session(
            &connection,
            &FixtureSession {
                id: ACTIVE_SESSION,
                source: "cli",
                model: "growing-model:8b",
                started_at: "1778141000.0",
                ended_at: None,
                tokens: [10, 2, 0, 0, 1],
                billing_provider: None,
                billing_base_url: None,
            },
        );
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES (?1, ?2)",
                params![ACTIVE_SESSION, "1778141010.0"],
            )
            .expect("insert initial message");

        let first = collect_records(temp.path(), None);
        assert_eq!(first.len(), 1);
        let first_updated = first[0].source_time_updated;
        assert_eq!(first[0].tok_input, 10);

        connection
            .execute(
                "UPDATE sessions SET input_tokens = 25, output_tokens = 4 WHERE id = ?1",
                [ACTIVE_SESSION],
            )
            .expect("grow active session totals");
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES (?1, ?2)",
                params![ACTIVE_SESSION, "1778141090.25"],
            )
            .expect("advance active session timestamp");

        let second = collect_records(temp.path(), Some(first_updated));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].message_id, first[0].message_id);
        assert_eq!(second[0].tok_input, 25);
        assert_eq!(second[0].tok_output, 4);
        assert_eq!(second[0].source_time_updated, 1_778_141_090_250);
        assert!(second[0].source_time_updated > first_updated);
    }

    #[test]
    fn scan_reports_missing_open_corrupt_and_schema_drift_errors() {
        let missing = tempfile::tempdir().expect("missing tempdir");
        let error = scan_data_dir(
            missing.path(),
            &ScanRequest::live("host-errors", None),
            |_| Ok(()),
        )
        .expect_err("missing state database must fail");
        assert!(matches!(
            error,
            HermesError::DataDirectoryNotFound { ref probed_paths }
                if probed_paths == &[missing.path().to_path_buf()]
        ));

        let error = open_read_only(missing.path()).expect_err("a directory is not a SQLite file");
        assert!(matches!(
            error,
            HermesError::Open { ref path, .. } if path == missing.path()
        ));

        let corrupt = tempfile::tempdir().expect("corrupt tempdir");
        std::fs::write(
            corrupt.path().join(STATE_DATABASE),
            b"not a sqlite database",
        )
        .expect("write corrupt state database");
        let error = scan_data_dir(
            corrupt.path(),
            &ScanRequest::live("host-errors", None),
            |_| Ok(()),
        )
        .expect_err("corrupt state database must fail");
        assert!(matches!(error, HermesError::Query { .. }));
        assert!(error.to_string().contains(STATE_DATABASE));

        let drifted = tempfile::tempdir().expect("drifted tempdir");
        let connection = Connection::open(drifted.path().join(STATE_DATABASE))
            .expect("create drifted state database");
        connection
            .execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL);
                 CREATE TABLE messages (session_id TEXT, timestamp REAL);",
            )
            .expect("create drifted schema");
        let error = scan_data_dir(
            drifted.path(),
            &ScanRequest::live("host-errors", None),
            |_| Ok(()),
        )
        .expect_err("missing session columns must fail");
        assert!(matches!(error, HermesError::Query { .. }));
        assert!(error.to_string().contains("started_at"));
    }

    #[test]
    fn scan_classifies_invalid_rows_timestamps_tokens_and_session_lifecycle() {
        let temp = tempfile::tempdir().expect("invalid rows tempdir");
        let connection = Connection::open(temp.path().join(STATE_DATABASE))
            .expect("create permissive state database");
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT,
                    source TEXT,
                    model TEXT,
                    started_at REAL,
                    ended_at REAL,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    billing_provider TEXT,
                    billing_base_url TEXT
                );
                CREATE TABLE messages (session_id TEXT, timestamp REAL);",
            )
            .expect("create permissive Hermes schema");

        insert_raw_session(
            &connection,
            Some("valid-open"),
            Value::Real(1_000.125),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("valid-closed"),
            Value::Real(1_001.25),
            Value::Real(1_002.5),
            Value::Integer(1),
        );
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES ('valid-closed', 1003.75)",
                [],
            )
            .expect("insert valid closed-session message");

        insert_raw_session(
            &connection,
            None,
            Value::Real(1_010.0),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("   "),
            Value::Real(1_011.0),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("negative-start"),
            Value::Real(-1.0),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("overflow-start"),
            Value::Real(1.0e30),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("negative-end"),
            Value::Real(1_012.0),
            Value::Real(-1.0),
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("negative-message"),
            Value::Real(1_013.0),
            Value::Null,
            Value::Integer(1),
        );
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES ('negative-message', -1)",
                [],
            )
            .expect("insert negative message timestamp");
        insert_raw_session(
            &connection,
            Some("overflow-message"),
            Value::Real(1_014.0),
            Value::Null,
            Value::Integer(1),
        );
        connection
            .execute(
                "INSERT INTO messages (session_id, timestamp) VALUES ('overflow-message', 1e30)",
                [],
            )
            .expect("insert overflow message timestamp");
        insert_raw_session(
            &connection,
            Some("invalid-start-text"),
            Value::Text("not-a-number".to_owned()),
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("null-start"),
            Value::Null,
            Value::Null,
            Value::Integer(1),
        );
        insert_raw_session(
            &connection,
            Some("null-token"),
            Value::Real(1_015.0),
            Value::Null,
            Value::Null,
        );
        insert_raw_session(
            &connection,
            Some("overflow-token"),
            Value::Real(1_016.0),
            Value::Null,
            Value::Real(1.0e30),
        );
        insert_raw_session(
            &connection,
            Some("negative-token"),
            Value::Real(1_017.0),
            Value::Null,
            Value::Integer(-1),
        );

        let mut records = Vec::new();
        let result = scan_data_dir(
            temp.path(),
            &ScanRequest::live("host-invalid", None),
            |batch| {
                records.extend_from_slice(batch);
                Ok(())
            },
        )
        .expect("scan permissive Hermes database");

        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, 2);
        assert_eq!(result.delivered_records, 2);
        assert_eq!(
            result.skipped_breakdown,
            SkippedBreakdown {
                invalid_row: 4,
                missing_session_id: 2,
                unparsable_timestamp: 5,
                invalid_tokens: 1,
            }
        );
        assert_eq!(result.skipped_count, 12);

        let open = records
            .iter()
            .find(|record| record.session_id == "valid-open")
            .expect("open session survives");
        assert_eq!(open.time_completed_utc, None);
        assert_eq!(open.source_time_updated, open.time_created_utc);
        assert_eq!(open.agent_raw, "cli");
        assert_eq!(open.provider_id, HERMES_SOURCE);
        assert_eq!(open.model_id, "unknown");

        let closed = records
            .iter()
            .find(|record| record.session_id == "valid-closed")
            .expect("closed session survives");
        assert_eq!(closed.time_completed_utc, Some(1_002_500));
        assert_eq!(closed.source_time_updated, 1_003_750);

        for seconds in [f64::NAN, f64::INFINITY, -1.0, 1.0e30] {
            assert_eq!(epoch_seconds_to_ms(seconds), None);
        }
        assert_eq!(epoch_seconds_to_ms(12.345_678), Some(12_345));
    }

    #[test]
    fn provider_model_normalization_covers_cloud_custom_local_and_empty_values() {
        let cases = [
            (
                Some("custom"),
                None,
                Some("global.anthropic.claude-fixture"),
                ("anthropic", "claude-fixture"),
            ),
            (
                Some("ignored"),
                None,
                Some("openai.gpt-fixture"),
                ("openai", "gpt-fixture"),
            ),
            (
                Some("custom"),
                None,
                Some("custom:Ollama-fixture:synthetic-publisher/demo-8b:q4_k_m"),
                ("ollama", "synthetic-publisher/demo-8b:q4_k_m"),
            ),
            (
                Some("OLLAMA-local"),
                None,
                Some("plain-model"),
                ("ollama", "plain-model"),
            ),
            (
                Some("custom"),
                Some("http://fixture-ollama/v1"),
                Some("plain-model"),
                ("ollama", "plain-model"),
            ),
            (
                Some("custom"),
                Some("http://127.0.0.1:11434/v1"),
                Some("plain-model"),
                ("ollama", "plain-model"),
            ),
            (
                None,
                None,
                Some("demo-model:9b"),
                ("ollama", "demo-model:9b"),
            ),
            (None, None, None, (HERMES_SOURCE, "unknown")),
            (
                Some("acme"),
                None,
                Some("acme.model"),
                ("acme", "acme.model"),
            ),
            (
                Some("acme"),
                None,
                Some("plain-model"),
                ("acme", "plain-model"),
            ),
        ];

        for (provider, base_url, model, expected) in cases {
            let normalized = normalize_provider_and_model(provider, base_url, model);
            assert_eq!(normalized, (expected.0.to_owned(), expected.1.to_owned()));
        }
        assert_eq!(cloud_model_namespace("global.anthropic."), None);
        assert_eq!(custom_ollama_model_id("custom:not-local:demo:tag"), None);
        assert_eq!(custom_ollama_model_id("not-custom:ollama:demo:tag"), None);
        assert_eq!(custom_ollama_model_id("custom:ollama-fixture:"), None);
        assert_eq!(strip_ollama_prefix("plain-model"), "plain-model");
    }

    #[test]
    fn discovery_scan_default_sink_interruption_and_overlap_boundaries_are_observable() {
        let explicit = tempfile::tempdir().expect("explicit Hermes dir");
        let home = tempfile::tempdir().expect("home dir");
        let home_hermes = home.path().join(".hermes");
        std::fs::create_dir(&home_hermes).expect("create home Hermes dir");
        std::fs::write(explicit.path().join(STATE_DATABASE), b"")
            .expect("write explicit state marker");
        std::fs::write(home_hermes.join(STATE_DATABASE), b"").expect("write home state marker");

        assert_eq!(
            discover_data_dir_from(Some(explicit.path()), Some(home.path()))
                .expect("explicit Hermes dir wins"),
            explicit.path()
        );
        assert_eq!(
            discover_data_dir_from(Some(&home.path().join("missing")), Some(home.path()))
                .expect("home fallback resolves"),
            home_hermes
        );
        let error = discover_data_dir_from(
            Some(&home.path().join("missing-explicit")),
            Some(&home.path().join("missing-home")),
        )
        .expect_err("missing discovery candidates must fail");
        let text = error.to_string();
        assert!(text.contains("missing-explicit"));
        assert!(text.contains("missing-home/.hermes"));

        let mut invalid_default = ScanRequest::live("host-default", None);
        invalid_default.batch_size = 0;
        assert!(matches!(
            scan_default(&invalid_default, |_| Ok(())),
            Err(HermesError::InvalidBatchSize | HermesError::DataDirectoryNotFound { .. })
        ));
        assert!(matches!(
            discover_data_dir(),
            Ok(_) | Err(HermesError::DataDirectoryNotFound { .. })
        ));

        let (temp, connection) = fixture();
        for (id, timestamp) in [
            ("before-overlap", "13599.999"),
            ("at-overlap", "13600.0"),
            ("after-overlap", "13601.0"),
        ] {
            insert_session(
                &connection,
                &FixtureSession {
                    id,
                    source: "cli",
                    model: "demo-model:9b",
                    started_at: timestamp,
                    ended_at: None,
                    tokens: [1, 2, 3, 4, 5],
                    billing_provider: None,
                    billing_base_url: None,
                },
            );
        }

        let mut request = ScanRequest::live("host-overlap", Some(100_000_000));
        request.last_success_utc = Some(42);
        request.batch_size = 1;
        let interrupted = scan_data_dir(temp.path(), &request, |_| {
            Err(SinkError::new("fixture sink stopped"))
        })
        .expect("sink interruption is recoverable");
        assert!(!interrupted.reached_eof);
        assert_eq!(interrupted.eligible_count, 2);
        assert_eq!(interrupted.delivered_records, 0);
        assert_eq!(interrupted.delivered_batches, 0);
        assert_eq!(interrupted.observed_max_time_updated, None);
        assert_eq!(interrupted.last_success_utc, Some(42));
        assert_eq!(
            interrupted.skip_reason,
            Some(ScanSkipReason::Interrupted(
                "fixture sink stopped".to_owned()
            ))
        );

        let mut records = Vec::new();
        let complete = scan_data_dir(temp.path(), &request, |batch| {
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("complete overlap scan");
        assert!(complete.reached_eof);
        assert_eq!(complete.delivered_records, 2);
        assert_eq!(complete.delivered_batches, 2);
        assert_eq!(complete.observed_max_time_updated, Some(13_601_000));
        assert_eq!(
            records
                .iter()
                .map(|record| record.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["after-overlap", "at-overlap"]
        );
        assert_eq!(request.window_start(), 13_600_000);
        assert_eq!(
            ScanRequest::live("host-overlap", Some(i64::MIN)).window_start(),
            i64::MIN
        );
    }

    fn collect_records(
        data_dir: &std::path::Path,
        watermark: Option<i64>,
    ) -> Vec<crate::archive::NormalizedUsageRecord> {
        let mut records = Vec::new();
        let result = scan_data_dir(
            data_dir,
            &ScanRequest::live("host-hermes-test", watermark),
            |batch| {
                records.extend_from_slice(batch);
                Ok(())
            },
        )
        .expect("scan synthetic Hermes records");
        assert!(result.reached_eof);
        records
    }
}
