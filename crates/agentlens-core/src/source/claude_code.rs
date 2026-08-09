//! Claude Code transcript (JSONL) 解析与增量扫描。
//!
//! 数据面是 `<data_dir>/projects/**/*.jsonl`，每行一个事件对象，只有
//! `type == "assistant"` 且 `message.usage` 为对象的行携带用量。源目录按
//! `cleanupPeriodDays`（默认 30 天）自动清理，所以本适配器只读、绝不写回，
//! 归档库才是权威历史。契约文档：`docs/adapters/claude-code.md`。
//!
//! 扫描算法与 `source::opencode` 对齐：watermark 减 24h 重叠窗口做下界，
//! 重复读到的行由 `crate::ingest` 的 upsert 幂等吸收；watermark 仅在完整扫完
//! 全部文件（EOF）后推进为本轮观察到的 `max(timestamp)`，中断 / 取消 / 出错则不推进。
//!
//! # 去重（与 OpenCode 的关键差异）
//!
//! 同一条 assistant 消息会被 Claude Code 多次写入 transcript：流式过程中每个
//! 快照写一行，续写 / 分支 / sidechain 也会重复。去重键是
//! `(message.id, requestId)`，**`requestId` 缺失时退化为只用 `message.id`**
//! —— 第三方 Anthropic 兼容网关与部分 sub-agent 传输路径根本不写 `requestId`，
//! 若此时放弃去重会把同一次调用重复计入（社区实测 1.6–3.7 倍高估）。
//!
//! 冲突时的替换优先级（先命中者胜出，与扫描顺序无关，因此幂等）：
//!
//! 1. `isSidechain == false` 优于 `true`；
//! 2. 五项 token 之和更大的一条胜出（流式快照里 `output_tokens` 是递增的，
//!    取最大即取最终值，取第一条会显著低估）；
//! 3. 携带 `speed` 字段的一条胜出；
//! 4. 全部相同则保留已入选的那条。
//!
//! 为保证顺序无关，本适配器必须先把整轮候选收进 map 再交付 sink，
//! 无法像 OpenCode 那样边扫边发。内存上界是重叠窗口内的合格行数。
//!
//! # 成本
//!
//! 顶层 `costUSD` 在当前版本普遍缺失或不可信，一律不入 `cost`：
//! `cost = None` + `CostSource::Unavailable`，绝不写 synthetic zero。
//! `usage.cache_creation` 的 1 小时档（`ephemeral_1h_input_tokens`）单独计数并
//! 由 [`ScanResult::ephemeral_1h_rows`] 上报：内置价目表对 Anthropic 的
//! cache write 取 5 分钟档单价，1h 档存在时按该单价估算会低估成本，
//! 这个事实必须可观测而不是被静默吞掉。
//!
//! 数据目录发现顺序：`CLAUDE_CODE_DATA_DIR` → `CLAUDE_CONFIG_DIR` →
//! `$XDG_CONFIG_HOME/claude` → `~/.config/claude` → `~/.claude`。

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::archive::{normalize_agent_key, CostSource, NormalizedUsageRecord, Origin};

/// Archive source key for every Claude Code record.
pub const CLAUDE_CODE_SOURCE: &str = "claude-code";
/// Transcripts never name a provider; Claude Code always talks to Anthropic-shaped APIs.
pub const PROVIDER_ID: &str = "anthropic";
/// Inclusive overlap applied before every stored watermark, mirroring [`super::opencode`].
pub const OVERLAP_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
/// Number of normalized records delivered to a sink at once.
pub const DEFAULT_BATCH_SIZE: usize = 1_000;
/// Directory below the data directory that holds per-project transcripts.
pub const PROJECTS_DIRECTORY: &str = "projects";
/// `agent_raw` written for a sidechain (sub-agent) message.
pub const SIDECHAIN_AGENT: &str = "sidechain";
/// `agent_raw` written for a main-thread message.
///
/// The reserved contract wrote an empty string here, but an empty `agent_key` renders blank and
/// groups with nothing in the aggregation layer. A concrete key is the smaller lie.
pub const MAIN_AGENT: &str = "main";

/// Result type returned by Claude Code discovery and scanning operations.
pub type Result<T> = std::result::Result<T, ClaudeCodeError>;

/// Errors that prevent a read-only Claude Code scan from producing a trustworthy result.
#[derive(Debug, Error)]
pub enum ClaudeCodeError {
    /// No candidate transcript root exists in the configured discovery chain.
    #[error(
        "Claude Code transcript directory was not found; probed paths: {}",
        display_paths(.probed_paths)
    )]
    ProjectsNotFound {
        /// Exact `projects` directories checked in precedence order.
        probed_paths: Vec<PathBuf>,
    },
    /// The transcript root exists but its directory tree cannot be walked.
    #[error("cannot enumerate Claude Code transcripts under {path}: {source}. Grant read and execute access with chmod, or add the AgentLens user to the owning group")]
    Enumerate {
        /// Directory whose listing failed.
        path: PathBuf,
        /// Original filesystem error.
        source: io::Error,
    },
    /// A caller supplied an impossible batch size.
    #[error("Claude Code scan batch_size must be greater than zero")]
    InvalidBatchSize,
    /// A non-retryable failure stopped a scan before every file was consumed.
    #[error("Claude Code scan stopped before EOF: {source}")]
    ScanFailed {
        /// Partial counters for diagnostics; its watermark is always absent.
        partial: Box<ScanResult>,
        /// Underlying streaming failure.
        source: StreamError,
    },
}

/// Failure emitted while transcripts are streamed.
#[derive(Debug, Error)]
pub enum StreamError {
    /// A transcript file could not be opened or read.
    #[error("cannot read Claude Code transcript {path}: {source}")]
    Io {
        /// Transcript whose read failed.
        path: PathBuf,
        /// Original filesystem error.
        source: io::Error,
    },
    /// The caller-supplied sink requested an orderly interruption.
    #[error("scan interrupted by sink: {0}")]
    Interrupted(String),
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

/// Reason one transcript line did not produce a normalized assistant usage record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The line is not valid JSON, or is not a JSON object.
    MalformedJson,
    /// The event `type` is absent or is not `assistant`.
    NonAssistant,
    /// An assistant line carries no `message.usage` key.
    MissingUsage,
    /// `message.usage` exists but is not an object.
    InvalidUsage,
    /// `message.id` is absent or blank, so the row cannot join the global dedup key.
    MissingMessageId,
    /// The top-level `timestamp` is absent or not ISO-8601.
    UnparsableTimestamp,
    /// The row lost a dedup conflict against a better candidate for the same key.
    DuplicateSuppressed,
}

impl SkipReason {
    /// Returns the stable snake_case name used in diagnostics and reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MalformedJson => "malformed_json",
            Self::NonAssistant => "non_assistant",
            Self::MissingUsage => "missing_usage",
            Self::InvalidUsage => "invalid_usage",
            Self::MissingMessageId => "missing_message_id",
            Self::UnparsableTimestamp => "unparsable_timestamp",
            Self::DuplicateSuppressed => "duplicate_suppressed",
        }
    }
}

/// Counts each supported skip category without stopping the scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkippedBreakdown {
    /// Lines whose text is invalid JSON or not a JSON object.
    pub malformed_json: u64,
    /// Lines whose `type` is not `assistant`.
    pub non_assistant: u64,
    /// Assistant lines with no `message.usage` key.
    pub missing_usage: u64,
    /// Assistant lines whose `message.usage` value is not an object.
    pub invalid_usage: u64,
    /// Assistant lines with no usable `message.id`.
    pub missing_message_id: u64,
    /// Assistant lines whose `timestamp` cannot be parsed.
    pub unparsable_timestamp: u64,
    /// Assistant lines discarded as duplicate writes of an already-selected message.
    pub duplicate_suppressed: u64,
}

impl SkippedBreakdown {
    fn increment(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::MalformedJson => self.malformed_json += 1,
            SkipReason::NonAssistant => self.non_assistant += 1,
            SkipReason::MissingUsage => self.missing_usage += 1,
            SkipReason::InvalidUsage => self.invalid_usage += 1,
            SkipReason::MissingMessageId => self.missing_message_id += 1,
            SkipReason::UnparsableTimestamp => self.unparsable_timestamp += 1,
            SkipReason::DuplicateSuppressed => self.duplicate_suppressed += 1,
        }
    }

    /// Returns the total across every category.
    pub const fn total(&self) -> u64 {
        self.malformed_json
            + self.non_assistant
            + self.missing_usage
            + self.invalid_usage
            + self.missing_message_id
            + self.unparsable_timestamp
            + self.duplicate_suppressed
    }
}

/// Observations that are not skips but do change how much the result can be trusted.
///
/// These exist because the archive contract forbids silently filling gaps: a cost that cannot be
/// estimated at the catalogued rate must be visible, not rounded to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanDiagnostics {
    /// Selected records whose `requestId` was absent, so dedup fell back to `message.id` alone.
    pub missing_request_id: u64,
    /// Selected records reporting one-hour ephemeral cache writes.
    ///
    /// The built-in price catalogue prices Anthropic cache writes at the five-minute rate, so any
    /// estimate covering these rows is a lower bound rather than an equality.
    pub ephemeral_1h_rows: u64,
    /// Sum of `usage.cache_creation.ephemeral_1h_input_tokens` across those records.
    pub ephemeral_1h_tokens: u64,
    /// Selected records whose flat `cache_creation_input_tokens` disagreed with the
    /// `cache_creation` breakdown; the breakdown wins and this counts the disagreement.
    pub cache_creation_mismatch: u64,
    /// Selected records whose model is a client-side placeholder such as `<synthetic>`.
    ///
    /// These never match the price catalogue. They are all-zero-token API error replies, so they
    /// are also `is_incomplete` and excluded from aggregation.
    pub synthetic_model: u64,
    /// Transcript files opened during the final scan attempt.
    pub files_scanned: u64,
    /// Transcript files that existed but could not be read; each is a coverage hole.
    pub files_unreadable: u64,
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
    /// Rust-side delivery batch size.
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

    /// Returns the inclusive lower time boundary after applying the 24-hour overlap.
    pub fn window_start(&self) -> i64 {
        self.watermark
            .map_or(i64::MIN, |value| value.saturating_sub(OVERLAP_WINDOW_MS))
    }
}

/// Why a scan returned without consuming every file but did not raise a fatal error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanSkipReason {
    /// The sink cancelled or rejected a delivered batch.
    Interrupted(String),
}

/// Observable result of one scan round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// Number of records actually handed to the sink.
    pub delivered_records: u64,
    /// Number of sink calls.
    pub delivered_batches: u64,
    /// Records selected as the winner of their dedup key.
    pub eligible_count: u64,
    /// Lines that produced no record, including suppressed duplicates.
    pub skipped_count: u64,
    /// Skip categories observed in this attempt.
    pub skipped_breakdown: SkippedBreakdown,
    /// Trust-affecting observations that are not skips.
    pub diagnostics: ScanDiagnostics,
    /// Maximum observed record timestamp, present only after every file was consumed.
    pub observed_max_time_updated: Option<i64>,
    /// The sole signal that the caller may use to advance its committed watermark.
    pub reached_eof: bool,
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
            diagnostics: ScanDiagnostics::default(),
            observed_max_time_updated: None,
            reached_eof: false,
            last_success_utc,
            skip_reason: None,
        }
    }

    /// Returns whether any observation makes an Anthropic cost estimate a lower bound.
    ///
    /// True when one-hour ephemeral cache writes were seen: the catalogue rate is the five-minute
    /// one, so an estimate built from it under-reports rather than matching.
    pub const fn cost_estimate_is_lower_bound(&self) -> bool {
        self.diagnostics.ephemeral_1h_rows > 0
    }
}

/// Global dedup key: `message.id` plus `requestId` when the source provides one.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DedupKey {
    /// `message.id`, guaranteed unique per API response by the Anthropic protocol.
    pub message_id: String,
    /// Top-level `requestId`, absent for third-party gateways and some sub-agent paths.
    pub request_id: Option<String>,
}

impl DedupKey {
    /// Renders the key as the archive `message_id`, matching the reserved contract.
    ///
    /// `<message.id>#<requestId>` when a request id exists, otherwise the bare `message.id`.
    /// `message.id` never contains `#`, so the two shapes cannot collide.
    pub fn archive_message_id(&self) -> String {
        match &self.request_id {
            Some(request_id) => format!("{}#{request_id}", self.message_id),
            None => self.message_id.clone(),
        }
    }
}

/// One parsed assistant usage line, before dedup selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Global dedup key this candidate competes for.
    pub key: DedupKey,
    /// Normalized record built from this line.
    pub record: NormalizedUsageRecord,
    /// Whether the line was written on a sidechain (sub-agent) branch.
    pub is_sidechain: bool,
    /// Whether the line carried `usage.speed`.
    pub has_speed: bool,
    /// One-hour ephemeral cache-write tokens, tracked for the cost lower-bound warning.
    pub ephemeral_1h_tokens: u64,
    /// Whether the flat cache-creation total disagreed with the breakdown.
    pub cache_creation_mismatch: bool,
}

impl Candidate {
    fn token_total(&self) -> u128 {
        u128::from(self.record.tok_input)
            + u128::from(self.record.tok_output)
            + u128::from(self.record.tok_reasoning)
            + u128::from(self.record.tok_cache_read)
            + u128::from(self.record.tok_cache_write)
    }

    /// Returns true when `self` must replace `incumbent` for the same dedup key.
    ///
    /// Ordering is a total, antisymmetric comparison, so the selected winner is independent of the
    /// order files and lines are visited. That is what makes repeated scans idempotent.
    fn outranks(&self, incumbent: &Self) -> bool {
        if self.is_sidechain != incumbent.is_sidechain {
            return !self.is_sidechain;
        }
        let (mine, theirs) = (self.token_total(), incumbent.token_total());
        if mine != theirs {
            return mine > theirs;
        }
        if self.has_speed != incumbent.has_speed {
            return self.has_speed;
        }
        false
    }
}

/// Parses one transcript line into a dedup candidate.
///
/// `project_dir_fallback` is used only when the line carries no top-level `cwd`.
pub fn parse_line(
    line: &str,
    project_dir_fallback: &str,
    context: &ParseContext,
) -> std::result::Result<Candidate, SkipReason> {
    let event: Value = serde_json::from_str(line).map_err(|_| SkipReason::MalformedJson)?;
    if !event.is_object() {
        return Err(SkipReason::MalformedJson);
    }
    if event.get("type").and_then(Value::as_str) != Some("assistant") {
        return Err(SkipReason::NonAssistant);
    }
    let Some(usage) = event.pointer("/message/usage") else {
        return Err(SkipReason::MissingUsage);
    };
    if !usage.is_object() {
        return Err(SkipReason::InvalidUsage);
    }

    let message_id = event
        .pointer("/message/id")
        .and_then(value_to_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(SkipReason::MissingMessageId)?;
    let request_id = event
        .get("requestId")
        .and_then(value_to_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let timestamp = event
        .get("timestamp")
        .and_then(parse_timestamp_ms)
        .ok_or(SkipReason::UnparsableTimestamp)?;

    let tok_input = lossy_u64(usage.get("input_tokens"));
    let tok_output = lossy_u64(usage.get("output_tokens"));
    let tok_cache_read = lossy_u64(usage.get("cache_read_input_tokens"));
    let flat_cache_write = lossy_u64(usage.get("cache_creation_input_tokens"));
    // The breakdown object was added after the flat field. When present it is authoritative;
    // adding both double-counts the same tokens.
    let breakdown = usage
        .get("cache_creation")
        .filter(|value| value.is_object());
    let ephemeral_5h = breakdown.map(|value| lossy_u64(value.get("ephemeral_5m_input_tokens")));
    let ephemeral_1h = breakdown.map(|value| lossy_u64(value.get("ephemeral_1h_input_tokens")));
    let tok_cache_write = match (ephemeral_5h, ephemeral_1h) {
        (Some(five_minute), Some(one_hour)) => five_minute.saturating_add(one_hour),
        _ => flat_cache_write,
    };
    let cache_creation_mismatch = breakdown.is_some() && tok_cache_write != flat_cache_write;

    let is_sidechain = event
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let agent_raw = if is_sidechain {
        SIDECHAIN_AGENT
    } else {
        MAIN_AGENT
    };
    let model_id = event
        .pointer("/message/model")
        .and_then(value_to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let session_id = event
        .get("sessionId")
        .or_else(|| event.get("session_id"))
        .and_then(value_to_string)
        .unwrap_or_default();
    let project_dir = event
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| project_dir_fallback.to_owned());

    let key = DedupKey {
        message_id,
        request_id,
    };
    let record = NormalizedUsageRecord {
        host_id: context.host_id.clone(),
        source: CLAUDE_CODE_SOURCE.to_owned(),
        granularity: crate::archive::UsageGranularity::Message,
        message_id: key.archive_message_id(),
        session_id,
        time_created_utc: timestamp,
        // Transcripts carry no completion timestamp; a synthesized one would be a fabrication.
        time_completed_utc: None,
        source_time_updated: timestamp,
        origin: context.origin,
        origin_priority: context.origin.priority(),
        agent_key: normalize_agent_key(agent_raw),
        agent_raw: agent_raw.to_owned(),
        provider_id: PROVIDER_ID.to_owned(),
        model_id,
        // Claude Code has no reasoning-token or variant concept in its usage object.
        variant: None,
        tok_input,
        tok_output,
        tok_reasoning: 0,
        tok_cache_read,
        tok_cache_write,
        // `costUSD` is missing or untrustworthy in every observed version, so cost stays absent
        // rather than becoming a synthetic zero.
        cost: None,
        cost_source: CostSource::Unavailable,
        is_incomplete: tok_input == 0
            && tok_output == 0
            && tok_cache_read == 0
            && tok_cache_write == 0,
        project_dir,
    };

    Ok(Candidate {
        key,
        record,
        is_sidechain,
        has_speed: usage.get("speed").is_some_and(|value| !value.is_null()),
        ephemeral_1h_tokens: ephemeral_1h.unwrap_or(0),
        cache_creation_mismatch,
    })
}

/// Injectable transcript enumeration seam.
///
/// Production walks a real directory tree; tests inject unreadable files and deterministic orders
/// without needing filesystem permission tricks.
pub trait TranscriptSource {
    /// Returns every transcript to consider, in a deterministic order.
    fn transcripts(&self) -> std::result::Result<Vec<PathBuf>, StreamError>;

    /// Streams one transcript, invoking `visitor` once per non-empty line.
    fn read_lines(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(&str) -> std::result::Result<(), StreamError>,
    ) -> std::result::Result<(), StreamError>;

    /// Returns the project directory fallback for a transcript.
    ///
    /// The `projects/<encoded>` directory name is a lossy encoding of the original path (both `/`
    /// and `.` collapse to `-`), so it cannot be reversed. It is returned verbatim and only used
    /// when a line carries no `cwd`.
    fn project_dir_hint(&self, path: &Path) -> String {
        path.parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Production [`TranscriptSource`] over a `projects` directory tree.
#[derive(Clone, Debug)]
pub struct DirectoryTranscriptSource {
    projects_dir: PathBuf,
}

impl DirectoryTranscriptSource {
    /// Wraps an explicit `projects` directory.
    pub fn new(projects_dir: impl Into<PathBuf>) -> Self {
        Self {
            projects_dir: projects_dir.into(),
        }
    }

    /// Resolves the transcript root through the documented environment precedence order.
    pub fn discover() -> Result<Self> {
        Ok(Self::new(discover_projects_dir()?))
    }

    /// Returns the `projects` directory this source walks.
    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }
}

impl TranscriptSource for DirectoryTranscriptSource {
    fn transcripts(&self) -> std::result::Result<Vec<PathBuf>, StreamError> {
        let mut files = Vec::new();
        collect_transcripts(&self.projects_dir, &mut files)?;
        // A stable order keeps counters and delivery batches reproducible across rounds.
        files.sort();
        Ok(files)
    }

    fn read_lines(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(&str) -> std::result::Result<(), StreamError>,
    ) -> std::result::Result<(), StreamError> {
        let file = File::open(path).map_err(|source| StreamError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|source| StreamError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            if line.trim().is_empty() {
                continue;
            }
            visitor(&line)?;
        }
        Ok(())
    }
}

fn collect_transcripts(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> std::result::Result<(), StreamError> {
    let entries = fs::read_dir(directory).map_err(|source| StreamError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| StreamError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| StreamError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_transcripts(&path, files)?;
        } else if path.extension().is_some_and(|value| value == "jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

/// Scans the discovered transcript root and streams normalized records to `sink`.
pub fn scan_default<F>(request: &ScanRequest, sink: F) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    let source = DirectoryTranscriptSource::discover()?;
    scan_source(&source, request, sink)
}

/// Scans an explicit `projects` directory and streams normalized records to `sink`.
pub fn scan_projects_dir<F>(
    projects_dir: impl AsRef<Path>,
    request: &ScanRequest,
    sink: F,
) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    let projects_dir = projects_dir.as_ref();
    if !projects_dir.is_dir() {
        return Err(ClaudeCodeError::ProjectsNotFound {
            probed_paths: vec![projects_dir.to_path_buf()],
        });
    }
    scan_source(&DirectoryTranscriptSource::new(projects_dir), request, sink)
}

/// Scans an injected transcript source, so interruption and unreadable files stay testable.
///
/// Dedup selection needs every candidate before a winner is known, so all eligible records are
/// buffered and only then delivered in `batch_size` chunks. Sink rejection therefore always leaves
/// `observed_max_time_updated` absent and the watermark unmoved.
pub fn scan_source<S, F>(source: &S, request: &ScanRequest, mut sink: F) -> Result<ScanResult>
where
    S: TranscriptSource + ?Sized,
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    if request.batch_size == 0 {
        return Err(ClaudeCodeError::InvalidBatchSize);
    }

    let mut result = ScanResult::empty(request.last_success_utc);
    let context = ParseContext::new(&request.host_id, request.origin);
    let window_start = request.window_start();

    let transcripts = match source.transcripts() {
        Ok(transcripts) => transcripts,
        Err(error) => {
            return Err(ClaudeCodeError::ScanFailed {
                partial: Box::new(result),
                source: error,
            })
        }
    };

    let mut selected = BTreeMap::<DedupKey, Candidate>::new();
    let mut observed_max = None::<i64>;

    for path in &transcripts {
        let hint = source.project_dir_hint(path);
        let mut line_error = None::<StreamError>;
        let read = source.read_lines(path, &mut |line| {
            match parse_line(line, &hint, &context) {
                Ok(candidate) => {
                    // The overlap window is applied to the record timestamp, mirroring the SQL
                    // predicate the OpenCode scanner pushes down.
                    if candidate.record.source_time_updated < window_start {
                        return Ok(());
                    }
                    observed_max = Some(match observed_max {
                        Some(current) => current.max(candidate.record.source_time_updated),
                        None => candidate.record.source_time_updated,
                    });
                    match selected.get(&candidate.key) {
                        Some(incumbent) if !candidate.outranks(incumbent) => {
                            result
                                .skipped_breakdown
                                .increment(SkipReason::DuplicateSuppressed);
                        }
                        Some(_) => {
                            result
                                .skipped_breakdown
                                .increment(SkipReason::DuplicateSuppressed);
                            selected.insert(candidate.key.clone(), candidate);
                        }
                        None => {
                            selected.insert(candidate.key.clone(), candidate);
                        }
                    }
                }
                Err(reason) => result.skipped_breakdown.increment(reason),
            }
            Ok(())
        });
        match read {
            Ok(()) => result.diagnostics.files_scanned += 1,
            Err(StreamError::Io { .. }) => {
                // One unreadable transcript is a coverage hole, not a reason to abandon the round
                // and lose every other file. It is counted so the gap stays visible.
                result.diagnostics.files_unreadable += 1;
            }
            Err(error) => line_error = Some(error),
        }
        if let Some(error) = line_error {
            result.observed_max_time_updated = None;
            return Err(ClaudeCodeError::ScanFailed {
                partial: Box::new(result),
                source: error,
            });
        }
    }

    for candidate in selected.values() {
        result.eligible_count += 1;
        if candidate.key.request_id.is_none() {
            result.diagnostics.missing_request_id += 1;
        }
        if candidate.ephemeral_1h_tokens > 0 {
            result.diagnostics.ephemeral_1h_rows += 1;
            result.diagnostics.ephemeral_1h_tokens = result
                .diagnostics
                .ephemeral_1h_tokens
                .saturating_add(candidate.ephemeral_1h_tokens);
        }
        if candidate.cache_creation_mismatch {
            result.diagnostics.cache_creation_mismatch += 1;
        }
        if is_placeholder_model(&candidate.record.model_id) {
            result.diagnostics.synthetic_model += 1;
        }
    }
    result.skipped_count = result.skipped_breakdown.total();

    let records = selected
        .into_values()
        .map(|candidate| candidate.record)
        .collect::<Vec<_>>();
    for batch in records.chunks(request.batch_size) {
        if let Err(error) = sink(batch) {
            result.observed_max_time_updated = None;
            result.skip_reason = Some(ScanSkipReason::Interrupted(error.to_string()));
            return Ok(result);
        }
        result.delivered_records += batch.len() as u64;
        result.delivered_batches += 1;
    }

    result.observed_max_time_updated = observed_max;
    result.reached_eof = true;
    Ok(result)
}

/// Returns true for client-side placeholder models that can never match the price catalogue.
pub fn is_placeholder_model(model_id: &str) -> bool {
    model_id.starts_with('<') && model_id.ends_with('>')
}

/// Resolves the first existing `projects` directory in the documented precedence order.
pub fn discover_projects_dir() -> Result<PathBuf> {
    let explicit = env::var_os("CLAUDE_CODE_DATA_DIR").map(PathBuf::from);
    let config = env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let xdg = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = dirs::home_dir();
    discover_projects_dir_from(
        explicit.as_deref(),
        config.as_deref(),
        xdg.as_deref(),
        home.as_deref(),
    )
}

fn discover_projects_dir_from(
    explicit_data_dir: Option<&Path>,
    claude_config_dir: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf> {
    let mut probed_paths = Vec::new();
    for directory in [explicit_data_dir, claude_config_dir].into_iter().flatten() {
        probed_paths.push(directory.join(PROJECTS_DIRECTORY));
    }
    if let Some(directory) = xdg_config_home {
        probed_paths.push(directory.join("claude").join(PROJECTS_DIRECTORY));
    }
    if let Some(directory) = home {
        probed_paths.push(
            directory
                .join(".config")
                .join("claude")
                .join(PROJECTS_DIRECTORY),
        );
        probed_paths.push(directory.join(".claude").join(PROJECTS_DIRECTORY));
    }
    if let Some(path) = probed_paths.iter().find(|path| path.is_dir()) {
        return Ok(path.clone());
    }
    Err(ClaudeCodeError::ProjectsNotFound { probed_paths })
}

fn parse_timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|value| value.timestamp_millis());
    }
    // Some third-party writers emit epoch milliseconds directly.
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
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

/// Deterministic synthetic transcripts plus the exact numbers a correct scan must report.
///
/// Real transcripts are never committed: they carry absolute project paths, git branch names and
/// conversation content. This module reproduces every shape observed on real data (streaming
/// duplicates, missing `requestId`, sidechain branches, `<synthetic>` error replies, the
/// `cache_creation` breakdown, one-hour ephemeral writes) as source code instead.
pub mod fixture {
    use std::fs;
    use std::io;
    use std::path::Path;

    /// Exact expectations for the tree written by [`write`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Manifest {
        /// Transcript files written.
        pub files: u64,
        /// Records that must survive dedup.
        pub eligible: u64,
        /// Lines that must be skipped, across every category.
        pub skipped: u64,
        /// Duplicate writes that must lose their dedup conflict.
        pub duplicate_suppressed: u64,
        /// Winners whose `requestId` was absent.
        pub missing_request_id: u64,
        /// Winners reporting one-hour ephemeral cache writes.
        pub ephemeral_1h_rows: u64,
        /// Sum of one-hour ephemeral cache-write tokens across winners.
        pub ephemeral_1h_tokens: u64,
        /// Winners whose flat cache-creation total disagreed with the breakdown.
        pub cache_creation_mismatch: u64,
        /// Winners whose model is a client-side placeholder.
        pub synthetic_model: u64,
        /// Winners with all four token counts at zero.
        pub incomplete: u64,
        /// Highest record timestamp in the tree, in UTC epoch milliseconds.
        pub max_timestamp_ms: i64,
    }

    /// Timestamp of the streaming-duplicate group's final snapshot.
    pub const STREAMING_FINAL_MS: i64 = 1_785_000_003_000;
    /// Dedup key of the streaming-duplicate group, which carries no `requestId`.
    pub const STREAMING_MESSAGE_ID: &str = "msg_stream_no_request_id";
    /// Final `output_tokens` of the streaming group; earlier snapshots report fewer.
    pub const STREAMING_FINAL_OUTPUT: u64 = 619;
    /// Dedup key of the group that owns a `requestId`.
    pub const REQUEST_ID_MESSAGE_ID: &str = "msg_with_request";
    /// `requestId` paired with [`REQUEST_ID_MESSAGE_ID`].
    pub const REQUEST_ID: &str = "req_0001";
    /// Dedup key whose main-thread write must beat its sidechain write.
    pub const SIDECHAIN_MESSAGE_ID: &str = "msg_sidechain_pair";
    /// Dedup key that reports one-hour ephemeral cache writes.
    pub const EPHEMERAL_1H_MESSAGE_ID: &str = "msg_ephemeral_1h";
    /// One-hour ephemeral cache-write tokens on [`EPHEMERAL_1H_MESSAGE_ID`].
    pub const EPHEMERAL_1H_TOKENS: u64 = 4_096;
    /// Dedup key whose flat cache-creation total disagrees with its breakdown.
    pub const MISMATCH_MESSAGE_ID: &str = "msg_cache_mismatch";
    /// Dedup key of the `<synthetic>` API-error reply.
    pub const SYNTHETIC_MESSAGE_ID: &str = "msg_synthetic_error";

    fn assistant(message_id: &str, timestamp_ms: i64, body: &str, extra_top_level: &str) -> String {
        let seconds = timestamp_ms / 1_000;
        let millis = timestamp_ms % 1_000;
        let stamp = chrono::DateTime::from_timestamp(seconds, (millis as u32) * 1_000_000)
            .expect("fixture timestamps are in range")
            .format("%Y-%m-%dT%H:%M:%S%.3fZ");
        format!(
            r#"{{"type":"assistant","uuid":"uuid-{message_id}-{timestamp_ms}","timestamp":"{stamp}","sessionId":"sess-{message_id}","cwd":"/synthetic/project","gitBranch":"synthetic","version":"9.9.9"{extra_top_level},"message":{{"id":"{message_id}","role":"assistant","model":"claude-sonnet-4-5-20250929",{body}}}}}"#
        )
    }

    /// Writes the synthetic `projects` tree and returns its exact expectations.
    pub fn write(projects_dir: &Path) -> io::Result<Manifest> {
        let alpha = projects_dir.join("-synthetic-alpha");
        let beta = projects_dir.join("-synthetic-beta").join("nested");
        fs::create_dir_all(&alpha)?;
        fs::create_dir_all(&beta)?;

        let streaming = [
            (1_785_000_000_000_i64, 3_u64),
            (1_785_000_001_000, 3),
            (1_785_000_002_000, 3),
            (STREAMING_FINAL_MS, STREAMING_FINAL_OUTPUT),
        ]
        .into_iter()
        .map(|(timestamp, output)| {
            assistant(
                STREAMING_MESSAGE_ID,
                timestamp,
                &format!(
                    r#""usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":15555,"cache_creation_input_tokens":11862,"cache_creation":{{"ephemeral_5m_input_tokens":11862,"ephemeral_1h_input_tokens":0}},"speed":"standard"}}"#
                ),
                "",
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

        let mut alpha_lines = vec![streaming];
        alpha_lines.push(assistant(
            REQUEST_ID_MESSAGE_ID,
            1_785_000_010_000,
            r#""usage":{"input_tokens":100,"output_tokens":200,"cache_read_input_tokens":300,"cache_creation_input_tokens":400}"#,
            &format!(r#","requestId":"{REQUEST_ID}""#),
        ));
        alpha_lines.push(r#"{"type":"user","timestamp":"2026-07-14T08:00:00.000Z"}"#.to_owned());
        alpha_lines
            .push(r#"{"type":"attachment","timestamp":"2026-07-14T08:00:01.000Z"}"#.to_owned());
        alpha_lines.push("{not json at all".to_owned());
        alpha_lines.push(r#"{"type":"assistant","timestamp":"2026-07-14T08:00:02.000Z","message":{"id":"msg_no_usage","role":"assistant"}}"#.to_owned());
        alpha_lines.push(r#"{"type":"assistant","timestamp":"2026-07-14T08:00:03.000Z","message":{"id":"msg_bad_usage","role":"assistant","usage":"not-an-object"}}"#.to_owned());
        alpha_lines.push(r#"{"type":"assistant","timestamp":"2026-07-14T08:00:04.000Z","message":{"role":"assistant","usage":{"input_tokens":1}}}"#.to_owned());
        alpha_lines.push(r#"{"type":"assistant","timestamp":"not-a-timestamp","message":{"id":"msg_bad_time","role":"assistant","usage":{"input_tokens":1}}}"#.to_owned());
        fs::write(
            alpha.join("11111111-1111-1111-1111-111111111111.jsonl"),
            format!("{}\n", alpha_lines.join("\n")),
        )?;

        let beta_lines = [
            assistant(
                SIDECHAIN_MESSAGE_ID,
                1_785_000_020_000,
                r#""usage":{"input_tokens":7,"output_tokens":8,"cache_read_input_tokens":9,"cache_creation_input_tokens":10}"#,
                r#","isSidechain":true"#,
            ),
            assistant(
                SIDECHAIN_MESSAGE_ID,
                1_785_000_021_000,
                r#""usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":1,"cache_creation_input_tokens":1}"#,
                r#","isSidechain":false"#,
            ),
            assistant(
                EPHEMERAL_1H_MESSAGE_ID,
                1_785_000_030_000,
                &format!(
                    r#""usage":{{"input_tokens":5,"output_tokens":6,"cache_read_input_tokens":7,"cache_creation_input_tokens":{},"cache_creation":{{"ephemeral_5m_input_tokens":1024,"ephemeral_1h_input_tokens":{EPHEMERAL_1H_TOKENS}}}}}"#,
                    1024 + EPHEMERAL_1H_TOKENS
                ),
                "",
            ),
            assistant(
                MISMATCH_MESSAGE_ID,
                1_785_000_040_000,
                r#""usage":{"input_tokens":2,"output_tokens":3,"cache_read_input_tokens":4,"cache_creation_input_tokens":9999,"cache_creation":{"ephemeral_5m_input_tokens":50,"ephemeral_1h_input_tokens":0}}"#,
                "",
            ),
            r#"{"type":"assistant","uuid":"uuid-synth","timestamp":"2026-07-14T08:05:00.000Z","sessionId":"sess-synth","cwd":"/synthetic/project","isApiErrorMessage":true,"error":"model_not_found","message":{"id":"msg_synthetic_error","role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#.to_owned(),
        ];

        fs::write(
            beta.join("22222222-2222-2222-2222-222222222222.jsonl"),
            format!("{}\n\n", beta_lines.join("\n")),
        )?;
        fs::write(alpha.join("ignored.json"), "{}")?;

        Ok(Manifest {
            files: 2,
            eligible: 6,
            skipped: 11,
            duplicate_suppressed: 4,
            missing_request_id: 5,
            ephemeral_1h_rows: 1,
            ephemeral_1h_tokens: EPHEMERAL_1H_TOKENS,
            cache_creation_mismatch: 1,
            synthetic_model: 1,
            incomplete: 1,
            max_timestamp_ms: 1_785_000_040_000,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::json;

    use crate::archive::Archive;
    use crate::ingest::{read_source_cursor, IngestRound, OPENCODE_SOURCE};

    use super::*;

    fn request(watermark: Option<i64>) -> ScanRequest {
        ScanRequest {
            host_id: "host-claude-test".to_owned(),
            watermark,
            origin: Origin::Live,
            last_success_utc: Some(1_785_000_000_000),
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    fn synthetic_tree() -> (tempfile::TempDir, PathBuf, fixture::Manifest) {
        let temp = tempfile::tempdir().expect("create fixture parent");
        let projects = temp.path().join(PROJECTS_DIRECTORY);
        std::fs::create_dir_all(&projects).expect("create projects dir");
        let manifest = fixture::write(&projects).expect("write synthetic transcripts");
        (temp, projects, manifest)
    }

    fn scan_tree(
        projects: &Path,
        watermark: Option<i64>,
    ) -> (ScanResult, Vec<NormalizedUsageRecord>, Vec<usize>) {
        let mut records = Vec::new();
        let mut batch_sizes = Vec::new();
        let result = scan_projects_dir(projects, &request(watermark), |batch| {
            batch_sizes.push(batch.len());
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("scan synthetic tree");
        (result, records, batch_sizes)
    }

    fn find<'a>(
        records: &'a [NormalizedUsageRecord],
        message_id: &str,
    ) -> &'a NormalizedUsageRecord {
        records
            .iter()
            .find(|record| record.message_id == message_id)
            .unwrap_or_else(|| panic!("missing normalized record {message_id}"))
    }

    #[test]
    fn claude_code_synthetic_manifest_counts_tokens_and_diagnostics_are_exact() {
        let (_temp, projects, manifest) = synthetic_tree();
        let (result, records, batch_sizes) = scan_tree(&projects, None);

        assert!(result.reached_eof);
        assert_eq!(result.diagnostics.files_scanned, manifest.files);
        assert_eq!(result.diagnostics.files_unreadable, 0);
        assert_eq!(result.eligible_count, manifest.eligible);
        assert_eq!(records.len() as u64, manifest.eligible);
        assert_eq!(result.delivered_records, manifest.eligible);
        assert_eq!(result.skipped_count, manifest.skipped);
        assert_eq!(
            result.skipped_breakdown.duplicate_suppressed,
            manifest.duplicate_suppressed
        );
        assert_eq!(result.skipped_breakdown.malformed_json, 1);
        assert_eq!(result.skipped_breakdown.non_assistant, 2);
        assert_eq!(result.skipped_breakdown.missing_usage, 1);
        assert_eq!(result.skipped_breakdown.invalid_usage, 1);
        assert_eq!(result.skipped_breakdown.missing_message_id, 1);
        assert_eq!(result.skipped_breakdown.unparsable_timestamp, 1);
        assert_eq!(
            result.skipped_breakdown.total(),
            result.skipped_count,
            "breakdown must account for every skip"
        );
        assert_eq!(
            result.diagnostics.missing_request_id,
            manifest.missing_request_id
        );
        assert_eq!(
            result.diagnostics.ephemeral_1h_rows,
            manifest.ephemeral_1h_rows
        );
        assert_eq!(
            result.diagnostics.ephemeral_1h_tokens,
            manifest.ephemeral_1h_tokens
        );
        assert_eq!(
            result.diagnostics.cache_creation_mismatch,
            manifest.cache_creation_mismatch
        );
        assert_eq!(result.diagnostics.synthetic_model, manifest.synthetic_model);
        assert!(result.cost_estimate_is_lower_bound());
        assert_eq!(
            result.observed_max_time_updated,
            Some(manifest.max_timestamp_ms)
        );
        assert_eq!(batch_sizes, vec![manifest.eligible as usize]);
        assert_eq!(
            records.iter().filter(|record| record.is_incomplete).count() as u64,
            manifest.incomplete
        );
        assert!(records.iter().all(|record| record.cost.is_none()
            && record.cost_source == CostSource::Unavailable
            && record.source == CLAUDE_CODE_SOURCE
            && record.provider_id == PROVIDER_ID
            && record.host_id == "host-claude-test"
            && record.variant.is_none()
            && record.tok_reasoning == 0
            && record.time_completed_utc.is_none()));

        let streaming = find(&records, fixture::STREAMING_MESSAGE_ID);
        assert_eq!(
            streaming.tok_output,
            fixture::STREAMING_FINAL_OUTPUT,
            "the final streaming snapshot must win, not the first"
        );
        assert_eq!(streaming.source_time_updated, fixture::STREAMING_FINAL_MS);
        assert_eq!(streaming.time_created_utc, fixture::STREAMING_FINAL_MS);
        assert_eq!(streaming.tok_cache_write, 11_862);
        assert_eq!(streaming.project_dir, "/synthetic/project");
        assert_eq!(streaming.agent_raw, MAIN_AGENT);
        assert_eq!(streaming.agent_key, MAIN_AGENT);

        let with_request = find(
            &records,
            &format!("{}#{}", fixture::REQUEST_ID_MESSAGE_ID, fixture::REQUEST_ID),
        );
        assert_eq!(
            with_request.tok_cache_write, 400,
            "flat field is the fallback"
        );

        let sidechain = find(&records, fixture::SIDECHAIN_MESSAGE_ID);
        assert_eq!(
            sidechain.agent_raw, MAIN_AGENT,
            "a main-thread write outranks a sidechain write even with fewer tokens"
        );
        assert_eq!(sidechain.tok_input, 1);

        let ephemeral = find(&records, fixture::EPHEMERAL_1H_MESSAGE_ID);
        assert_eq!(
            ephemeral.tok_cache_write,
            1_024 + fixture::EPHEMERAL_1H_TOKENS
        );

        let mismatch = find(&records, fixture::MISMATCH_MESSAGE_ID);
        assert_eq!(
            mismatch.tok_cache_write, 50,
            "the breakdown wins over a disagreeing flat total, and is never added to it"
        );

        let synthetic = find(&records, fixture::SYNTHETIC_MESSAGE_ID);
        assert!(synthetic.is_incomplete);
        assert!(is_placeholder_model(&synthetic.model_id));

        println!(
            "synthetic_manifest files={} eligible={}={} skipped={}={} dup={} 1h_rows={} 1h_tokens={} mismatch={} synthetic={} max_ts={:?}",
            manifest.files,
            manifest.eligible,
            result.eligible_count,
            manifest.skipped,
            result.skipped_count,
            result.skipped_breakdown.duplicate_suppressed,
            result.diagnostics.ephemeral_1h_rows,
            result.diagnostics.ephemeral_1h_tokens,
            result.diagnostics.cache_creation_mismatch,
            result.diagnostics.synthetic_model,
            result.observed_max_time_updated
        );
    }

    struct ReorderedSource {
        inner: DirectoryTranscriptSource,
        reverse_files: bool,
        reverse_lines: bool,
    }

    impl TranscriptSource for ReorderedSource {
        fn transcripts(&self) -> std::result::Result<Vec<PathBuf>, StreamError> {
            let mut files = self.inner.transcripts()?;
            if self.reverse_files {
                files.reverse();
            }
            Ok(files)
        }

        fn read_lines(
            &self,
            path: &Path,
            visitor: &mut dyn FnMut(&str) -> std::result::Result<(), StreamError>,
        ) -> std::result::Result<(), StreamError> {
            let mut lines = Vec::new();
            self.inner.read_lines(path, &mut |line| {
                lines.push(line.to_owned());
                Ok(())
            })?;
            if self.reverse_lines {
                lines.reverse();
            }
            for line in &lines {
                visitor(line)?;
            }
            Ok(())
        }

        fn project_dir_hint(&self, path: &Path) -> String {
            self.inner.project_dir_hint(path)
        }
    }

    #[test]
    fn claude_code_dedup_winner_is_independent_of_file_and_line_order() {
        let (_temp, projects, manifest) = synthetic_tree();
        let inner = DirectoryTranscriptSource::new(&projects);
        let mut fingerprints = Vec::new();

        for (reverse_files, reverse_lines) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let source = ReorderedSource {
                inner: inner.clone(),
                reverse_files,
                reverse_lines,
            };
            let mut records = Vec::new();
            let result = scan_source(&source, &request(None), |batch| {
                records.extend_from_slice(batch);
                Ok(())
            })
            .expect("reordered scan");
            assert_eq!(result.eligible_count, manifest.eligible);
            assert_eq!(result.skipped_count, manifest.skipped);
            assert_eq!(
                result.observed_max_time_updated,
                Some(manifest.max_timestamp_ms)
            );
            let mut fingerprint = records
                .iter()
                .map(|record| {
                    (
                        record.message_id.clone(),
                        record.tok_input,
                        record.tok_output,
                        record.tok_cache_read,
                        record.tok_cache_write,
                        record.agent_raw.clone(),
                        record.source_time_updated,
                    )
                })
                .collect::<Vec<_>>();
            fingerprint.sort();
            fingerprints.push(fingerprint);
        }

        assert!(
            fingerprints.windows(2).all(|pair| pair[0] == pair[1]),
            "dedup selection must not depend on scan order"
        );
        println!(
            "order_independent permutations={} records_per_scan={}",
            fingerprints.len(),
            fingerprints[0].len()
        );
    }

    #[test]
    fn claude_code_overlap_window_filters_by_timestamp_and_reports_stable_watermark() {
        let (_temp, projects, manifest) = synthetic_tree();

        let base = request(None);
        assert_eq!(base.window_start(), i64::MIN);
        let windowed = request(Some(fixture::STREAMING_FINAL_MS));
        assert_eq!(
            windowed.window_start(),
            fixture::STREAMING_FINAL_MS - OVERLAP_WINDOW_MS
        );

        let (first, first_records, _) = scan_tree(&projects, Some(manifest.max_timestamp_ms));
        let (second, second_records, _) = scan_tree(&projects, Some(manifest.max_timestamp_ms));
        assert_eq!(first.eligible_count, second.eligible_count);
        assert_eq!(
            first_records
                .iter()
                .map(|record| &record.message_id)
                .collect::<BTreeSet<_>>(),
            second_records
                .iter()
                .map(|record| &record.message_id)
                .collect::<BTreeSet<_>>(),
            "the same watermark must produce the same record set"
        );

        let beyond = manifest.max_timestamp_ms + OVERLAP_WINDOW_MS + 1;
        let (narrow, narrow_records, narrow_batches) = scan_tree(&projects, Some(beyond));
        assert!(narrow.reached_eof);
        assert_eq!(narrow.eligible_count, 0);
        assert!(narrow_records.is_empty());
        assert!(narrow_batches.is_empty());
        assert_eq!(
            narrow.observed_max_time_updated, None,
            "an empty window observes no timestamp, so the watermark must not move"
        );
        assert_eq!(narrow.skipped_breakdown.duplicate_suppressed, 0);
        println!(
            "overlap_window full_eligible={} same_watermark_stable={} narrow_eligible={} narrow_max={:?}",
            manifest.eligible,
            first.eligible_count == second.eligible_count,
            narrow.eligible_count,
            narrow.observed_max_time_updated
        );
    }

    #[test]
    fn claude_code_cancelled_sink_reports_partial_without_watermark() {
        let (_temp, projects, _manifest) = synthetic_tree();
        let mut attempts = 0_u32;
        let mut small = request(None);
        small.batch_size = 2;
        let result = scan_projects_dir(&projects, &small, |_| {
            attempts += 1;
            Err(SinkError::new("injected cancellation"))
        })
        .expect("sink cancellation returns an interrupted result");

        assert_eq!(attempts, 1);
        assert!(!result.reached_eof);
        assert_eq!(result.delivered_records, 0);
        assert_eq!(result.observed_max_time_updated, None);
        assert_eq!(
            result.skip_reason,
            Some(ScanSkipReason::Interrupted(
                "injected cancellation".to_owned()
            ))
        );
        assert_eq!(result.last_success_utc, Some(1_785_000_000_000));
        println!(
            "cancelled_sink attempts={attempts} reached_eof={} observed_max={:?}",
            result.reached_eof, result.observed_max_time_updated
        );
    }

    #[test]
    fn claude_code_batches_split_at_batch_size_and_zero_is_rejected() {
        let (_temp, projects, manifest) = synthetic_tree();
        let mut small = request(None);
        small.batch_size = 2;
        let mut sizes = Vec::new();
        let result = scan_projects_dir(&projects, &small, |batch| {
            sizes.push(batch.len());
            Ok(())
        })
        .expect("batched scan");
        assert_eq!(sizes, vec![2, 2, 2]);
        assert_eq!(result.delivered_batches, 3);
        assert_eq!(result.delivered_records, manifest.eligible);

        let mut invalid = request(None);
        invalid.batch_size = 0;
        assert!(matches!(
            scan_projects_dir(&projects, &invalid, |_| Ok(())),
            Err(ClaudeCodeError::InvalidBatchSize)
        ));
    }

    struct FailingSource {
        transcripts: Vec<PathBuf>,
        unreadable: BTreeSet<PathBuf>,
        interrupt: Option<PathBuf>,
        enumerate_error: bool,
        lines: BTreeMap<PathBuf, Vec<String>>,
    }

    impl TranscriptSource for FailingSource {
        fn transcripts(&self) -> std::result::Result<Vec<PathBuf>, StreamError> {
            if self.enumerate_error {
                return Err(StreamError::Io {
                    path: PathBuf::from("/synthetic/projects"),
                    source: io::Error::from(io::ErrorKind::PermissionDenied),
                });
            }
            Ok(self.transcripts.clone())
        }

        fn read_lines(
            &self,
            path: &Path,
            visitor: &mut dyn FnMut(&str) -> std::result::Result<(), StreamError>,
        ) -> std::result::Result<(), StreamError> {
            if self.unreadable.contains(path) {
                return Err(StreamError::Io {
                    path: path.to_path_buf(),
                    source: io::Error::from(io::ErrorKind::PermissionDenied),
                });
            }
            if self.interrupt.as_deref() == Some(path) {
                return Err(StreamError::Interrupted(
                    "injected mid-file stop".to_owned(),
                ));
            }
            for line in self.lines.get(path).into_iter().flatten() {
                visitor(line)?;
            }
            Ok(())
        }
    }

    fn one_usage_line(message_id: &str, timestamp: &str) -> String {
        json!({
            "type": "assistant",
            "timestamp": timestamp,
            "sessionId": "sess",
            "cwd": "/synthetic",
            "message": {
                "id": message_id,
                "model": "claude-sonnet-4-5-20250929",
                "role": "assistant",
                "usage": {"input_tokens": 1, "output_tokens": 2}
            }
        })
        .to_string()
    }

    #[test]
    fn claude_code_unreadable_file_is_a_counted_coverage_hole_not_a_lost_round() {
        let readable = PathBuf::from("/synthetic/readable.jsonl");
        let denied = PathBuf::from("/synthetic/denied.jsonl");
        let source = FailingSource {
            transcripts: vec![denied.clone(), readable.clone()],
            unreadable: [denied].into_iter().collect(),
            interrupt: None,
            enumerate_error: false,
            lines: [(
                readable,
                vec![one_usage_line("msg_ok", "2026-07-14T08:00:00.000Z")],
            )]
            .into_iter()
            .collect(),
        };

        let mut records = Vec::new();
        let result = scan_source(&source, &request(None), |batch| {
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("one unreadable file must not abandon the round");

        assert!(result.reached_eof);
        assert_eq!(result.diagnostics.files_unreadable, 1);
        assert_eq!(result.diagnostics.files_scanned, 1);
        assert_eq!(result.eligible_count, 1);
        assert_eq!(records[0].message_id, "msg_ok");
        println!(
            "unreadable_file scanned={} unreadable={} eligible={} reached_eof={}",
            result.diagnostics.files_scanned,
            result.diagnostics.files_unreadable,
            result.eligible_count,
            result.reached_eof
        );
    }

    #[test]
    fn claude_code_enumeration_and_mid_file_failures_never_report_a_watermark() {
        let mut source = FailingSource {
            transcripts: vec![PathBuf::from("/synthetic/a.jsonl")],
            unreadable: BTreeSet::new(),
            interrupt: None,
            enumerate_error: true,
            lines: BTreeMap::new(),
        };
        match scan_source(&source, &request(None), |_| Ok(())) {
            Err(ClaudeCodeError::ScanFailed { partial, source }) => {
                assert_eq!(partial.observed_max_time_updated, None);
                assert!(!partial.reached_eof);
                assert_eq!(partial.last_success_utc, Some(1_785_000_000_000));
                assert!(source.to_string().contains("/synthetic/projects"));
            }
            other => panic!("expected a failed enumeration, got {other:?}"),
        }

        source.enumerate_error = false;
        source.interrupt = Some(PathBuf::from("/synthetic/a.jsonl"));
        match scan_source(&source, &request(None), |_| Ok(())) {
            Err(ClaudeCodeError::ScanFailed { partial, source }) => {
                assert_eq!(partial.observed_max_time_updated, None);
                assert!(
                    matches!(source, StreamError::Interrupted(ref message) if message == "injected mid-file stop")
                );
            }
            other => panic!("expected a mid-file interruption, got {other:?}"),
        }
    }

    #[test]
    fn claude_code_skip_reasons_and_dedup_key_shapes_are_exact() {
        let context = ParseContext::new("host-parse", Origin::Bak);
        let cases = [
            ("{not-json", SkipReason::MalformedJson),
            ("[]", SkipReason::MalformedJson),
            (r#"{"type":"user"}"#, SkipReason::NonAssistant),
            (
                r#"{"type":"assistant","message":{}}"#,
                SkipReason::MissingUsage,
            ),
            (
                r#"{"type":"assistant","message":{"usage":7}}"#,
                SkipReason::InvalidUsage,
            ),
            (
                r#"{"type":"assistant","message":{"id":"  ","usage":{}}}"#,
                SkipReason::MissingMessageId,
            ),
            (
                r#"{"type":"assistant","timestamp":true,"message":{"id":"m","usage":{}}}"#,
                SkipReason::UnparsableTimestamp,
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(
                parse_line(line, "hint", &context),
                Err(expected),
                "line {line}"
            );
            assert!(!expected.as_str().is_empty());
        }
        assert_eq!(
            SkipReason::DuplicateSuppressed.as_str(),
            "duplicate_suppressed"
        );

        let bare = parse_line(
            &one_usage_line("msg_bare", "2026-07-14T08:00:00.000Z"),
            "hint",
            &context,
        )
        .expect("eligible");
        assert_eq!(bare.key.request_id, None);
        assert_eq!(bare.record.message_id, "msg_bare");
        assert_eq!(bare.record.origin, Origin::Bak);
        assert_eq!(bare.record.origin_priority, Origin::Bak.priority());

        let paired = parse_line(
            &json!({
                "type": "assistant",
                "timestamp": "2026-07-14T08:00:00.000Z",
                "requestId": "  req_9  ",
                "isSidechain": true,
                "message": {"id": "msg_paired", "usage": {"output_tokens": 5, "speed": "fast"}}
            })
            .to_string(),
            "-fallback-project",
            &context,
        )
        .expect("eligible");
        assert_eq!(paired.key.request_id.as_deref(), Some("req_9"));
        assert_eq!(paired.record.message_id, "msg_paired#req_9");
        assert_eq!(paired.record.agent_raw, SIDECHAIN_AGENT);
        assert_eq!(paired.record.agent_key, SIDECHAIN_AGENT);
        assert!(paired.has_speed);
        assert!(paired.is_sidechain);
        assert_eq!(
            paired.record.project_dir, "-fallback-project",
            "a line without cwd falls back to the encoded directory name"
        );
        assert_eq!(paired.record.model_id, "unknown");
        assert!(!paired.record.is_incomplete);
    }

    #[test]
    fn claude_code_conflict_priority_is_total_and_antisymmetric() {
        let context = ParseContext::new("host-rank", Origin::Live);
        let build = |sidechain: bool, output: u64, speed: bool| {
            let mut usage = json!({"output_tokens": output});
            if speed {
                usage["speed"] = json!("standard");
            }
            parse_line(
                &json!({
                    "type": "assistant",
                    "timestamp": "2026-07-14T08:00:00.000Z",
                    "isSidechain": sidechain,
                    "message": {"id": "msg_rank", "usage": usage}
                })
                .to_string(),
                "hint",
                &context,
            )
            .expect("eligible")
        };

        let main_small = build(false, 1, false);
        let side_large = build(true, 900, true);
        assert!(main_small.outranks(&side_large));
        assert!(!side_large.outranks(&main_small));

        let main_large = build(false, 900, false);
        assert!(main_large.outranks(&main_small));
        assert!(!main_small.outranks(&main_large));

        let main_speed = build(false, 1, true);
        assert!(main_speed.outranks(&main_small));
        assert!(!main_small.outranks(&main_speed));

        let twin = build(false, 1, false);
        assert!(
            !twin.outranks(&main_small) && !main_small.outranks(&twin),
            "identical candidates must tie so the incumbent is retained"
        );
    }

    #[test]
    fn claude_code_numeric_and_timestamp_coercion_is_lossy_but_never_negative() {
        assert_eq!(lossy_u64(None), 0);
        assert_eq!(lossy_u64(Some(&json!(-5))), 0);
        assert_eq!(lossy_u64(Some(&json!(4.9))), 4);
        assert_eq!(lossy_u64(Some(&json!(-4.9))), 0);
        assert_eq!(lossy_u64(Some(&json!("7.9"))), 7);
        assert_eq!(lossy_u64(Some(&json!("NaN"))), 0);
        assert_eq!(lossy_u64(Some(&json!(true))), 0);
        assert_eq!(lossy_u64(Some(&json!(u64::MAX))), u64::MAX);

        assert_eq!(
            parse_timestamp_ms(&json!("2026-07-14T08:40:04.721Z")),
            Some(1_784_018_404_721)
        );
        assert_eq!(
            parse_timestamp_ms(&json!("  2026-07-14T08:40:04.721Z  ")),
            Some(1_784_018_404_721)
        );
        assert_eq!(
            parse_timestamp_ms(&json!(1_785_000_000_000_i64)),
            Some(1_785_000_000_000)
        );
        assert_eq!(parse_timestamp_ms(&json!(-1)), Some(-1));
        assert_eq!(parse_timestamp_ms(&json!("nope")), None);
        assert_eq!(parse_timestamp_ms(&json!(null)), None);

        assert_eq!(value_to_string(&json!(12)).as_deref(), Some("12"));
        assert_eq!(value_to_string(&json!(null)), None);
        assert!(is_placeholder_model("<synthetic>"));
        assert!(!is_placeholder_model("claude-sonnet-5"));
        assert!(!is_placeholder_model("<unterminated"));
    }

    #[test]
    fn claude_code_discovery_follows_documented_precedence_and_reports_probed_paths() {
        let temp = tempfile::tempdir().expect("temp");
        let explicit = temp.path().join("explicit");
        let config = temp.path().join("config-dir");
        let xdg = temp.path().join("xdg");
        let home = temp.path().join("home");
        for root in [&explicit, &config] {
            std::fs::create_dir_all(root.join(PROJECTS_DIRECTORY)).expect("mkdir");
        }
        std::fs::create_dir_all(xdg.join("claude").join(PROJECTS_DIRECTORY)).expect("mkdir");
        std::fs::create_dir_all(home.join(".claude").join(PROJECTS_DIRECTORY)).expect("mkdir");

        assert_eq!(
            discover_projects_dir_from(Some(&explicit), Some(&config), Some(&xdg), Some(&home))
                .expect("explicit wins"),
            explicit.join(PROJECTS_DIRECTORY)
        );
        assert_eq!(
            discover_projects_dir_from(None, Some(&config), Some(&xdg), Some(&home))
                .expect("config dir wins"),
            config.join(PROJECTS_DIRECTORY)
        );
        assert_eq!(
            discover_projects_dir_from(None, None, Some(&xdg), Some(&home)).expect("xdg wins"),
            xdg.join("claude").join(PROJECTS_DIRECTORY)
        );
        assert_eq!(
            discover_projects_dir_from(None, None, None, Some(&home)).expect("home fallback"),
            home.join(".claude").join(PROJECTS_DIRECTORY)
        );

        let empty = temp.path().join("nothing");
        let error = discover_projects_dir_from(Some(&empty), None, None, None)
            .expect_err("an absent tree must name every probed path");
        let message = error.to_string();
        assert!(message.contains("nothing"), "{message}");
        assert!(matches!(
            error,
            ClaudeCodeError::ProjectsNotFound { ref probed_paths } if probed_paths.len() == 1
        ));

        assert!(matches!(
            scan_projects_dir(&empty, &request(None), |_| Ok(())),
            Err(ClaudeCodeError::ProjectsNotFound { .. })
        ));
        assert!(DirectoryTranscriptSource::new(&empty)
            .transcripts()
            .is_err());
        assert_eq!(
            DirectoryTranscriptSource::new(&explicit).projects_dir(),
            explicit.as_path()
        );
    }

    #[test]
    fn claude_code_directory_source_walks_nested_trees_and_skips_non_jsonl() {
        let (_temp, projects, manifest) = synthetic_tree();
        let source = DirectoryTranscriptSource::new(&projects);
        let transcripts = source.transcripts().expect("walk");
        assert_eq!(transcripts.len() as u64, manifest.files);
        assert!(transcripts
            .iter()
            .all(|path| path.extension().is_some_and(|value| value == "jsonl")));
        assert!(transcripts.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(
            source.project_dir_hint(&transcripts[0]),
            "-synthetic-alpha",
            "the hint is the raw encoded directory name, which is not reversible"
        );

        let counted = RefCell::new(0_usize);
        source
            .read_lines(&transcripts[0], &mut |_| {
                *counted.borrow_mut() += 1;
                Ok(())
            })
            .expect("read");
        assert!(*counted.borrow() > 0);
        assert!(source
            .read_lines(&projects.join("missing.jsonl"), &mut |_| Ok(()))
            .is_err());
    }

    #[test]
    fn claude_code_records_ingest_idempotently_and_keep_a_cursor_apart_from_opencode() {
        let (_temp, projects, manifest) = synthetic_tree();
        let archive_dir = tempfile::tempdir().expect("archive dir");
        let mut archive = Archive::open_in_data_dir(archive_dir.path()).expect("open archive");

        let ingest_round = |archive: &mut Archive| {
            let mut round = IngestRound::begin_for_source(
                archive.connection_mut(),
                "host-claude-test",
                CLAUDE_CODE_SOURCE,
                Origin::Live,
            )
            .expect("begin round");
            assert_eq!(round.source(), CLAUDE_CODE_SOURCE);
            let mut batches = Vec::new();
            let result = scan_projects_dir(&projects, &request(None), |batch| {
                batches.push(batch.to_vec());
                Ok(())
            })
            .expect("scan");
            for batch in &batches {
                round.ingest_batch(batch).expect("ingest batch");
            }
            round
                .finish_with(result.reached_eof, result.observed_max_time_updated)
                .expect("finish round")
        };

        let first = ingest_round(&mut archive);
        assert!(first.committed);
        assert_eq!(first.received_records, manifest.eligible);
        assert_eq!(first.changed_records, manifest.eligible);
        assert_eq!(first.cursor_time_updated, Some(manifest.max_timestamp_ms));

        let second = ingest_round(&mut archive);
        assert!(second.committed);
        assert_eq!(second.received_records, manifest.eligible);
        assert_eq!(
            second.cursor_time_updated,
            Some(manifest.max_timestamp_ms),
            "a repeated round must not move the watermark backwards"
        );

        let rows: i64 = archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE source = ?1",
                [CLAUDE_CODE_SOURCE],
                |row| row.get(0),
            )
            .expect("count rows");
        assert_eq!(
            rows as u64, manifest.eligible,
            "upsert must not create duplicate rows on re-collection"
        );
        let distinct_costs: i64 = archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE source = ?1 AND (cost IS NOT NULL OR cost_source <> 'unavailable')",
                [CLAUDE_CODE_SOURCE],
                |row| row.get(0),
            )
            .expect("count costs");
        assert_eq!(
            distinct_costs, 0,
            "cost must stay absent, never a synthetic zero"
        );

        assert_eq!(
            read_source_cursor(archive.connection(), "host-claude-test", CLAUDE_CODE_SOURCE)
                .expect("read cursor"),
            Some(manifest.max_timestamp_ms)
        );
        assert_eq!(
            read_source_cursor(archive.connection(), "host-claude-test", OPENCODE_SOURCE)
                .expect("read cursor"),
            None,
            "one adapter's watermark must never be visible as another's"
        );

        let mut rejected = IngestRound::begin_for_source(
            archive.connection_mut(),
            "host-claude-test",
            CLAUDE_CODE_SOURCE,
            Origin::Live,
        )
        .expect("begin round");
        let mut foreign = scan_first_record(&projects);
        foreign.source = OPENCODE_SOURCE.to_owned();
        let error = rejected
            .ingest_batch(&[foreign])
            .expect_err("a foreign source must be rejected, not silently relabelled");
        assert!(error.to_string().contains(CLAUDE_CODE_SOURCE), "{error}");

        println!(
            "ingest_round rows={rows} first_changed={} second_changed={} cursor={:?} opencode_cursor=None",
            first.changed_records, second.changed_records, second.cursor_time_updated
        );
    }

    fn scan_first_record(projects: &Path) -> NormalizedUsageRecord {
        let mut records = Vec::new();
        scan_projects_dir(projects, &request(None), |batch| {
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("scan");
        records.remove(0)
    }

    #[test]
    #[ignore = "requires this machine's real ~/.claude/projects tree"]
    fn claude_code_real_local_transcripts_report_observable_totals() {
        let projects = dirs::home_dir()
            .expect("home directory")
            .join(".claude")
            .join(PROJECTS_DIRECTORY);
        if !projects.is_dir() {
            println!("real_data skipped: {} is absent", projects.display());
            return;
        }
        let mut records = Vec::new();
        let result = scan_projects_dir(&projects, &request(None), |batch| {
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("scan real transcripts");

        let sessions = records
            .iter()
            .map(|record| record.session_id.as_str())
            .collect::<BTreeSet<_>>();
        let models = records
            .iter()
            .map(|record| record.model_id.as_str())
            .collect::<BTreeSet<_>>();
        let total_input: u64 = records
            .iter()
            .map(|record| record.tok_input + record.tok_cache_read + record.tok_cache_write)
            .sum();
        let total_output: u64 = records.iter().map(|record| record.tok_output).sum();

        assert!(result.reached_eof);
        assert!(records.iter().all(|record| record.cost.is_none()));
        println!(
            "real_data files={} unreadable={} eligible={} sessions={} models={models:?} total_input={total_input} total_output={total_output} skipped={} dup={} missing_request_id={} 1h_rows={} synthetic={} lower_bound={}",
            result.diagnostics.files_scanned,
            result.diagnostics.files_unreadable,
            result.eligible_count,
            sessions.len(),
            result.skipped_count,
            result.skipped_breakdown.duplicate_suppressed,
            result.diagnostics.missing_request_id,
            result.diagnostics.ephemeral_1h_rows,
            result.diagnostics.synthetic_model,
            result.cost_estimate_is_lower_bound()
        );
    }
}
