//! 归档库：rusqlite 建库/打开（`dirs::data_dir()/agentlens/archive.db`）、
//! `PRAGMA user_version` 顺序迁移器、schema 指纹校验与不兼容库安全重建、
//! `usage_record`/`hosts`/`source_cursor`/`coverage_interval`/`app_settings` 表定义、
//! 归一化 record 结构体与 `agent_key` 规范化。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ARCHIVE_DIRECTORY: &str = "agentlens";
const ARCHIVE_DATABASE: &str = "archive.db";

/// Latest schema version understood by this build.
pub const LATEST_SCHEMA_VERSION: u32 = 3;
/// Default lock-wait budget applied before any archive migration or query.
pub const ARCHIVE_BUSY_TIMEOUT_MS: u64 = 5_000;

const MIGRATIONS: [Migration; LATEST_SCHEMA_VERSION as usize] = [
    Migration::new(1, migration_v1),
    Migration::new(2, migration_v2),
    Migration::new(3, migration_v3),
];

/// 当前代码能够安全查询的完整表列指纹。
///
/// 每次修改基线或迁移产生的列时必须同步更新这里；全新建库测试会校验两者严格一致。
const EXPECTED_SCHEMA_COLUMNS: [(&str, &[&str]); 5] = [
    (
        "usage_record",
        &[
            "host_id",
            "source",
            "granularity",
            "message_id",
            "session_id",
            "time_created_utc",
            "time_completed_utc",
            "source_time_updated",
            "origin",
            "origin_priority",
            "agent_raw",
            "agent_key",
            "provider_id",
            "model_id",
            "variant",
            "tok_input",
            "tok_output",
            "tok_reasoning",
            "tok_cache_read",
            "tok_cache_write",
            "cost",
            "cost_source",
            "is_incomplete",
            "project_dir",
        ],
    ),
    (
        "hosts",
        &[
            "host_id",
            "display_name",
            "kind",
            "ssh_target",
            "remote_data_dir",
            "last_success_utc",
            "machine_id_hash",
            "enabled_sources",
        ],
    ),
    (
        "source_cursor",
        &["host_id", "source", "cursor_time_updated"],
    ),
    (
        "coverage_interval",
        &[
            "host_id",
            "source",
            "origin",
            "interval_start",
            "interval_end",
        ],
    ),
    ("app_settings", &["key", "value"]),
];

/// Result type returned by archive operations.
pub type Result<T> = std::result::Result<T, ArchiveError>;

/// Errors returned while resolving, opening, configuring, or migrating an archive.
#[derive(Debug, Error)]
pub enum ArchiveError {
    /// The operating system did not expose a per-user data directory.
    #[error("cannot resolve the user data directory for AgentLens archive.db")]
    DataDirectoryUnavailable,
    /// The database path has no usable parent or file name.
    #[error("archive database path is invalid: {0}")]
    InvalidArchivePath(PathBuf),
    /// The archive directory could not be created or inspected.
    #[error("cannot prepare archive directory at {path}: {source}")]
    Directory {
        /// Directory involved in the failed operation.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// The directory permissions do not allow creating or updating the archive.
    #[error(
        "archive directory is not writable: {0}; grant write permission or choose another data directory"
    )]
    DirectoryNotWritable(PathBuf),
    /// SQLite could not open the archive path.
    #[error("cannot open archive database at {path}: {source}")]
    Open {
        /// Archive path that SQLite could not open.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// SQLite opened the file handle but could not read its database header or schema metadata.
    #[error(
        "archive database at {path} is unreadable; restore a backup or move the corrupt file aside: {source}"
    )]
    Unreadable {
        /// Corrupt or otherwise unreadable archive path.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// A newer AgentLens build has already upgraded this archive.
    #[error(
        "archive database schema version {found} is newer than this build supports ({supported}); upgrade AgentLens instead of downgrading the database"
    )]
    FutureSchema {
        /// Version stored in `PRAGMA user_version`.
        found: u32,
        /// Latest version understood by this build.
        supported: u32,
    },
    /// The migration list is not the required contiguous `1..=latest` sequence.
    #[error("invalid archive migration plan: {0}")]
    InvalidMigrationPlan(String),
    /// The pre-migration `VACUUM INTO` backup could not be created.
    #[error("cannot create pre-migration archive backup at {path}: {source}")]
    Backup {
        /// Intended backup path.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// A migration failed and its enclosing transaction was rolled back.
    #[error(
        "archive migration from version {from_version} to {target_version} failed; database was rolled back and backup remains at {backup_path}: {source}"
    )]
    Migration {
        /// Version observed before the migration run.
        from_version: u32,
        /// Migration step that failed.
        target_version: u32,
        /// Valid pre-migration backup retained for recovery.
        backup_path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// SQLite also failed while rolling back a failed migration.
    #[error(
        "archive migration from version {from_version} to {target_version} failed ({migration_error}) and rollback also failed; restore {backup_path}: {rollback_error}"
    )]
    Rollback {
        /// Version observed before the migration run.
        from_version: u32,
        /// Migration step that failed.
        target_version: u32,
        /// Valid pre-migration backup retained for recovery.
        backup_path: PathBuf,
        /// Original migration error text.
        migration_error: String,
        /// Rollback error from SQLite.
        rollback_error: Box<rusqlite::Error>,
    },
    /// Archive connection configuration failed.
    #[error("cannot configure archive database at {path}: {source}")]
    Configure {
        /// Archive path being configured.
        path: PathBuf,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// SQLite rejected an `app_settings` read or transactional merge.
    #[error("app settings database operation failed: {0}")]
    AppSettings(#[source] rusqlite::Error),
    /// A fresh archive could not apply its baseline migrations.
    #[error("初始化全新归档库 {path} 到 schema v{target_version} 失败: {source}")]
    Initialize {
        /// Fresh archive path.
        path: PathBuf,
        /// Migration step that failed.
        target_version: u32,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// SQLite could not read one table's column fingerprint.
    #[error("读取归档库 {path} 的表 {table} schema 指纹失败: {source}")]
    SchemaFingerprint {
        /// Archive path being inspected.
        path: PathBuf,
        /// Table whose columns could not be read.
        table: &'static str,
        /// Original SQLite error.
        source: rusqlite::Error,
    },
    /// Files belonging to an incompatible archive could not be removed after backup.
    #[error("归档库已成功备份，但删除不兼容文件 {path} 失败: {source}")]
    RemoveIncompatibleArchive {
        /// Main database or WAL sidecar path.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// The migrations and the declared schema fingerprint disagree.
    #[error("按当前代码重建归档库 {path} 后 schema 指纹仍不匹配；请同步更新基线 schema 与 EXPECTED_SCHEMA_COLUMNS")]
    RebuiltSchemaMismatch {
        /// Freshly rebuilt archive path.
        path: PathBuf,
    },
}

/// Provenance of one normalized record.
///
/// The integer priority is part of the conflict-resolution contract: live source data wins over a
/// database backup, and a database backup wins over legacy JSON.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Current live source database (`origin_priority = 3`).
    Live,
    /// Imported source database backup (`origin_priority = 2`).
    Bak,
    /// Legacy JSON storage (`origin_priority = 1`).
    Legacy,
}

impl Origin {
    /// Returns the canonical integer used by conditional archive upserts.
    pub const fn priority(self) -> i32 {
        match self {
            Self::Live => 3,
            Self::Bak => 2,
            Self::Legacy => 1,
        }
    }

    /// Returns the exact text stored in `usage_record.origin`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Bak => "bak",
            Self::Legacy => "legacy",
        }
    }
}

/// Provenance of the nullable normalized cost.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CostSource {
    /// A positive source-reported cost is stored in `cost`.
    Actual,
    /// No trustworthy source cost exists; `cost` must remain `None`, never synthetic zero.
    Unavailable,
    /// A local price override produced the value.
    Estimated,
}

impl CostSource {
    /// Returns the exact text stored in `usage_record.cost_source`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Actual => "actual",
            Self::Unavailable => "unavailable",
            Self::Estimated => "estimated",
        }
    }
}

/// 一条归档记录代表消息级用量还是会话级汇总。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageGranularity {
    /// 单条消息或模型调用。
    #[default]
    Message,
    /// 整个会话的累计用量。
    Session,
}

impl UsageGranularity {
    /// 返回 `usage_record.granularity` 使用的稳定文本。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Session => "session",
        }
    }
}

/// Cross-source normalized usage contract.
///
/// Rust field names mirror archive columns. JSON uses camelCase so todo 13 can add `ts-rs` derives
/// without introducing a second wire DTO. All timestamps are UTC epoch milliseconds. Callers must
/// set `origin_priority` to [`Origin::priority`] for `origin`; keeping both fields explicit makes
/// the conditional SQL conflict contract visible on the wire and in the archive.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedUsageRecord {
    /// Stable machine-derived host identifier.
    pub host_id: String,
    /// Open source name such as `opencode`; unknown future names remain valid.
    pub source: String,
    /// Whether this record measures one message or aggregates one session.
    #[serde(default)]
    pub granularity: UsageGranularity,
    /// Source message identifier, unique together with `host_id` and `source`.
    pub message_id: String,
    /// Source session identifier.
    pub session_id: String,
    /// Message creation timestamp in UTC epoch milliseconds.
    pub time_created_utc: i64,
    /// Optional completion timestamp in UTC epoch milliseconds.
    pub time_completed_utc: Option<i64>,
    /// Source update timestamp used by overlap-window conflict resolution.
    pub source_time_updated: i64,
    /// Provenance category.
    pub origin: Origin,
    /// Canonical integer priority copied from [`Origin::priority`].
    pub origin_priority: i32,
    /// Source display name or legacy slug before normalization.
    pub agent_raw: String,
    /// Stable normalized agent key.
    pub agent_key: String,
    /// Source provider identifier.
    pub provider_id: String,
    /// Source model identifier.
    pub model_id: String,
    /// Optional model variant such as `xhigh`.
    pub variant: Option<String>,
    /// Cache-miss input tokens.
    pub tok_input: u64,
    /// Output tokens.
    pub tok_output: u64,
    /// Reasoning tokens.
    pub tok_reasoning: u64,
    /// Cache-read input tokens.
    pub tok_cache_read: u64,
    /// Cache-write input tokens.
    pub tok_cache_write: u64,
    /// Trustworthy source or estimated cost; unavailable cost is `None`.
    pub cost: Option<f64>,
    /// Provenance and interpretation of `cost`.
    pub cost_source: CostSource,
    /// True for an all-zero assistant record without a completion timestamp.
    pub is_incomplete: bool,
    /// Source project working directory.
    pub project_dir: String,
}

/// Open archive database plus migration metadata from this open operation.
pub struct Archive {
    path: PathBuf,
    connection: Connection,
    last_backup_path: Option<PathBuf>,
}

impl Archive {
    /// Opens the standard `dirs::data_dir()/agentlens/archive.db` archive.
    pub fn open_default() -> Result<Self> {
        let path = default_archive_path()?;
        Self::open(path)
    }

    /// Opens `agentlens/archive.db` below an injected data directory.
    ///
    /// Tests and embedders use this entry point to avoid touching the real user data directory.
    pub fn open_in_data_dir(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::open(archive_path_in(data_dir))
    }

    /// Opens or creates an archive at an explicit injectable path and applies pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_schema_backup(path, create_backup)
    }

    fn open_with_schema_backup<F>(path: impl AsRef<Path>, backup_schema: F) -> Result<Self>
    where
        F: FnOnce(&Connection, &Path) -> Result<PathBuf>,
    {
        let path = path.as_ref().to_path_buf();
        prepare_parent_directory(&path)?;
        if !path.exists() {
            let connection = create_fresh_archive(&path)?;
            ensure_rebuilt_schema_matches(&connection, &path)?;
            return Ok(Self {
                path,
                connection,
                last_backup_path: None,
            });
        }

        let mut connection = Connection::open(&path).map_err(|source| ArchiveError::Open {
            path: path.clone(),
            source,
        })?;
        configure_busy_timeout(&connection, &path)?;
        let last_backup_path = migrate_with(&mut connection, &path, &MIGRATIONS)?;
        if archive_schema_is_compatible(&connection, &path)? {
            configure_connection(&connection, &path)?;
            return Ok(Self {
                path,
                connection,
                last_backup_path,
            });
        }

        // 用户明确选择不维护历史 schema 迁移：只有备份成功后才关闭连接并移除整组 WAL 文件。
        let backup_path = backup_schema(&connection, &path)?;
        drop(connection);
        remove_incompatible_archive_files(&path)?;

        let connection = create_fresh_archive(&path)?;
        ensure_rebuilt_schema_matches(&connection, &path)?;
        configure_connection(&connection, &path)?;
        tracing::warn!(
            archive_path = %path.display(),
            backup_path = %backup_path.display(),
            "归档库 schema 不兼容，已备份到 {} 并重建",
            backup_path.display()
        );

        Ok(Self {
            path,
            connection,
            last_backup_path: Some(backup_path),
        })
    }

    /// Returns the archive path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the live SQLite connection for downstream archive modules.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Returns a mutable connection for downstream transactional archive modules.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Returns the migration/schema-reset backup created by this open, if any.
    pub fn last_backup_path(&self) -> Option<&Path> {
        self.last_backup_path.as_deref()
    }
}

fn create_fresh_archive(database_path: &Path) -> Result<Connection> {
    let mut connection = Connection::open(database_path).map_err(|source| ArchiveError::Open {
        path: database_path.to_path_buf(),
        source,
    })?;
    configure_busy_timeout(&connection, database_path)?;
    validate_migration_plan(&MIGRATIONS)?;
    let transaction = connection
        .transaction()
        .map_err(|source| ArchiveError::Initialize {
            path: database_path.to_path_buf(),
            target_version: 1,
            source,
        })?;

    for migration in &MIGRATIONS {
        (migration.apply)(&transaction).map_err(|source| ArchiveError::Initialize {
            path: database_path.to_path_buf(),
            target_version: migration.target_version,
            source,
        })?;
        transaction
            .pragma_update(None, "user_version", migration.target_version)
            .map_err(|source| ArchiveError::Initialize {
                path: database_path.to_path_buf(),
                target_version: migration.target_version,
                source,
            })?;
    }

    transaction
        .commit()
        .map_err(|source| ArchiveError::Initialize {
            path: database_path.to_path_buf(),
            target_version: LATEST_SCHEMA_VERSION,
            source,
        })?;
    configure_connection(&connection, database_path)?;
    Ok(connection)
}

fn archive_schema_is_compatible(connection: &Connection, database_path: &Path) -> Result<bool> {
    for (table, expected_columns) in EXPECTED_SCHEMA_COLUMNS {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|source| ArchiveError::SchemaFingerprint {
                path: database_path.to_path_buf(),
                table,
                source,
            })?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|source| ArchiveError::SchemaFingerprint {
                path: database_path.to_path_buf(),
                table,
                source,
            })?;
        let actual_columns = rows
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .map_err(|source| ArchiveError::SchemaFingerprint {
                path: database_path.to_path_buf(),
                table,
                source,
            })?;
        let expected_columns = expected_columns
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<BTreeSet<_>>();
        if actual_columns != expected_columns {
            return Ok(false);
        }
    }
    Ok(true)
}

fn ensure_rebuilt_schema_matches(connection: &Connection, database_path: &Path) -> Result<()> {
    if archive_schema_is_compatible(connection, database_path)? {
        Ok(())
    } else {
        Err(ArchiveError::RebuiltSchemaMismatch {
            path: database_path.to_path_buf(),
        })
    }
}

fn remove_incompatible_archive_files(database_path: &Path) -> Result<()> {
    let mut wal_path = database_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let mut shm_path = database_path.as_os_str().to_os_string();
    shm_path.push("-shm");

    // 主库最后删除；任一 sidecar 删除失败时，用户至少仍保有主文件与刚生成的完整备份。
    for path in [
        PathBuf::from(wal_path),
        PathBuf::from(shm_path),
        database_path.to_path_buf(),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ArchiveError::RemoveIncompatibleArchive { path, source });
            }
        }
    }
    Ok(())
}

/// Resolves the standard archive path without creating directories or opening the database.
pub fn default_archive_path() -> Result<PathBuf> {
    dirs::data_dir()
        .map(archive_path_in)
        .ok_or(ArchiveError::DataDirectoryUnavailable)
}

/// Builds an injectable archive path below an arbitrary data directory.
pub fn archive_path_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir
        .as_ref()
        .join(ARCHIVE_DIRECTORY)
        .join(ARCHIVE_DATABASE)
}

/// Reads the complete `app_settings` key/value store in deterministic key order.
pub fn read_app_settings(connection: &Connection) -> Result<BTreeMap<String, String>> {
    let mut statement = connection
        .prepare("SELECT key, value FROM app_settings ORDER BY key")
        .map_err(ArchiveError::AppSettings)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(ArchiveError::AppSettings)?;
    rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(ArchiveError::AppSettings)
}

/// Atomically merges settings into `app_settings` by key.
///
/// Existing keys in `settings` are replaced and new keys are inserted. Keys absent from the input
/// remain untouched, so a caller changing one preference cannot erase unrelated preferences owned
/// by another view. The entire merge runs in one transaction; either every supplied key becomes
/// visible or none does.
pub fn write_app_settings(
    connection: &mut Connection,
    settings: &BTreeMap<String, String>,
) -> Result<()> {
    let transaction = connection
        .transaction()
        .map_err(ArchiveError::AppSettings)?;
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .map_err(ArchiveError::AppSettings)?;
        for (key, value) in settings {
            statement
                .execute(params![key, value])
                .map_err(ArchiveError::AppSettings)?;
        }
    }
    transaction.commit().map_err(ArchiveError::AppSettings)
}

/// Normalizes display names and legacy slugs to a stable agent key.
///
/// The function lowercases Unicode, trims leading and trailing separators, and collapses any run
/// of Unicode whitespace and ASCII hyphens to one `-`. CJK and other non-separator characters are
/// retained. Empty or separator-only input intentionally returns an empty key so parsers can
/// decide whether to substitute an `unknown` display value.
pub fn normalize_agent_key(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut separator_pending = false;

    for character in raw.trim().chars() {
        if character.is_whitespace() || character == '-' {
            separator_pending = !normalized.is_empty();
            continue;
        }

        if separator_pending {
            normalized.push('-');
            separator_pending = false;
        }
        normalized.extend(character.to_lowercase());
    }

    normalized
}

#[derive(Clone, Copy)]
struct Migration {
    target_version: u32,
    apply: for<'connection> fn(&Transaction<'connection>) -> rusqlite::Result<()>,
}

impl Migration {
    const fn new(
        target_version: u32,
        apply: for<'connection> fn(&Transaction<'connection>) -> rusqlite::Result<()>,
    ) -> Self {
        Self {
            target_version,
            apply,
        }
    }
}

fn migrate_with(
    connection: &mut Connection,
    database_path: &Path,
    migrations: &[Migration],
) -> Result<Option<PathBuf>> {
    let latest_version = validate_migration_plan(migrations)?;
    let current_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(|source| ArchiveError::Unreadable {
            path: database_path.to_path_buf(),
            source,
        })?;

    if current_version > latest_version {
        return Err(ArchiveError::FutureSchema {
            found: current_version,
            supported: latest_version,
        });
    }
    if current_version == latest_version {
        return Ok(None);
    }

    let backup_path = create_backup(connection, database_path)?;
    let transaction = connection
        .transaction()
        .map_err(|source| ArchiveError::Migration {
            from_version: current_version,
            target_version: current_version + 1,
            backup_path: backup_path.clone(),
            source,
        })?;

    for migration in migrations
        .iter()
        .filter(|migration| migration.target_version > current_version)
    {
        let target_version = migration.target_version;
        if let Err(source) = transaction.execute_batch("SAVEPOINT agentlens_migration_step") {
            return Err(rollback_error(
                transaction,
                current_version,
                target_version,
                backup_path,
                source,
            ));
        }
        if let Err(source) = (migration.apply)(&transaction) {
            return Err(rollback_error(
                transaction,
                current_version,
                target_version,
                backup_path,
                source,
            ));
        }
        if let Err(source) = transaction.pragma_update(None, "user_version", target_version) {
            return Err(rollback_error(
                transaction,
                current_version,
                target_version,
                backup_path,
                source,
            ));
        }
        if let Err(source) = transaction.execute_batch("RELEASE agentlens_migration_step") {
            return Err(rollback_error(
                transaction,
                current_version,
                target_version,
                backup_path,
                source,
            ));
        }
    }

    transaction
        .commit()
        .map_err(|source| ArchiveError::Migration {
            from_version: current_version,
            target_version: latest_version,
            backup_path: backup_path.clone(),
            source,
        })?;
    Ok(Some(backup_path))
}

fn validate_migration_plan(migrations: &[Migration]) -> Result<u32> {
    for (index, migration) in migrations.iter().enumerate() {
        let expected = index as u32 + 1;
        if migration.target_version != expected {
            return Err(ArchiveError::InvalidMigrationPlan(format!(
                "expected target version {expected}, found {}",
                migration.target_version
            )));
        }
    }
    Ok(migrations
        .last()
        .map_or(0, |migration| migration.target_version))
}

fn create_backup(connection: &Connection, database_path: &Path) -> Result<PathBuf> {
    let backup_path = next_backup_path(database_path)?;
    let backup_text = backup_path
        .to_str()
        .ok_or_else(|| ArchiveError::InvalidArchivePath(backup_path.clone()))?;
    connection
        .execute("VACUUM INTO ?1", params![backup_text])
        .map_err(|source| ArchiveError::Backup {
            path: backup_path.clone(),
            source,
        })?;
    Ok(backup_path)
}

fn next_backup_path(database_path: &Path) -> Result<PathBuf> {
    let parent = database_parent(database_path)?;
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ArchiveError::InvalidArchivePath(database_path.to_path_buf()))?;
    let timestamp = Utc::now().timestamp_millis();

    for suffix in 0_u32.. {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let candidate = parent.join(format!("{file_name}.backup-{timestamp}{suffix}.db"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!("u32 backup suffix space exhausted")
}

fn rollback_error(
    transaction: Transaction<'_>,
    from_version: u32,
    target_version: u32,
    backup_path: PathBuf,
    source: rusqlite::Error,
) -> ArchiveError {
    match transaction.rollback() {
        Ok(()) => ArchiveError::Migration {
            from_version,
            target_version,
            backup_path,
            source,
        },
        Err(rollback_error) => ArchiveError::Rollback {
            from_version,
            target_version,
            backup_path,
            migration_error: source.to_string(),
            rollback_error: Box::new(rollback_error),
        },
    }
}

fn prepare_parent_directory(database_path: &Path) -> Result<()> {
    let parent = database_parent(database_path)?;
    fs::create_dir_all(parent).map_err(|source| ArchiveError::Directory {
        path: parent.to_path_buf(),
        source,
    })?;
    let metadata = fs::metadata(parent).map_err(|source| ArchiveError::Directory {
        path: parent.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(ArchiveError::InvalidArchivePath(
            database_path.to_path_buf(),
        ));
    }
    if !directory_has_write_bit(&metadata) {
        return Err(ArchiveError::DirectoryNotWritable(parent.to_path_buf()));
    }
    Ok(())
}

fn database_parent(database_path: &Path) -> Result<&Path> {
    match database_path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err(ArchiveError::InvalidArchivePath(
            database_path.to_path_buf(),
        )),
    }
}

fn directory_has_write_bit(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn configure_connection(connection: &Connection, database_path: &Path) -> Result<()> {
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|source| ArchiveError::Configure {
            path: database_path.to_path_buf(),
            source,
        })?;
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|source| ArchiveError::Configure {
            path: database_path.to_path_buf(),
            source,
        })?;
    if journal_mode != "wal" {
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(|source| ArchiveError::Configure {
                path: database_path.to_path_buf(),
                source,
            })?;
        if journal_mode != "wal" {
            return Err(ArchiveError::InvalidMigrationPlan(format!(
                "SQLite returned journal_mode={journal_mode}, expected wal"
            )));
        }
    }
    Ok(())
}

fn configure_busy_timeout(connection: &Connection, database_path: &Path) -> Result<()> {
    connection
        .busy_timeout(Duration::from_millis(ARCHIVE_BUSY_TIMEOUT_MS))
        .map_err(|source| ArchiveError::Configure {
            path: database_path.to_path_buf(),
            source,
        })
}

// 用户明确选择不维护逐版本 ALTER：granularity 留在基线，旧库由指纹校验先备份再整体重建。
fn migration_v1(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE usage_record (
            host_id TEXT NOT NULL,
            source TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('message', 'session')) DEFAULT 'message',
            message_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            time_created_utc INTEGER NOT NULL,
            time_completed_utc INTEGER,
            source_time_updated INTEGER NOT NULL,
            origin TEXT NOT NULL CHECK (origin IN ('live', 'bak', 'legacy')),
            origin_priority INTEGER NOT NULL,
            agent_raw TEXT NOT NULL,
            agent_key TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            variant TEXT,
            tok_input INTEGER NOT NULL,
            tok_output INTEGER NOT NULL,
            tok_reasoning INTEGER NOT NULL,
            tok_cache_read INTEGER NOT NULL,
            tok_cache_write INTEGER NOT NULL,
            cost REAL,
            cost_source TEXT NOT NULL CHECK (cost_source IN ('actual', 'unavailable', 'estimated')),
            is_incomplete INTEGER NOT NULL CHECK (is_incomplete IN (0, 1)),
            project_dir TEXT NOT NULL,
            UNIQUE(host_id, source, message_id)
        );

        CREATE TABLE hosts (
            host_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            ssh_target TEXT,
            remote_data_dir TEXT,
            last_success_utc INTEGER,
            machine_id_hash TEXT NOT NULL UNIQUE
        );

        CREATE TABLE source_cursor (
            host_id TEXT NOT NULL,
            source TEXT NOT NULL,
            cursor_time_updated INTEGER NOT NULL,
            PRIMARY KEY(host_id, source)
        );

        CREATE TABLE coverage_interval (
            host_id TEXT NOT NULL,
            source TEXT NOT NULL,
            origin TEXT NOT NULL,
            interval_start INTEGER NOT NULL,
            interval_end INTEGER NOT NULL
        );

        CREATE TABLE app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX usage_record_source_time_created_utc_idx
            ON usage_record(source, time_created_utc);
        CREATE INDEX usage_record_agent_key_time_created_utc_idx
            ON usage_record(agent_key, time_created_utc);
        CREATE INDEX usage_record_provider_id_time_created_utc_idx
            ON usage_record(provider_id, time_created_utc);
        CREATE INDEX usage_record_model_id_time_created_utc_idx
            ON usage_record(model_id, time_created_utc);",
    )
}

fn migration_v2(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    // Aggregate and detail queries are time-range-first, so v2 adds the useful forward index while
    // proving that production supports a genuine v1 -> v2 migration without rewriting records.
    transaction.execute_batch(
        "CREATE INDEX usage_record_time_created_utc_idx
         ON usage_record(time_created_utc);",
    )
}

/// Adds the per-host source set that lets one host be collected by several adapters.
///
/// The `DEFAULT 'opencode'` is the whole upgrade contract: every host row written before this
/// version was collected by exactly the OpenCode adapter, so it keeps being collected by exactly
/// that one. Nothing starts scanning `~/.claude/projects/` until a user enables it on that host.
/// The column is deliberately added here rather than declared in v1, so the v1 -> v3 upgrade path
/// and a fresh create-at-v3 converge on the same schema.
fn migration_v3(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "ALTER TABLE hosts ADD COLUMN enabled_sources TEXT NOT NULL DEFAULT 'opencode';",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use rusqlite::{params, Connection, Transaction};

    use super::*;

    fn create_v1_database(path: &Path) -> Connection {
        let mut connection = Connection::open(path).expect("open v1 database");
        let backup =
            migrate_with(&mut connection, path, &MIGRATIONS[..1]).expect("apply v1 migration");
        assert!(backup.is_some(), "v0 to v1 must create a backup");
        connection
    }

    fn seed_usage_record(connection: &Connection, message_id: &str) {
        connection
            .execute(
                "INSERT INTO usage_record (
                    host_id, source, message_id, session_id,
                    time_created_utc, time_completed_utc, source_time_updated,
                    origin, origin_priority, agent_raw, agent_key,
                    provider_id, model_id, variant,
                    tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
                    cost, cost_source, is_incomplete, project_dir
                ) VALUES (
                    ?1, 'opencode', ?2, 'ses_archive_test',
                    1785468844419, 1785468845419, 1785468846419,
                    'live', 3, 'Atlas - Plan Executor', 'atlas-plan-executor',
                    'myopenai', 'test-model', 'xhigh',
                    10, 20, 30, 40, 50,
                    NULL, 'unavailable', 0, '/tmp/archive-test'
                )",
                params!["host_archive_test", message_id],
            )
            .expect("seed usage_record");
    }

    fn table_columns(connection: &Connection, table: &str) -> BTreeSet<String> {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("prepare table_info");
        statement
            .query_map([], |row| row.get(1))
            .expect("query table_info")
            .collect::<rusqlite::Result<_>>()
            .expect("collect table columns")
    }

    fn index_columns(connection: &Connection, index: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("PRAGMA index_info({index})"))
            .expect("prepare index_info");
        statement
            .query_map([], |row| row.get(2))
            .expect("query index_info")
            .collect::<rusqlite::Result<_>>()
            .expect("collect index columns")
    }

    fn remove_archive_column(path: &Path, table: &str, column: &str) {
        let connection = Connection::open(path).expect("open archive for schema damage");
        connection
            .execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])
            .expect("remove archive column");
        assert_eq!(archive_user_version(&connection), LATEST_SCHEMA_VERSION);
    }

    fn remove_backup_files(directory: &Path) {
        for path in backup_files(directory) {
            fs::remove_file(path).expect("remove setup backup");
        }
    }

    #[test]
    fn archive_smoke_bundled_sqlite_works() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("smoke.db");
        let conn = Connection::open(&db).expect("open");

        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(mode, "wal");

        conn.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            [],
        )
        .expect("create");
        conn.execute("INSERT INTO t (id, v) VALUES (1, ?1)", ["agentlens"])
            .expect("insert");

        let v: String = conn
            .query_row("SELECT v FROM t WHERE id = 1", [], |row| row.get(0))
            .expect("select");
        assert_eq!(v, "agentlens");

        let version: String = conn
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .expect("sqlite_version");
        assert!(
            version.starts_with('3'),
            "unexpected sqlite version: {version}"
        );
    }

    #[test]
    fn archive_error_stays_below_clippy_large_error_threshold() {
        assert!(std::mem::size_of::<ArchiveError>() < 128);
    }

    #[test]
    fn archive_busy_timeout_prevents_parallel_writer_lock_failures() {
        use rusqlite::ffi::ErrorCode;
        use rusqlite::TransactionBehavior;
        use std::sync::mpsc;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let mut holder = Archive::open(&path).expect("open holder archive");
        let waiter = Archive::open(&path).expect("open waiter archive");

        let configured_ms: i64 = waiter
            .connection()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .expect("read archive busy_timeout");
        assert_eq!(configured_ms, ARCHIVE_BUSY_TIMEOUT_MS as i64);

        waiter
            .connection()
            .busy_timeout(Duration::ZERO)
            .expect("disable timeout for negative control");
        let locked_transaction = holder
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("acquire negative-control writer lock");
        locked_transaction
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('negative-holder', 'locked')",
                [],
            )
            .expect("write negative-control holder row");
        let without_timeout = waiter
            .connection()
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('negative-waiter', 'blocked')",
                [],
            )
            .expect_err("a writer without busy_timeout must fail while the lock is held");
        assert!(matches!(
            without_timeout.sqlite_error_code(),
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        ));
        locked_transaction
            .rollback()
            .expect("release negative-control writer lock");
        drop(waiter);

        let waiter = Archive::open(&path).expect("reopen waiter with production timeout");
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let (done_sender, done_receiver) = mpsc::sync_channel(0);
        let rounds = 8;
        let holder_thread = std::thread::spawn(move || {
            for round in 0..rounds {
                let transaction = holder
                    .connection_mut()
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .expect("acquire writer lock");
                transaction
                    .execute(
                        "INSERT INTO app_settings (key, value) VALUES (?1, 'holder')",
                        [format!("holder-{round}")],
                    )
                    .expect("write holder row");
                ready_sender.send(round).expect("signal held writer lock");
                std::thread::sleep(Duration::from_millis(25));
                transaction.commit().expect("commit holder transaction");
                done_receiver.recv().expect("wait for waiter write");
            }
        });

        for round in 0..rounds {
            assert_eq!(
                ready_receiver.recv().expect("wait for held writer lock"),
                round
            );
            waiter
                .connection()
                .execute(
                    "INSERT INTO app_settings (key, value) VALUES (?1, 'waiter')",
                    [format!("waiter-{round}")],
                )
                .expect("busy_timeout must wait for the holder to commit");
            done_sender
                .send(round)
                .expect("signal completed waiter write");
        }
        holder_thread.join().expect("join holder writer");

        let rows: i64 = waiter
            .connection()
            .query_row(
                "SELECT count(*) FROM app_settings WHERE key LIKE 'holder-%' OR key LIKE 'waiter-%'",
                [],
                |row| row.get(0),
            )
            .expect("count contention writes");
        assert_eq!(rows, i64::from(rounds * 2));
        println!(
            "busy_timeout_negative={:?} configured_ms={configured_ms} rounds={rounds} rows={rows}",
            without_timeout.sqlite_error_code()
        );
    }

    #[test]
    fn archive_v1_migration_preserves_rows_and_reaches_the_latest_user_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let connection = create_v1_database(&path);
        seed_usage_record(&connection, "msg_migration_one");
        seed_usage_record(&connection, "msg_migration_two");
        connection
            .execute(
                "INSERT INTO hosts (host_id, display_name, kind, machine_id_hash)
                 VALUES ('host_archive_test', '本机', 'local', ?1)",
                params!["c".repeat(64)],
            )
            .expect("seed a pre-multi-source host row");
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM usage_record", [], |row| row.get(0))
            .expect("count v1 rows");
        drop(connection);

        let archive = Archive::open(&path).expect("migrate v1 archive to the latest version");
        let version: u32 = archive
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read migrated user_version");
        let after: i64 = archive
            .connection()
            .query_row("SELECT COUNT(*) FROM usage_record", [], |row| row.get(0))
            .expect("count migrated rows");
        // The upgrade contract: a host registered before multi-source keeps collecting exactly
        // OpenCode. A different default here would silently start scanning a second data
        // directory on every existing installation.
        let enabled_sources: String = archive
            .connection()
            .query_row(
                "SELECT enabled_sources FROM hosts WHERE host_id = 'host_archive_test'",
                [],
                |row| row.get(0),
            )
            .expect("read backfilled enabled_sources");

        assert_eq!(version, LATEST_SCHEMA_VERSION);
        assert_eq!(before, 2);
        assert_eq!(after, before);
        assert_eq!(enabled_sources, "opencode");
    }

    #[test]
    fn archive_baseline_defaults_usage_granularity_to_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create archive from baseline schema");
        seed_usage_record(archive.connection(), "msg_default_granularity");

        let granularity: String = archive
            .connection()
            .query_row(
                "SELECT granularity FROM usage_record WHERE message_id = 'msg_default_granularity'",
                [],
                |row| row.get(0),
            )
            .expect("read default record granularity");

        assert_eq!(granularity, "message");
        assert_eq!(
            archive_user_version(archive.connection()),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn archive_fresh_database_matches_declared_schema_without_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create fresh archive");

        for (table, expected_columns) in EXPECTED_SCHEMA_COLUMNS {
            let expected_columns = expected_columns
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                table_columns(archive.connection(), table),
                expected_columns,
                "fresh table {table} must match its declared schema fingerprint"
            );
        }
        for (index, dimension) in [
            ("usage_record_source_time_created_utc_idx", "source"),
            ("usage_record_agent_key_time_created_utc_idx", "agent_key"),
            (
                "usage_record_provider_id_time_created_utc_idx",
                "provider_id",
            ),
            ("usage_record_model_id_time_created_utc_idx", "model_id"),
        ] {
            assert_eq!(
                index_columns(archive.connection(), index),
                vec![dimension.to_string(), "time_created_utc".to_string()],
                "fresh archive index {index} must preserve dimension-first range lookup"
            );
        }
        assert!(archive.last_backup_path().is_none());
        assert!(backup_files(dir.path()).is_empty());
    }

    #[test]
    fn archive_incompatible_usage_schema_is_backed_up_and_rebuilt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create current archive");
        seed_usage_record(archive.connection(), "msg_before_schema_reset");
        drop(archive);
        remove_backup_files(dir.path());
        remove_archive_column(&path, "usage_record", "granularity");

        let archive = Archive::open(&path).expect("rebuild incompatible archive");
        let backup_path = archive
            .last_backup_path()
            .expect("schema reset must expose its backup path")
            .to_path_buf();

        assert!(table_columns(archive.connection(), "usage_record").contains("granularity"));
        assert_eq!(
            archive
                .connection()
                .query_row("SELECT COUNT(*) FROM usage_record", [], |row| row
                    .get::<_, i64>(0))
                .expect("count rebuilt usage records"),
            0
        );
        assert!(backup_path.exists());
        let backup = Connection::open(&backup_path).expect("open incompatible-schema backup");
        assert!(!table_columns(&backup, "usage_record").contains("granularity"));
        assert_eq!(
            backup
                .query_row(
                    "SELECT COUNT(*) FROM usage_record WHERE message_id = 'msg_before_schema_reset'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read preserved old usage record"),
            1
        );
    }

    #[test]
    fn archive_incompatible_non_usage_schema_is_backed_up_and_rebuilt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create current archive");
        archive
            .connection()
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('old-setting', 'preserved')",
                [],
            )
            .expect("seed old settings row");
        drop(archive);
        remove_backup_files(dir.path());
        remove_archive_column(&path, "hosts", "enabled_sources");

        let archive = Archive::open(&path).expect("rebuild archive with non-usage schema mismatch");
        let backup_path = archive
            .last_backup_path()
            .expect("generic schema mismatch must create a backup")
            .to_path_buf();

        assert!(table_columns(archive.connection(), "hosts").contains("enabled_sources"));
        assert!(read_app_settings(archive.connection())
            .expect("read rebuilt settings")
            .is_empty());
        let backup = Connection::open(backup_path).expect("open generic mismatch backup");
        assert!(!table_columns(&backup, "hosts").contains("enabled_sources"));
        assert_eq!(
            backup
                .query_row(
                    "SELECT value FROM app_settings WHERE key = 'old-setting'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("read preserved old setting"),
            "preserved"
        );
    }

    #[test]
    fn archive_backup_failure_preserves_incompatible_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create current archive");
        seed_usage_record(archive.connection(), "msg_must_survive_backup_failure");
        drop(archive);
        remove_backup_files(dir.path());
        remove_archive_column(&path, "usage_record", "granularity");
        let failed_backup_path = dir.path().join("injected-backup-failure.db");

        let error = Archive::open_with_schema_backup(&path, |_connection, _database_path| {
            Err(ArchiveError::Backup {
                path: failed_backup_path.clone(),
                source: rusqlite::Error::ExecuteReturnedResults,
            })
        })
        .err()
        .expect("backup failure must abort schema reset");

        assert!(matches!(error, ArchiveError::Backup { .. }));
        assert!(path.exists(), "backup failure must not delete the archive");
        assert!(!failed_backup_path.exists());
        let preserved = Connection::open(&path).expect("reopen preserved incompatible archive");
        assert!(!table_columns(&preserved, "usage_record").contains("granularity"));
        assert_eq!(
            preserved
                .query_row(
                    "SELECT COUNT(*) FROM usage_record WHERE message_id = 'msg_must_survive_backup_failure'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read usage row after backup failure"),
            1
        );
    }

    #[test]
    fn archive_migration_creates_openable_pre_migration_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let connection = create_v1_database(&path);
        seed_usage_record(&connection, "msg_backup_probe");
        drop(connection);

        let archive = Archive::open(&path).expect("migrate archive");
        let backup_path = archive
            .last_backup_path()
            .expect("v1 to v2 migration backup path");
        assert!(backup_path.exists(), "migration backup must exist");

        let backup = Connection::open(backup_path).expect("open migration backup");
        let backup_version: u32 = backup
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read backup user_version");
        let backup_rows: i64 = backup
            .query_row("SELECT COUNT(*) FROM usage_record", [], |row| row.get(0))
            .expect("count backup rows");
        assert_eq!(backup_version, 1);
        assert_eq!(backup_rows, 1);
    }

    #[test]
    fn archive_failed_migration_rolls_back_to_v1_and_returns_error() {
        fn fail_after_partial_change(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
            transaction.execute_batch(
                "CREATE TABLE migration_must_roll_back (id INTEGER PRIMARY KEY);
                 THIS IS NOT VALID SQL;",
            )
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let connection = create_v1_database(&path);
        seed_usage_record(&connection, "msg_rollback_probe");
        drop(connection);

        let injected = [MIGRATIONS[0], Migration::new(2, fail_after_partial_change)];
        let mut connection = Connection::open(&path).expect("reopen v1 database");
        let error = migrate_with(&mut connection, &path, &injected)
            .expect_err("injected migration must fail");
        let error_text = error.to_string();
        assert!(
            error_text.contains("migration from version 1 to 2 failed"),
            "unexpected migration error: {error_text}"
        );

        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read rolled-back user_version");
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM usage_record", [], |row| row.get(0))
            .expect("count rows after failed migration");
        let partial_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'migration_must_roll_back'",
                [],
                |row| row.get(0),
            )
            .expect("check partial migration table");
        assert_eq!(version, 1);
        assert_eq!(rows, 1);
        assert_eq!(partial_table_count, 0);
    }

    #[test]
    fn archive_normalize_agent_key_table() {
        let cases = [
            ("Atlas - Plan Executor", "atlas-plan-executor"),
            ("librarian", "librarian"),
            ("  ATLAS   Plan  ", "atlas-plan"),
            ("Atlas---Plan", "atlas-plan"),
            ("Atlas -  - Plan", "atlas-plan"),
            ("研究 - 助手", "研究-助手"),
            ("", ""),
            ("  ---  ", ""),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_agent_key(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn archive_app_settings_empty_store_reads_as_empty_map() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = Archive::open_in_data_dir(dir.path()).expect("create archive");

        assert_eq!(
            read_app_settings(archive.connection()).expect("read empty settings"),
            BTreeMap::new()
        );
    }

    #[test]
    fn archive_app_settings_transactionally_merges_without_erasing_other_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut archive = Archive::open_in_data_dir(dir.path()).expect("create archive");
        let initial = BTreeMap::from([
            ("report_timezone".to_owned(), "UTC".to_owned()),
            ("week_start".to_owned(), "monday".to_owned()),
        ]);
        write_app_settings(archive.connection_mut(), &initial).expect("write initial settings");

        let update = BTreeMap::from([
            ("report_timezone".to_owned(), "Asia/Shanghai".to_owned()),
            ("local_refresh_ms".to_owned(), "300000".to_owned()),
        ]);
        write_app_settings(archive.connection_mut(), &update).expect("merge settings");

        assert_eq!(
            read_app_settings(archive.connection()).expect("read merged settings"),
            BTreeMap::from([
                ("local_refresh_ms".to_owned(), "300000".to_owned()),
                ("report_timezone".to_owned(), "Asia/Shanghai".to_owned()),
                ("week_start".to_owned(), "monday".to_owned()),
            ])
        );
    }

    #[test]
    fn archive_corrupt_database_returns_readable_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        fs::write(&path, b"this is not a sqlite database").expect("write corrupt database");

        let error = Archive::open(&path)
            .err()
            .expect("opening corrupt database must return Err");
        let error_text = error.to_string();
        println!("corrupt_db_error={error_text}");
        assert!(error_text.contains(path.to_string_lossy().as_ref()));
        assert!(error_text.contains("restore a backup or move the corrupt file aside"));
        assert!(error_text.contains("file is not a database"));
    }

    #[test]
    fn archive_reopen_is_idempotent_without_backup_spam() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let first = Archive::open(&path).expect("create archive");
        assert_eq!(
            archive_user_version(first.connection()),
            LATEST_SCHEMA_VERSION
        );
        first
            .connection()
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('reopen-proof', 'preserved')",
                [],
            )
            .expect("seed compatible archive");
        assert!(first.last_backup_path().is_none());
        drop(first);

        let backups_before = backup_files(dir.path());
        assert!(backups_before.is_empty());
        let second = Archive::open(&path).expect("reopen archive");
        let backups_after = backup_files(dir.path());

        assert_eq!(
            archive_user_version(second.connection()),
            LATEST_SCHEMA_VERSION
        );
        assert!(second.last_backup_path().is_none());
        assert_eq!(backups_after, backups_before);
        assert_eq!(
            second
                .connection()
                .query_row(
                    "SELECT value FROM app_settings WHERE key = 'reopen-proof'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("compatible reopen must preserve data"),
            "preserved"
        );
    }

    #[test]
    fn archive_future_schema_returns_clear_error_without_downgrade() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let archive = Archive::open(&path).expect("create archive");
        drop(archive);
        let connection = Connection::open(&path).expect("open archive for future version probe");
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("set future user_version");
        drop(connection);
        let backups_before = backup_files(dir.path());

        let error = Archive::open(&path)
            .err()
            .expect("future schema must be rejected");
        let error_text = error.to_string();
        println!("future_version_error={error_text}");
        assert!(error_text.contains(&format!(
            "schema version {} is newer",
            LATEST_SCHEMA_VERSION + 1
        )));
        assert!(error_text.contains("upgrade AgentLens instead of downgrading"));
        assert_eq!(backup_files(dir.path()), backups_before);

        let connection = Connection::open(&path).expect("reopen rejected future archive");
        assert_eq!(archive_user_version(&connection), LATEST_SCHEMA_VERSION + 1);
    }

    #[cfg(unix)]
    #[test]
    fn archive_injected_data_directory_rejects_non_writable_archive_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let archive_directory = dir.path().join(ARCHIVE_DIRECTORY);
        fs::create_dir(&archive_directory).expect("create archive directory");
        fs::set_permissions(&archive_directory, fs::Permissions::from_mode(0o555))
            .expect("make archive directory read-only");

        let error = Archive::open_in_data_dir(dir.path())
            .err()
            .expect("non-writable directory must be rejected");
        fs::set_permissions(&archive_directory, fs::Permissions::from_mode(0o755))
            .expect("restore archive directory permissions");

        let error_text = error.to_string();
        println!("non_writable_directory_error={error_text}");
        assert!(error_text.contains("archive directory is not writable"));
        assert!(error_text.contains("grant write permission"));
        assert!(!archive_path_in(dir.path()).exists());
    }

    #[test]
    fn archive_interrupted_second_of_three_migrations_rolls_back_and_keeps_backup() {
        fn fail_second_migration(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
            transaction.execute_batch(
                "CREATE TABLE interrupted_partial_table (id INTEGER PRIMARY KEY);
                 INTERRUPT THIS MIGRATION;",
            )
        }

        fn third_migration(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
            transaction.execute_batch(
                "CREATE TABLE third_migration_must_not_run (id INTEGER PRIMARY KEY);",
            )
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("archive.db");
        let injected = [
            Migration::new(1, migration_v1),
            Migration::new(2, fail_second_migration),
            Migration::new(3, third_migration),
        ];
        let mut connection = Connection::open(&path).expect("open empty database");
        let error = migrate_with(&mut connection, &path, &injected)
            .expect_err("second migration must interrupt the run");
        let backup_path = match &error {
            ArchiveError::Migration { backup_path, .. } => backup_path.clone(),
            other => panic!("unexpected interruption error: {other}"),
        };

        assert_eq!(archive_user_version(&connection), 0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('usage_record', 'interrupted_partial_table', 'third_migration_must_not_run')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("inspect rolled-back schema"),
            0
        );
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity check rolled-back archive");
        assert_eq!(integrity, "ok");

        assert!(backup_path.exists());
        let backup = Connection::open(&backup_path).expect("open interruption backup");
        assert_eq!(archive_user_version(&backup), 0);
        let backup_integrity: String = backup
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity check interruption backup");
        assert_eq!(backup_integrity, "ok");
    }

    #[test]
    fn archive_migration_plan_rejects_gaps_and_accepts_an_empty_zero_version_database() {
        fn no_op(_transaction: &Transaction<'_>) -> rusqlite::Result<()> {
            Ok(())
        }

        let starts_at_two = [Migration::new(2, no_op)];
        let error = validate_migration_plan(&starts_at_two)
            .expect_err("a migration plan must start at version one");
        assert!(matches!(error, ArchiveError::InvalidMigrationPlan(_)));
        assert!(error
            .to_string()
            .contains("expected target version 1, found 2"));

        let skips_two = [Migration::new(1, no_op), Migration::new(3, no_op)];
        let error = validate_migration_plan(&skips_two)
            .expect_err("a migration plan must remain contiguous");
        assert!(error
            .to_string()
            .contains("expected target version 2, found 3"));

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("empty-plan.db");
        let mut connection = Connection::open(&path).expect("open zero-version database");
        assert_eq!(
            migrate_with(&mut connection, &path, &[]).expect("empty plan is already current"),
            None
        );
        assert_eq!(archive_user_version(&connection), 0);
        assert!(backup_files(temp.path()).is_empty());
    }

    #[test]
    fn archive_open_reports_directory_targets_and_blocked_parents_with_exact_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory_target = temp.path().join("database-is-directory");
        fs::create_dir(&directory_target).expect("create directory target");
        let error = Archive::open(&directory_target)
            .err()
            .expect("SQLite cannot open a directory as a database");
        assert!(matches!(
            error,
            ArchiveError::Open { ref path, .. } if path == &directory_target
        ));

        let blocked_parent = temp.path().join("parent-is-file");
        fs::write(&blocked_parent, b"not a directory").expect("create blocking parent file");
        let blocked_path = blocked_parent.join("archive.db");
        let error = Archive::open(&blocked_path)
            .err()
            .expect("a regular file cannot become the archive parent");
        assert!(matches!(
            error,
            ArchiveError::Directory { ref path, .. } if path == &blocked_parent
        ));
        assert!(error
            .to_string()
            .contains("cannot prepare archive directory"));

        assert!(matches!(
            next_backup_path(Path::new("")),
            Err(ArchiveError::InvalidArchivePath(path)) if path.as_os_str().is_empty()
        ));
    }

    #[test]
    fn archive_app_settings_errors_remain_typed_when_schema_is_absent() {
        let mut connection = Connection::open_in_memory().expect("open schema-less database");
        let read_error = read_app_settings(&connection)
            .expect_err("reading settings without its table must fail");
        assert!(matches!(read_error, ArchiveError::AppSettings(_)));
        assert!(read_error
            .to_string()
            .contains("no such table: app_settings"));

        let settings = BTreeMap::from([("timezone".to_string(), "UTC".to_string())]);
        let write_error = write_app_settings(&mut connection, &settings)
            .expect_err("writing settings without its table must fail atomically");
        assert!(matches!(write_error, ArchiveError::AppSettings(_)));
        assert!(write_error
            .to_string()
            .contains("no such table: app_settings"));
    }

    #[test]
    fn archive_connection_configuration_rejects_backends_that_cannot_enter_wal_mode() {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        let path = Path::new(":memory:");
        let error = configure_connection(&connection, path)
            .expect_err("an in-memory backend cannot satisfy the persistent WAL contract");
        assert!(matches!(error, ArchiveError::InvalidMigrationPlan(_)));
        assert!(error
            .to_string()
            .contains("SQLite returned journal_mode=memory, expected wal"));
    }

    #[test]
    #[ignore = "manual QA requires the external sqlite3 binary"]
    fn archive_manual_qa_external_sqlite3() {
        let dir = tempfile::tempdir().expect("tempdir");
        let directory = dir.path().to_path_buf();
        let archive = Archive::open_in_data_dir(&directory).expect("create archive through API");
        let path = archive.path().to_path_buf();
        drop(archive);

        for (label, statement) in [
            ("schema", ".schema"),
            ("user_version", "PRAGMA user_version;"),
            ("integrity_check", "PRAGMA integrity_check;"),
            (
                "table_info_usage_record",
                "PRAGMA table_info(usage_record);",
            ),
        ] {
            let output = Command::new("sqlite3")
                .arg(&path)
                .arg(statement)
                .output()
                .expect("run external sqlite3");
            assert!(output.status.success(), "sqlite3 {label} failed");
            println!(
                "--- sqlite3 {label} ---\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }

        let first_insert = "INSERT INTO usage_record (
            host_id, source, message_id, session_id,
            time_created_utc, time_completed_utc, source_time_updated,
            origin, origin_priority, agent_raw, agent_key, provider_id, model_id, variant,
            tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
            cost, cost_source, is_incomplete, project_dir
        ) VALUES (
            'qa-host', 'opencode', 'qa-message', 'qa-session',
            1785468844419, NULL, 1785468845419,
            'live', 3, 'Atlas - Plan Executor', 'atlas-plan-executor',
            'qa-provider', 'qa-model', NULL,
            1, 2, 3, 4, 5,
            NULL, 'unavailable', 0, '/tmp/qa-project'
        );";
        let output = Command::new("sqlite3")
            .arg(&path)
            .arg(first_insert)
            .output()
            .expect("insert first external row");
        assert!(output.status.success(), "first sqlite3 insert failed");

        let conflicting_insert = "INSERT INTO usage_record (
            host_id, source, message_id, session_id,
            time_created_utc, time_completed_utc, source_time_updated,
            origin, origin_priority, agent_raw, agent_key, provider_id, model_id, variant,
            tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
            cost, cost_source, is_incomplete, project_dir
        ) SELECT
            host_id, source, message_id, session_id,
            time_created_utc, time_completed_utc, source_time_updated,
            origin, 2, agent_raw, agent_key, provider_id, model_id, variant,
            tok_input, tok_output, tok_reasoning, tok_cache_read, tok_cache_write,
            cost, cost_source, is_incomplete, project_dir
        FROM usage_record WHERE host_id = 'qa-host' AND source = 'opencode'
            AND message_id = 'qa-message';";
        let output = Command::new("sqlite3")
            .arg(&path)
            .arg(conflicting_insert)
            .output()
            .expect("insert conflicting external row");
        let constraint_error = String::from_utf8_lossy(&output.stderr);
        println!(
            "--- sqlite3 unique_constraint ---\nexit={}\n{}",
            output.status, constraint_error
        );
        assert!(!output.status.success());
        assert!(constraint_error.contains(
            "UNIQUE constraint failed: usage_record.host_id, usage_record.source, usage_record.message_id"
        ));

        dir.close().expect("remove manual QA tempdir");
        assert!(!directory.exists());
        println!("--- cleanup_receipt ---\nremoved {}", directory.display());
    }

    fn archive_user_version(connection: &Connection) -> u32 {
        connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read archive user_version")
    }

    fn backup_files(directory: &Path) -> Vec<PathBuf> {
        let mut files = fs::read_dir(directory)
            .expect("read archive directory")
            .map(|entry| entry.expect("read archive directory entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("archive.db.backup-"))
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }
}
