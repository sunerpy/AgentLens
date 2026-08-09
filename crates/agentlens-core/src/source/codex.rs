//! Codex rollout（JSONL）解析与增量扫描。
//!
//! 数据面是 `<data_dir>/sessions/**/*.jsonl` 与
//! `<data_dir>/archived_sessions/**/*.jsonl`。同一相对路径优先使用 `sessions`，归档树只补齐
//! 已经从活动树消失的会话。契约见 `docs/adapters/codex.md`。
//!
//! 本轮有意不引入 zstd 依赖：发现 `.jsonl.zst` 时整文件跳过，并分别计入
//! `skipped_count` 与 `unsupported_compression`。这与契约要求的“解压失败不得部分入库”语义
//! 一致；后续只有在依赖获批后才增加流式解压路径。

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::archive::{normalize_agent_key, CostSource, NormalizedUsageRecord, Origin};

/// Codex 记录写入归档时使用的 source 键。
pub const CODEX_SOURCE: &str = "codex";
/// 活动 rollout 树目录名。
pub const SESSIONS_DIRECTORY: &str = "sessions";
/// 归档 rollout 树目录名。
pub const ARCHIVED_SESSIONS_DIRECTORY: &str = "archived_sessions";
/// 每次水位线扫描向前重叠 24 小时。
pub const OVERLAP_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
/// 默认交付批大小。
pub const DEFAULT_BATCH_SIZE: usize = 1_000;
/// rollout 未提供 provider 时的稳定回退值。
pub const DEFAULT_PROVIDER_ID: &str = "codex";

/// Codex 发现与扫描操作的结果类型。
pub type Result<T> = std::result::Result<T, CodexError>;

/// 阻止只读 Codex 扫描启动的错误。
#[derive(Debug, Error)]
pub enum CodexError {
    /// 没有候选数据根目录。
    #[error("未找到 Codex rollout 数据目录；已检查：{}", display_paths(.probed_paths))]
    DataDirectoryNotFound {
        /// 按优先级检查过的路径。
        probed_paths: Vec<PathBuf>,
    },
    /// rollout 树无法枚举。
    #[error("无法枚举 Codex rollout 目录 {path}：{source}")]
    Enumerate {
        /// 枚举失败的目录。
        path: PathBuf,
        /// 原始文件系统错误。
        source: io::Error,
    },
    /// 调用方给出了不可能的批大小。
    #[error("Codex 扫描 batch_size 必须大于 0")]
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

/// 一个 rollout 文件的稳定解析上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseContext {
    /// 归一化记录使用的主机标识。
    pub host_id: String,
    /// 当前文件来自活动树还是归档树。
    pub origin: Origin,
    /// 相对 `sessions` 根的逻辑路径。
    pub relative_path: String,
}

impl ParseContext {
    /// 创建文件级解析上下文。
    pub fn new(
        host_id: impl Into<String>,
        origin: Origin,
        relative_path: impl Into<String>,
    ) -> Self {
        Self {
            host_id: host_id.into(),
            origin,
            relative_path: relative_path.into(),
        }
    }
}

/// 一行或一个文件没有产生归一化记录的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// JSON 语法损坏；为避免部分入库，整个文件丢弃。
    MalformedJson,
    /// 合法事件不是 token_count 用量事件。
    NonUsageEvent,
    /// token_count 缺少 total_token_usage。
    MissingTotalUsage,
    /// total_token_usage 不是对象。
    InvalidTotalUsage,
    /// token_count 时间戳不可解析。
    UnparsableTimestamp,
    /// 累计计数器回退，逐桶负差值已归零。
    NegativeDelta,
    /// 归档副本被同相对路径的活动副本遮蔽。
    ShadowedArchive,
    /// 当前范围不支持 zstd，压缩文件整体跳过。
    UnsupportedCompression,
    /// 文件无法完整读取。
    UnreadableFile,
}

impl SkipReason {
    /// 返回诊断和报告使用的稳定 snake_case 名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MalformedJson => "malformed_json",
            Self::NonUsageEvent => "non_usage_event",
            Self::MissingTotalUsage => "missing_total_usage",
            Self::InvalidTotalUsage => "invalid_total_usage",
            Self::UnparsableTimestamp => "unparsable_timestamp",
            Self::NegativeDelta => "negative_delta",
            Self::ShadowedArchive => "shadowed_archive",
            Self::UnsupportedCompression => "unsupported_compression",
            Self::UnreadableFile => "unreadable_file",
        }
    }
}

/// 各类跳过原因计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkippedBreakdown {
    /// 包含非法 JSON 的整文件数。
    pub malformed_json: u64,
    /// 不携带用量的合法事件行数。
    pub non_usage_event: u64,
    /// 缺少累计用量的 token_count 行数。
    pub missing_total_usage: u64,
    /// 累计用量不是对象的行数。
    pub invalid_total_usage: u64,
    /// 时间戳不可解析的 token_count 行数。
    pub unparsable_timestamp: u64,
    /// 累计计数器发生回退的行数。
    pub negative_delta: u64,
    /// 被活动树遮蔽的归档文件数。
    pub shadowed_archive: u64,
    /// 本轮未解压的 `.jsonl.zst` 文件数。
    pub unsupported_compression: u64,
    /// 无法完整读取的文件数。
    pub unreadable_file: u64,
}

impl SkippedBreakdown {
    fn increment(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::MalformedJson => self.malformed_json += 1,
            SkipReason::NonUsageEvent => self.non_usage_event += 1,
            SkipReason::MissingTotalUsage => self.missing_total_usage += 1,
            SkipReason::InvalidTotalUsage => self.invalid_total_usage += 1,
            SkipReason::UnparsableTimestamp => self.unparsable_timestamp += 1,
            SkipReason::NegativeDelta => self.negative_delta += 1,
            SkipReason::ShadowedArchive => self.shadowed_archive += 1,
            SkipReason::UnsupportedCompression => self.unsupported_compression += 1,
            SkipReason::UnreadableFile => self.unreadable_file += 1,
        }
    }

    fn merge(&mut self, other: Self) {
        self.malformed_json += other.malformed_json;
        self.non_usage_event += other.non_usage_event;
        self.missing_total_usage += other.missing_total_usage;
        self.invalid_total_usage += other.invalid_total_usage;
        self.unparsable_timestamp += other.unparsable_timestamp;
        self.negative_delta += other.negative_delta;
        self.shadowed_archive += other.shadowed_archive;
        self.unsupported_compression += other.unsupported_compression;
        self.unreadable_file += other.unreadable_file;
    }

    /// 返回所有跳过类别之和。
    pub const fn total(&self) -> u64 {
        self.malformed_json
            + self.non_usage_event
            + self.missing_total_usage
            + self.invalid_total_usage
            + self.unparsable_timestamp
            + self.negative_delta
            + self.shadowed_archive
            + self.unsupported_compression
            + self.unreadable_file
    }
}

/// 文件级扫描诊断。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanDiagnostics {
    /// 活动树与归档树发现的 rollout 文件数，包括被遮蔽副本。
    pub files_discovered: u64,
    /// 成功完整解析的 JSONL 文件数。
    pub files_scanned: u64,
    /// 无法完整读取的文件数。
    pub files_unreadable: u64,
    /// 因非法 JSON 整体丢弃的文件数。
    pub files_malformed: u64,
    /// 因本轮不支持 zstd 而整体跳过的文件数。
    pub files_unsupported_compression: u64,
}

/// 一次重叠窗口扫描的不可变输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRequest {
    /// 写入每条记录的稳定主机标识。
    pub host_id: String,
    /// 上次成功提交的游标；首次全扫为 `None`。
    pub watermark: Option<i64>,
    /// 保留与其他适配器一致的调用形状；文件树本身决定 live/bak origin。
    pub origin: Origin,
    /// 本轮跳过时保留的上次成功时间。
    pub last_success_utc: Option<i64>,
    /// Rust 侧交付批大小。
    pub batch_size: usize,
}

impl ScanRequest {
    /// 创建默认批大小的正常活动源请求。
    pub fn live(host_id: impl Into<String>, watermark: Option<i64>) -> Self {
        Self {
            host_id: host_id.into(),
            watermark,
            origin: Origin::Live,
            last_success_utc: None,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    /// 应用 24 小时重叠后的含端点下界。
    pub fn window_start(&self) -> i64 {
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

/// 一轮 Codex 扫描的可观测结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// 已交给 sink 的记录数。
    pub delivered_records: u64,
    /// sink 调用次数。
    pub delivered_batches: u64,
    /// 落在窗口内并产生记录的 token_count 行数。
    pub eligible_count: u64,
    /// 所有跳过与计数器异常的数量。
    pub skipped_count: u64,
    /// 按稳定原因拆分的跳过计数。
    pub skipped_breakdown: SkippedBreakdown,
    /// 文件级诊断。
    pub diagnostics: ScanDiagnostics,
    /// 完整扫描时观察到的最大用量时间戳。
    pub observed_max_time_updated: Option<i64>,
    /// 只有全部候选文件处理完且 sink 未中断时才为 true。
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
            diagnostics: ScanDiagnostics::default(),
            observed_max_time_updated: None,
            reached_eof: false,
            last_success_utc,
            skip_reason: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TokenUsage {
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl TokenUsage {
    fn from_value(value: &Value) -> Option<Self> {
        value.is_object().then(|| Self {
            input: lossy_u64(value.get("input_tokens")),
            cache_read: lossy_u64(value.get("cached_input_tokens")),
            cache_write: lossy_u64(value.get("cache_write_input_tokens")),
            output: lossy_u64(value.get("output_tokens")),
            reasoning: lossy_u64(value.get("reasoning_output_tokens")),
            total: lossy_u64(value.get("total_tokens")),
        })
    }

    const fn is_zero(self) -> bool {
        self.input == 0
            && self.cache_read == 0
            && self.cache_write == 0
            && self.output == 0
            && self.reasoning == 0
            && self.total == 0
    }

    const fn has_advanced_from(self, previous: Self) -> bool {
        !self.has_regressed_from(previous)
            && (self.input > previous.input
                || self.cache_read > previous.cache_read
                || self.cache_write > previous.cache_write
                || self.output > previous.output
                || self.reasoning > previous.reasoning
                || self.total > previous.total)
    }

    const fn has_regressed_from(self, previous: Self) -> bool {
        self.input < previous.input
            || self.cache_read < previous.cache_read
            || self.cache_write < previous.cache_write
            || self.output < previous.output
            || self.reasoning < previous.reasoning
            || self.total < previous.total
    }

    const fn saturating_difference(self, previous: Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            cache_read: self.cache_read.saturating_sub(previous.cache_read),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            output: self.output.saturating_sub(previous.output),
            reasoning: self.reasoning.saturating_sub(previous.reasoning),
            total: self.total.saturating_sub(previous.total),
        }
    }
}

#[derive(Clone, Debug)]
struct RolloutFile {
    path: PathBuf,
    relative_path: String,
    origin: Origin,
    compressed: bool,
}

#[derive(Debug)]
enum FileFailure {
    Io,
    Malformed,
}

struct ParsedRollout {
    records: Vec<NormalizedUsageRecord>,
    skipped: SkippedBreakdown,
}

/// 扫描自动发现的 Codex 数据根目录。
pub fn scan_default<F>(request: &ScanRequest, sink: F) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    let data_dir = discover_data_dir()?;
    scan_data_dir(data_dir, request, sink)
}

/// 扫描显式 Codex 数据根目录。
pub fn scan_data_dir<F>(
    data_dir: impl AsRef<Path>,
    request: &ScanRequest,
    mut sink: F,
) -> Result<ScanResult>
where
    F: FnMut(&[NormalizedUsageRecord]) -> std::result::Result<(), SinkError>,
{
    if request.batch_size == 0 {
        return Err(CodexError::InvalidBatchSize);
    }
    let data_dir = data_dir.as_ref();
    if !is_data_dir(data_dir) {
        return Err(CodexError::DataDirectoryNotFound {
            probed_paths: vec![data_dir.to_path_buf()],
        });
    }

    let (files, discovered, shadowed) = enumerate_rollouts(data_dir)?;
    let mut result = ScanResult::empty(request.last_success_utc);
    result.diagnostics.files_discovered = discovered;
    result.skipped_breakdown.shadowed_archive = shadowed;
    let mut records = Vec::new();
    let mut observed_max = None::<i64>;

    for file in files.values() {
        if file.compressed {
            result
                .skipped_breakdown
                .increment(SkipReason::UnsupportedCompression);
            result.diagnostics.files_unsupported_compression += 1;
            continue;
        }
        let context = ParseContext::new(&request.host_id, file.origin, &file.relative_path);
        match parse_rollout(file, &context, request.window_start()) {
            Ok(parsed) => {
                result.diagnostics.files_scanned += 1;
                result.skipped_breakdown.merge(parsed.skipped);
                for record in parsed.records {
                    observed_max =
                        Some(observed_max.map_or(record.source_time_updated, |current| {
                            current.max(record.source_time_updated)
                        }));
                    records.push(record);
                }
            }
            Err(FileFailure::Io) => {
                result
                    .skipped_breakdown
                    .increment(SkipReason::UnreadableFile);
                result.diagnostics.files_unreadable += 1;
            }
            Err(FileFailure::Malformed) => {
                result
                    .skipped_breakdown
                    .increment(SkipReason::MalformedJson);
                result.diagnostics.files_malformed += 1;
            }
        }
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

    result.observed_max_time_updated = observed_max;
    result.reached_eof = true;
    Ok(result)
}

/// 按环境优先级发现 Codex 数据根目录。
pub fn discover_data_dir() -> Result<PathBuf> {
    let explicit = env::var_os("CODEX_HOME").map(PathBuf::from);
    let home = dirs::home_dir();
    discover_data_dir_from(explicit.as_deref(), home.as_deref())
}

fn discover_data_dir_from(explicit: Option<&Path>, home: Option<&Path>) -> Result<PathBuf> {
    let mut probed_paths = Vec::new();
    if let Some(path) = explicit {
        probed_paths.push(path.to_path_buf());
    }
    if let Some(path) = home {
        probed_paths.push(path.join(".codex"));
    }
    if let Some(path) = probed_paths.iter().find(|path| is_data_dir(path)) {
        return Ok(path.clone());
    }
    Err(CodexError::DataDirectoryNotFound { probed_paths })
}

fn is_data_dir(path: &Path) -> bool {
    path.is_dir()
        && (path.join(SESSIONS_DIRECTORY).is_dir()
            || path.join(ARCHIVED_SESSIONS_DIRECTORY).is_dir())
}

fn enumerate_rollouts(data_dir: &Path) -> Result<(BTreeMap<String, RolloutFile>, u64, u64)> {
    let mut files = BTreeMap::<String, RolloutFile>::new();
    let mut discovered = 0_u64;
    let mut shadowed = 0_u64;

    let archived_root = data_dir.join(ARCHIVED_SESSIONS_DIRECTORY);
    if archived_root.is_dir() {
        for file in collect_rollouts(&archived_root, Origin::Bak)? {
            discovered += 1;
            files.insert(file.relative_path.clone(), file);
        }
    }

    let sessions_root = data_dir.join(SESSIONS_DIRECTORY);
    if sessions_root.is_dir() {
        for file in collect_rollouts(&sessions_root, Origin::Live)? {
            discovered += 1;
            if files.insert(file.relative_path.clone(), file).is_some() {
                shadowed += 1;
            }
        }
    }
    Ok((files, discovered, shadowed))
}

fn collect_rollouts(root: &Path, origin: Origin) -> Result<Vec<RolloutFile>> {
    let mut files = Vec::new();
    collect_rollouts_recursive(root, root, origin, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_rollouts_recursive(
    root: &Path,
    directory: &Path,
    origin: Origin,
    files: &mut Vec<RolloutFile>,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|source| CodexError::Enumerate {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CodexError::Enumerate {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| CodexError::Enumerate {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_rollouts_recursive(root, &path, origin, files)?;
            continue;
        }
        let Some((compressed, relative_path)) = rollout_relative_path(root, &path) else {
            continue;
        };
        files.push(RolloutFile {
            path,
            relative_path,
            origin,
            compressed,
        });
    }
    Ok(())
}

fn rollout_relative_path(root: &Path, path: &Path) -> Option<(bool, String)> {
    let relative = path.strip_prefix(root).ok()?;
    let mut key = path_components(relative);
    let compressed = if key.ends_with(".jsonl.zst") {
        key.truncate(key.len() - ".zst".len());
        true
    } else if key.ends_with(".jsonl") {
        false
    } else {
        return None;
    };
    Some((compressed, key))
}

fn path_components(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn parse_rollout(
    file: &RolloutFile,
    context: &ParseContext,
    window_start: i64,
) -> std::result::Result<ParsedRollout, FileFailure> {
    let source = File::open(&file.path).map_err(|_| FileFailure::Io)?;
    let reader = BufReader::new(source);
    let fallback_session_id = session_id_from_rollout_path(&file.relative_path);
    let mut session_id = fallback_session_id;
    let mut model_provider = DEFAULT_PROVIDER_ID.to_owned();
    let mut model = "unknown".to_owned();
    let mut project_dir = String::new();
    let mut previous_total = TokenUsage::default();
    let mut records = Vec::new();
    let mut skipped = SkippedBreakdown::default();

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|_| FileFailure::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(&line).map_err(|_| FileFailure::Malformed)?;
        if !event.is_object() {
            return Err(FileFailure::Malformed);
        }
        match event.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                update_session_state(&event, &mut session_id, &mut model_provider);
                skipped.increment(SkipReason::NonUsageEvent);
            }
            Some("turn_context") => {
                update_turn_state(&event, &mut model, &mut project_dir);
                skipped.increment(SkipReason::NonUsageEvent);
            }
            Some("event_msg")
                if event.pointer("/payload/type").and_then(Value::as_str)
                    == Some("token_count") =>
            {
                let Some(total_value) = event.pointer("/payload/info/total_token_usage") else {
                    skipped.increment(SkipReason::MissingTotalUsage);
                    continue;
                };
                let Some(total) = TokenUsage::from_value(total_value) else {
                    skipped.increment(SkipReason::InvalidTotalUsage);
                    continue;
                };
                let timestamp = event.get("timestamp").and_then(parse_timestamp_ms);
                let Some(timestamp) = timestamp else {
                    skipped.increment(SkipReason::UnparsableTimestamp);
                    previous_total = total;
                    continue;
                };
                let last = event
                    .pointer("/payload/info/last_token_usage")
                    .and_then(TokenUsage::from_value);
                let regressed = total.has_regressed_from(previous_total);
                let delta = if total.has_advanced_from(previous_total)
                    && last.is_some_and(|usage| !usage.is_zero())
                {
                    last.unwrap_or_default()
                } else {
                    total.saturating_difference(previous_total)
                };
                previous_total = total;
                if regressed {
                    skipped.increment(SkipReason::NegativeDelta);
                }
                if timestamp < window_start {
                    continue;
                }
                let ordinal = event
                    .get("ordinal")
                    .and_then(value_to_string)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| (index + 1).to_string());
                let (provider_id, model_id) = normalize_provider_and_model(&model_provider, &model);
                let agent_raw = "";
                records.push(NormalizedUsageRecord {
                    host_id: context.host_id.clone(),
                    source: CODEX_SOURCE.to_owned(),
                    granularity: crate::archive::UsageGranularity::Message,
                    message_id: format!("{}#{ordinal}", context.relative_path),
                    session_id: session_id.clone(),
                    time_created_utc: timestamp,
                    time_completed_utc: None,
                    source_time_updated: timestamp,
                    origin: context.origin,
                    origin_priority: context.origin.priority(),
                    agent_raw: agent_raw.to_owned(),
                    agent_key: normalize_agent_key(agent_raw),
                    provider_id,
                    model_id,
                    variant: None,
                    tok_input: delta.input,
                    tok_output: delta.output,
                    tok_reasoning: delta.reasoning,
                    tok_cache_read: delta.cache_read,
                    tok_cache_write: delta.cache_write,
                    cost: None,
                    cost_source: CostSource::Unavailable,
                    is_incomplete: delta.input == 0
                        && delta.output == 0
                        && delta.cache_read == 0
                        && delta.cache_write == 0,
                    project_dir: project_dir.clone(),
                });
            }
            _ => skipped.increment(SkipReason::NonUsageEvent),
        }
    }

    Ok(ParsedRollout { records, skipped })
}

fn update_session_state(event: &Value, session_id: &mut String, model_provider: &mut String) {
    if let Some(value) = event
        .pointer("/payload/id")
        .and_then(value_to_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        *session_id = value;
    }
    if let Some(value) = event
        .pointer("/payload/model_provider")
        .and_then(value_to_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        *model_provider = value;
    }
}

fn update_turn_state(event: &Value, model: &mut String, project_dir: &mut String) {
    if let Some(value) = event
        .pointer("/payload/model")
        .and_then(value_to_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        *model = value;
    }
    if let Some(value) = event
        .pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        *project_dir = value.to_owned();
    }
}

/// 将 Codex 的接入通道与可定价模型命名空间拆成归档 provider/model。
///
/// 这里有意偏离原契约第 101 行“直接使用 `session_meta.model_provider`”的约定：
/// `model_provider` 描述接入通道，例如 `amazon-bedrock`，而 `turn_context.model` 的
/// `openai.gpt-5.4` 前缀才是模型所属的可定价 provider。价格匹配的 exact、normalized、family
/// 三层都要求 provider 相等，且目录中的 GPT-5.4 / GPT-5.6 只登记在 `openai` 下；若保留
/// `amazon-bedrock`，实测 17,317 / 20,252（约 85%）条记录会永久无法估价。无合法 namespace
/// 时仍回退 `model_provider`，因此 `codex-auto-review` 等无前缀标识不会被错误改写。
fn normalize_provider_and_model(provider: &str, model: &str) -> (String, String) {
    let model = model.trim();
    if let Some((namespace, model_id)) = model.split_once('.') {
        let namespace_is_provider = !namespace.is_empty()
            && namespace
                .chars()
                .all(|character| character.is_ascii_alphabetic() || matches!(character, '-' | '_'));
        if namespace_is_provider && !model_id.trim().is_empty() {
            return (namespace.to_owned(), model_id.trim().to_owned());
        }
    }
    let provider = provider.trim();
    (
        if provider.is_empty() {
            DEFAULT_PROVIDER_ID.to_owned()
        } else {
            provider.to_owned()
        },
        if model.is_empty() {
            "unknown".to_owned()
        } else {
            model.to_owned()
        },
    )
}

fn session_id_from_rollout_path(relative_path: &str) -> String {
    let file_name = relative_path.rsplit('/').next().unwrap_or(relative_path);
    let stem = file_name.strip_suffix(".jsonl").unwrap_or(file_name);
    let bytes = stem.as_bytes();
    if bytes.len() >= 36 {
        let candidate = &stem[stem.len() - 36..];
        if candidate.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        }) {
            return candidate.to_owned();
        }
    }
    stem.to_owned()
}

fn parse_timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|value| value.timestamp_millis());
    }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{json, Value};

    use super::*;

    const SESSION_ID: &str = "019f68e5-62ed-77f1-85d2-1d75912b0001";

    fn rollout_path(root: &Path, tree: &str, session_id: &str) -> PathBuf {
        root.join(tree)
            .join("2026/07/16")
            .join(format!("rollout-2026-07-16T11-08-11-{session_id}.jsonl"))
    }

    fn event(timestamp: &str, ordinal: Option<u64>, event_type: &str, payload: Value) -> Value {
        let mut value = json!({
            "timestamp": timestamp,
            "type": event_type,
            "payload": payload,
        });
        if let Some(ordinal) = ordinal {
            value["ordinal"] = json!(ordinal);
        }
        value
    }

    fn token_count(
        timestamp: &str,
        ordinal: Option<u64>,
        total: Value,
        last: Option<Value>,
    ) -> Value {
        let mut info = json!({ "total_token_usage": total });
        if let Some(last) = last {
            info["last_token_usage"] = last;
        }
        event(
            timestamp,
            ordinal,
            "event_msg",
            json!({ "type": "token_count", "info": info }),
        )
    }

    fn usage(
        input: u64,
        cache_read: u64,
        cache_write: Option<u64>,
        output: u64,
        reasoning: u64,
    ) -> Value {
        let mut usage = json!({
            "input_tokens": input,
            "cached_input_tokens": cache_read,
            "output_tokens": output,
            "reasoning_output_tokens": reasoning,
            "total_tokens": input + cache_read + output,
        });
        if let Some(cache_write) = cache_write {
            usage["cache_write_input_tokens"] = json!(cache_write);
        }
        usage
    }

    fn write_rollout(path: &Path, events: &[Value]) {
        fs::create_dir_all(path.parent().expect("rollout parent")).expect("create rollout parent");
        let body = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).expect("write synthetic rollout");
    }

    #[test]
    fn scan_maps_state_tokens_origins_and_archive_shadowing() {
        let temp = tempfile::tempdir().expect("create synthetic Codex root");
        let live = rollout_path(temp.path(), SESSIONS_DIRECTORY, SESSION_ID);
        let state = [
            event(
                "2026-07-16T11:08:11.000Z",
                Some(1),
                "session_meta",
                json!({ "id": SESSION_ID, "model_provider": "amazon-bedrock" }),
            ),
            event(
                "2026-07-16T11:08:12.000Z",
                Some(2),
                "turn_context",
                json!({
                    "model": "openai.gpt-5.4",
                    "cwd": "/synthetic/codex-project",
                    "effort": "xhigh",
                }),
            ),
        ];
        let mut live_events = state.to_vec();
        live_events.push(token_count(
            "2026-07-16T11:08:13.000Z",
            Some(3),
            usage(100, 40, None, 30, 12),
            Some(usage(100, 40, None, 30, 12)),
        ));
        write_rollout(&live, &live_events);

        let archived_shadow = rollout_path(temp.path(), ARCHIVED_SESSIONS_DIRECTORY, SESSION_ID);
        let mut shadow_events = state.to_vec();
        shadow_events.push(token_count(
            "2026-07-16T11:08:13.000Z",
            Some(3),
            usage(9_999, 9_999, Some(9_999), 9_999, 9_999),
            Some(usage(9_999, 9_999, Some(9_999), 9_999, 9_999)),
        ));
        write_rollout(&archived_shadow, &shadow_events);

        let archived_id = "019f68e5-62ed-77f1-85d2-1d75912b0002";
        let archived = rollout_path(temp.path(), ARCHIVED_SESSIONS_DIRECTORY, archived_id);
        write_rollout(
            &archived,
            &[
                event(
                    "2026-07-15T10:00:00.000Z",
                    Some(1),
                    "session_meta",
                    json!({ "id": archived_id, "model_provider": "openai" }),
                ),
                event(
                    "2026-07-15T10:00:01.000Z",
                    Some(2),
                    "turn_context",
                    json!({ "model": "gpt-5.6-sol", "cwd": "/synthetic/archived" }),
                ),
                token_count(
                    "2026-07-15T10:00:02.000Z",
                    None,
                    usage(7, 5, Some(3), 11, 4),
                    Some(usage(7, 5, Some(3), 11, 4)),
                ),
            ],
        );

        let compressed = temp
            .path()
            .join(SESSIONS_DIRECTORY)
            .join("2026/07/14/rollout-synthetic.jsonl.zst");
        fs::create_dir_all(compressed.parent().expect("compressed parent"))
            .expect("create compressed parent");
        fs::write(compressed, b"synthetic unsupported compressed payload")
            .expect("write synthetic compressed file");

        let mut records = Vec::new();
        let result = scan_data_dir(
            temp.path(),
            &ScanRequest::live("host-codex-test", None),
            |batch| {
                records.extend_from_slice(batch);
                Ok(())
            },
        )
        .expect("scan synthetic Codex tree");

        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, 2);
        assert_eq!(result.delivered_records, 2);
        assert_eq!(result.skipped_breakdown.shadowed_archive, 1);
        assert_eq!(result.skipped_breakdown.unsupported_compression, 1);

        let live_record = records
            .iter()
            .find(|record| record.session_id == SESSION_ID)
            .expect("live record");
        assert_eq!(live_record.source, CODEX_SOURCE);
        assert_eq!(live_record.provider_id, "openai");
        assert_eq!(live_record.model_id, "gpt-5.4");
        assert_eq!(live_record.variant, None);
        assert_eq!(live_record.tok_input, 100);
        assert_eq!(live_record.tok_cache_read, 40);
        assert_eq!(live_record.tok_cache_write, 0);
        assert_eq!(live_record.tok_output, 30);
        assert_eq!(live_record.tok_reasoning, 12);
        assert_eq!(live_record.origin, Origin::Live);
        assert_eq!(live_record.project_dir, "/synthetic/codex-project");
        assert!(live_record.message_id.ends_with("#3"));
        let estimated = crate::pricing::PriceTable::default()
            .resolve_record(live_record)
            .estimated()
            .expect("normalized provider/model pair resolves a catalog price");
        assert!(estimated > 0.0);

        let archived_record = records
            .iter()
            .find(|record| record.session_id == archived_id)
            .expect("archived record");
        assert_eq!(archived_record.provider_id, "openai");
        assert_eq!(archived_record.model_id, "gpt-5.6-sol");
        assert_eq!(archived_record.origin, Origin::Bak);
        assert_eq!(archived_record.tok_cache_write, 3);
        assert!(archived_record.message_id.ends_with("#3"));
    }

    #[test]
    fn totals_gate_last_usage_and_negative_deltas_are_observable() {
        let temp = tempfile::tempdir().expect("create synthetic Codex root");
        let live = rollout_path(temp.path(), SESSIONS_DIRECTORY, SESSION_ID);
        write_rollout(
            &live,
            &[
                event(
                    "2026-07-16T11:08:11.000Z",
                    Some(1),
                    "session_meta",
                    json!({ "id": SESSION_ID, "model_provider": "openai" }),
                ),
                event(
                    "2026-07-16T11:08:12.000Z",
                    Some(2),
                    "turn_context",
                    json!({ "model": "gpt-5.4" }),
                ),
                token_count(
                    "2026-07-16T11:08:13.000Z",
                    Some(3),
                    usage(100, 20, Some(10), 30, 12),
                    Some(usage(100, 20, Some(10), 30, 12)),
                ),
                // Totals advance by more than `last`; the contract takes `last` after the gate.
                token_count(
                    "2026-07-16T11:08:14.000Z",
                    Some(4),
                    usage(150, 30, Some(15), 50, 20),
                    Some(usage(7, 3, Some(2), 5, 2)),
                ),
                // Duplicate delivery: totals do not advance, so the repeated `last` is ignored.
                token_count(
                    "2026-07-16T11:08:15.000Z",
                    Some(5),
                    usage(150, 30, Some(15), 50, 20),
                    Some(usage(7, 3, Some(2), 5, 2)),
                ),
                // Counter reset: negative bucket deltas clamp to zero and remain observable.
                token_count(
                    "2026-07-16T11:08:16.000Z",
                    Some(6),
                    usage(1, 1, Some(1), 1, 1),
                    None,
                ),
            ],
        );

        let mut records = Vec::new();
        let result = scan_data_dir(
            temp.path(),
            &ScanRequest::live("host-codex-test", None),
            |batch| {
                records.extend_from_slice(batch);
                Ok(())
            },
        )
        .expect("scan totals fixture");

        assert_eq!(result.eligible_count, 4);
        assert_eq!(result.skipped_breakdown.negative_delta, 1);
        assert_eq!(records[0].tok_input, 100);
        assert_eq!(records[1].tok_input, 7);
        assert_eq!(records[1].tok_reasoning, 2);
        assert!(records[2].is_incomplete);
        assert!(records[3].is_incomplete);
        assert_eq!(
            records.iter().map(|record| record.tok_input).sum::<u64>(),
            107
        );
        assert_eq!(
            records.iter().map(|record| record.tok_output).sum::<u64>(),
            35
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.tok_reasoning)
                .sum::<u64>(),
            14
        );
    }

    #[test]
    fn scan_reports_file_failures_line_skips_batches_and_interruption() {
        let temp = tempfile::tempdir().expect("create synthetic Codex root");
        let sessions = temp.path().join(SESSIONS_DIRECTORY);
        fs::create_dir_all(&sessions).expect("create sessions tree");
        fs::write(sessions.join("ignored.txt"), "not a rollout").expect("write ignored file");
        fs::write(
            sessions.join("malformed.jsonl"),
            format!(
                "{}\n{{not-json\n",
                token_count(
                    "2026-07-16T11:08:13.000Z",
                    Some(1),
                    usage(1, 0, None, 1, 0),
                    Some(usage(1, 0, None, 1, 0)),
                )
            ),
        )
        .expect("write malformed rollout");
        fs::write(sessions.join("non-object.jsonl"), "[]\n").expect("write non-object rollout");

        let edge_path = sessions.join("nested/edge.jsonl");
        write_rollout(
            &edge_path,
            &[
                event(
                    "2026-07-16T11:08:11.000Z",
                    None,
                    "session_meta",
                    json!({ "id": " ", "model_provider": " " }),
                ),
                event(
                    "2026-07-16T11:08:12.000Z",
                    None,
                    "turn_context",
                    json!({ "model": " ", "cwd": " " }),
                ),
                event(
                    "2026-07-16T11:08:13.000Z",
                    None,
                    "event_msg",
                    json!({ "type": "token_count", "info": {} }),
                ),
                event(
                    "2026-07-16T11:08:14.000Z",
                    None,
                    "event_msg",
                    json!({
                        "type": "token_count",
                        "info": { "total_token_usage": "invalid" },
                    }),
                ),
                token_count("invalid timestamp", None, usage(2, 1, None, 1, 0), None),
                event("2026-07-16T11:08:15.000Z", None, "unknown", json!({})),
                json!({
                    "timestamp": 1_784_200_096_000_i64,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": usage(4, 2, None, 3, 1),
                            "last_token_usage": usage(0, 0, None, 0, 0),
                        },
                    },
                }),
                token_count(
                    "2026-07-16T11:08:17.000Z",
                    None,
                    usage(8, 4, None, 6, 2),
                    None,
                ),
            ],
        );

        #[cfg(unix)]
        std::os::unix::fs::symlink(
            sessions.join("missing-target"),
            sessions.join("unreadable.jsonl"),
        )
        .expect("create broken rollout symlink");

        let mut request = ScanRequest::live("host-codex-errors", None);
        request.batch_size = 1;
        request.last_success_utc = Some(123);
        let mut records = Vec::new();
        let result = scan_data_dir(temp.path(), &request, |batch| {
            records.extend_from_slice(batch);
            Ok(())
        })
        .expect("scan edge tree");
        assert!(result.reached_eof);
        assert_eq!(result.last_success_utc, Some(123));
        assert_eq!(result.eligible_count, 2);
        assert_eq!(result.delivered_batches, 2);
        assert_eq!(result.skipped_breakdown.malformed_json, 2);
        assert_eq!(result.skipped_breakdown.missing_total_usage, 1);
        assert_eq!(result.skipped_breakdown.invalid_total_usage, 1);
        assert_eq!(result.skipped_breakdown.unparsable_timestamp, 1);
        #[cfg(unix)]
        assert_eq!(result.skipped_breakdown.unreadable_file, 1);
        assert_eq!(records[0].session_id, "edge");
        assert_eq!(records[0].provider_id, DEFAULT_PROVIDER_ID);
        assert_eq!(records[0].model_id, "unknown");
        assert_eq!(records[0].message_id, "nested/edge.jsonl#7");

        let interrupted = scan_data_dir(temp.path(), &request, |_| {
            Err(SinkError::new("synthetic cancellation"))
        })
        .expect("interruption is a recoverable scan result");
        assert!(!interrupted.reached_eof);
        assert_eq!(interrupted.delivered_records, 0);
        assert_eq!(
            interrupted.skip_reason,
            Some(ScanSkipReason::Interrupted(
                "synthetic cancellation".to_owned()
            ))
        );

        request.batch_size = 0;
        assert!(matches!(
            scan_data_dir(temp.path(), &request, |_| Ok(())),
            Err(CodexError::InvalidBatchSize)
        ));
        assert!(matches!(
            scan_data_dir(
                temp.path().join("absent"),
                &ScanRequest::live("host", None),
                |_| Ok(())
            ),
            Err(CodexError::DataDirectoryNotFound { .. })
        ));
    }

    #[test]
    fn helper_contracts_cover_discovery_identifiers_and_lossy_values() {
        let temp = tempfile::tempdir().expect("create discovery tree");
        let explicit = temp.path().join("explicit");
        let home = temp.path().join("home");
        fs::create_dir_all(explicit.join(ARCHIVED_SESSIONS_DIRECTORY))
            .expect("create explicit archive tree");
        fs::create_dir_all(home.join(".codex").join(SESSIONS_DIRECTORY))
            .expect("create home sessions tree");
        assert_eq!(
            discover_data_dir_from(Some(&explicit), Some(&home)).expect("discover explicit root"),
            explicit
        );
        assert_eq!(
            discover_data_dir_from(None, Some(&home)).expect("discover home root"),
            home.join(".codex")
        );
        let missing = discover_data_dir_from(None, None).expect_err("missing roots must fail");
        assert_eq!(
            missing.to_string(),
            "未找到 Codex rollout 数据目录；已检查：[]"
        );
        let displayed_paths = CodexError::DataDirectoryNotFound {
            probed_paths: vec![PathBuf::from("first"), PathBuf::from("second")],
        };
        assert!(displayed_paths.to_string().contains("first, second"));
        let enumerate = collect_rollouts(&temp.path().join("missing"), Origin::Live)
            .expect_err("missing tree cannot be enumerated");
        assert!(enumerate
            .to_string()
            .contains("无法枚举 Codex rollout 目录"));

        let context = ParseContext::new("host", Origin::Bak, "nested/file.jsonl");
        assert_eq!(context.host_id, "host");
        assert_eq!(context.origin, Origin::Bak);
        assert_eq!(context.relative_path, "nested/file.jsonl");
        assert_eq!(
            ScanRequest::live("host", Some(OVERLAP_WINDOW_MS)).window_start(),
            0
        );

        for reason in [
            SkipReason::MalformedJson,
            SkipReason::NonUsageEvent,
            SkipReason::MissingTotalUsage,
            SkipReason::InvalidTotalUsage,
            SkipReason::UnparsableTimestamp,
            SkipReason::NegativeDelta,
            SkipReason::ShadowedArchive,
            SkipReason::UnsupportedCompression,
            SkipReason::UnreadableFile,
        ] {
            assert!(!reason.as_str().is_empty());
        }

        let root = Path::new("/synthetic/root");
        assert_eq!(
            rollout_relative_path(root, Path::new("/synthetic/root/a.jsonl.zst")),
            Some((true, "a.jsonl".to_owned()))
        );
        assert_eq!(
            rollout_relative_path(root, Path::new("/synthetic/root/a.jsonl")),
            Some((false, "a.jsonl".to_owned()))
        );
        assert_eq!(
            rollout_relative_path(root, Path::new("/synthetic/root/a.txt")),
            None
        );
        assert_eq!(
            rollout_relative_path(root, Path::new("/outside/a.jsonl")),
            None
        );

        assert_eq!(
            normalize_provider_and_model("amazon-bedrock", "openai.gpt-5.4"),
            ("openai".to_owned(), "gpt-5.4".to_owned())
        );
        assert_eq!(
            normalize_provider_and_model("", ""),
            ("codex".to_owned(), "unknown".to_owned())
        );
        assert_eq!(
            normalize_provider_and_model("openai", "bad prefix.model"),
            ("openai".to_owned(), "bad prefix.model".to_owned())
        );
        assert_eq!(
            normalize_provider_and_model("openai", "openai."),
            ("openai".to_owned(), "openai.".to_owned())
        );
        assert_eq!(
            normalize_provider_and_model("openai", "codex-auto-review"),
            ("openai".to_owned(), "codex-auto-review".to_owned())
        );
        assert_eq!(
            session_id_from_rollout_path(&format!("x/rollout-{SESSION_ID}.jsonl")),
            SESSION_ID
        );
        assert_eq!(session_id_from_rollout_path("plain.jsonl"), "plain");
        assert_eq!(session_id_from_rollout_path("plain"), "plain");

        assert_eq!(parse_timestamp_ms(&json!(1_234_i64)), Some(1_234));
        assert_eq!(parse_timestamp_ms(&json!("bad")), None);
        assert_eq!(value_to_string(&json!(42)), Some("42".to_owned()));
        assert_eq!(value_to_string(&json!(true)), None);
        assert_eq!(lossy_u64(None), 0);
        assert_eq!(lossy_u64(Some(&json!(-2))), 0);
        assert_eq!(lossy_u64(Some(&json!(3.9))), 3);
        assert_eq!(lossy_u64(Some(&json!("4.8"))), 4);
        assert_eq!(lossy_u64(Some(&json!("bad"))), 0);
        assert_eq!(lossy_u64(Some(&json!(true))), 0);
        assert_eq!(finite_nonnegative_u64(f64::INFINITY), 0);
    }

    #[test]
    fn default_scan_uses_codex_home_without_touching_real_data() {
        let temp = tempfile::tempdir().expect("create synthetic Codex home");
        fs::create_dir_all(temp.path().join(SESSIONS_DIRECTORY)).expect("create sessions tree");
        let previous = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", temp.path());

        assert_eq!(
            discover_data_dir().expect("discover CODEX_HOME"),
            temp.path()
        );
        let result = scan_default(&ScanRequest::live("host-default", None), |_| Ok(()))
            .expect("scan synthetic CODEX_HOME");
        assert!(result.reached_eof);
        assert_eq!(result.eligible_count, 0);

        match previous {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }
}
