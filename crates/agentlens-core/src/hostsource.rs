//! 主机来源抽象与刷新调度（todo 12）。
//!
//! 本模块定义 `HostSource` trait（`probe` + `collect_incremental`）及其实现：
//! `LocalHostSource`（进程内直扫 OpenCode）、`ClaudeCodeLocalSource`（进程内直扫 Claude Code
//! transcript）与 `SshHostSource`（复用 `crate::transport::ssh`，见 todo 11；
//! 通过 `with_source` 选择远端适配器）。同一主机的多个 source 各自持有
//! `source_cursor` 与 `coverage_interval` 行，互不回退、互不遮蔽。
//!
//! 同时承载刷新调度器：本地源定时增量（默认间隔 `max(300s, 3×上轮扫描耗时)`，可配）
//! 加手动触发；远程源默认手动、可配定时（默认 15min）；单源串行、多源并行、同源防重入；
//! 每源维护 `idle` / `running` / `error { last_error, last_success }` 状态机供 UI 查询。
//!
//! daemon 形态仅在 trait 文档注释中注明预留，本期不实现。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::archive::{Archive, NormalizedUsageRecord, Origin};
use crate::host::{HostError, HostKind, HostRecord, HostRegistry, MachineIdentity};
use crate::ingest::{
    ingest_by_origin_with_coverage, read_cursor, read_source_cursor, CoverageWindow, IngestError,
    IngestRound, IngestStats, OPENCODE_SOURCE,
};
use crate::source::claude_code::{self, CLAUDE_CODE_SOURCE};
use crate::source::codex::{self, CODEX_SOURCE};
use crate::source::hermes::{self, HERMES_SOURCE};
use crate::source::opencode::{
    scan_connection, OpenCodeError, ScanRequest, ScanResult, SinkError, SkippedBreakdown,
    SourceConnection, SqliteSourceConnection,
};
use crate::source::opencode_legacy::{CoverageInterval, LegacyError};
use crate::transport::ssh::{
    CommandRunner, SshCollectRequest, SshCollection, SshError, SshProbe, SshTransport,
};

/// Provenance of every scheduled incremental round. Backfills (`bak`/`legacy`) are never
/// scheduled: they are one-shot imports owned by todo 7 and must not move the live watermark.
pub const INCREMENTAL_ORIGIN: Origin = Origin::Live;

/// Wire protocol version accepted from a remote collector.
pub const REMOTE_PROTOCOL_VERSION: u32 = 1;

/// Hard floor for **any** auto-refresh interval, in milliseconds (10 min).
///
/// [`SourceSchedule::validate`] rejects anything below it, so this is the guarantee rather than
/// the UI clamp. One remote round starts six `ssh`/`scp` processes and a full local scan of a real
/// 43 GB archive measured 23.3 s, so a shorter poll across several hosts risks overlapping rounds.
pub const MIN_AUTO_REFRESH_INTERVAL_MS: u64 = 600_000;

/// Floor of the adaptive local refresh interval, in milliseconds (10 min).
///
/// Equal to [`MIN_AUTO_REFRESH_INTERVAL_MS`]: a default below the configurable floor would poll
/// more aggressively than a user is allowed to ask for.
pub const DEFAULT_LOCAL_MIN_INTERVAL_MS: u64 = MIN_AUTO_REFRESH_INTERVAL_MS;

/// Default remote timer interval, in milliseconds (15 min). Remote sources are manual by default;
/// this value applies only once a timer is configured.
pub const DEFAULT_REMOTE_INTERVAL_MS: u64 = 900_000;

/// Multiplier applied to the previous round's measured duration.
pub const DEFAULT_DURATION_MULTIPLIER: u32 = 3;

/// Busy timeout applied to archive handles that participate in cross-source parallel rounds.
pub const DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS: u64 = 30_000;

/// Result alias for host-source operations.
pub type Result<T> = std::result::Result<T, HostSourceError>;

/// Errors raised while probing or collecting one host source.
#[derive(Debug, Error)]
pub enum HostSourceError {
    /// The host record claims [`HostKind::Ssh`] but carries no usable destination.
    #[error("主机 {host_id} 声明为 ssh 源，但缺少 ssh_target")]
    MissingSshTarget {
        /// Host that failed validation.
        host_id: String,
    },
    /// A host record of the wrong kind was handed to a source implementation.
    #[error("主机 {host_id} 的 kind 为 {found}，无法构造 {expected} 源")]
    KindMismatch {
        /// Host that failed validation.
        host_id: String,
        /// Kind required by the source implementation.
        expected: &'static str,
        /// Kind actually stored on the record.
        found: &'static str,
    },
    /// The remote collector reported a different machine than the one registered for this host.
    #[error(
        "远端机器身份不匹配：注册主机 {expected_host_id} 期望 machine_id_hash {expected_hash}，\
远端返回 {found_hash}（host_id {found_host_id}）"
    )]
    MachineIdentityMismatch {
        /// Host identifier stored in the archive.
        expected_host_id: String,
        /// Machine hash stored in the archive.
        expected_hash: String,
        /// Host identifier derived from the remote response.
        found_host_id: String,
        /// Machine hash reported by the remote collector.
        found_hash: String,
    },
    /// The remote response carried no metadata line, or an unsupported protocol version.
    #[error("远端响应不符合 v1 协议：{detail}")]
    InvalidRemoteResponse {
        /// Human-readable explanation.
        detail: String,
    },
    /// The remote response did not describe the source this host collects.
    #[error("远端响应缺少 source={expected_source} 的 meta 条目（收到 {found:?}）")]
    RemoteSourceMissing {
        /// Source key expected by this host source.
        expected_source: String,
        /// Source keys actually present.
        found: Vec<String>,
    },
    /// A record line could not be decoded into the shared archive contract.
    #[error("远端 record 行 {line} 无法解析为归一化记录：{detail}")]
    MalformedRemoteRecord {
        /// One-based NDJSON line number.
        line: usize,
        /// Serde error text.
        detail: String,
    },
    /// Host registry failure, including "the host row disappeared".
    #[error(transparent)]
    Host(#[from] HostError),
    /// OpenCode source scanner failure.
    #[error(transparent)]
    OpenCode(#[from] OpenCodeError),
    /// Claude Code source scanner failure.
    #[error(transparent)]
    ClaudeCode(#[from] claude_code::ClaudeCodeError),
    /// Codex source scanner failure.
    #[error(transparent)]
    Codex(#[from] codex::CodexError),
    /// Hermes source scanner failure.
    #[error(transparent)]
    Hermes(#[from] hermes::HermesError),
    /// Archive ingest failure.
    #[error(transparent)]
    Ingest(#[from] IngestError),
    /// Coverage bookkeeping failure.
    #[error(transparent)]
    Legacy(#[from] LegacyError),
    /// SSH transport failure.
    #[error(transparent)]
    Ssh(#[from] SshError),
    /// Direct SQLite failure while configuring an archive handle.
    #[error("归档连接配置失败：{0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl HostSourceError {
    /// UI-ready Chinese remediation, when the underlying layer publishes one.
    ///
    /// Only [`SshError`] currently carries per-variant remediation (todo 11); the remaining
    /// variants are surfaced through their `Display` text.
    pub const fn remediation(&self) -> Option<&'static str> {
        match self {
            Self::Ssh(error) => Some(error.remediation()),
            _ => None,
        }
    }
}

/// Configuration-level readiness facts for one source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProbe {
    /// Archive host identifier this source writes under.
    pub host_id: String,
    /// Collection mode.
    pub kind: HostKind,
    /// Source key written into `usage_record.source`.
    pub source: String,
    /// Local database path or SSH destination, whichever identifies this source.
    pub location: String,
    /// `true` when the deep (remote) facts are only observable during a collection round.
    ///
    /// Todo 11 fuses the remote STAGE1 probe into `collect`, so an SSH source reports its remote
    /// architecture, XDG data home, free space and machine-id source through
    /// [`CollectOutcome::remote_probe`] rather than here.
    pub remote_facts_deferred: bool,
}

/// Facts a remote collector reported about itself in the v1 metadata line.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSourceReport {
    /// 64-hex machine hash, reproduced by the collector from todo 4's contract.
    pub machine_id_hash: String,
    /// Remote hostname, for display only.
    pub hostname: String,
    /// Collector binary version.
    pub collector_version: String,
    /// Remote OpenCode data directory actually scanned.
    pub data_dir: String,
    /// Cursor the collector was asked to resume from.
    pub since: i64,
    /// Highest observed `time_updated`, or `since` when the remote scan saw nothing.
    pub cutoff: i64,
    /// Eligible assistant rows the remote scanner produced.
    pub eligible_count: u64,
    /// Lossy skips the remote scanner counted.
    pub skipped_count: u64,
}

/// Observable result of one incremental collection round.
#[derive(Clone, Debug)]
pub struct CollectOutcome {
    /// Archive host identifier.
    pub host_id: String,
    /// Source key.
    pub source: String,
    /// Whether the source stream reached EOF; the sole gate for watermark progress (todo 6).
    pub reached_eof: bool,
    /// Eligible rows observed by the source scanner.
    pub eligible_count: u64,
    /// Lossy skips counted by the source scanner.
    pub skipped_count: u64,
    /// Ingest counters, including whether the whole-round transaction committed.
    pub stats: IngestStats,
    /// Live coverage interval after a successful round; `None` when nothing committed.
    pub coverage: Option<CoverageInterval>,
    /// Remote collector self-report; `None` for local sources.
    pub remote: Option<RemoteSourceReport>,
    /// Remote STAGE1 facts; `None` for local sources.
    pub remote_probe: Option<SshProbe>,
}

impl CollectOutcome {
    /// A round counts as success only when the stream reached EOF **and** the round committed.
    ///
    /// A collector that returns `Ok` with zero rows and `reached_eof == false` is therefore not a
    /// success: it advances neither the watermark nor `last_success`.
    pub const fn is_success(&self) -> bool {
        self.reached_eof && self.stats.committed
    }

    /// Scheduler-facing projection of this outcome.
    pub const fn summary(&self) -> CollectSummary {
        CollectSummary {
            reached_eof: self.reached_eof,
            committed: self.stats.committed,
            eligible_count: self.eligible_count,
            changed_records: self.stats.changed_records,
            cursor_time_updated: self.stats.cursor_time_updated,
        }
    }
}

/// One collectable host source.
///
/// Implementations are **stateless with respect to scheduling**: they perform exactly one
/// incremental round when asked and never decide *when* to run. Ordering, re-entrancy and status
/// live in [`RefreshScheduler`].
///
/// Two implementations ship in this release:
///
/// * [`LocalHostSource`] — in-process read-only scan of the local OpenCode database (todo 5) fed
///   through the archive ingest round (todo 6).
/// * [`SshHostSource`] — the constant-command SSH collector transport (todo 11).
///
/// # Reserved shapes (documented, intentionally not implemented)
///
/// * **`RemoteService`** — an HTTP source backed by the AgentLens Remote Source API v1
///   (`docs/remote-source-api.md`): `GET /v1/meta` plus per-source `GET /v1/records?source=…&since=…`,
///   bearer token obtained through the one-shot pairing code, plaintext HTTP restricted to
///   loopback. Its wire DTOs are already the same serde types the SSH collector emits, so adding
///   it is a transport swap behind this trait rather than a new archive contract. Not implemented
///   in this release.
/// * **daemon form** — a long-lived background process that ticks [`RefreshScheduler`] outside the
///   desktop shell. Reserved so the scheduler stays a pure state machine (no runtime, no threads
///   inside this library); not implemented in this release.
pub trait HostSource {
    /// Archive host identifier this source writes under.
    fn host_id(&self) -> &str;

    /// Collection mode, mirroring `hosts.kind`.
    fn kind(&self) -> HostKind;

    /// Validates that this source is configured and reachable enough to attempt a round.
    fn probe(&self) -> Result<SourceProbe>;

    /// Runs exactly one incremental round, ingesting into `archive`.
    ///
    /// `now_utc_ms` is the caller-supplied wall clock (UTC epoch milliseconds). It is used as the
    /// live coverage cutoff, so it must never be read from inside an implementation: callers inject
    /// it through [`Clock`] and tests inject a fixed value.
    fn collect_incremental(&self, archive: &mut Archive, now_utc_ms: i64)
        -> Result<CollectOutcome>;
}

/// Applies a SQLite busy timeout to an archive handle.
///
/// Cross-source parallelism gives each in-flight round its own [`Archive`] handle to the same WAL
/// database. [`IngestRound`] holds one transaction for the whole round, so a second writer would
/// otherwise fail immediately with `SQLITE_BUSY`; with a timeout it queues instead.
pub fn set_archive_busy_timeout(archive: &Archive, timeout_ms: u64) -> Result<()> {
    archive
        .connection()
        .busy_timeout(Duration::from_millis(timeout_ms))?;
    Ok(())
}

/// In-process incremental source for the local OpenCode database.
#[derive(Clone, Debug)]
pub struct LocalHostSource {
    host_id: String,
    database_path: PathBuf,
    batch_size: usize,
}

impl LocalHostSource {
    /// Builds a source over an explicit database path.
    pub fn with_database(host_id: impl Into<String>, database_path: impl Into<PathBuf>) -> Self {
        Self {
            host_id: host_id.into(),
            database_path: database_path.into(),
            batch_size: crate::source::opencode::DEFAULT_BATCH_SIZE,
        }
    }

    /// Builds a source using todo 5's documented data-directory precedence order.
    pub fn discover(host_id: impl Into<String>) -> Result<Self> {
        let database_path = crate::source::opencode::discover_database_path()?;
        Ok(Self::with_database(host_id, database_path))
    }

    /// Overrides the Rust-side delivery batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Database this source scans read-only.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Runs one round against an injected [`SourceConnection`].
    ///
    /// This is the seam that makes interrupted rounds (`reached_eof == false`) testable without
    /// corrupting a real database; [`HostSource::collect_incremental`] opens the production
    /// read-only connection and delegates here.
    pub fn collect_from_connection<C: SourceConnection>(
        &self,
        archive: &mut Archive,
        connection: &mut C,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let watermark = read_cursor(archive.connection(), &self.host_id)?;
        let last_success_utc = HostRegistry::new(archive.connection())
            .get(&self.host_id)?
            .and_then(|host| host.last_success_utc);
        let request = ScanRequest {
            host_id: self.host_id.clone(),
            watermark,
            origin: INCREMENTAL_ORIGIN,
            last_success_utc,
            batch_size: self.batch_size,
        };
        let coverage_window = CoverageWindow::new(watermark.unwrap_or(0).max(0), now_utc_ms)?;

        let mut ingest_failure = None::<IngestError>;
        let mut round =
            IngestRound::begin(archive.connection_mut(), &self.host_id, INCREMENTAL_ORIGIN)?;
        let scan = scan_connection(connection, &request, |batch| {
            match round.ingest_batch(batch) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let message = error.to_string();
                    ingest_failure = Some(error);
                    Err(SinkError::new(message))
                }
            }
        })?;
        if let Some(error) = ingest_failure {
            drop(round);
            return Err(HostSourceError::Ingest(error));
        }
        let stats = round.finish_with_coverage(
            scan.reached_eof,
            scan.observed_max_time_updated,
            Some(coverage_window),
        )?;
        let coverage = committed_coverage(&stats, scan.reached_eof, coverage_window)?;

        Ok(CollectOutcome {
            host_id: self.host_id.clone(),
            source: OPENCODE_SOURCE.to_owned(),
            reached_eof: scan.reached_eof,
            eligible_count: scan.eligible_count,
            skipped_count: scan.skipped_count,
            stats,
            coverage,
            remote: None,
            remote_probe: None,
        })
    }
}

impl HostSource for LocalHostSource {
    fn host_id(&self) -> &str {
        &self.host_id
    }

    fn kind(&self) -> HostKind {
        HostKind::Local
    }

    fn probe(&self) -> Result<SourceProbe> {
        let connection = SqliteSourceConnection::open(&self.database_path)?;
        if !connection.query_only()? {
            return Err(HostSourceError::OpenCode(OpenCodeError::QueryOnlyDisabled));
        }
        Ok(SourceProbe {
            host_id: self.host_id.clone(),
            kind: HostKind::Local,
            source: OPENCODE_SOURCE.to_owned(),
            location: self.database_path.display().to_string(),
            remote_facts_deferred: false,
        })
    }

    fn collect_incremental(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let mut connection = SqliteSourceConnection::open(&self.database_path)?;
        self.collect_from_connection(archive, &mut connection, now_utc_ms)
    }
}

/// In-process incremental source for the local Claude Code transcript tree.
///
/// Registered alongside [`LocalHostSource`] on the same host: both write under one `host_id` but
/// keep separate `source_cursor` and `coverage_interval` rows, so neither can rewind or mask the
/// other. Transcripts are read-only; the source directory is never written.
#[derive(Clone, Debug)]
pub struct ClaudeCodeLocalSource {
    host_id: String,
    projects_dir: PathBuf,
    batch_size: usize,
}

impl ClaudeCodeLocalSource {
    /// Builds a source over an explicit `projects` directory.
    pub fn with_projects_dir(host_id: impl Into<String>, projects_dir: impl Into<PathBuf>) -> Self {
        Self {
            host_id: host_id.into(),
            projects_dir: projects_dir.into(),
            batch_size: claude_code::DEFAULT_BATCH_SIZE,
        }
    }

    /// Builds a source using the documented data-directory precedence order.
    pub fn discover(host_id: impl Into<String>) -> Result<Self> {
        let projects_dir = claude_code::discover_projects_dir()?;
        Ok(Self::with_projects_dir(host_id, projects_dir))
    }

    /// Overrides the Rust-side delivery batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Transcript root this source walks read-only.
    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    /// Runs one round against an injected [`claude_code::TranscriptSource`].
    ///
    /// This is the seam that makes interrupted rounds and unreadable transcripts testable without
    /// filesystem permission tricks.
    pub fn collect_from_transcripts<S>(
        &self,
        archive: &mut Archive,
        transcripts: &S,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome>
    where
        S: claude_code::TranscriptSource + ?Sized,
    {
        let watermark =
            read_source_cursor(archive.connection(), &self.host_id, CLAUDE_CODE_SOURCE)?;
        let last_success_utc = HostRegistry::new(archive.connection())
            .get(&self.host_id)?
            .and_then(|host| host.last_success_utc);
        let request = claude_code::ScanRequest {
            host_id: self.host_id.clone(),
            watermark,
            origin: INCREMENTAL_ORIGIN,
            last_success_utc,
            batch_size: self.batch_size,
        };
        let coverage_window = CoverageWindow::new(watermark.unwrap_or(0).max(0), now_utc_ms)?;

        let mut ingest_failure = None::<IngestError>;
        let mut round = IngestRound::begin_for_source(
            archive.connection_mut(),
            &self.host_id,
            CLAUDE_CODE_SOURCE,
            INCREMENTAL_ORIGIN,
        )?;
        let scan = claude_code::scan_source(transcripts, &request, |batch| {
            match round.ingest_batch(batch) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let message = error.to_string();
                    ingest_failure = Some(error);
                    Err(claude_code::SinkError::new(message))
                }
            }
        })?;
        if let Some(error) = ingest_failure {
            drop(round);
            return Err(HostSourceError::Ingest(error));
        }
        let stats = round.finish_with_coverage(
            scan.reached_eof,
            scan.observed_max_time_updated,
            Some(coverage_window),
        )?;
        let coverage = committed_coverage(&stats, scan.reached_eof, coverage_window)?;

        Ok(CollectOutcome {
            host_id: self.host_id.clone(),
            source: CLAUDE_CODE_SOURCE.to_owned(),
            reached_eof: scan.reached_eof,
            eligible_count: scan.eligible_count,
            skipped_count: scan.skipped_count,
            stats,
            coverage,
            remote: None,
            remote_probe: None,
        })
    }
}

impl HostSource for ClaudeCodeLocalSource {
    fn host_id(&self) -> &str {
        &self.host_id
    }

    fn kind(&self) -> HostKind {
        HostKind::Local
    }

    fn probe(&self) -> Result<SourceProbe> {
        if !self.projects_dir.is_dir() {
            return Err(HostSourceError::ClaudeCode(
                claude_code::ClaudeCodeError::ProjectsNotFound {
                    probed_paths: vec![self.projects_dir.clone()],
                },
            ));
        }
        Ok(SourceProbe {
            host_id: self.host_id.clone(),
            kind: HostKind::Local,
            source: CLAUDE_CODE_SOURCE.to_owned(),
            location: self.projects_dir.display().to_string(),
            remote_facts_deferred: false,
        })
    }

    fn collect_incremental(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let transcripts = claude_code::DirectoryTranscriptSource::new(&self.projects_dir);
        self.collect_from_transcripts(archive, &transcripts, now_utc_ms)
    }
}

/// In-process incremental source for the local Codex rollout tree.
#[derive(Clone, Debug)]
pub struct CodexLocalSource {
    host_id: String,
    data_dir: PathBuf,
    batch_size: usize,
}

impl CodexLocalSource {
    /// Builds a source over an explicit Codex data directory.
    pub fn with_data_dir(host_id: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            host_id: host_id.into(),
            data_dir: data_dir.into(),
            batch_size: codex::DEFAULT_BATCH_SIZE,
        }
    }

    /// Builds a source using `CODEX_HOME` and `~/.codex` discovery.
    pub fn discover(host_id: impl Into<String>) -> Result<Self> {
        let data_dir = codex::discover_data_dir()?;
        Ok(Self::with_data_dir(host_id, data_dir))
    }

    /// Overrides the Rust-side delivery batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Codex data root this source walks read-only.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn collect_from_data_dir(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let watermark = read_source_cursor(archive.connection(), &self.host_id, CODEX_SOURCE)?;
        let last_success_utc = HostRegistry::new(archive.connection())
            .get(&self.host_id)?
            .and_then(|host| host.last_success_utc);
        let request = codex::ScanRequest {
            host_id: self.host_id.clone(),
            watermark,
            origin: INCREMENTAL_ORIGIN,
            last_success_utc,
            batch_size: self.batch_size,
        };
        let coverage_window = CoverageWindow::new(watermark.unwrap_or(0).max(0), now_utc_ms)?;

        // Buffered rather than ingested inside the sink because the tier a record belongs to is a
        // property of the record, and one open transaction can only accept one tier. The scanner
        // already materializes every record before it delivers the first batch, so this adds no
        // asymptotic memory the walk did not already require.
        let mut collected = Vec::new();
        let scan = codex::scan_data_dir(&self.data_dir, &request, |batch| {
            collected.extend_from_slice(batch);
            Ok(())
        })?;
        let stats = ingest_by_origin_with_coverage(
            archive.connection_mut(),
            &self.host_id,
            CODEX_SOURCE,
            &collected,
            scan.reached_eof,
            None,
            Some(coverage_window),
        )?;
        let coverage = committed_coverage(&stats, scan.reached_eof, coverage_window)?;

        Ok(CollectOutcome {
            host_id: self.host_id.clone(),
            source: CODEX_SOURCE.to_owned(),
            reached_eof: scan.reached_eof,
            eligible_count: scan.eligible_count,
            skipped_count: scan.skipped_count,
            stats,
            coverage,
            remote: None,
            remote_probe: None,
        })
    }
}

impl HostSource for CodexLocalSource {
    fn host_id(&self) -> &str {
        &self.host_id
    }

    fn kind(&self) -> HostKind {
        HostKind::Local
    }

    fn probe(&self) -> Result<SourceProbe> {
        if !self.data_dir.is_dir()
            || !(self.data_dir.join(codex::SESSIONS_DIRECTORY).is_dir()
                || self
                    .data_dir
                    .join(codex::ARCHIVED_SESSIONS_DIRECTORY)
                    .is_dir())
        {
            return Err(HostSourceError::Codex(
                codex::CodexError::DataDirectoryNotFound {
                    probed_paths: vec![self.data_dir.clone()],
                },
            ));
        }
        Ok(SourceProbe {
            host_id: self.host_id.clone(),
            kind: HostKind::Local,
            source: CODEX_SOURCE.to_owned(),
            location: self.data_dir.display().to_string(),
            remote_facts_deferred: false,
        })
    }

    fn collect_incremental(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        self.collect_from_data_dir(archive, now_utc_ms)
    }
}

/// In-process incremental source for the local Hermes state database.
#[derive(Clone, Debug)]
pub struct HermesLocalSource {
    host_id: String,
    data_dir: PathBuf,
    batch_size: usize,
}

impl HermesLocalSource {
    /// Builds a source over an explicit Hermes data directory.
    pub fn with_data_dir(host_id: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            host_id: host_id.into(),
            data_dir: data_dir.into(),
            batch_size: hermes::DEFAULT_BATCH_SIZE,
        }
    }

    /// Builds a source using `HERMES_HOME` and `~/.hermes` discovery.
    pub fn discover(host_id: impl Into<String>) -> Result<Self> {
        let data_dir = hermes::discover_data_dir()?;
        Ok(Self::with_data_dir(host_id, data_dir))
    }

    /// Overrides the Rust-side delivery batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Hermes data root containing `state.db`.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn collect_from_data_dir(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let watermark = read_source_cursor(archive.connection(), &self.host_id, HERMES_SOURCE)?;
        let last_success_utc = HostRegistry::new(archive.connection())
            .get(&self.host_id)?
            .and_then(|host| host.last_success_utc);
        let request = hermes::ScanRequest {
            host_id: self.host_id.clone(),
            watermark,
            origin: INCREMENTAL_ORIGIN,
            last_success_utc,
            batch_size: self.batch_size,
        };
        let coverage_window = CoverageWindow::new(watermark.unwrap_or(0).max(0), now_utc_ms)?;

        let mut ingest_failure = None::<IngestError>;
        let mut round = IngestRound::begin_for_source(
            archive.connection_mut(),
            &self.host_id,
            HERMES_SOURCE,
            INCREMENTAL_ORIGIN,
        )?;
        let scan = hermes::scan_data_dir(&self.data_dir, &request, |batch| {
            match round.ingest_batch(batch) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let message = error.to_string();
                    ingest_failure = Some(error);
                    Err(hermes::SinkError::new(message))
                }
            }
        })?;
        if let Some(error) = ingest_failure {
            drop(round);
            return Err(HostSourceError::Ingest(error));
        }
        let stats = round.finish_with_coverage(
            scan.reached_eof,
            scan.observed_max_time_updated,
            Some(coverage_window),
        )?;
        let coverage = committed_coverage(&stats, scan.reached_eof, coverage_window)?;

        Ok(CollectOutcome {
            host_id: self.host_id.clone(),
            source: HERMES_SOURCE.to_owned(),
            reached_eof: scan.reached_eof,
            eligible_count: scan.eligible_count,
            skipped_count: scan.skipped_count,
            stats,
            coverage,
            remote: None,
            remote_probe: None,
        })
    }
}

impl HostSource for HermesLocalSource {
    fn host_id(&self) -> &str {
        &self.host_id
    }

    fn kind(&self) -> HostKind {
        HostKind::Local
    }

    fn probe(&self) -> Result<SourceProbe> {
        if !self.data_dir.join(hermes::STATE_DATABASE).is_file() {
            return Err(HostSourceError::Hermes(
                hermes::HermesError::DataDirectoryNotFound {
                    probed_paths: vec![self.data_dir.clone()],
                },
            ));
        }
        Ok(SourceProbe {
            host_id: self.host_id.clone(),
            kind: HostKind::Local,
            source: HERMES_SOURCE.to_owned(),
            location: self.data_dir.display().to_string(),
            remote_facts_deferred: false,
        })
    }

    fn collect_incremental(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        self.collect_from_data_dir(archive, now_utc_ms)
    }
}

/// Incremental source backed by todo 11's four-stage SSH collector transport.
pub struct SshHostSource<R: CommandRunner> {
    host_id: String,
    machine_id_hash: String,
    ssh_target: String,
    remote_data_dir: Option<PathBuf>,
    snapshot: bool,
    source: String,
    transport: SshTransport<R>,
}

impl<R: CommandRunner> std::fmt::Debug for SshHostSource<R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshHostSource")
            .field("host_id", &self.host_id)
            .field("ssh_target", &self.ssh_target)
            .field("remote_data_dir", &self.remote_data_dir)
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

impl<R: CommandRunner> SshHostSource<R> {
    /// Builds an SSH source from a registered [`HostRecord`].
    ///
    /// The transport has already proved `ssh -V` runs during its own construction, so this only
    /// validates the record's own consistency.
    pub fn new(host: &HostRecord, transport: SshTransport<R>) -> Result<Self> {
        if host.kind != HostKind::Ssh {
            return Err(HostSourceError::KindMismatch {
                host_id: host.host_id().to_owned(),
                expected: HostKind::Ssh.as_str(),
                found: host.kind.as_str(),
            });
        }
        let ssh_target = host
            .ssh_target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .ok_or_else(|| HostSourceError::MissingSshTarget {
                host_id: host.host_id().to_owned(),
            })?
            .to_owned();
        Ok(Self {
            host_id: host.host_id().to_owned(),
            machine_id_hash: host.machine_id_hash().to_owned(),
            ssh_target,
            remote_data_dir: host.remote_data_dir.as_deref().map(PathBuf::from),
            snapshot: false,
            source: OPENCODE_SOURCE.to_owned(),
            transport,
        })
    }

    /// Requests a remote `VACUUM INTO` snapshot before scanning.
    pub fn with_snapshot(mut self, snapshot: bool) -> Self {
        self.snapshot = snapshot;
        self
    }

    /// Selects which remote adapter this source collects.
    ///
    /// One host may be collected once per source. Each round reads and advances only its own
    /// `source_cursor` row, so an interrupted Claude Code round never rewinds OpenCode.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Archive source key this instance collects.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// SSH destination used for every stage of this source.
    pub fn ssh_target(&self) -> &str {
        &self.ssh_target
    }
}

impl<R: CommandRunner> HostSource for SshHostSource<R> {
    fn host_id(&self) -> &str {
        &self.host_id
    }

    fn kind(&self) -> HostKind {
        HostKind::Ssh
    }

    fn probe(&self) -> Result<SourceProbe> {
        Ok(SourceProbe {
            host_id: self.host_id.clone(),
            kind: HostKind::Ssh,
            source: self.source.clone(),
            location: self.ssh_target.clone(),
            remote_facts_deferred: true,
        })
    }

    fn collect_incremental(
        &self,
        archive: &mut Archive,
        now_utc_ms: i64,
    ) -> Result<CollectOutcome> {
        let watermark = read_source_cursor(archive.connection(), &self.host_id, &self.source)?;
        let last_success_utc = HostRegistry::new(archive.connection())
            .get(&self.host_id)?
            .and_then(|host| host.last_success_utc);
        let since = watermark.unwrap_or(0).max(0);
        let coverage_window = CoverageWindow::new(since, now_utc_ms)?;
        let collection = self.transport.collect(&SshCollectRequest {
            ssh_target: self.ssh_target.clone(),
            since,
            data_dir: self.remote_data_dir.clone(),
            snapshot: self.snapshot,
            source: remote_source_argument(&self.source),
        })?;
        let (report, records) = self.decode_collection(&collection, since)?;

        let scan = ScanResult {
            delivered_records: records.len() as u64,
            delivered_batches: 1,
            eligible_count: report.eligible_count,
            skipped_count: report.skipped_count,
            skipped_breakdown: SkippedBreakdown::default(),
            observed_max_time_updated: Some(report.cutoff.max(since)),
            reached_eof: true,
            busy_retry_count: 0,
            last_success_utc,
            skip_reason: None,
        };
        // Tiered for the same reason as the local Codex round: `--source codex` on the remote
        // collector reports archived sessions as `bak`, so a single live round would reject the
        // whole payload. Single-origin adapters take the live tier alone and behave as before.
        let stats = ingest_by_origin_with_coverage(
            archive.connection_mut(),
            &self.host_id,
            &self.source,
            &records,
            scan.reached_eof,
            scan.observed_max_time_updated,
            Some(coverage_window),
        )?;
        let coverage = committed_coverage(&stats, scan.reached_eof, coverage_window)?;

        Ok(CollectOutcome {
            host_id: self.host_id.clone(),
            source: self.source.clone(),
            reached_eof: scan.reached_eof,
            eligible_count: scan.eligible_count,
            skipped_count: scan.skipped_count,
            stats,
            coverage,
            remote: Some(report),
            remote_probe: Some(collection.probe.clone()),
        })
    }
}

/// Returns the `--source` value to send remotely, or `None` for the collector's default.
///
/// Omitting the default keeps an OpenCode payload byte-identical to the pre-multi-source format.
fn remote_source_argument(source: &str) -> Option<String> {
    (source != OPENCODE_SOURCE).then(|| source.to_owned())
}

impl<R: CommandRunner> SshHostSource<R> {
    fn decode_collection(
        &self,
        collection: &SshCollection,
        since: i64,
    ) -> Result<(RemoteSourceReport, Vec<NormalizedUsageRecord>)> {
        let text = std::str::from_utf8(&collection.ndjson).map_err(|error| {
            HostSourceError::InvalidRemoteResponse {
                detail: format!("NDJSON 不是 UTF-8：{error}"),
            }
        })?;
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let meta_line = lines
            .next()
            .ok_or_else(|| HostSourceError::InvalidRemoteResponse {
                detail: "响应为空，缺少 v1 meta 行".into(),
            })?;
        let meta: RemoteMetaV1 = serde_json::from_str(meta_line).map_err(|error| {
            HostSourceError::InvalidRemoteResponse {
                detail: format!("meta 行无法解析：{error}"),
            }
        })?;
        if meta.protocol_version != REMOTE_PROTOCOL_VERSION {
            return Err(HostSourceError::InvalidRemoteResponse {
                detail: format!(
                    "protocol_version {} 不受支持，期望 {REMOTE_PROTOCOL_VERSION}",
                    meta.protocol_version
                ),
            });
        }

        let identity = MachineIdentity::from_machine_id_hash(&meta.machine_id_hash)?;
        if identity.machine_id_hash() != self.machine_id_hash {
            return Err(HostSourceError::MachineIdentityMismatch {
                expected_host_id: self.host_id.clone(),
                expected_hash: self.machine_id_hash.clone(),
                found_host_id: identity.host_id().to_owned(),
                found_hash: identity.machine_id_hash().to_owned(),
            });
        }

        let source_meta = meta
            .sources
            .iter()
            .find(|entry| entry.source == self.source)
            .ok_or_else(|| HostSourceError::RemoteSourceMissing {
                expected_source: self.source.clone(),
                found: meta
                    .sources
                    .iter()
                    .map(|entry| entry.source.clone())
                    .collect(),
            })?;

        let mut records = Vec::with_capacity(source_meta.eligible_count as usize);
        for (index, line) in lines.enumerate() {
            let record: NormalizedUsageRecord = serde_json::from_str(line).map_err(|error| {
                HostSourceError::MalformedRemoteRecord {
                    line: index + 2,
                    detail: error.to_string(),
                }
            })?;
            // A collector may report several sources in one meta line. Only this round's source
            // belongs in this round's transaction; the rest would be rejected by ingest anyway.
            if record.source == self.source {
                records.push(record);
            }
        }

        let report = RemoteSourceReport {
            machine_id_hash: meta.machine_id_hash,
            hostname: meta.hostname,
            collector_version: meta.collector_version,
            data_dir: source_meta.data_dir.clone(),
            since: source_meta.scan_window.since,
            cutoff: source_meta.scan_window.cutoff.max(since),
            eligible_count: source_meta.eligible_count,
            skipped_count: source_meta.skipped_count,
        };
        Ok((report, records))
    }
}

/// Metadata line of the collector's v1 NDJSON protocol. Field names are `snake_case` because the
/// collector's meta structs carry no `rename_all` (todo 10); record lines are `camelCase`.
#[derive(Debug, Deserialize)]
struct RemoteMetaV1 {
    protocol_version: u32,
    machine_id_hash: String,
    hostname: String,
    collector_version: String,
    sources: Vec<RemoteSourceMetaV1>,
}

#[derive(Debug, Deserialize)]
struct RemoteSourceMetaV1 {
    source: String,
    data_dir: String,
    scan_window: RemoteScanWindowV1,
    eligible_count: u64,
    skipped_count: u64,
}

#[derive(Debug, Deserialize)]
struct RemoteScanWindowV1 {
    since: i64,
    cutoff: i64,
}

fn committed_coverage(
    stats: &IngestStats,
    reached_eof: bool,
    window: CoverageWindow,
) -> Result<Option<CoverageInterval>> {
    if !(reached_eof && stats.committed) {
        return Ok(None);
    }
    Ok(Some(CoverageInterval::new(window.start, window.end)?))
}

/// Identity of one collectable slot: a host **and** the adapter collected on it.
///
/// The archive has always been keyed per source — `UNIQUE(host_id, source, message_id)` on
/// `usage_record`, `PRIMARY KEY(host_id, source)` on `source_cursor` — so a host with both OpenCode
/// and Claude Code enabled needs two independent scheduler slots, each advancing its own watermark.
/// Keying [`RefreshScheduler`] by `host_id` alone made the second source unregisterable.
///
/// Ordering is `host_id` then `source`, which keeps [`RefreshScheduler::statuses`] grouped by host.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceKey {
    /// Archive host identifier.
    pub host_id: String,
    /// Source key written into `usage_record.source`.
    pub source: String,
}

impl SourceKey {
    /// Builds a key from its two parts.
    pub fn new(host_id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            host_id: host_id.into(),
            source: source.into(),
        }
    }

    /// Builds the OpenCode key for a host, the only source that existed before multi-source.
    pub fn opencode(host_id: impl Into<String>) -> Self {
        Self::new(host_id, OPENCODE_SOURCE)
    }
}

impl std::fmt::Display for SourceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.host_id, self.source)
    }
}

/// Whether a source is refreshed on a timer or only on explicit user action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    /// Refreshed by [`RefreshScheduler::tick`] once the interval elapses.
    Auto,
    /// Refreshed only through [`RefreshScheduler::trigger_manual`].
    Manual,
}

/// Why a round was started.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerReason {
    /// The adaptive timer elapsed.
    Scheduled,
    /// A user (or IPC caller) asked for it.
    Manual,
}

/// Per-source refresh policy.
///
/// The interval formula is uniform:
/// `next_interval_ms = max(min_interval_ms, duration_multiplier × last_duration_ms)`.
/// Only the defaults differ by kind: local sources get a 300 s floor and `Auto`, remote sources get
/// a 900 s floor and `Manual`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSchedule {
    /// Timer or manual-only.
    pub trigger: TriggerMode,
    /// Interval floor in milliseconds; must be greater than zero.
    pub min_interval_ms: u64,
    /// Multiplier applied to the previous round's measured duration.
    pub duration_multiplier: u32,
}

impl SourceSchedule {
    /// Documented defaults for a host kind.
    pub const fn for_kind(kind: HostKind) -> Self {
        match kind {
            HostKind::Local => Self {
                trigger: TriggerMode::Auto,
                min_interval_ms: DEFAULT_LOCAL_MIN_INTERVAL_MS,
                duration_multiplier: DEFAULT_DURATION_MULTIPLIER,
            },
            HostKind::Ssh => Self {
                trigger: TriggerMode::Manual,
                min_interval_ms: DEFAULT_REMOTE_INTERVAL_MS,
                duration_multiplier: DEFAULT_DURATION_MULTIPLIER,
            },
        }
    }

    /// Builds a schedule from a user- or IPC-supplied interval.
    ///
    /// Rejects zero, negatives and anything below [`MIN_AUTO_REFRESH_INTERVAL_MS`]. Rejection is
    /// deliberate rather than a silent clamp: a user who typed one minute and was quietly given ten
    /// would keep believing the app polls every minute.
    pub fn from_configured_interval(
        kind: HostKind,
        interval_ms: i64,
    ) -> std::result::Result<Self, SchedulerError> {
        if interval_ms <= 0 {
            return Err(SchedulerError::InvalidInterval {
                detail: format!("刷新间隔必须为正毫秒数，收到 {interval_ms}"),
            });
        }
        let schedule = Self {
            min_interval_ms: interval_ms.unsigned_abs(),
            ..Self::for_kind(kind)
        };
        schedule.validate()?;
        Ok(schedule)
    }

    /// Overrides the trigger mode.
    pub const fn with_trigger(mut self, trigger: TriggerMode) -> Self {
        self.trigger = trigger;
        self
    }

    /// Overrides the interval floor.
    pub const fn with_min_interval_ms(mut self, min_interval_ms: u64) -> Self {
        self.min_interval_ms = min_interval_ms;
        self
    }

    /// Overrides the duration multiplier. Zero disables adaptivity and pins the floor.
    pub const fn with_duration_multiplier(mut self, duration_multiplier: u32) -> Self {
        self.duration_multiplier = duration_multiplier;
        self
    }

    /// Rejects a degenerate interval that would let the scheduler spin, and enforces the floor.
    ///
    /// `Manual` schedules are validated too: a source can be switched to `Auto` later without
    /// revalidating, so letting a below-floor value in while it is manual would smuggle it past
    /// the floor.
    pub fn validate(&self) -> std::result::Result<(), SchedulerError> {
        if self.min_interval_ms == 0 {
            return Err(SchedulerError::InvalidInterval {
                detail: "刷新间隔下限不能为 0，否则调度会退化为忙轮询".into(),
            });
        }
        if self.min_interval_ms < MIN_AUTO_REFRESH_INTERVAL_MS {
            return Err(SchedulerError::InvalidInterval {
                detail: format!(
                    "刷新间隔下限不能小于 {MIN_AUTO_REFRESH_INTERVAL_MS} 毫秒（10 分钟），收到 {}",
                    self.min_interval_ms
                ),
            });
        }
        Ok(())
    }

    /// Applies the adaptive formula to the previous round's measured duration.
    pub fn next_interval_ms(&self, last_duration_ms: Option<u64>) -> u64 {
        let scaled = last_duration_ms
            .unwrap_or(0)
            .saturating_mul(u64::from(self.duration_multiplier));
        self.min_interval_ms.max(scaled)
    }
}

/// Lifecycle state of one source, as serialized for the UI.
///
/// The three variant names (`idle` / `running` / `error`) are a contract: todo 13 exports them to
/// TypeScript and todo 18 renders them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SourceState {
    /// Not running; triggerable now.
    Idle,
    /// A round is in flight; further triggers are refused.
    Running,
    /// The last round failed. Retained alongside the last known-good timestamp.
    Error {
        /// Failure text from the last round.
        last_error: String,
        /// UTC epoch milliseconds of the last successful round, if any.
        last_success: Option<i64>,
    },
}

/// Flat per-source status for `get_refresh_status()` (todo 13).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    /// Archive host identifier.
    pub host_id: String,
    /// Source key collected under that host; one status row exists per enabled source.
    pub source: String,
    /// Host display name.
    pub display_name: String,
    /// Collection mode.
    pub kind: HostKind,
    /// Lifecycle state.
    pub state: SourceState,
    /// Timer or manual-only.
    pub trigger: TriggerMode,
    /// Last failure text, retained until the next successful round.
    pub last_error: Option<String>,
    /// UTC epoch milliseconds of the last successful round.
    pub last_success_utc: Option<i64>,
    /// UTC epoch milliseconds when the last round finished, successful or not.
    pub last_completed_utc: Option<i64>,
    /// Measured duration of the last round, in milliseconds.
    pub last_duration_ms: Option<u64>,
    /// Interval currently in force, after the adaptive formula.
    pub interval_ms: u64,
    /// When the timer next fires. `None` means "due on the next tick" for `Auto` sources and
    /// "never auto-due" for `Manual` ones.
    pub next_due_utc: Option<i64>,
    /// `true` when the last round returned without reaching EOF, so it made no progress.
    pub interrupted: bool,
    /// Watermark committed by the last successful round.
    pub cursor_time_updated: Option<i64>,
}

/// A round the caller should now execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshAction {
    /// Host to collect.
    pub host_id: String,
    /// Source to collect, so the caller can pick the right adapter on a multi-source host.
    pub source: String,
    /// Collection mode, so the caller can pick the right [`HostSource`].
    pub kind: HostKind,
    /// Why the round started.
    pub reason: TriggerReason,
    /// UTC epoch milliseconds the round was admitted.
    pub started_at_utc: i64,
}

impl RefreshAction {
    /// Scheduler slot this round belongs to.
    pub fn key(&self) -> SourceKey {
        SourceKey::new(self.host_id.clone(), self.source.clone())
    }
}

/// Result of asking for a manual refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriggerOutcome {
    /// The round was admitted.
    Started(RefreshAction),
    /// Refused because this source already has a round in flight.
    AlreadyRunning {
        /// Slot that refused.
        key: SourceKey,
        /// When the in-flight round started.
        started_at_utc: i64,
    },
    /// No such source is registered.
    UnknownSource {
        /// Requested slot.
        key: SourceKey,
    },
}

/// Scheduler-facing projection of one finished round.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectSummary {
    /// Whether the source stream reached EOF.
    pub reached_eof: bool,
    /// Whether the ingest transaction committed.
    pub committed: bool,
    /// Eligible rows observed.
    pub eligible_count: u64,
    /// Rows actually inserted or updated.
    pub changed_records: u64,
    /// Watermark committed by this round.
    pub cursor_time_updated: Option<i64>,
}

impl CollectSummary {
    /// Only an EOF round whose transaction committed counts as progress.
    pub const fn is_success(&self) -> bool {
        self.reached_eof && self.committed
    }
}

/// What happened in one round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundResult {
    /// The source returned a result; inspect [`CollectSummary::is_success`].
    Collected(CollectSummary),
    /// The source raised an error.
    Failed {
        /// Failure text shown to the user.
        error: String,
    },
}

/// One finished round, reported back to the scheduler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundReport {
    /// Measured duration, in milliseconds. Supplied by the caller's [`Clock`], never measured
    /// inside the scheduler.
    pub duration_ms: u64,
    /// Outcome of the round.
    pub result: RoundResult,
}

impl RoundReport {
    /// Reports a round that returned a result.
    pub const fn collected(duration_ms: u64, summary: CollectSummary) -> Self {
        Self {
            duration_ms,
            result: RoundResult::Collected(summary),
        }
    }

    /// Reports a round that raised an error.
    pub fn failed(duration_ms: u64, error: impl Into<String>) -> Self {
        Self {
            duration_ms,
            result: RoundResult::Failed {
                error: error.into(),
            },
        }
    }
}

/// Errors raised by [`RefreshScheduler`] bookkeeping.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    /// A blank host identifier cannot key a source slot.
    #[error("刷新调度的 host_id 不能为空白")]
    BlankHostId,
    /// A blank source key cannot key a source slot.
    #[error("刷新调度的 source 不能为空白")]
    BlankSource,
    /// The slot is already registered.
    #[error("采集源 {key} 已在刷新调度中注册")]
    DuplicateSource {
        /// Duplicate slot.
        key: SourceKey,
    },
    /// No such source is registered.
    #[error("刷新调度中没有采集源 {key}")]
    UnknownSource {
        /// Requested slot.
        key: SourceKey,
    },
    /// A completion arrived for a source that is not running.
    #[error("采集源 {key} 当前没有进行中的刷新轮次，无法登记结果")]
    NotRunning {
        /// Requested slot.
        key: SourceKey,
    },
    /// A configured interval was zero or negative.
    #[error("刷新间隔无效：{detail}")]
    InvalidInterval {
        /// Human-readable explanation.
        detail: String,
    },
}

/// One source registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRegistration {
    /// Archive host identifier.
    pub host_id: String,
    /// Source key collected under that host.
    pub source: String,
    /// Host display name, echoed into status for the UI.
    pub display_name: String,
    /// Collection mode.
    pub kind: HostKind,
    /// Refresh policy.
    pub schedule: SourceSchedule,
}

impl SourceRegistration {
    /// Derives a registration for one source of a stored [`HostRecord`].
    pub fn for_host_source(host: &HostRecord, source: impl Into<String>) -> Self {
        Self {
            host_id: host.host_id().to_owned(),
            source: source.into(),
            display_name: host.display_name.clone(),
            kind: host.kind,
            schedule: SourceSchedule::for_kind(host.kind),
        }
    }

    /// Derives one registration per source the host has enabled.
    ///
    /// This is the upgrade path: a host row written before multi-source carries exactly
    /// `["opencode"]`, so it keeps being collected as before and no Claude Code scan starts on its
    /// own. Enabling a second source is an explicit edit to the host record.
    pub fn all_for_host(host: &HostRecord) -> Vec<Self> {
        host.enabled_sources()
            .iter()
            .map(|source| Self::for_host_source(host, source.clone()))
            .collect()
    }

    /// Slot this registration occupies.
    pub fn key(&self) -> SourceKey {
        SourceKey::new(self.host_id.clone(), self.source.clone())
    }

    /// Overrides the trigger mode of the derived schedule.
    pub const fn with_trigger(mut self, trigger: TriggerMode) -> Self {
        self.schedule = self.schedule.with_trigger(trigger);
        self
    }

    /// Replaces the derived schedule.
    pub const fn with_schedule(mut self, schedule: SourceSchedule) -> Self {
        self.schedule = schedule;
        self
    }
}

#[derive(Clone, Debug)]
struct SourceSlot {
    display_name: String,
    kind: HostKind,
    schedule: SourceSchedule,
    state: SourceState,
    running_since_utc: Option<i64>,
    last_error: Option<String>,
    last_success_utc: Option<i64>,
    last_completed_utc: Option<i64>,
    last_duration_ms: Option<u64>,
    interrupted: bool,
    cursor_time_updated: Option<i64>,
}

impl SourceSlot {
    fn interval_ms(&self) -> u64 {
        self.schedule.next_interval_ms(self.last_duration_ms)
    }

    fn next_due_utc(&self) -> Option<i64> {
        if self.schedule.trigger == TriggerMode::Manual {
            return None;
        }
        self.last_completed_utc
            .map(|completed| completed.saturating_add(self.interval_ms() as i64))
    }

    fn is_due(&self, now: i64) -> bool {
        if self.schedule.trigger != TriggerMode::Auto || self.state == SourceState::Running {
            return false;
        }
        self.next_due_utc().is_none_or(|due| now >= due)
    }

    fn start(&mut self, now: i64) {
        self.state = SourceState::Running;
        self.running_since_utc = Some(now);
    }

    fn status(&self, key: &SourceKey) -> SourceStatus {
        SourceStatus {
            host_id: key.host_id.clone(),
            source: key.source.clone(),
            display_name: self.display_name.clone(),
            kind: self.kind,
            state: self.state.clone(),
            trigger: self.schedule.trigger,
            last_error: self.last_error.clone(),
            last_success_utc: self.last_success_utc,
            last_completed_utc: self.last_completed_utc,
            last_duration_ms: self.last_duration_ms,
            interval_ms: self.interval_ms(),
            next_due_utc: self.next_due_utc(),
            interrupted: self.interrupted,
            cursor_time_updated: self.cursor_time_updated,
        }
    }
}

/// Synchronous refresh state machine.
///
/// # Threading model
///
/// This is a **pure state machine that the caller ticks**; it owns no runtime, spawns no threads
/// and never sleeps. [`RefreshScheduler::tick`] takes the current time, admits every due source,
/// and returns one [`RefreshAction`] per admitted source. The caller decides how to run them —
/// todo 13's IPC layer runs them on its own worker threads — and reports each one back through
/// [`RefreshScheduler::complete`].
///
/// Slots are keyed by [`SourceKey`] — `(host_id, source)`, not `host_id` — so one host can have
/// OpenCode and Claude Code registered at the same time, each with its own state machine, its own
/// interval and its own watermark.
///
/// The scheduling guarantees follow from that:
///
/// * **serial per slot** — a slot in [`SourceState::Running`] is skipped by `tick` and refused
///   by `trigger_manual` with [`TriggerOutcome::AlreadyRunning`], so it can never have two rounds
///   in flight. Two sources of the *same* host are different slots and may run concurrently.
/// * **parallel across slots** — a single `tick` can admit every due slot at once; nothing in
///   this type orders them relative to each other.
/// * **no re-entrancy** — a refused trigger is a distinguishable value, not a silent no-op, so the
///   UI can say "已在刷新中".
/// * **isolated failures** — each source keeps its own state; one source in [`SourceState::Error`]
///   neither blocks nor degrades any other, and both `last_error` and `last_success` are retained.
///
/// Because a round holds one archive transaction for its whole duration, callers running rounds
/// concurrently should give each round its own [`Archive`] handle and call
/// [`set_archive_busy_timeout`] on it.
#[derive(Clone, Debug, Default)]
pub struct RefreshScheduler {
    slots: BTreeMap<SourceKey, SourceSlot>,
}

impl RefreshScheduler {
    /// Creates an empty scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one source. Rejects blank and duplicate slots, and degenerate intervals.
    pub fn register(
        &mut self,
        registration: SourceRegistration,
    ) -> std::result::Result<(), SchedulerError> {
        let host_id = registration.host_id.trim().to_owned();
        if host_id.is_empty() {
            return Err(SchedulerError::BlankHostId);
        }
        let source = registration.source.trim().to_owned();
        if source.is_empty() {
            return Err(SchedulerError::BlankSource);
        }
        registration.schedule.validate()?;
        let key = SourceKey::new(host_id, source);
        if self.slots.contains_key(&key) {
            return Err(SchedulerError::DuplicateSource { key });
        }
        self.slots.insert(
            key,
            SourceSlot {
                display_name: registration.display_name,
                kind: registration.kind,
                schedule: registration.schedule,
                state: SourceState::Idle,
                running_since_utc: None,
                last_error: None,
                last_success_utc: None,
                last_completed_utc: None,
                last_duration_ms: None,
                interrupted: false,
                cursor_time_updated: None,
            },
        );
        Ok(())
    }

    /// Removes one source slot. Returns whether it was registered.
    ///
    /// Removing a source with a round in flight is allowed: the eventual
    /// [`RefreshScheduler::complete`] returns [`SchedulerError::UnknownSource`] instead of
    /// panicking.
    pub fn remove(&mut self, key: &SourceKey) -> bool {
        self.slots.remove(key).is_some()
    }

    /// Removes every source slot of one host. Returns how many were registered.
    ///
    /// Deleting a host must not leave orphan slots behind: they would keep ticking against a host
    /// row that no longer exists and fail every round.
    pub fn remove_host(&mut self, host_id: &str) -> usize {
        let doomed = self.keys_for_host(host_id);
        for key in &doomed {
            self.slots.remove(key);
        }
        doomed.len()
    }

    /// Replaces one source's refresh policy, recomputing its interval and due time.
    pub fn set_schedule(
        &mut self,
        key: &SourceKey,
        schedule: SourceSchedule,
    ) -> std::result::Result<(), SchedulerError> {
        schedule.validate()?;
        let slot = self
            .slots
            .get_mut(key)
            .ok_or_else(|| SchedulerError::UnknownSource { key: key.clone() })?;
        slot.schedule = schedule;
        Ok(())
    }

    /// Every registered slot, in stable order.
    pub fn keys(&self) -> Vec<SourceKey> {
        self.slots.keys().cloned().collect()
    }

    /// Slots belonging to one host, in stable order.
    pub fn keys_for_host(&self, host_id: &str) -> Vec<SourceKey> {
        self.slots
            .keys()
            .filter(|key| key.host_id == host_id)
            .cloned()
            .collect()
    }

    /// Distinct host identifiers, in stable order. A multi-source host appears once.
    pub fn host_ids(&self) -> Vec<String> {
        let mut host_ids: Vec<String> = Vec::new();
        for key in self.slots.keys() {
            if host_ids.last().is_none_or(|last| last != &key.host_id) {
                host_ids.push(key.host_id.clone());
            }
        }
        host_ids
    }

    /// Status of one source, or `None` when it is not registered.
    pub fn status(&self, key: &SourceKey) -> Option<SourceStatus> {
        self.slots.get(key).map(|slot| slot.status(key))
    }

    /// Status of every source, in stable order. This is what todo 13 exposes as
    /// `get_refresh_status()`.
    pub fn statuses(&self) -> Vec<SourceStatus> {
        self.slots
            .iter()
            .map(|(key, slot)| slot.status(key))
            .collect()
    }

    /// Interval currently in force for one source.
    pub fn interval_ms(&self, key: &SourceKey) -> Option<u64> {
        self.slots.get(key).map(SourceSlot::interval_ms)
    }

    /// Admits every source whose timer has elapsed and returns the rounds to run.
    ///
    /// Admitted sources move to [`SourceState::Running`] before returning, so ticking repeatedly
    /// with the same `now` cannot double-trigger anything.
    pub fn tick(&mut self, now_utc_ms: i64) -> Vec<RefreshAction> {
        let mut actions = Vec::new();
        for (key, slot) in &mut self.slots {
            if !slot.is_due(now_utc_ms) {
                continue;
            }
            slot.start(now_utc_ms);
            actions.push(RefreshAction {
                host_id: key.host_id.clone(),
                source: key.source.clone(),
                kind: slot.kind,
                reason: TriggerReason::Scheduled,
                started_at_utc: now_utc_ms,
            });
        }
        actions
    }

    /// Requests an immediate round, regardless of trigger mode.
    ///
    /// A source already running is refused with [`TriggerOutcome::AlreadyRunning`] rather than a
    /// silent no-op. A source in [`SourceState::Error`] is triggerable, which is how the UI retries.
    pub fn trigger_manual(&mut self, key: &SourceKey, now_utc_ms: i64) -> TriggerOutcome {
        let Some(slot) = self.slots.get_mut(key) else {
            return TriggerOutcome::UnknownSource { key: key.clone() };
        };
        if slot.state == SourceState::Running {
            return TriggerOutcome::AlreadyRunning {
                key: key.clone(),
                started_at_utc: slot.running_since_utc.unwrap_or(now_utc_ms),
            };
        }
        slot.start(now_utc_ms);
        TriggerOutcome::Started(RefreshAction {
            host_id: key.host_id.clone(),
            source: key.source.clone(),
            kind: slot.kind,
            reason: TriggerReason::Manual,
            started_at_utc: now_utc_ms,
        })
    }

    /// Records the result of an in-flight round.
    ///
    /// A successful round clears the error state and advances `last_success`. An interrupted round
    /// (`reached_eof == false`, therefore no committed progress) returns the source to
    /// [`SourceState::Idle`] with `interrupted = true` and leaves `last_success` untouched. A failed
    /// round moves the source to [`SourceState::Error`], retaining both texts for the UI.
    pub fn complete(
        &mut self,
        key: &SourceKey,
        now_utc_ms: i64,
        report: RoundReport,
    ) -> std::result::Result<(), SchedulerError> {
        let slot = self
            .slots
            .get_mut(key)
            .ok_or_else(|| SchedulerError::UnknownSource { key: key.clone() })?;
        if slot.state != SourceState::Running {
            return Err(SchedulerError::NotRunning { key: key.clone() });
        }

        slot.running_since_utc = None;
        slot.last_duration_ms = Some(report.duration_ms);
        slot.last_completed_utc = Some(now_utc_ms);
        match report.result {
            RoundResult::Collected(summary) if summary.is_success() => {
                slot.state = SourceState::Idle;
                slot.last_error = None;
                slot.interrupted = false;
                slot.last_success_utc = Some(now_utc_ms);
                if summary.cursor_time_updated.is_some() {
                    slot.cursor_time_updated = summary.cursor_time_updated;
                }
            }
            RoundResult::Collected(_) => {
                slot.state = SourceState::Idle;
                slot.interrupted = true;
            }
            RoundResult::Failed { error } => {
                slot.interrupted = false;
                slot.last_error = Some(error.clone());
                slot.state = SourceState::Error {
                    last_error: error,
                    last_success: slot.last_success_utc,
                };
            }
        }
        Ok(())
    }
}

/// Injected time source. Nothing in this module reads the clock directly, so scheduler behaviour is
/// deterministic in tests without sleeping.
pub trait Clock: Send + Sync {
    /// Wall-clock UTC epoch milliseconds; used for `last_success`, due times and coverage cutoffs.
    fn now_utc_ms(&self) -> i64;

    /// Monotonic milliseconds from an arbitrary fixed origin; used only to measure round duration.
    fn monotonic_ms(&self) -> u64;
}

/// Production [`Clock`] backed by [`SystemTime`] and a process-lifetime [`Instant`] origin.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            })
    }

    fn monotonic_ms(&self) -> u64 {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let origin = ORIGIN.get_or_init(Instant::now);
        u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Runs one admitted round and builds its [`RoundReport`].
///
/// Duration is measured only through `clock`, so a caller can make round timing fully
/// deterministic. On success this also stamps `hosts.last_success_utc`, which is what the host list
/// in todo 18 renders; a missing host row is reported as a failed round rather than ignored.
pub fn execute_action<S: HostSource + ?Sized, C: Clock + ?Sized>(
    source: &S,
    archive: &mut Archive,
    clock: &C,
    action: &RefreshAction,
) -> RoundReport {
    let started = clock.monotonic_ms();
    let outcome = source.collect_incremental(archive, clock.now_utc_ms());
    let duration_ms = clock.monotonic_ms().saturating_sub(started);

    match outcome {
        Ok(outcome) => {
            if outcome.is_success() {
                let stamped_at = clock.now_utc_ms();
                if let Err(error) = HostRegistry::new(archive.connection())
                    .update_last_success(&action.host_id, stamped_at)
                {
                    return RoundReport::failed(duration_ms, error.to_string());
                }
            }
            RoundReport::collected(duration_ms, outcome.summary())
        }
        Err(error) => {
            let text = match error.remediation() {
                Some(remediation) => format!("{error}｜{remediation}"),
                None => error.to_string(),
            };
            RoundReport::failed(duration_ms, text)
        }
    }
}

#[cfg(test)]
#[allow(unexpected_cfgs)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::archive::{Archive, Origin};
    use crate::fixture;
    use crate::host::{HostKind, HostRecord, HostRegistry, MachineIdentity};
    use crate::ingest::{read_cursor, read_source_cursor, OPENCODE_SOURCE};
    use crate::source::opencode::{SourceConnection, SourceMessageRow, StreamError};
    use crate::transport::ssh::{
        CollectorArtifacts, CommandOutput, CommandRunner, CommandSpec, CommandStage,
        SshAuthentication, SshTools, SshTransport,
    };

    const PROBE_X86_64: &str = "AGENTLENS_ARCH=x86_64\n\
AGENTLENS_XDG_DATA_HOME=/home/test/.local/share\n\
AGENTLENS_AVAILABLE_KIB=1048576\n\
AGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\n\
AGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n";
    const REMOTE_RUN_DIR: &str = "/home/test/.cache/agentlens/run.A1b2C3";
    const REMOTE_MACHINE_ID: &str = "fixture-remote-machine-id";
    const LOCAL_MACHINE_ID: &str = "fixture-local-machine-id";

    /// Deterministic clock. Wall time is scripted and monotonic readings are popped in order,
    /// so no test sleeps and no test reads the real clock.
    struct ManualClock {
        now_ms: AtomicI64,
        monotonic: Mutex<VecDeque<u64>>,
    }

    impl ManualClock {
        fn new(now_ms: i64) -> Self {
            Self {
                now_ms: AtomicI64::new(now_ms),
                monotonic: Mutex::new(VecDeque::new()),
            }
        }

        fn script_round(&self, start_ms: u64, duration_ms: u64) {
            let mut readings = self.monotonic.lock().expect("monotonic lock");
            readings.push_back(start_ms);
            readings.push_back(start_ms + duration_ms);
        }
    }

    impl Clock for ManualClock {
        fn now_utc_ms(&self) -> i64 {
            self.now_ms.load(Ordering::SeqCst)
        }

        fn monotonic_ms(&self) -> u64 {
            self.monotonic
                .lock()
                .expect("monotonic lock")
                .pop_front()
                .expect("scripted monotonic reading")
        }
    }

    #[derive(Clone, Default)]
    struct FakeSshRunner {
        stage1: Arc<Mutex<VecDeque<CommandOutput>>>,
        stage4: Arc<Mutex<VecDeque<CommandOutput>>>,
        stages: Arc<Mutex<Vec<CommandStage>>>,
    }

    impl FakeSshRunner {
        fn ok(stdout: &str, stderr: &str) -> CommandOutput {
            CommandOutput {
                status: 0,
                stdout: stdout.as_bytes().to_vec(),
                stderr: stderr.as_bytes().to_vec(),
            }
        }

        fn push_ndjson(&self, ndjson: &str) {
            self.stage4
                .lock()
                .expect("stage4 lock")
                .push_back(Self::ok(ndjson, ""));
        }

        fn push_auth_failure(&self) {
            self.stage1
                .lock()
                .expect("stage1 lock")
                .push_back(CommandOutput {
                    status: 255,
                    stdout: Vec::new(),
                    stderr: b"Permission denied (publickey).".to_vec(),
                });
        }

        fn stages(&self) -> Vec<CommandStage> {
            self.stages.lock().expect("stage log lock").clone()
        }
    }

    impl CommandRunner for FakeSshRunner {
        fn run(&self, command: &CommandSpec) -> io::Result<CommandOutput> {
            self.stages
                .lock()
                .expect("stage log lock")
                .push(command.stage);
            let output = match command.stage {
                CommandStage::StartupProbe => Self::ok("", "OpenSSH_9.9p1 fixture"),
                CommandStage::Stage1 => self
                    .stage1
                    .lock()
                    .expect("stage1 lock")
                    .pop_front()
                    .unwrap_or_else(|| Self::ok(PROBE_X86_64, "")),
                CommandStage::Stage2 => Self::ok(&format!("{REMOTE_RUN_DIR}\n"), ""),
                CommandStage::Stage3 | CommandStage::Gc => Self::ok("", ""),
                CommandStage::Stage4 => self
                    .stage4
                    .lock()
                    .expect("stage4 lock")
                    .pop_front()
                    .unwrap_or_else(|| CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"no scripted stage4 response".to_vec(),
                    }),
            };
            Ok(output)
        }
    }

    /// Source connection that delivers `deliver` rows then reports an orderly interruption,
    /// which forces `reached_eof == false` through the real scanner.
    struct InterruptingConnection {
        rows: Vec<SourceMessageRow>,
        deliver: usize,
    }

    impl SourceConnection for InterruptingConnection {
        fn query_only(&self) -> rusqlite::Result<bool> {
            Ok(true)
        }

        fn stream_messages(
            &mut self,
            window_start: i64,
            visitor: &mut dyn FnMut(SourceMessageRow) -> std::result::Result<(), StreamError>,
        ) -> std::result::Result<(), StreamError> {
            for row in self
                .rows
                .iter()
                .filter(|row| row.time_updated >= window_start)
                .take(self.deliver)
            {
                visitor(row.clone())?;
            }
            Err(StreamError::Interrupted("注入的中断".into()))
        }
    }

    fn ssh_tools() -> SshTools {
        SshTools::new("/usr/bin/ssh", "/usr/bin/scp").expect("paired fixture tools")
    }

    fn ssh_artifacts() -> (tempfile::TempDir, CollectorArtifacts) {
        let temp = tempfile::tempdir().expect("collector tempdir");
        let collector = temp.path().join("agentlens-collector");
        std::fs::write(&collector, b"fixture collector bytes").expect("write collector artifact");
        (temp, CollectorArtifacts::default().with_x86_64(collector))
    }

    fn ssh_transport(runner: &FakeSshRunner) -> (tempfile::TempDir, SshTransport<FakeSshRunner>) {
        let (temp, artifacts) = ssh_artifacts();
        let transport = SshTransport::new(
            runner.clone(),
            ssh_tools(),
            SshAuthentication::Batch {
                identity_file: None,
            },
            artifacts,
        )
        .expect("construct fake transport");
        (temp, transport)
    }

    fn remote_identity() -> MachineIdentity {
        MachineIdentity::from_machine_id(REMOTE_MACHINE_ID).expect("remote identity")
    }

    fn local_identity() -> MachineIdentity {
        MachineIdentity::from_machine_id(LOCAL_MACHINE_ID).expect("local identity")
    }

    fn meta_line(identity: &MachineIdentity, since: i64, cutoff: i64, eligible: u64) -> String {
        format!(
            "{{\"protocol_version\":1,\"machine_id_hash\":\"{hash}\",\"hostname\":\"fixture-remote\",\
\"collector_version\":\"0.1.0\",\"sources\":[{{\"source\":\"opencode\",\"data_dir\":\"/data\",\
\"scan_window\":{{\"since\":{since},\"cutoff\":{cutoff}}},\"eligible_count\":{eligible},\
\"skipped_count\":2}}]}}",
            hash = identity.machine_id_hash(),
        )
    }

    fn remote_record_line(host_id: &str, message_id: &str, time_created: i64) -> String {
        format!(
            "{{\"hostId\":\"{host_id}\",\"source\":\"opencode\",\"messageId\":\"{message_id}\",\
\"sessionId\":\"ses_remote\",\"timeCreatedUtc\":{time_created},\
\"timeCompletedUtc\":{completed},\"sourceTimeUpdated\":{time_created},\"origin\":\"live\",\
\"originPriority\":3,\"agentRaw\":\"build\",\"agentKey\":\"build\",\
\"providerId\":\"kiro-auth\",\"modelId\":\"claude-opus-5-max\",\"variant\":\"high\",\
\"tokInput\":100,\"tokOutput\":10,\"tokReasoning\":0,\"tokCacheRead\":5,\"tokCacheWrite\":1,\
\"cost\":null,\"costSource\":\"unavailable\",\"isIncomplete\":false,\
\"projectDir\":\"/home/test/project\"}}",
            completed = time_created + 1,
        )
    }

    fn remote_ndjson(identity: &MachineIdentity, since: i64, cutoff: i64, ids: &[&str]) -> String {
        let mut lines = vec![meta_line(identity, since, cutoff, ids.len() as u64)];
        for (index, id) in ids.iter().enumerate() {
            lines.push(remote_record_line(
                identity.host_id(),
                id,
                cutoff - 1_000 + index as i64,
            ));
        }
        let mut ndjson = lines.join("\n");
        ndjson.push('\n');
        ndjson
    }

    fn assistant_row(message_id: &str, time_updated: i64) -> SourceMessageRow {
        SourceMessageRow {
            message_id: message_id.into(),
            session_id: "ses_interrupt".into(),
            time_created: time_updated,
            time_updated,
            data: "{\"role\":\"assistant\",\"agent\":\"build\",\"modelID\":\"m\",\
\"providerID\":\"p\",\"path\":{\"directory\":\"/tmp\"},\"cost\":0,\
\"tokens\":{\"input\":1,\"output\":2,\"reasoning\":0,\"cache\":{\"read\":0,\"write\":0}},\
\"time\":{\"created\":1,\"completed\":2}}"
                .into(),
        }
    }

    fn opencode_slot(host_id: &str) -> SourceKey {
        SourceKey::opencode(host_id)
    }

    fn claude_slot(host_id: &str) -> SourceKey {
        SourceKey::new(host_id, CLAUDE_CODE_SOURCE)
    }

    fn action_for_source<'actions>(
        actions: &'actions [RefreshAction],
        source: &str,
    ) -> &'actions RefreshAction {
        actions
            .iter()
            .find(|action| action.source == source)
            .unwrap_or_else(|| panic!("no admitted round for source {source}"))
    }

    fn local_registration(host_id: &str) -> SourceRegistration {
        SourceRegistration {
            host_id: host_id.into(),
            source: OPENCODE_SOURCE.into(),
            display_name: "本机".into(),
            kind: HostKind::Local,
            schedule: SourceSchedule::for_kind(HostKind::Local),
        }
    }

    fn ssh_registration(host_id: &str, trigger: TriggerMode) -> SourceRegistration {
        SourceRegistration {
            host_id: host_id.into(),
            source: OPENCODE_SOURCE.into(),
            display_name: "远端".into(),
            kind: HostKind::Ssh,
            schedule: SourceSchedule::for_kind(HostKind::Ssh).with_trigger(trigger),
        }
    }

    fn successful_summary(eligible: u64) -> CollectSummary {
        CollectSummary {
            reached_eof: true,
            committed: true,
            eligible_count: eligible,
            changed_records: eligible,
            cursor_time_updated: Some(1_700_000_000_000),
        }
    }

    fn archive_rows(archive: &Archive, host_id: &str) -> u64 {
        archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE host_id = ?1 AND source = ?2",
                rusqlite::params![host_id, OPENCODE_SOURCE],
                |row| row.get::<_, i64>(0),
            )
            .expect("count archived rows")
            .unsigned_abs()
    }

    fn claude_meta_line(
        identity: &MachineIdentity,
        source: &str,
        since: i64,
        cutoff: i64,
        eligible: u64,
    ) -> String {
        format!(
            "{{\"protocol_version\":1,\"machine_id_hash\":\"{hash}\",\"hostname\":\"fixture-remote\",\
\"collector_version\":\"0.1.0\",\"sources\":[{{\"source\":\"{source}\",\"data_dir\":\"/home/test/.claude/projects\",\
\"scan_window\":{{\"since\":{since},\"cutoff\":{cutoff}}},\"eligible_count\":{eligible},\
\"skipped_count\":7}}]}}",
            hash = identity.machine_id_hash(),
        )
    }

    fn claude_record_line(host_id: &str, source: &str, message_id: &str, created: i64) -> String {
        format!(
            "{{\"hostId\":\"{host_id}\",\"source\":\"{source}\",\"messageId\":\"{message_id}\",\
\"sessionId\":\"ses_claude\",\"timeCreatedUtc\":{created},\"timeCompletedUtc\":null,\
\"sourceTimeUpdated\":{created},\"origin\":\"live\",\"originPriority\":3,\
\"agentRaw\":\"main\",\"agentKey\":\"main\",\"providerId\":\"anthropic\",\
\"modelId\":\"claude-sonnet-4-5-20250929\",\"variant\":null,\
\"tokInput\":10,\"tokOutput\":20,\"tokReasoning\":0,\"tokCacheRead\":30,\"tokCacheWrite\":40,\
\"cost\":null,\"costSource\":\"unavailable\",\"isIncomplete\":false,\
\"projectDir\":\"/home/test/project\"}}"
        )
    }

    fn rows_for_source(archive: &Archive, host_id: &str, source: &str) -> u64 {
        archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE host_id = ?1 AND source = ?2",
                rusqlite::params![host_id, source],
                |row| row.get::<_, i64>(0),
            )
            .expect("count archived rows")
            .unsigned_abs()
    }

    fn synthetic_claude_projects(root: &std::path::Path) -> claude_code::fixture::Manifest {
        let projects = root.join(claude_code::PROJECTS_DIRECTORY);
        std::fs::create_dir_all(&projects).expect("create projects dir");
        claude_code::fixture::write(&projects).expect("write synthetic transcripts")
    }

    fn open_temp_archive(directory: &std::path::Path) -> Archive {
        let archive = Archive::open_in_data_dir(directory).expect("open temp archive");
        set_archive_busy_timeout(&archive, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS)
            .expect("configure busy timeout");
        archive
    }

    #[test]
    fn sched_reentrancy_refuses_second_trigger_while_running() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");

        let started = scheduler.trigger_manual(&opencode_slot("local-1"), 1_000);
        let TriggerOutcome::Started(action) = started else {
            panic!("first manual trigger must start a round");
        };
        assert_eq!(action.host_id, "local-1");
        assert_eq!(action.reason, TriggerReason::Manual);
        assert_eq!(action.started_at_utc, 1_000);
        assert_eq!(
            scheduler
                .status(&opencode_slot("local-1"))
                .expect("status")
                .state,
            SourceState::Running
        );

        let refused = scheduler.trigger_manual(&opencode_slot("local-1"), 2_000);
        assert_eq!(
            refused,
            TriggerOutcome::AlreadyRunning {
                key: opencode_slot("local-1"),
                started_at_utc: 1_000,
            }
        );
        assert!(
            scheduler.tick(9_999_999).is_empty(),
            "a running source must never be re-triggered by tick"
        );
        assert_eq!(
            scheduler
                .status(&opencode_slot("local-1"))
                .expect("status")
                .state,
            SourceState::Running
        );
    }

    #[test]
    fn sched_adaptive_interval_uses_floor_then_three_times_last_duration() {
        let schedule = SourceSchedule::for_kind(HostKind::Local);
        assert_eq!(schedule.min_interval_ms, DEFAULT_LOCAL_MIN_INTERVAL_MS);
        assert_eq!(DEFAULT_LOCAL_MIN_INTERVAL_MS, 600_000);
        assert_eq!(DEFAULT_LOCAL_MIN_INTERVAL_MS, MIN_AUTO_REFRESH_INTERVAL_MS);
        assert_eq!(schedule.duration_multiplier, DEFAULT_DURATION_MULTIPLIER);
        assert_eq!(DEFAULT_DURATION_MULTIPLIER, 3);
        assert_eq!(schedule.next_interval_ms(None), 600_000);
        assert_eq!(schedule.next_interval_ms(Some(20_000)), 600_000);
        assert_eq!(schedule.next_interval_ms(Some(250_000)), 750_000);

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 0),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot("local-1"),
                20_000,
                RoundReport::collected(20_000, successful_summary(7)),
            )
            .expect("complete 20s round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(status.last_duration_ms, Some(20_000));
        assert_eq!(status.interval_ms, 600_000);
        assert!(status.interval_ms >= MIN_AUTO_REFRESH_INTERVAL_MS);
        assert_eq!(status.next_due_utc, Some(620_000));
        assert!(scheduler.tick(619_999).is_empty());
        assert_eq!(scheduler.tick(620_000).len(), 1);

        scheduler
            .complete(
                &opencode_slot("local-1"),
                900_000,
                RoundReport::collected(250_000, successful_summary(9)),
            )
            .expect("complete 250s round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(status.last_duration_ms, Some(250_000));
        assert_eq!(status.interval_ms, 750_000);
        assert!(status.interval_ms >= MIN_AUTO_REFRESH_INTERVAL_MS);
        assert_eq!(status.next_due_utc, Some(1_650_000));
        assert!(scheduler.tick(1_649_999).is_empty());
        assert_eq!(scheduler.tick(1_650_000).len(), 1);
    }

    #[test]
    fn sched_error_round_records_last_error_then_next_success_recovers() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 1_000),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot("local-1"),
                2_000,
                RoundReport::failed(1_000, "远端 OpenCode 数据库或 WAL/SHM 不可读"),
            )
            .expect("complete failed round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(
            status.state,
            SourceState::Error {
                last_error: "远端 OpenCode 数据库或 WAL/SHM 不可读".into(),
                last_success: None,
            }
        );
        assert_eq!(
            status.last_error.as_deref(),
            Some("远端 OpenCode 数据库或 WAL/SHM 不可读")
        );
        assert_eq!(status.last_success_utc, None);

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 3_000),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot("local-1"),
                4_000,
                RoundReport::collected(500, successful_summary(3)),
            )
            .expect("complete recovery round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(status.state, SourceState::Idle);
        assert_eq!(status.last_error, None);
        assert_eq!(status.last_success_utc, Some(4_000));
        assert!(!status.interrupted);
    }

    #[test]
    fn sched_two_sources_progress_independently_in_one_tick() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");
        scheduler
            .register(ssh_registration("remote-1", TriggerMode::Auto))
            .expect("register remote source");

        let actions = scheduler.tick(0);
        assert_eq!(actions.len(), 2);
        let mut host_ids: Vec<_> = actions
            .iter()
            .map(|action| action.host_id.clone())
            .collect();
        host_ids.sort();
        assert_eq!(host_ids, vec!["local-1".to_owned(), "remote-1".to_owned()]);

        scheduler
            .complete(
                &opencode_slot("remote-1"),
                5_000,
                RoundReport::failed(5_000, "SSH 认证失败"),
            )
            .expect("complete failing remote round");
        assert!(matches!(
            scheduler
                .status(&opencode_slot("remote-1"))
                .expect("status")
                .state,
            SourceState::Error { .. }
        ));
        assert_eq!(
            scheduler
                .status(&opencode_slot("local-1"))
                .expect("status")
                .state,
            SourceState::Running,
            "a failing remote source must not disturb the still-running local round"
        );

        scheduler
            .complete(
                &opencode_slot("local-1"),
                9_000,
                RoundReport::collected(400, successful_summary(11)),
            )
            .expect("complete local round");
        assert_eq!(
            scheduler
                .status(&opencode_slot("local-1"))
                .expect("status")
                .state,
            SourceState::Idle
        );
        assert_eq!(
            scheduler
                .status(&opencode_slot("local-1"))
                .expect("status")
                .last_success_utc,
            Some(9_000)
        );
        assert!(matches!(
            scheduler
                .status(&opencode_slot("remote-1"))
                .expect("status")
                .state,
            SourceState::Error { .. }
        ));
    }

    #[test]
    fn sched_repeated_ssh_failure_keeps_error_while_local_keeps_succeeding() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");
        scheduler
            .register(ssh_registration("remote-1", TriggerMode::Auto))
            .expect("register remote source");

        for round in 0..3_i64 {
            let now = round * 1_000_000;
            let actions = scheduler.tick(now);
            assert_eq!(actions.len(), 2, "round {round} must schedule both sources");
            scheduler
                .complete(
                    &opencode_slot("remote-1"),
                    now + 1_000,
                    RoundReport::failed(1_000, format!("SSH 认证失败 #{round}")),
                )
                .expect("complete failing remote round");
            scheduler
                .complete(
                    &opencode_slot("local-1"),
                    now + 2_000,
                    RoundReport::collected(400, successful_summary(5)),
                )
                .expect("complete local round");

            let remote = scheduler
                .status(&opencode_slot("remote-1"))
                .expect("remote status");
            assert_eq!(
                remote.state,
                SourceState::Error {
                    last_error: format!("SSH 认证失败 #{round}"),
                    last_success: None,
                }
            );
            assert_eq!(remote.last_success_utc, None);

            let local = scheduler
                .status(&opencode_slot("local-1"))
                .expect("local status");
            assert_eq!(local.state, SourceState::Idle);
            assert_eq!(local.last_success_utc, Some(now + 2_000));
            assert_eq!(local.last_error, None);
        }
    }

    #[test]
    fn sched_remote_source_is_manual_by_default_and_fires_at_fifteen_minutes_when_timed() {
        let default_schedule = SourceSchedule::for_kind(HostKind::Ssh);
        assert_eq!(default_schedule.trigger, TriggerMode::Manual);
        assert_eq!(default_schedule.min_interval_ms, DEFAULT_REMOTE_INTERVAL_MS);
        assert_eq!(DEFAULT_REMOTE_INTERVAL_MS, 900_000);

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration {
                host_id: "remote-1".into(),
                source: OPENCODE_SOURCE.into(),
                display_name: "远端".into(),
                kind: HostKind::Ssh,
                schedule: default_schedule,
            })
            .expect("register remote source");

        assert!(scheduler.tick(0).is_empty());
        assert!(scheduler.tick(i64::MAX / 2).is_empty());
        let status = scheduler
            .status(&opencode_slot("remote-1"))
            .expect("status");
        assert_eq!(status.trigger, TriggerMode::Manual);
        assert_eq!(status.next_due_utc, None);

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("remote-1"), 10),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot("remote-1"),
                20,
                RoundReport::collected(10, successful_summary(1)),
            )
            .expect("complete manual remote round");
        assert!(
            scheduler.tick(i64::MAX / 2).is_empty(),
            "manual remote sources are never auto-triggered"
        );

        scheduler
            .set_schedule(
                &opencode_slot("remote-1"),
                SourceSchedule::for_kind(HostKind::Ssh).with_trigger(TriggerMode::Auto),
            )
            .expect("configure remote timer");
        let status = scheduler
            .status(&opencode_slot("remote-1"))
            .expect("status");
        assert_eq!(status.trigger, TriggerMode::Auto);
        assert_eq!(status.interval_ms, 900_000);
        assert_eq!(status.next_due_utc, Some(900_020));
        assert!(scheduler.tick(900_019).is_empty());
        assert_eq!(scheduler.tick(900_020).len(), 1);
    }

    #[test]
    fn sched_stale_state_repeated_ticks_and_mid_flight_removal_are_safe() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");

        assert_eq!(scheduler.tick(1_000).len(), 1);
        assert!(scheduler.tick(1_000).is_empty());
        assert!(scheduler.tick(1_000).is_empty());

        assert!(scheduler.remove(&opencode_slot("local-1")));
        assert!(scheduler.status(&opencode_slot("local-1")).is_none());
        assert!(scheduler.tick(1_000).is_empty());
        assert!(!scheduler.remove(&opencode_slot("local-1")));

        let error = scheduler
            .complete(
                &opencode_slot("local-1"),
                2_000,
                RoundReport::collected(1, successful_summary(0)),
            )
            .expect_err("completing a removed source must fail, not panic");
        assert!(matches!(error, SchedulerError::UnknownSource { .. }));
        assert_eq!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 3_000),
            TriggerOutcome::UnknownSource {
                key: opencode_slot("local-1")
            }
        );
    }

    /// One host, two adapters: the pair-keyed scheduler must run both and let each advance its own
    /// watermark. Before the rekey the second `register` was refused, so this whole flow was
    /// unreachable and a user could collect either OpenCode or Claude Code on a machine, not both.
    #[test]
    fn sched_one_host_runs_both_sources_with_independent_cursors_and_states() {
        let temp = tempfile::tempdir().expect("tempdir");
        let opencode_dir = temp.path().join("opencode-fixture");
        let opencode_manifest = fixture::generate(&opencode_dir).expect("generate fixture");
        let claude_root = temp.path().join("claude");
        let claude_manifest = synthetic_claude_projects(&claude_root);
        let mut archive = open_temp_archive(&temp.path().join("archive"));

        let identity = local_identity();
        let host = HostRecord::local("本机", &identity)
            .with_enabled_sources(vec![OPENCODE_SOURCE.into(), CLAUDE_CODE_SOURCE.into()]);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let mut scheduler = RefreshScheduler::new();
        for registration in SourceRegistration::all_for_host(&host) {
            scheduler
                .register(registration)
                .expect("both sources of one host must be registerable");
        }
        assert_eq!(
            scheduler.keys(),
            vec![claude_slot(host.host_id()), opencode_slot(host.host_id()),],
            "a multi-source host occupies one slot per source"
        );
        assert_eq!(
            scheduler.host_ids(),
            vec![host.host_id().to_owned()],
            "two slots of one host collapse to a single host id"
        );

        let actions = scheduler.tick(1_000);
        assert_eq!(actions.len(), 2, "one tick admits both sources at once");
        let mut admitted: Vec<String> =
            actions.iter().map(|action| action.source.clone()).collect();
        admitted.sort();
        assert_eq!(
            admitted,
            vec![CLAUDE_CODE_SOURCE.to_owned(), OPENCODE_SOURCE.to_owned()]
        );
        for action in &actions {
            assert_eq!(action.host_id, host.host_id());
            assert_eq!(action.reason, TriggerReason::Scheduled);
        }

        let clock = ManualClock::new(claude_manifest.max_timestamp_ms + 60_000);
        let claude = ClaudeCodeLocalSource::with_projects_dir(
            host.host_id(),
            claude_root.join(claude_code::PROJECTS_DIRECTORY),
        );
        clock.script_round(0, 40);
        let claude_report = execute_action(
            &claude,
            &mut archive,
            &clock,
            action_for_source(&actions, CLAUDE_CODE_SOURCE),
        );
        scheduler
            .complete(&claude_slot(host.host_id()), 2_000, claude_report)
            .expect("complete the claude round");

        let opencode =
            LocalHostSource::with_database(host.host_id(), opencode_dir.join("opencode.db"));
        clock.script_round(100, 70);
        let opencode_report = execute_action(
            &opencode,
            &mut archive,
            &clock,
            action_for_source(&actions, OPENCODE_SOURCE),
        );
        scheduler
            .complete(&opencode_slot(host.host_id()), 3_000, opencode_report)
            .expect("complete the opencode round");

        let claude_status = scheduler
            .status(&claude_slot(host.host_id()))
            .expect("claude status");
        let opencode_status = scheduler
            .status(&opencode_slot(host.host_id()))
            .expect("opencode status");
        assert_eq!(claude_status.source, CLAUDE_CODE_SOURCE);
        assert_eq!(opencode_status.source, OPENCODE_SOURCE);
        assert_eq!(claude_status.state, SourceState::Idle);
        assert_eq!(opencode_status.state, SourceState::Idle);
        assert_eq!(claude_status.last_success_utc, Some(2_000));
        assert_eq!(opencode_status.last_success_utc, Some(3_000));
        assert_eq!(claude_status.last_duration_ms, Some(40));
        assert_eq!(opencode_status.last_duration_ms, Some(70));

        let claude_cursor =
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read claude cursor");
        let opencode_cursor =
            read_cursor(archive.connection(), host.host_id()).expect("read opencode cursor");
        assert_eq!(claude_cursor, Some(claude_manifest.max_timestamp_ms));
        assert_eq!(claude_status.cursor_time_updated, claude_cursor);
        assert_eq!(opencode_status.cursor_time_updated, opencode_cursor);
        assert_ne!(
            claude_cursor, opencode_cursor,
            "each slot must own an independent watermark"
        );
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            claude_manifest.eligible
        );
        assert_eq!(
            rows_for_source(&archive, host.host_id(), OPENCODE_SOURCE),
            opencode_manifest.eligible_assistant_count
        );

        assert!(matches!(
            scheduler.trigger_manual(&claude_slot(host.host_id()), 4_000),
            TriggerOutcome::Started(_)
        ));
        assert_eq!(
            scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("opencode status")
                .state,
            SourceState::Idle,
            "a running claude round must not block the opencode slot of the same host"
        );
        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot(host.host_id()), 4_100),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &claude_slot(host.host_id()),
                5_000,
                RoundReport::failed(10, "transcript 不可读"),
            )
            .expect("complete failing claude round");
        assert!(matches!(
            scheduler
                .status(&claude_slot(host.host_id()))
                .expect("claude status")
                .state,
            SourceState::Error { .. }
        ));
        assert_eq!(
            scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("opencode status")
                .state,
            SourceState::Running,
            "one failing source must not disturb the other source of the same host"
        );

        assert_eq!(scheduler.remove_host(host.host_id()), 2);
        assert!(scheduler.keys().is_empty());
    }

    /// Upgrade path: a host row written before multi-source registers exactly its OpenCode slot,
    /// so an existing installation keeps collecting and never starts a Claude Code scan by itself.
    #[test]
    fn sched_pre_multi_source_host_registers_only_opencode() {
        let identity = local_identity();
        let legacy = HostRecord::local("本机", &identity);
        assert_eq!(legacy.enabled_sources(), [OPENCODE_SOURCE]);

        let registrations = SourceRegistration::all_for_host(&legacy);
        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].source, OPENCODE_SOURCE);
        assert_eq!(registrations[0].key(), opencode_slot(identity.host_id()));

        let mut scheduler = RefreshScheduler::new();
        for registration in registrations {
            scheduler.register(registration).expect("register slot");
        }
        assert_eq!(scheduler.keys(), vec![opencode_slot(identity.host_id())]);
        assert!(
            scheduler.status(&claude_slot(identity.host_id())).is_none(),
            "Claude Code must stay unscheduled until it is explicitly enabled"
        );

        let actions = scheduler.tick(1_000);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].source, OPENCODE_SOURCE);
    }

    /// The 10-minute floor is enforced in the core, not only in the UI, and it is enforced by
    /// rejection: a silently clamped value would let a user believe a one-minute poll is running.
    #[test]
    fn sched_interval_below_the_ten_minute_floor_is_rejected_everywhere() {
        assert_eq!(MIN_AUTO_REFRESH_INTERVAL_MS, 600_000);

        for below in [
            1_i64,
            60_000,
            300_000,
            MIN_AUTO_REFRESH_INTERVAL_MS as i64 - 1,
        ] {
            let error = SourceSchedule::from_configured_interval(HostKind::Local, below)
                .expect_err("a below-floor interval must be refused, not clamped");
            let SchedulerError::InvalidInterval { detail } = error else {
                panic!("expected an interval error for {below}")
            };
            assert!(
                detail.contains("600000"),
                "the message must name the floor: {detail}"
            );
            assert!(
                SourceSchedule::from_configured_interval(HostKind::Ssh, below).is_err(),
                "the floor applies to remote sources too"
            );
        }

        for at_or_above in [
            MIN_AUTO_REFRESH_INTERVAL_MS as i64,
            DEFAULT_REMOTE_INTERVAL_MS as i64,
            3_600_000,
        ] {
            assert_eq!(
                SourceSchedule::from_configured_interval(HostKind::Local, at_or_above)
                    .expect("at or above the floor is accepted")
                    .min_interval_ms,
                at_or_above.unsigned_abs()
            );
        }

        // A manual schedule is validated too: switching it to Auto later does not revalidate, so
        // an unchecked manual value would be a way around the floor.
        assert!(matches!(
            SourceSchedule::for_kind(HostKind::Ssh)
                .with_trigger(TriggerMode::Manual)
                .with_min_interval_ms(1_000)
                .validate(),
            Err(SchedulerError::InvalidInterval { .. })
        ));

        let mut scheduler = RefreshScheduler::new();
        assert!(matches!(
            scheduler.register(
                local_registration("local-1").with_schedule(
                    SourceSchedule::for_kind(HostKind::Local).with_min_interval_ms(1)
                )
            ),
            Err(SchedulerError::InvalidInterval { .. })
        ));
        scheduler
            .register(local_registration("local-1"))
            .expect("the default schedule satisfies the floor");
        assert!(matches!(
            scheduler.set_schedule(
                &opencode_slot("local-1"),
                SourceSchedule::for_kind(HostKind::Local).with_min_interval_ms(599_999)
            ),
            Err(SchedulerError::InvalidInterval { .. })
        ));
        assert_eq!(
            scheduler.interval_ms(&opencode_slot("local-1")),
            Some(MIN_AUTO_REFRESH_INTERVAL_MS),
            "a rejected replacement must leave the previous interval in force"
        );
    }

    #[test]
    fn sched_malformed_configuration_is_rejected_with_typed_errors() {
        assert!(matches!(
            SourceSchedule::from_configured_interval(HostKind::Local, 0),
            Err(SchedulerError::InvalidInterval { .. })
        ));
        assert!(matches!(
            SourceSchedule::from_configured_interval(HostKind::Local, -1),
            Err(SchedulerError::InvalidInterval { .. })
        ));
        assert_eq!(
            SourceSchedule::from_configured_interval(
                HostKind::Local,
                MIN_AUTO_REFRESH_INTERVAL_MS as i64
            )
            .expect("the floor itself is accepted")
            .min_interval_ms,
            MIN_AUTO_REFRESH_INTERVAL_MS
        );

        let mut scheduler = RefreshScheduler::new();
        assert!(matches!(
            scheduler.register(local_registration("   ")),
            Err(SchedulerError::BlankHostId)
        ));
        assert!(matches!(
            scheduler.register(SourceRegistration {
                source: "   ".into(),
                ..local_registration("local-blank-source")
            }),
            Err(SchedulerError::BlankSource)
        ));
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");
        assert!(matches!(
            scheduler.register(local_registration("local-1")),
            Err(SchedulerError::DuplicateSource { .. })
        ));
        assert!(matches!(
            scheduler.set_schedule(
                &opencode_slot("missing"),
                SourceSchedule::for_kind(HostKind::Local)
            ),
            Err(SchedulerError::UnknownSource { .. })
        ));
        assert!(matches!(
            scheduler.set_schedule(
                &opencode_slot("local-1"),
                SourceSchedule::for_kind(HostKind::Local).with_min_interval_ms(0)
            ),
            Err(SchedulerError::InvalidInterval { .. })
        ));
        assert!(matches!(
            scheduler.complete(&opencode_slot("local-1"), 1, RoundReport::failed(0, "boom")),
            Err(SchedulerError::NotRunning { .. })
        ));
        assert!(scheduler.status(&opencode_slot("missing")).is_none());

        let runner = FakeSshRunner::default();
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let identity = remote_identity();
        let mut host = HostRecord::ssh("远端", "user@example.invalid", &identity);
        host.ssh_target = None;
        let error =
            SshHostSource::new(&host, transport).expect_err("ssh host without target is invalid");
        assert!(matches!(error, HostSourceError::MissingSshTarget { .. }));

        let (_artifact_temp2, transport2) = ssh_transport(&runner);
        let local_host = HostRecord::local("本机", &local_identity());
        let error = SshHostSource::new(&local_host, transport2)
            .expect_err("a local host record is not an ssh source");
        assert!(matches!(error, HostSourceError::KindMismatch { .. }));
    }

    #[test]
    fn sched_zero_row_non_eof_round_is_not_recorded_as_success() {
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(local_registration("local-1"))
            .expect("register local source");

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 1_000),
            TriggerOutcome::Started(_)
        ));
        let misleading = CollectSummary {
            reached_eof: false,
            committed: false,
            eligible_count: 0,
            changed_records: 0,
            cursor_time_updated: None,
        };
        assert!(!misleading.is_success());
        scheduler
            .complete(
                &opencode_slot("local-1"),
                2_000,
                RoundReport::collected(400, misleading),
            )
            .expect("complete interrupted round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(status.state, SourceState::Idle);
        assert_eq!(status.last_success_utc, None);
        assert!(status.interrupted);

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot("local-1"), 3_000),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot("local-1"),
                4_000,
                RoundReport::collected(
                    10,
                    CollectSummary {
                        reached_eof: true,
                        committed: true,
                        eligible_count: 0,
                        changed_records: 0,
                        cursor_time_updated: Some(7),
                    },
                ),
            )
            .expect("complete empty but successful round");
        let status = scheduler.status(&opencode_slot("local-1")).expect("status");
        assert_eq!(status.last_success_utc, Some(4_000));
        assert!(!status.interrupted);
    }

    #[test]
    fn sched_local_source_ingests_fixture_and_advances_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fixture_dir = temp.path().join("fixture");
        let manifest = fixture::generate(&fixture_dir).expect("generate fixture");
        let mut archive = open_temp_archive(&temp.path().join("archive"));

        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let source =
            LocalHostSource::with_database(host.host_id(), fixture_dir.join("opencode.db"));
        let probe = source.probe().expect("probe local source");
        assert_eq!(probe.kind, HostKind::Local);
        assert_eq!(probe.source, OPENCODE_SOURCE);
        assert!(!probe.remote_facts_deferred);

        let now = manifest.coverage.live_cutoff;
        let outcome = source
            .collect_incremental(&mut archive, now)
            .expect("collect local round");
        assert!(outcome.is_success());
        assert_eq!(outcome.eligible_count, manifest.eligible_assistant_count);
        assert_eq!(outcome.stats.received_records, outcome.eligible_count);
        assert!(outcome.coverage.is_some());
        assert_eq!(
            archive_rows(&archive, host.host_id()),
            outcome.eligible_count
        );
        let cursor = read_cursor(archive.connection(), host.host_id()).expect("read cursor");
        assert!(cursor.is_some());
        assert_eq!(cursor, outcome.stats.cursor_time_updated);
    }

    #[test]
    fn claude_local_source_collects_and_keeps_its_cursor_and_coverage_apart_from_opencode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let opencode_dir = temp.path().join("opencode-fixture");
        let opencode_manifest = fixture::generate(&opencode_dir).expect("generate fixture");
        let claude_root = temp.path().join("claude");
        let claude_manifest = synthetic_claude_projects(&claude_root);
        let mut archive = open_temp_archive(&temp.path().join("archive"));

        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let claude = ClaudeCodeLocalSource::with_projects_dir(
            host.host_id(),
            claude_root.join(claude_code::PROJECTS_DIRECTORY),
        );
        assert_eq!(claude.host_id(), host.host_id());
        assert_eq!(claude.kind(), HostKind::Local);
        assert_eq!(
            claude.projects_dir(),
            claude_root.join(claude_code::PROJECTS_DIRECTORY)
        );
        let probe = claude.probe().expect("probe claude source");
        assert_eq!(probe.kind, HostKind::Local);
        assert_eq!(probe.source, CLAUDE_CODE_SOURCE);
        assert!(!probe.remote_facts_deferred);

        let now = claude_manifest.max_timestamp_ms + 60_000;
        let outcome = claude
            .collect_incremental(&mut archive, now)
            .expect("collect claude round");
        assert!(outcome.is_success());
        assert_eq!(outcome.source, CLAUDE_CODE_SOURCE);
        assert_eq!(outcome.eligible_count, claude_manifest.eligible);
        assert_eq!(outcome.skipped_count, claude_manifest.skipped);
        assert!(outcome.coverage.is_some());
        assert_eq!(
            outcome.stats.cursor_time_updated,
            Some(claude_manifest.max_timestamp_ms)
        );
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            claude_manifest.eligible
        );

        let opencode =
            LocalHostSource::with_database(host.host_id(), opencode_dir.join("opencode.db"));
        let opencode_outcome = opencode
            .collect_incremental(&mut archive, opencode_manifest.coverage.live_cutoff)
            .expect("collect opencode round");
        assert!(opencode_outcome.is_success());
        assert_eq!(
            rows_for_source(&archive, host.host_id(), OPENCODE_SOURCE),
            opencode_manifest.eligible_assistant_count
        );
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            claude_manifest.eligible,
            "an OpenCode round must not disturb Claude Code rows"
        );

        let claude_cursor =
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read claude cursor");
        let opencode_cursor =
            read_cursor(archive.connection(), host.host_id()).expect("read opencode cursor");
        assert_eq!(claude_cursor, Some(claude_manifest.max_timestamp_ms));
        assert_eq!(opencode_cursor, opencode_outcome.stats.cursor_time_updated);
        assert_ne!(
            claude_cursor, opencode_cursor,
            "each source must own an independent watermark"
        );

        let coverage_sources: Vec<String> = archive
            .connection()
            .prepare(
                "SELECT DISTINCT source FROM coverage_interval WHERE host_id = ?1 ORDER BY source",
            )
            .expect("prepare coverage query")
            .query_map(rusqlite::params![host.host_id()], |row| row.get(0))
            .expect("query coverage")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("collect coverage sources");
        assert_eq!(
            coverage_sources,
            vec![CLAUDE_CODE_SOURCE.to_owned(), OPENCODE_SOURCE.to_owned()]
        );

        let repeat = claude
            .collect_incremental(&mut archive, now)
            .expect("second claude round");
        assert!(repeat.is_success());
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            claude_manifest.eligible,
            "re-collection must upsert, never duplicate"
        );

        println!(
            "claude_local claude_rows={} opencode_rows={} claude_cursor={claude_cursor:?} opencode_cursor={opencode_cursor:?} coverage_sources={coverage_sources:?}",
            claude_manifest.eligible, opencode_manifest.eligible_assistant_count
        );
    }

    fn codex_rollout_body(session_id: &str, day: &str, clock: &str) -> String {
        let events = [
            serde_json::json!({
                "timestamp": format!("{day}T{clock}.000Z"),
                "type": "session_meta",
                "payload": { "id": session_id, "model_provider": "openai" },
            }),
            serde_json::json!({
                "timestamp": format!("{day}T{clock}.000Z"),
                "type": "turn_context",
                "payload": { "model": "openai.gpt-5.4", "cwd": "/synthetic/codex" },
            }),
            serde_json::json!({
                "timestamp": format!("{day}T{clock}.000Z"),
                "ordinal": 3,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 10,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 60,
                        },
                        "last_token_usage": {
                            "input_tokens": 10,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 60,
                        },
                    },
                },
            }),
        ];
        let body = events
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        format!("{body}\n")
    }

    /// Codex is the only adapter that derives `origin` per file, so it is the only one that can
    /// hand one round records from two provenance tiers. A single `Origin::Live` round rejects the
    /// first `archived_sessions/` record with `OriginMismatch` and rolls the live records back with
    /// it, which is why enabling Codex used to archive nothing at all.
    ///
    /// The archived session here is deliberately **newer** than the live one: that is what proves
    /// the backup tier commits its rows without dragging the live watermark forward past data a
    /// later incremental round still has to read.
    #[test]
    fn codex_local_source_commits_live_and_archived_tiers_without_moving_the_live_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_root = temp.path().join("codex");
        for (directory, session, day, clock) in [
            (
                codex::SESSIONS_DIRECTORY,
                "session-codex-live",
                "2026-07-16",
                "11:08:13",
            ),
            (
                codex::ARCHIVED_SESSIONS_DIRECTORY,
                "session-codex-archived",
                "2026-07-20",
                "09:15:33",
            ),
        ] {
            let rollout = codex_root
                .join(directory)
                .join(format!("2026/07/rollout-{session}.jsonl"));
            std::fs::create_dir_all(rollout.parent().expect("rollout parent"))
                .expect("create Codex tree");
            std::fs::write(&rollout, codex_rollout_body(session, day, clock))
                .expect("write Codex rollout");
        }

        let mut archive = open_temp_archive(&temp.path().join("archive"));
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let source = CodexLocalSource::with_data_dir(host.host_id(), &codex_root);
        let outcome = source
            .collect_incremental(&mut archive, 1_784_600_000_000)
            .expect("collect a mixed-origin Codex round");

        assert!(
            outcome.is_success(),
            "mixed origins must not fail the round"
        );
        assert_eq!(outcome.eligible_count, 2);
        assert_eq!(rows_for_source(&archive, host.host_id(), CODEX_SOURCE), 2);

        let mut statement = archive
            .connection()
            .prepare(
                "SELECT origin, origin_priority FROM usage_record
                 WHERE source = ?1 ORDER BY origin",
            )
            .expect("prepare origin rollup");
        let tiers = statement
            .query_map([CODEX_SOURCE], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
            })
            .expect("read origin rollup")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("collect origin rollup");
        drop(statement);
        assert_eq!(
            tiers,
            vec![("bak".to_owned(), 2), ("live".to_owned(), 3)],
            "both tiers must survive with their own provenance"
        );

        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CODEX_SOURCE)
                .expect("read Codex cursor"),
            Some(1_784_200_093_000),
            "the watermark must come from the live tier, not the newer archived record"
        );

        let repeat = source
            .collect_incremental(&mut archive, 1_784_600_000_000)
            .expect("repeat the mixed-origin round");
        assert!(repeat.is_success());
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CODEX_SOURCE),
            2,
            "replaying both tiers must upsert, never duplicate"
        );
    }

    #[test]
    fn codex_local_source_probes_collects_and_keeps_an_independent_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_root = temp.path().join("codex");
        let rollout = codex_root
            .join(codex::SESSIONS_DIRECTORY)
            .join("2026/07/16/rollout-synthetic.jsonl");
        std::fs::create_dir_all(rollout.parent().expect("rollout parent"))
            .expect("create Codex sessions tree");
        let events = [
            serde_json::json!({
                "timestamp": "2026-07-16T11:08:11.000Z",
                "type": "session_meta",
                "payload": { "id": "session-codex", "model_provider": "openai" },
            }),
            serde_json::json!({
                "timestamp": "2026-07-16T11:08:12.000Z",
                "type": "turn_context",
                "payload": { "model": "openai.gpt-5.4", "cwd": "/synthetic/codex" },
            }),
            serde_json::json!({
                "timestamp": "2026-07-16T11:08:13.000Z",
                "ordinal": 3,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 10,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 60,
                        },
                        "last_token_usage": {
                            "input_tokens": 10,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 12,
                            "total_tokens": 60,
                        },
                    },
                },
            }),
        ];
        let body = events
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&rollout, format!("{body}\n")).expect("write Codex rollout");

        let mut archive = open_temp_archive(&temp.path().join("archive"));
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let source =
            CodexLocalSource::with_data_dir(host.host_id(), &codex_root).with_batch_size(1);
        assert_eq!(source.host_id(), host.host_id());
        assert_eq!(source.kind(), HostKind::Local);
        assert_eq!(source.data_dir(), codex_root);
        let probe = source.probe().expect("probe Codex source");
        assert_eq!(probe.source, CODEX_SOURCE);
        assert_eq!(probe.kind, HostKind::Local);
        assert!(!probe.remote_facts_deferred);

        let now = 1_784_200_200_000;
        let outcome = source
            .collect_incremental(&mut archive, now)
            .expect("collect Codex round");
        assert!(outcome.is_success());
        assert_eq!(outcome.source, CODEX_SOURCE);
        assert_eq!(outcome.eligible_count, 1);
        assert_eq!(outcome.skipped_count, 2);
        assert_eq!(rows_for_source(&archive, host.host_id(), CODEX_SOURCE), 1);
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CODEX_SOURCE)
                .expect("read Codex cursor"),
            Some(1_784_200_093_000)
        );
        assert!(outcome.coverage.is_some());

        let repeat = source
            .collect_incremental(&mut archive, now)
            .expect("repeat Codex round");
        assert!(repeat.is_success());
        assert_eq!(rows_for_source(&archive, host.host_id(), CODEX_SOURCE), 1);

        let missing = CodexLocalSource::with_data_dir(host.host_id(), temp.path().join("missing"));
        assert!(matches!(missing.probe(), Err(HostSourceError::Codex(_))));
        let invalid_batch = source.clone().with_batch_size(0);
        assert!(matches!(
            invalid_batch.collect_incremental(&mut archive, now),
            Err(HostSourceError::Codex(codex::CodexError::InvalidBatchSize))
        ));
    }

    #[test]
    fn hermes_local_source_probes_collects_session_records_and_keeps_its_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let hermes_root = temp.path().join("hermes");
        std::fs::create_dir_all(&hermes_root).expect("create Hermes root");
        let connection = rusqlite::Connection::open(hermes_root.join(hermes::STATE_DATABASE))
            .expect("create Hermes state database");
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
                );
                INSERT INTO sessions (
                    id, source, model, started_at, ended_at,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    reasoning_tokens, billing_provider, billing_base_url
                ) VALUES (
                    'session-hermes', 'cli', 'global.anthropic.claude-sonnet-4-5',
                    1784200000.125, NULL, 10, 20, 30, 40, 50, 'custom',
                    'https://gateway.example.test/v1'
                );
                INSERT INTO messages (session_id, timestamp)
                VALUES ('session-hermes', 1784200093.75);",
            )
            .expect("create Hermes fixture");

        let mut archive = open_temp_archive(&temp.path().join("archive"));
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let source =
            HermesLocalSource::with_data_dir(host.host_id(), &hermes_root).with_batch_size(1);
        assert_eq!(source.host_id(), host.host_id());
        assert_eq!(source.kind(), HostKind::Local);
        assert_eq!(source.data_dir(), hermes_root);
        let probe = source.probe().expect("probe Hermes source");
        assert_eq!(probe.source, HERMES_SOURCE);
        assert_eq!(probe.kind, HostKind::Local);
        assert!(!probe.remote_facts_deferred);

        let now = 1_784_200_200_000;
        let outcome = source
            .collect_incremental(&mut archive, now)
            .expect("collect Hermes round");
        assert!(outcome.is_success());
        assert_eq!(outcome.source, HERMES_SOURCE);
        assert_eq!(outcome.eligible_count, 1);
        assert_eq!(outcome.skipped_count, 0);
        assert_eq!(rows_for_source(&archive, host.host_id(), HERMES_SOURCE), 1);
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), HERMES_SOURCE)
                .expect("read Hermes cursor"),
            Some(1_784_200_093_750)
        );
        assert!(outcome.coverage.is_some());

        let archived: (String, String, String, i64, i64, i64, i64, i64, bool) = archive
            .connection()
            .query_row(
                "SELECT granularity, provider_id, model_id, tok_input, tok_output,
                        tok_cache_read, tok_cache_write, tok_reasoning, is_incomplete
                 FROM usage_record
                 WHERE host_id = ?1 AND source = ?2 AND message_id = ?3",
                rusqlite::params![host.host_id(), HERMES_SOURCE, "session-hermes"],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .expect("read archived Hermes record");
        assert_eq!(
            archived,
            (
                "session".to_owned(),
                "anthropic".to_owned(),
                "claude-sonnet-4-5".to_owned(),
                10,
                20,
                30,
                40,
                50,
                false,
            )
        );

        let repeat = source
            .collect_incremental(&mut archive, now)
            .expect("repeat Hermes round");
        assert!(repeat.is_success());
        assert_eq!(rows_for_source(&archive, host.host_id(), HERMES_SOURCE), 1);

        let missing = HermesLocalSource::with_data_dir(host.host_id(), temp.path().join("missing"));
        assert!(matches!(missing.probe(), Err(HostSourceError::Hermes(_))));
        let invalid_batch = source.clone().with_batch_size(0);
        assert!(matches!(
            invalid_batch.collect_incremental(&mut archive, now),
            Err(HostSourceError::Hermes(
                hermes::HermesError::InvalidBatchSize
            ))
        ));
    }

    #[test]
    fn claude_local_source_reports_absent_tree_and_rolls_back_an_interrupted_round() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let absent = ClaudeCodeLocalSource::discover("host-discover")
            .map(|source| source.projects_dir().to_path_buf());
        assert!(absent.is_ok() || absent.is_err());

        let missing =
            ClaudeCodeLocalSource::with_projects_dir(host.host_id(), temp.path().join("nope"));
        let probe_error = missing
            .probe()
            .expect_err("an absent tree must fail probing");
        assert!(matches!(probe_error, HostSourceError::ClaudeCode(_)));
        let collect_error = missing
            .collect_incremental(&mut archive, 1_000)
            .expect_err("an absent tree cannot be collected");
        assert!(matches!(collect_error, HostSourceError::ClaudeCode(_)));
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read cursor"),
            None,
            "a failed round must leave no watermark"
        );

        struct InterruptingTranscripts;

        impl claude_code::TranscriptSource for InterruptingTranscripts {
            fn transcripts(&self) -> std::result::Result<Vec<PathBuf>, claude_code::StreamError> {
                Err(claude_code::StreamError::Interrupted("injected".into()))
            }

            fn read_lines(
                &self,
                _path: &Path,
                _visitor: &mut dyn FnMut(&str) -> std::result::Result<(), claude_code::StreamError>,
            ) -> std::result::Result<(), claude_code::StreamError> {
                Ok(())
            }
        }

        let source = ClaudeCodeLocalSource::with_projects_dir(host.host_id(), temp.path())
            .with_batch_size(3);
        let error = source
            .collect_from_transcripts(&mut archive, &InterruptingTranscripts, 2_000)
            .expect_err("an interrupted enumeration must fail the round");
        assert!(matches!(error, HostSourceError::ClaudeCode(_)));
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            0
        );
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read cursor"),
            None
        );
    }

    #[test]
    fn claude_ssh_source_selects_its_own_remote_meta_and_ignores_foreign_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "user@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let cutoff = 1_785_000_100_000_i64;
        let mut ndjson = vec![claude_meta_line(
            &identity,
            CLAUDE_CODE_SOURCE,
            0,
            cutoff,
            2,
        )];
        ndjson.push(claude_record_line(
            identity.host_id(),
            CLAUDE_CODE_SOURCE,
            "msg_a#req_a",
            cutoff - 2_000,
        ));
        ndjson.push(claude_record_line(
            identity.host_id(),
            CLAUDE_CODE_SOURCE,
            "msg_b",
            cutoff - 1_000,
        ));
        ndjson.push(claude_record_line(
            identity.host_id(),
            OPENCODE_SOURCE,
            "msg_foreign",
            cutoff - 500,
        ));
        let mut payload = ndjson.join(
            "
",
        );
        payload.push('\n');

        let runner = FakeSshRunner::default();
        runner.push_ndjson(&payload);
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport)
            .expect("build ssh source")
            .with_source(CLAUDE_CODE_SOURCE);
        assert_eq!(source.source(), CLAUDE_CODE_SOURCE);
        assert_eq!(source.probe().expect("probe").source, CLAUDE_CODE_SOURCE);

        let outcome = source
            .collect_incremental(&mut archive, cutoff + 1_000)
            .expect("collect remote claude round");
        assert_eq!(outcome.source, CLAUDE_CODE_SOURCE);
        assert!(outcome.is_success());
        assert_eq!(
            rows_for_source(&archive, host.host_id(), CLAUDE_CODE_SOURCE),
            2
        );
        assert_eq!(
            rows_for_source(&archive, host.host_id(), OPENCODE_SOURCE),
            0,
            "a foreign record line must be dropped, not relabelled"
        );
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read cursor"),
            Some(cutoff)
        );
        assert_eq!(
            read_cursor(archive.connection(), host.host_id()).expect("read cursor"),
            None
        );
        assert_eq!(remote_source_argument(OPENCODE_SOURCE), None);
        assert_eq!(
            remote_source_argument(CLAUDE_CODE_SOURCE),
            Some(CLAUDE_CODE_SOURCE.to_owned())
        );
        println!(
            "claude_ssh rows={} foreign_rows=0 cursor={:?} opencode_cursor=None",
            2,
            Some(cutoff)
        );
    }

    #[test]
    fn claude_ssh_source_rejects_a_response_that_omits_its_source_meta() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "user@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let payload = remote_ndjson(&identity, 0, 1_000, &["msg_only_opencode"]);
        let runner = FakeSshRunner::default();
        runner.push_ndjson(&payload);
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport)
            .expect("build ssh source")
            .with_source(CLAUDE_CODE_SOURCE);

        let error = source
            .collect_incremental(&mut archive, 2_000)
            .expect_err("a response without this source must not be accepted as empty");
        match error {
            HostSourceError::RemoteSourceMissing {
                ref expected_source,
                ref found,
            } => {
                assert_eq!(expected_source, CLAUDE_CODE_SOURCE);
                assert_eq!(found, &vec![OPENCODE_SOURCE.to_owned()]);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(
            read_source_cursor(archive.connection(), host.host_id(), CLAUDE_CODE_SOURCE)
                .expect("read cursor"),
            None
        );
    }

    #[test]
    fn sched_repeated_interruption_leaves_cursor_unadvanced_and_source_triggerable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let source = LocalHostSource::with_database(host.host_id(), temp.path().join("absent.db"));
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration::for_host_source(&host, OPENCODE_SOURCE))
            .expect("register source");

        let rows = vec![
            assistant_row("msg_a", 1_700_000_000_000),
            assistant_row("msg_b", 1_700_000_001_000),
        ];
        for round in 0..2 {
            let action = match scheduler.trigger_manual(&opencode_slot(host.host_id()), 10 + round)
            {
                TriggerOutcome::Started(action) => action,
                other => panic!("expected a started round, got {other:?}"),
            };
            let mut connection = InterruptingConnection {
                rows: rows.clone(),
                deliver: 1,
            };
            let outcome = source
                .collect_from_connection(&mut archive, &mut connection, 1_700_000_100_000)
                .expect("interrupted round returns Ok with reached_eof=false");
            assert!(!outcome.reached_eof);
            assert!(!outcome.is_success());
            assert!(!outcome.stats.committed);
            assert_eq!(outcome.coverage, None);
            assert_eq!(
                read_cursor(archive.connection(), host.host_id()).expect("read cursor"),
                None,
                "an interrupted round must never advance the watermark"
            );
            assert_eq!(archive_rows(&archive, host.host_id()), 0);

            scheduler
                .complete(
                    &action.key(),
                    20 + round,
                    RoundReport::collected(1_000, outcome.summary()),
                )
                .expect("complete interrupted round");
            let status = scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status");
            assert_eq!(status.state, SourceState::Idle);
            assert!(status.interrupted);
            assert_eq!(status.last_success_utc, None);
        }
    }

    #[test]
    fn sched_ssh_source_ingests_remote_ndjson_and_updates_hosts_last_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "user@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");
        assert_eq!(host.last_success_utc, None);

        let runner = FakeSshRunner::default();
        let cutoff = 1_700_000_200_000;
        runner.push_ndjson(&remote_ndjson(
            &identity,
            0,
            cutoff,
            &["msg_remote_1", "msg_remote_2", "msg_remote_3"],
        ));
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport).expect("build ssh source");

        let probe = source.probe().expect("probe ssh source");
        assert_eq!(probe.kind, HostKind::Ssh);
        assert_eq!(probe.location, "user@example.invalid");
        assert!(probe.remote_facts_deferred);

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration::for_host_source(&host, OPENCODE_SOURCE))
            .expect("register ssh source");
        let action =
            match scheduler.trigger_manual(&opencode_slot(host.host_id()), 1_700_000_300_000) {
                TriggerOutcome::Started(action) => action,
                other => panic!("expected a started round, got {other:?}"),
            };

        let clock = ManualClock::new(1_700_000_300_000);
        clock.script_round(5_000, 2_500);
        let report = execute_action(&source, &mut archive, &clock, &action);
        assert_eq!(report.duration_ms, 2_500);
        let RoundResult::Collected(summary) = &report.result else {
            panic!("ssh round must complete, got {:?}", report.result);
        };
        assert!(summary.is_success());
        assert_eq!(summary.eligible_count, 3);
        scheduler
            .complete(&action.key(), clock.now_utc_ms(), report)
            .expect("complete ssh round");

        assert_eq!(archive_rows(&archive, host.host_id()), 3);
        assert_eq!(
            read_cursor(archive.connection(), host.host_id()).expect("read cursor"),
            Some(cutoff)
        );
        let stored = HostRegistry::new(archive.connection())
            .get(host.host_id())
            .expect("read host")
            .expect("host row present");
        assert_eq!(stored.last_success_utc, Some(1_700_000_300_000));
        assert_eq!(
            scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status")
                .state,
            SourceState::Idle
        );
        assert!(runner.stages().contains(&CommandStage::Stage4));
    }

    #[test]
    fn sched_ssh_source_rejects_remote_machine_identity_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let registered = remote_identity();
        let host = HostRecord::ssh("远端", "user@example.invalid", &registered);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let other = MachineIdentity::from_machine_id("some-other-machine").expect("other identity");
        let runner = FakeSshRunner::default();
        runner.push_ndjson(&remote_ndjson(&other, 0, 1_700_000_200_000, &["msg_x"]));
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport).expect("build ssh source");

        let error = source
            .collect_incremental(&mut archive, 1_700_000_300_000)
            .expect_err("a different remote machine must be rejected");
        assert!(matches!(
            error,
            HostSourceError::MachineIdentityMismatch { .. }
        ));
        assert_eq!(archive_rows(&archive, host.host_id()), 0);
    }

    #[test]
    fn sched_ssh_source_failure_surfaces_transport_remediation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "user@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let runner = FakeSshRunner::default();
        runner.push_auth_failure();
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport).expect("build ssh source");

        let error = source
            .collect_incremental(&mut archive, 1_700_000_300_000)
            .expect_err("auth failure must not be silently ignored");
        let remediation = error.remediation().expect("ssh errors carry remediation");
        assert!(remediation
            .chars()
            .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character)));
        assert_eq!(archive_rows(&archive, host.host_id()), 0);
        let stored = HostRegistry::new(archive.connection())
            .get(host.host_id())
            .expect("read host")
            .expect("host row present");
        assert_eq!(stored.last_success_utc, None);
    }

    #[test]
    fn sched_local_and_fake_ssh_sources_collect_in_parallel_without_blocking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fixture_dir = temp.path().join("fixture");
        let manifest = fixture::generate(&fixture_dir).expect("generate fixture");
        let archive_dir = temp.path().join("archive");
        let archive = open_temp_archive(&archive_dir);

        let local_identity = local_identity();
        let local_host = HostRecord::local("本机", &local_identity);
        let remote_identity = remote_identity();
        let remote_host = HostRecord::ssh("远端", "user@example.invalid", &remote_identity);
        {
            let registry = HostRegistry::new(archive.connection());
            registry.insert(&local_host).expect("insert local host");
            registry.insert(&remote_host).expect("insert remote host");
        }
        drop(archive);

        let local_source =
            LocalHostSource::with_database(local_host.host_id(), fixture_dir.join("opencode.db"));
        let runner = FakeSshRunner::default();
        let remote_cutoff = 1_700_000_200_000;
        runner.push_ndjson(&remote_ndjson(
            &remote_identity,
            0,
            remote_cutoff,
            &["msg_par_1", "msg_par_2"],
        ));
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let remote_source = SshHostSource::new(&remote_host, transport).expect("build ssh source");

        let scheduler = Mutex::new(RefreshScheduler::new());
        {
            let mut guard = scheduler.lock().expect("scheduler lock");
            guard
                .register(SourceRegistration::for_host_source(
                    &local_host,
                    OPENCODE_SOURCE,
                ))
                .expect("register local source");
            guard
                .register(
                    SourceRegistration::for_host_source(&remote_host, OPENCODE_SOURCE)
                        .with_trigger(TriggerMode::Auto),
                )
                .expect("register remote source");
        }
        let now = manifest.coverage.live_cutoff;
        let actions = scheduler.lock().expect("scheduler lock").tick(now);
        assert_eq!(actions.len(), 2);

        std::thread::scope(|scope| {
            for action in &actions {
                let scheduler = &scheduler;
                let archive_dir = archive_dir.clone();
                let local_source = &local_source;
                let remote_source = &remote_source;
                scope.spawn(move || {
                    let mut archive = open_temp_archive(&archive_dir);
                    let clock = ManualClock::new(now);
                    clock.script_round(0, 1_234);
                    let report = if action.kind == HostKind::Local {
                        execute_action(local_source, &mut archive, &clock, action)
                    } else {
                        execute_action(remote_source, &mut archive, &clock, action)
                    };
                    assert!(
                        matches!(&report.result, RoundResult::Collected(summary) if summary.is_success()),
                        "{} round must succeed, got {:?}",
                        action.host_id,
                        report.result
                    );
                    scheduler
                        .lock()
                        .expect("scheduler lock")
                        .complete(&action.key(), now + 1_234, report)
                        .expect("complete parallel round");
                });
            }
        });

        let archive = open_temp_archive(&archive_dir);
        assert_eq!(
            archive_rows(&archive, local_host.host_id()),
            manifest.eligible_assistant_count
        );
        assert_eq!(archive_rows(&archive, remote_host.host_id()), 2);
        let guard = scheduler.lock().expect("scheduler lock");
        for host_id in [local_host.host_id(), remote_host.host_id()] {
            let status = guard.status(&opencode_slot(host_id)).expect("status");
            assert_eq!(status.state, SourceState::Idle, "{host_id} must be idle");
            assert_eq!(status.last_success_utc, Some(now + 1_234));
        }
    }

    #[test]
    fn sched_execute_action_measures_duration_from_injected_clock_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration::for_host_source(&host, OPENCODE_SOURCE))
            .expect("register source");
        let action = match scheduler.trigger_manual(&opencode_slot(host.host_id()), 1_000) {
            TriggerOutcome::Started(action) => action,
            other => panic!("expected a started round, got {other:?}"),
        };

        let source = LocalHostSource::with_database(host.host_id(), temp.path().join("absent.db"));
        let clock = ManualClock::new(1_700_000_000_000);
        clock.script_round(42, 250_000);
        let report = execute_action(&source, &mut archive, &clock, &action);
        assert_eq!(report.duration_ms, 250_000);
        assert!(matches!(report.result, RoundResult::Failed { .. }));

        scheduler
            .complete(&opencode_slot(host.host_id()), 1_700_000_000_000, report)
            .expect("complete failed round");
        let status = scheduler
            .status(&opencode_slot(host.host_id()))
            .expect("status");
        assert!(matches!(status.state, SourceState::Error { .. }));
        assert_eq!(status.interval_ms, 750_000);
    }

    #[test]
    fn sched_source_construction_preserves_configuration_without_remote_io() {
        let local =
            LocalHostSource::with_database("local-config", "/tmp/opencode.db").with_batch_size(17);
        assert_eq!(local.host_id(), "local-config");
        assert_eq!(local.kind(), HostKind::Local);
        assert_eq!(local.database_path(), Path::new("/tmp/opencode.db"));

        let runner = FakeSshRunner::default();
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "  deploy@example.invalid  ", &identity)
            .with_remote_data_dir(Some("/srv/opencode".into()));
        let source = SshHostSource::new(&host, transport)
            .expect("valid ssh configuration")
            .with_snapshot(true);

        assert_eq!(source.host_id(), identity.host_id());
        assert_eq!(source.kind(), HostKind::Ssh);
        assert_eq!(source.ssh_target(), "deploy@example.invalid");
        let probe = source.probe().expect("configuration-only ssh probe");
        assert_eq!(probe.host_id, identity.host_id());
        assert_eq!(probe.location, "deploy@example.invalid");
        assert!(probe.remote_facts_deferred);
        let debug = format!("{source:?}");
        assert!(debug.contains("deploy@example.invalid"));
        assert!(debug.contains("/srv/opencode"));
        assert!(debug.contains("snapshot: true"));
        assert!(runner.stages().contains(&CommandStage::StartupProbe));
        assert!(!runner.stages().contains(&CommandStage::Stage1));
    }

    #[test]
    fn sched_remote_protocol_rejects_malformed_metadata_sources_and_records() {
        let runner = FakeSshRunner::default();
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "deploy@example.invalid", &identity);
        let source = SshHostSource::new(&host, transport).expect("valid ssh source");
        let probe = SshProbe {
            architecture: crate::transport::ssh::RemoteArchitecture::X86_64,
            xdg_data_home: Some("/home/test/.local/share".into()),
            available_kib: 1_048_576,
            machine_id_source: "/etc/machine-id".into(),
            machine_id_hash: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
                .into(),
        };
        let decode = |ndjson: Vec<u8>| {
            source.decode_collection(
                &SshCollection {
                    probe: probe.clone(),
                    ndjson,
                },
                123,
            )
        };

        let invalid_utf8 = decode(vec![0xff]).expect_err("NDJSON must be UTF-8");
        assert!(matches!(
            invalid_utf8,
            HostSourceError::InvalidRemoteResponse { ref detail } if detail.contains("UTF-8")
        ));

        let empty = decode(b"\n \r\n".to_vec()).expect_err("meta line is mandatory");
        assert!(matches!(
            empty,
            HostSourceError::InvalidRemoteResponse { ref detail } if detail.contains("响应为空")
        ));

        let malformed_meta = decode(b"{not-json}\n".to_vec()).expect_err("meta must be JSON");
        assert!(matches!(
            malformed_meta,
            HostSourceError::InvalidRemoteResponse { ref detail } if detail.contains("meta 行无法解析")
        ));

        let unsupported = meta_line(&identity, 0, 200, 0).replacen(
            "\"protocol_version\":1",
            "\"protocol_version\":2",
            1,
        );
        let unsupported = decode(unsupported.into_bytes()).expect_err("v2 is not accepted");
        assert!(matches!(
            unsupported,
            HostSourceError::InvalidRemoteResponse { ref detail }
                if detail.contains("protocol_version 2")
        ));

        let missing_source = meta_line(&identity, 0, 200, 0).replacen(
            "\"source\":\"opencode\"",
            "\"source\":\"codex\"",
            1,
        );
        let missing_source =
            decode(missing_source.into_bytes()).expect_err("opencode meta is mandatory");
        assert!(matches!(
            missing_source,
            HostSourceError::RemoteSourceMissing { found, .. } if found == vec!["codex"]
        ));

        let malformed_record = format!("{}\nnot-json\n", meta_line(&identity, 0, 200, 1));
        let malformed_record =
            decode(malformed_record.into_bytes()).expect_err("record must be normalized JSON");
        assert!(matches!(
            malformed_record,
            HostSourceError::MalformedRemoteRecord { line: 2, .. }
        ));
    }

    #[test]
    fn sched_inventory_schedule_replacement_and_saturating_due_time_are_observable() {
        let identity = local_identity();
        let host = HostRecord::local("本机", &identity);
        let pinned = SourceSchedule::for_kind(HostKind::Local)
            .with_min_interval_ms(MIN_AUTO_REFRESH_INTERVAL_MS)
            .with_duration_multiplier(0);
        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(
                SourceRegistration::for_host_source(&host, OPENCODE_SOURCE).with_schedule(pinned),
            )
            .expect("register pinned source");
        scheduler
            .register(ssh_registration("aaa-remote", TriggerMode::Manual))
            .expect("register manual remote");

        assert_eq!(
            scheduler.host_ids(),
            vec!["aaa-remote".to_owned(), identity.host_id().to_owned()]
        );
        assert_eq!(scheduler.statuses().len(), 2);
        assert_eq!(
            scheduler.interval_ms(&opencode_slot(identity.host_id())),
            Some(MIN_AUTO_REFRESH_INTERVAL_MS)
        );
        assert_eq!(scheduler.interval_ms(&opencode_slot("missing")), None);

        assert!(matches!(
            scheduler.trigger_manual(&opencode_slot(identity.host_id()), i64::MAX - 2),
            TriggerOutcome::Started(_)
        ));
        scheduler
            .complete(
                &opencode_slot(identity.host_id()),
                i64::MAX - 2,
                RoundReport::collected(
                    u64::MAX,
                    CollectSummary {
                        cursor_time_updated: None,
                        ..successful_summary(0)
                    },
                ),
            )
            .expect("complete source near timestamp ceiling");
        let status = scheduler
            .status(&opencode_slot(identity.host_id()))
            .expect("local status");
        assert_eq!(
            status.interval_ms, MIN_AUTO_REFRESH_INTERVAL_MS,
            "zero multiplier pins the floor"
        );
        assert_eq!(status.next_due_utc, Some(i64::MAX));
        assert_eq!(status.cursor_time_updated, None);

        scheduler
            .set_schedule(
                &opencode_slot(identity.host_id()),
                SourceSchedule::from_configured_interval(
                    HostKind::Local,
                    MIN_AUTO_REFRESH_INTERVAL_MS as i64,
                )
                .expect("the floor is a valid configured interval"),
            )
            .expect("replace schedule");
        assert_eq!(
            scheduler.interval_ms(&opencode_slot(identity.host_id())),
            Some(u64::MAX)
        );
    }

    #[test]
    fn sched_execute_action_reports_stamp_failure_and_ssh_remediation() {
        struct SuccessfulSource;

        impl HostSource for SuccessfulSource {
            fn host_id(&self) -> &str {
                "missing-host"
            }

            fn kind(&self) -> HostKind {
                HostKind::Local
            }

            fn probe(&self) -> Result<SourceProbe> {
                unreachable!("execute_action does not probe a source")
            }

            fn collect_incremental(
                &self,
                _archive: &mut Archive,
                _now_utc_ms: i64,
            ) -> Result<CollectOutcome> {
                Ok(CollectOutcome {
                    host_id: self.host_id().into(),
                    source: OPENCODE_SOURCE.into(),
                    reached_eof: true,
                    eligible_count: 0,
                    skipped_count: 0,
                    stats: IngestStats {
                        committed: true,
                        ..IngestStats::default()
                    },
                    coverage: None,
                    remote: None,
                    remote_probe: None,
                })
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let clock = ManualClock::new(7_000);
        clock.script_round(100, 25);
        let report = execute_action(
            &SuccessfulSource,
            &mut archive,
            &clock,
            &RefreshAction {
                host_id: "missing-host".into(),
                source: OPENCODE_SOURCE.into(),
                kind: HostKind::Local,
                reason: TriggerReason::Manual,
                started_at_utc: 6_000,
            },
        );
        assert_eq!(report.duration_ms, 25);
        assert!(matches!(
            report.result,
            RoundResult::Failed { ref error } if error.contains("not registered")
        ));

        let identity = remote_identity();
        let host = HostRecord::ssh("远端", "deploy@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register remote host");
        let runner = FakeSshRunner::default();
        runner.push_auth_failure();
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport).expect("valid ssh source");
        let clock = ManualClock::new(8_000);
        clock.script_round(200, 30);
        let report = execute_action(
            &source,
            &mut archive,
            &clock,
            &RefreshAction {
                host_id: host.host_id().into(),
                source: OPENCODE_SOURCE.into(),
                kind: HostKind::Ssh,
                reason: TriggerReason::Manual,
                started_at_utc: 8_000,
            },
        );
        assert_eq!(report.duration_ms, 30);
        let RoundResult::Failed { error } = report.result else {
            panic!("authentication failure must produce a failed round")
        };
        assert!(error.contains("SSH 认证失败"));
        assert!(error.contains("｜请检查 SSH 用户、密钥、agent 或钥匙串口令后重试。"));
        assert_eq!(archive_rows(&archive, host.host_id()), 0);
    }

    #[test]
    fn sched_system_clock_reports_epoch_time_and_monotonic_progress() {
        let clock = SystemClock;
        let wall = clock.now_utc_ms();
        let first = clock.monotonic_ms();
        let second = clock.monotonic_ms();

        assert!(wall > 1_700_000_000_000, "wall clock must be Unix epoch ms");
        assert!(second >= first, "monotonic time must never move backwards");
    }

    #[test]
    fn sched_origin_is_always_live_for_incremental_rounds() {
        assert_eq!(INCREMENTAL_ORIGIN, Origin::Live);
        assert_eq!(INCREMENTAL_ORIGIN.priority(), 3);
    }

    #[cfg(not(coverage))]
    const ELIGIBLE_PREDICATE: &str = "json_valid(data) \
AND json_extract(data,'$.role')='assistant' AND json_type(data,'$.tokens')='object'";

    #[cfg(not(coverage))]
    struct ExternalReconciliation {
        source_eligible: u64,
        archived: u64,
        archived_not_in_source: u64,
        source_not_archived: u64,
    }

    /// Reconciles the archive against the live source using only the external `sqlite3` binary.
    ///
    /// The archive is the main database (a temporary file) and the user's real source is ATTACHed
    /// `mode=ro`, so the external process cannot write to the source either. Comparison is by
    /// `message_id` set rather than by a `time_updated`-bounded count: the live source is being
    /// appended to and its rows' `time_updated` are bumped in place while this runs, so any
    /// timestamp bound would drift, whereas identifiers do not.
    #[cfg(not(coverage))]
    fn external_reconciliation(
        archive_path: &Path,
        source_path: &Path,
        host_id: &str,
    ) -> ExternalReconciliation {
        let script = format!(
            "ATTACH DATABASE 'file:{source}?mode=ro' AS src;\n\
CREATE TEMP TABLE eligible AS SELECT id FROM src.message WHERE {predicate};\n\
SELECT (SELECT count(*) FROM eligible),\n\
       (SELECT count(*) FROM usage_record WHERE host_id='{host_id}' AND source='opencode'),\n\
       (SELECT count(*) FROM usage_record r WHERE r.host_id='{host_id}' \
AND r.source='opencode' AND r.message_id NOT IN (SELECT id FROM eligible)),\n\
       (SELECT count(*) FROM eligible e WHERE e.id NOT IN \
(SELECT message_id FROM usage_record WHERE host_id='{host_id}' AND source='opencode'));",
            source = source_path.display(),
            predicate = ELIGIBLE_PREDICATE,
        );
        let output = std::process::Command::new("sqlite3")
            .arg(archive_path)
            .arg(&script)
            .output()
            .expect("run external sqlite3 binary");
        assert!(
            output.status.success(),
            "sqlite3 failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        let fields: Vec<u64> = text
            .trim()
            .split('|')
            .map(|field| field.parse().expect("parse sqlite3 count"))
            .collect();
        assert_eq!(fields.len(), 4, "unexpected sqlite3 output {text:?}");
        ExternalReconciliation {
            source_eligible: fields[0],
            archived: fields[1],
            archived_not_in_source: fields[2],
            source_not_archived: fields[3],
        }
    }

    #[cfg(not(coverage))]
    fn print_timeline_row(step: &str, now: i64, status: &SourceStatus) {
        println!(
            "{step:<28} now={now:<16} state={:<9} interval_ms={:<8} next_due_utc={:<16} \
last_success_utc={:?} interrupted={}",
            match &status.state {
                SourceState::Idle => "idle",
                SourceState::Running => "running",
                SourceState::Error { .. } => "error",
            },
            status.interval_ms,
            status
                .next_due_utc
                .map_or_else(|| "-".to_owned(), |due| due.to_string()),
            status.last_success_utc,
            status.interrupted,
        );
    }

    /// Manual QA: drives the real local OpenCode database through [`LocalHostSource`] into a
    /// temporary archive and cross-checks the archived row count with the external `sqlite3`
    /// binary. Assertions are on row counts and state names only; wall-clock durations are printed
    /// for the record and never asserted.
    #[cfg(not(coverage))]
    #[test]
    #[ignore = "manual QA scans the real 43 GB local database and invokes the external sqlite3 binary"]
    fn sched_manual_qa_real_local_database_matches_external_sqlite3() {
        let database_path = match crate::source::opencode::discover_database_path() {
            Ok(path) => path,
            Err(error) => {
                println!("SKIP: 本机没有可发现的 OpenCode 数据库：{error}");
                return;
            }
        };
        println!("real database  : {}", database_path.display());

        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity =
            MachineIdentity::from_machine_id("agentlens-manual-qa-local").expect("qa identity");
        let host = HostRecord::local("本机（manual QA）", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");
        println!("archive        : {}", archive.path().display());
        println!("host_id        : {}", host.host_id());

        let source = LocalHostSource::with_database(host.host_id(), &database_path);
        let probe = source.probe().expect("probe the real database read-only");
        println!("probe          : {probe:?}");

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration::for_host_source(&host, OPENCODE_SOURCE))
            .expect("register source");
        let clock = SystemClock;

        let start_utc = clock.now_utc_ms();
        print_timeline_row(
            "0 registered",
            start_utc,
            &scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status"),
        );
        let actions = scheduler.tick(start_utc);
        assert_eq!(actions.len(), 1, "a fresh auto source is due immediately");
        print_timeline_row(
            "1 tick -> running",
            start_utc,
            &scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status"),
        );

        let wall = Instant::now();
        let outcome = source
            .collect_incremental(&mut archive, clock.now_utc_ms())
            .expect("first real round");
        let first_elapsed = wall.elapsed();
        let observed_max = outcome
            .stats
            .cursor_time_updated
            .expect("a committed live round records its watermark");
        println!(
            "round 1 (full) : reached_eof={} eligible={} skipped={} received={} changed={} \
committed={} cursor={} coverage={:?} elapsed={:.3}s",
            outcome.reached_eof,
            outcome.eligible_count,
            outcome.skipped_count,
            outcome.stats.received_records,
            outcome.stats.changed_records,
            outcome.stats.committed,
            observed_max,
            outcome.coverage,
            first_elapsed.as_secs_f64()
        );
        assert!(outcome.is_success());

        let archived = archive_rows(&archive, host.host_id());
        assert_eq!(archived, outcome.eligible_count);
        let external = external_reconciliation(archive.path(), &database_path, host.host_id());
        println!("archived rows  : {archived}  (rust: archive.usage_record)");
        println!(
            "sqlite3 rows   : archived={} source_eligible={} archived_not_in_source={} \
source_not_archived={}",
            external.archived,
            external.source_eligible,
            external.archived_not_in_source,
            external.source_not_archived
        );
        assert_eq!(
            archived, external.archived,
            "the external binary must see exactly the rows this round archived"
        );
        assert_eq!(
            external.archived_not_in_source, 0,
            "every archived row must still be an eligible row of the real source"
        );
        assert_eq!(
            external.archived + external.source_not_archived,
            external.source_eligible,
            "archived rows plus rows created after the scan snapshot must reconcile with the source"
        );
        println!(
            "reconciliation : {} archived + {} created after the scan snapshot = {} eligible now",
            external.archived, external.source_not_archived, external.source_eligible
        );

        let after_first = clock.now_utc_ms();
        scheduler
            .complete(
                &opencode_slot(host.host_id()),
                after_first,
                RoundReport::collected(
                    u64::try_from(first_elapsed.as_millis()).unwrap_or(u64::MAX),
                    outcome.summary(),
                ),
            )
            .expect("complete first round");
        let status = scheduler
            .status(&opencode_slot(host.host_id()))
            .expect("status");
        print_timeline_row("2 complete -> idle", after_first, &status);
        assert_eq!(status.state, SourceState::Idle);
        assert_eq!(
            status.interval_ms,
            DEFAULT_LOCAL_MIN_INTERVAL_MS
                .max(3 * u64::try_from(first_elapsed.as_millis()).unwrap_or(u64::MAX))
        );
        assert!(
            scheduler.tick(after_first).is_empty(),
            "the adaptive interval must gate the very next tick"
        );
        print_timeline_row("3 tick (not due)", after_first, &status);

        let TriggerOutcome::Started(_) =
            scheduler.trigger_manual(&opencode_slot(host.host_id()), after_first)
        else {
            panic!("manual trigger must bypass the timer");
        };
        print_timeline_row(
            "4 manual -> running",
            after_first,
            &scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status"),
        );
        let wall = Instant::now();
        let second = source
            .collect_incremental(&mut archive, clock.now_utc_ms())
            .expect("second incremental round");
        let second_elapsed = wall.elapsed();
        let window_start = observed_max - crate::source::opencode::OVERLAP_WINDOW_MS;
        let second_max = second
            .stats
            .cursor_time_updated
            .expect("second live round records its watermark");
        println!(
            "round 2 (24h)  : window_start={window_start} reached_eof={} eligible={} skipped={} \
changed={} cursor={} elapsed={:.3}s",
            second.reached_eof,
            second.eligible_count,
            second.skipped_count,
            second.stats.changed_records,
            second_max,
            second_elapsed.as_secs_f64()
        );
        assert!(second.is_success());
        assert!(
            second_max >= observed_max,
            "an incremental round must never move the watermark backwards"
        );
        let after_second_external =
            external_reconciliation(archive.path(), &database_path, host.host_id());
        println!(
            "sqlite3 rows   : archived={} source_eligible={} archived_not_in_source={} \
source_not_archived={}",
            after_second_external.archived,
            after_second_external.source_eligible,
            after_second_external.archived_not_in_source,
            after_second_external.source_not_archived
        );
        assert_eq!(
            after_second_external.archived_not_in_source, 0,
            "the incremental round must not invent rows the source does not have"
        );
        assert_eq!(
            after_second_external.archived + after_second_external.source_not_archived,
            after_second_external.source_eligible
        );

        let after_second = clock.now_utc_ms();
        scheduler
            .complete(
                &opencode_slot(host.host_id()),
                after_second,
                RoundReport::collected(
                    u64::try_from(second_elapsed.as_millis()).unwrap_or(u64::MAX),
                    second.summary(),
                ),
            )
            .expect("complete second round");
        print_timeline_row(
            "5 complete -> idle",
            after_second,
            &scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status"),
        );
        println!(
            "final archived : {} rows, cursor={:?}",
            archive_rows(&archive, host.host_id()),
            read_cursor(archive.connection(), host.host_id()).expect("read cursor")
        );
        println!(
            "source db size : {} bytes (opened mode=ro, query_only=ON, never written)",
            std::fs::metadata(&database_path)
                .expect("stat source database")
                .len()
        );
    }

    /// Manual QA: drives the SSH path end-to-end over the fake [`CommandRunner`] and prints the
    /// archived row count plus the `hosts.last_success_utc` it stamped.
    #[cfg(not(coverage))]
    #[test]
    #[ignore = "manual QA prints the fake-SSH end-to-end row count and hosts.last_success value"]
    fn sched_manual_qa_fake_ssh_end_to_end_rows_and_last_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut archive = open_temp_archive(temp.path());
        let identity = remote_identity();
        let host = HostRecord::ssh("远端（manual QA）", "qa@example.invalid", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("register host row");
        println!("archive        : {}", archive.path().display());
        println!(
            "host_id        : {}  machine_id_hash={}",
            host.host_id(),
            host.machine_id_hash()
        );
        println!("hosts.last_success_utc before: {:?}", host.last_success_utc);

        let runner = FakeSshRunner::default();
        let cutoff = 1_785_509_214_952;
        runner.push_ndjson(&remote_ndjson(
            &identity,
            0,
            cutoff,
            &["qa_msg_1", "qa_msg_2", "qa_msg_3", "qa_msg_4"],
        ));
        let (_artifact_temp, transport) = ssh_transport(&runner);
        let source = SshHostSource::new(&host, transport).expect("build ssh source");

        let mut scheduler = RefreshScheduler::new();
        scheduler
            .register(SourceRegistration::for_host_source(&host, OPENCODE_SOURCE))
            .expect("register ssh source");
        let stamped_at = 1_785_509_300_000;
        let TriggerOutcome::Started(action) =
            scheduler.trigger_manual(&opencode_slot(host.host_id()), stamped_at)
        else {
            panic!("manual trigger must start the remote round");
        };
        print_timeline_row(
            "1 manual -> running",
            stamped_at,
            &scheduler
                .status(&opencode_slot(host.host_id()))
                .expect("status"),
        );

        let clock = ManualClock::new(stamped_at);
        clock.script_round(1_000, 3_400);
        let report = execute_action(&source, &mut archive, &clock, &action);
        println!("round report   : {report:?}");
        scheduler
            .complete(&opencode_slot(host.host_id()), stamped_at, report)
            .expect("complete remote round");
        let status = scheduler
            .status(&opencode_slot(host.host_id()))
            .expect("status");
        print_timeline_row("2 complete -> idle", stamped_at, &status);

        let rows = archive_rows(&archive, host.host_id());
        let stored = HostRegistry::new(archive.connection())
            .get(host.host_id())
            .expect("read host")
            .expect("host row present");
        println!("archived rows  : {rows}");
        println!(
            "hosts.last_success_utc after : {:?}",
            stored.last_success_utc
        );
        println!(
            "source_cursor  : {:?}",
            read_cursor(archive.connection(), host.host_id()).expect("read cursor")
        );
        println!("ssh stages     : {:?}", runner.stages());
        assert_eq!(rows, 4);
        assert_eq!(stored.last_success_utc, Some(stamped_at));
        assert_eq!(status.state, SourceState::Idle);
    }
}
