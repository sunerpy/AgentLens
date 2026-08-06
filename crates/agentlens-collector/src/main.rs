//! AgentLens headless collector: read OpenCode through the shared read-only scanner and emit NDJSON.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use agentlens_core::archive::NormalizedUsageRecord;
use agentlens_core::host::{self, MachineIdentity};
use agentlens_core::source::opencode::{
    self, OpenCodeError, ScanRequest, ScanResult, SinkError, SqliteSourceConnection,
};
use base64::Engine as _;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
const SOURCE_NAME: &str = "opencode";
const DATABASE_FILE: &str = "opencode.db";
const SNAPSHOT_SPACE_NUMERATOR: u64 = 6;
const SNAPSHOT_SPACE_DENOMINATOR: u64 = 5;
const USAGE: &str = "用法: agentlens-collector collect --since <cursor_ms> \
     [--data-dir <path>] [--request-base64url <payload>] [--snapshot]";

static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SNAPSHOT_MUTEX: Mutex<()> = Mutex::new(());

/// AgentLens Remote Source API v1 metadata line.
///
/// These wire DTOs intentionally live in the collector crate for this todo because the shared core
/// files are owned by parallel work. Moving them to `agentlens-core` later is a mechanical module
/// move that does not change their serde field names or types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectorMetaV1 {
    pub protocol_version: u32,
    pub machine_id_hash: String,
    pub hostname: String,
    pub collector_version: String,
    pub sources: Vec<CollectorSourceMetaV1>,
}

/// Per-source counters and scan window nested in [`CollectorMetaV1::sources`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectorSourceMetaV1 {
    pub source: String,
    pub data_dir: String,
    pub scan_window: CollectorScanWindowV1,
    pub eligible_count: u64,
    pub skipped_count: u64,
}

/// Requested cursor and deterministic source cutoff for one scan.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectorScanWindowV1 {
    pub since: i64,
    pub cutoff: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CollectRequest {
    since: i64,
    data_dir: Option<PathBuf>,
    snapshot: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedCollectRequest {
    since: Option<i64>,
    data_dir: Option<String>,
    snapshot: Option<bool>,
}

#[derive(Debug, PartialEq)]
struct Collection {
    meta: CollectorMetaV1,
    records: Vec<NormalizedUsageRecord>,
}

#[derive(Debug)]
enum ProgramAction {
    Collect(CollectRequest),
    Help,
    Version,
}

type CollectorResult<T> = std::result::Result<T, CollectorError>;

#[derive(Debug)]
enum CollectorError {
    InvalidInput(String),
    NoDataDirectory(String),
    SnapshotSpaceInsufficient {
        available: u64,
        required: u64,
        target: PathBuf,
    },
    SourceUnreadable(String),
    Snapshot(String),
    Identity(String),
    Output(String),
    Interrupted,
}

impl CollectorError {
    const fn exit_code(&self) -> i32 {
        match self {
            Self::NoDataDirectory(_) => 2,
            Self::SnapshotSpaceInsufficient { .. } => 3,
            Self::SourceUnreadable(_) => 4,
            Self::InvalidInput(_)
            | Self::Snapshot(_)
            | Self::Identity(_)
            | Self::Output(_)
            | Self::Interrupted => 1,
        }
    }
}

impl fmt::Display for CollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "参数错误：{message}。{USAGE}"),
            Self::NoDataDirectory(message) => write!(
                formatter,
                "未找到可用的 OpenCode 数据目录：{message}；请用 --data-dir 指向包含 opencode.db 的目录"
            ),
            Self::SnapshotSpaceInsufficient {
                available,
                required,
                target,
            } => write!(
                formatter,
                "创建 snapshot 的目标文件系统空间不足：{} 可用 {available} 字节，需要至少 {required} 字节（源库大小的 1.2 倍）",
                target.display()
            ),
            Self::SourceUnreadable(message) => write!(formatter, "OpenCode 源库不可读：{message}"),
            Self::Snapshot(message) => write!(formatter, "创建 OpenCode snapshot 失败：{message}"),
            Self::Identity(message) => write!(formatter, "无法确定远端机器身份：{message}"),
            Self::Output(message) => write!(formatter, "写入 NDJSON 失败：{message}"),
            Self::Interrupted => write!(formatter, "snapshot/采集已被中断，临时文件已清理"),
        }
    }
}

impl std::error::Error for CollectorError {}

fn main() {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let stderr = io::stderr();
    let exit_code = program_exit_code(
        parse_program_action(&args).and_then(run_action),
        &mut stderr.lock(),
    );
    if exit_code != 0 {
        process::exit(exit_code);
    }
}

fn program_exit_code(result: CollectorResult<()>, stderr: &mut impl io::Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            writeln!(stderr, "{error}").expect("write collector error");
            error.exit_code()
        }
    }
}

fn run_action(action: ProgramAction) -> CollectorResult<()> {
    match action {
        ProgramAction::Help => {
            println!("{USAGE}");
            Ok(())
        }
        ProgramAction::Version => {
            println!("agentlens-collector {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ProgramAction::Collect(request) => {
            let identity = host::local_machine_identity()
                .map_err(|error| CollectorError::Identity(error.to_string()))?;
            let snapshot_dir = env::temp_dir();
            let collection = collect_with_space_probe(
                &request,
                &identity,
                &local_hostname(),
                &snapshot_dir,
                &free_space_bytes,
            )?;
            match write_ndjson(io::stdout().lock(), &collection) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                Err(error) => Err(CollectorError::Output(error.to_string())),
            }
        }
    }
}

fn parse_program_action(args: &[OsString]) -> CollectorResult<ProgramAction> {
    match args.first().and_then(|argument| argument.to_str()) {
        Some("--help" | "-h") if args.len() == 1 => Ok(ProgramAction::Help),
        Some("--version" | "-V") if args.len() == 1 => Ok(ProgramAction::Version),
        Some("collect") => parse_collect_request(args).map(ProgramAction::Collect),
        Some(command) => Err(CollectorError::InvalidInput(format!(
            "未知子命令或选项 {command:?}"
        ))),
        None if args.is_empty() => Err(CollectorError::InvalidInput("缺少 collect 子命令".into())),
        None => Err(CollectorError::InvalidInput(
            "命令参数必须是有效的 UTF-8".into(),
        )),
    }
}

fn parse_collect_request(args: &[OsString]) -> CollectorResult<CollectRequest> {
    if args.first().and_then(|argument| argument.to_str()) != Some("collect") {
        return Err(CollectorError::InvalidInput("缺少 collect 子命令".into()));
    }

    let mut since = None;
    let mut data_dir = None;
    let mut snapshot = false;
    let mut payload = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| CollectorError::InvalidInput("选项名必须是有效的 UTF-8".into()))?;
        match flag {
            "--since" => {
                ensure_not_set(since.is_some(), "--since")?;
                let value = required_value(args, index, "--since")?;
                let text = value.to_str().ok_or_else(|| {
                    CollectorError::InvalidInput("--since 必须是有效的 UTF-8 整数".into())
                })?;
                since = Some(parse_since(text)?);
                index += 2;
            }
            "--data-dir" => {
                ensure_not_set(data_dir.is_some(), "--data-dir")?;
                data_dir = Some(PathBuf::from(required_value(args, index, "--data-dir")?));
                index += 2;
            }
            "--request-base64url" => {
                ensure_not_set(payload.is_some(), "--request-base64url")?;
                let value = required_value(args, index, "--request-base64url")?;
                payload = Some(value.to_str().ok_or_else(|| {
                    CollectorError::InvalidInput(
                        "--request-base64url 必须是有效的 UTF-8 base64url".into(),
                    )
                })?);
                index += 2;
            }
            "--snapshot" => {
                if snapshot {
                    return Err(CollectorError::InvalidInput(
                        "--snapshot 不能重复指定".into(),
                    ));
                }
                snapshot = true;
                index += 1;
            }
            unknown => {
                return Err(CollectorError::InvalidInput(format!(
                    "未知 collect 选项 {unknown:?}"
                )));
            }
        }
    }

    if let Some(encoded) = payload {
        let decoded = decode_request_payload(encoded)?;
        if let Some(payload_since) = decoded.since {
            since = Some(validate_since(payload_since)?);
        }
        if let Some(payload_data_dir) = decoded.data_dir {
            data_dir = Some(PathBuf::from(payload_data_dir));
        }
        if let Some(payload_snapshot) = decoded.snapshot {
            snapshot = payload_snapshot;
        }
    }

    Ok(CollectRequest {
        since: since.ok_or_else(|| {
            CollectorError::InvalidInput(
                "缺少 --since；也可在 --request-base64url JSON 中提供 since".into(),
            )
        })?,
        data_dir,
        snapshot,
    })
}

fn ensure_not_set(is_set: bool, flag: &str) -> CollectorResult<()> {
    if is_set {
        Err(CollectorError::InvalidInput(format!("{flag} 不能重复指定")))
    } else {
        Ok(())
    }
}

fn required_value<'a>(
    args: &'a [OsString],
    flag_index: usize,
    flag: &str,
) -> CollectorResult<&'a OsStr> {
    args.get(flag_index + 1)
        .map(OsString::as_os_str)
        .ok_or_else(|| CollectorError::InvalidInput(format!("{flag} 缺少值")))
}

fn parse_since(value: &str) -> CollectorResult<i64> {
    let parsed = value.parse::<i64>().map_err(|_| {
        CollectorError::InvalidInput(format!("--since 必须是非负整数，收到 {value:?}"))
    })?;
    validate_since(parsed)
}

fn validate_since(value: i64) -> CollectorResult<i64> {
    if value < 0 {
        Err(CollectorError::InvalidInput(format!(
            "--since 不能为负数，收到 {value}"
        )))
    } else {
        Ok(value)
    }
}

fn decode_request_payload(encoded: &str) -> CollectorResult<EncodedCollectRequest> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| {
            CollectorError::InvalidInput(format!("--request-base64url 解码失败：{error}"))
        })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        CollectorError::InvalidInput(format!(
            "--request-base64url 解码后不是合法的 collect JSON：{error}"
        ))
    })
}

fn collect_with_space_probe(
    request: &CollectRequest,
    identity: &MachineIdentity,
    hostname: &str,
    snapshot_dir: &Path,
    space_probe: &dyn Fn(&Path) -> CollectorResult<u64>,
) -> CollectorResult<Collection> {
    let database = resolve_database_path(request.data_dir.as_deref())?;
    let snapshot_target = new_snapshot_path(snapshot_dir);
    let snapshot = if request.snapshot {
        Some(snapshot_database_with(
            &database,
            &snapshot_target,
            space_probe,
            &vacuum_into_read_only_source,
        )?)
    } else {
        None
    };
    let scan_path = snapshot
        .as_ref()
        .map_or(database.as_path(), SnapshotDatabase::path);
    let mut records = Vec::new();
    let scan_request = ScanRequest::live(identity.host_id(), Some(request.since));
    let scan_result = opencode::scan_database(scan_path, &scan_request, |batch| {
        if request.snapshot && snapshot_interrupted() {
            return Err(SinkError::new("snapshot collection interrupted"));
        }
        records.extend_from_slice(batch);
        Ok(())
    })
    .map_err(map_source_error)?;

    validate_scan_completion(request, &scan_result, &records)?;

    let cutoff = scan_result
        .observed_max_time_updated
        .unwrap_or(request.since)
        .max(request.since);
    let data_dir = database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .into_owned();
    let meta = CollectorMetaV1 {
        protocol_version: PROTOCOL_VERSION,
        machine_id_hash: identity.machine_id_hash().to_owned(),
        hostname: hostname.to_owned(),
        collector_version: env!("CARGO_PKG_VERSION").to_owned(),
        sources: vec![CollectorSourceMetaV1 {
            source: SOURCE_NAME.to_owned(),
            data_dir,
            scan_window: CollectorScanWindowV1 {
                since: request.since,
                cutoff,
            },
            eligible_count: scan_result.eligible_count,
            skipped_count: scan_result.skipped_count,
        }],
    };
    Ok(Collection { meta, records })
}

fn validate_scan_completion(
    request: &CollectRequest,
    scan_result: &ScanResult,
    records: &[NormalizedUsageRecord],
) -> CollectorResult<()> {
    if request.snapshot && snapshot_interrupted() {
        return Err(CollectorError::Interrupted);
    }
    if !scan_result.reached_eof {
        return Err(CollectorError::SourceUnreadable(format!(
            "扫描未到达 EOF：{:?}",
            scan_result.skip_reason
        )));
    }
    if records.iter().any(|record| record.source != SOURCE_NAME) {
        return Err(CollectorError::SourceUnreadable(
            "共享 scanner 返回了缺失或错误的 source 字段".into(),
        ));
    }
    Ok(())
}

fn resolve_database_path(data_dir: Option<&Path>) -> CollectorResult<PathBuf> {
    if let Some(directory) = data_dir {
        if !directory.is_dir() {
            return Err(CollectorError::NoDataDirectory(format!(
                "{} 不是目录",
                directory.display()
            )));
        }
        let database = directory.join(DATABASE_FILE);
        if !database.is_file() {
            return Err(CollectorError::NoDataDirectory(format!(
                "{} 中不存在 {DATABASE_FILE}",
                directory.display()
            )));
        }
        return Ok(database);
    }
    opencode::discover_database_path().map_err(map_source_error)
}

fn map_source_error(error: OpenCodeError) -> CollectorError {
    if matches!(&error, OpenCodeError::DatabaseNotFound { .. }) {
        CollectorError::NoDataDirectory(error.to_string())
    } else {
        CollectorError::SourceUnreadable(error.to_string())
    }
}

fn new_snapshot_path(directory: &Path) -> PathBuf {
    let sequence = SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        ".agentlens-collector-snapshot-{}-{sequence}.db",
        process::id()
    ))
}

struct SnapshotDatabase {
    path: PathBuf,
    _cleanup: SnapshotCleanup,
    _signals: snapshot_signal::SignalGuard,
    _lock: MutexGuard<'static, ()>,
}

impl SnapshotDatabase {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for SnapshotDatabase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotDatabase")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

struct SnapshotCleanup {
    path: PathBuf,
}

impl Drop for SnapshotCleanup {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "警告：无法清理 snapshot 临时文件 {}：{error}",
                self.path.display()
            ),
        }
    }
}

fn snapshot_database_with(
    source: &Path,
    target: &Path,
    space_probe: &dyn Fn(&Path) -> CollectorResult<u64>,
    vacuum: &dyn Fn(&Path, &Path) -> CollectorResult<()>,
) -> CollectorResult<SnapshotDatabase> {
    let source_probe = SqliteSourceConnection::open(source).map_err(map_source_error)?;
    drop(source_probe);

    let source_size = fs::metadata(source)
        .map_err(|error| {
            CollectorError::SourceUnreadable(format!(
                "无法读取源库 {} 的大小：{error}",
                source.display()
            ))
        })?
        .len();
    let required = source_size
        .saturating_mul(SNAPSHOT_SPACE_NUMERATOR)
        .saturating_add(SNAPSHOT_SPACE_DENOMINATOR - 1)
        / SNAPSHOT_SPACE_DENOMINATOR;
    let target_filesystem = target.parent().unwrap_or_else(|| Path::new("."));
    let available = space_probe(target_filesystem)?;
    if available < required {
        return Err(CollectorError::SnapshotSpaceInsufficient {
            available,
            required,
            target: target_filesystem.to_path_buf(),
        });
    }
    if target.exists() {
        return Err(CollectorError::Snapshot(format!(
            "目标 {} 已存在，拒绝覆盖",
            target.display()
        )));
    }

    let lock = SNAPSHOT_MUTEX
        .lock()
        .map_err(|_| CollectorError::Snapshot("snapshot 串行锁已损坏".into()))?;
    SNAPSHOT_INTERRUPTED.store(false, Ordering::SeqCst);
    let signals = snapshot_signal::SignalGuard::install().map_err(CollectorError::Snapshot)?;
    let cleanup = SnapshotCleanup {
        path: target.to_path_buf(),
    };
    vacuum(source, target)?;
    if snapshot_interrupted() {
        return Err(CollectorError::Interrupted);
    }

    Ok(SnapshotDatabase {
        path: target.to_path_buf(),
        _cleanup: cleanup,
        _signals: signals,
        _lock: lock,
    })
}

fn vacuum_into_read_only_source(source: &Path, target: &Path) -> CollectorResult<()> {
    let uri = format!("file:{}?mode=ro", source.display());
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| snapshot_source_error(source, error))?;
    let target_text = target.to_str().ok_or_else(|| {
        CollectorError::Snapshot(format!(
            "snapshot 目标路径不是有效 UTF-8：{}",
            target.display()
        ))
    })?;

    // SNAPSHOT_QUERY_ONLY_EXEMPTION_TODO_10: this is the single documented query_only exemption.
    // The source remains URI mode=ro and this connection executes exactly this parameterized
    // VACUUM INTO statement; every source/snapshot scan still uses SqliteSourceConnection with
    // query_only=ON. VACUUM INTO itself fails with SQLITE_READONLY when query_only is enabled.
    connection
        .execute("VACUUM INTO ?1", params![target_text])
        .map_err(|error| snapshot_source_error(source, error))?;
    Ok(())
}

fn snapshot_source_error(source: &Path, error: rusqlite::Error) -> CollectorError {
    CollectorError::SourceUnreadable(format!(
        "snapshot 无法读取 {}（{error}）；请用 chmod 授予 WAL/SHM 读取权限或将 AgentLens 用户加入文件所属 group；不可用 snapshot/VACUUM 绕过权限",
        source.display()
    ))
}

fn free_space_bytes(path: &Path) -> CollectorResult<u64> {
    let output = Command::new("df").arg("-Pk").arg(path).output();
    free_space_from_df_output(path, output)
}

fn free_space_from_df_output(path: &Path, output: io::Result<Output>) -> CollectorResult<u64> {
    let output = output.map_err(|error| {
        CollectorError::Snapshot(format!("无法执行 df -Pk {}：{error}", path.display()))
    })?;
    if !output.status.success() {
        return Err(CollectorError::Snapshot(format!(
            "df -Pk {} 退出状态为 {}：{}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| CollectorError::Snapshot(format!("df 输出不是 UTF-8：{error}")))?;
    let line = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .ok_or_else(|| CollectorError::Snapshot("df 没有输出文件系统数据".into()))?;
    let available_kib = line
        .split_whitespace()
        .nth(3)
        .ok_or_else(|| CollectorError::Snapshot(format!("无法解析 df 输出：{line:?}")))?
        .parse::<u64>()
        .map_err(|error| {
            CollectorError::Snapshot(format!("无法解析 df 可用空间 {line:?}：{error}"))
        })?;
    Ok(available_kib.saturating_mul(1024))
}

fn snapshot_interrupted() -> bool {
    SNAPSHOT_INTERRUPTED.load(Ordering::SeqCst)
}

fn write_ndjson(mut writer: impl io::Write, collection: &Collection) -> io::Result<()> {
    serde_json::to_writer(&mut writer, &collection.meta).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    for record in &collection.records {
        serde_json::to_writer(&mut writer, record).map_err(io::Error::other)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

fn local_hostname() -> String {
    ["HOSTNAME", "COMPUTERNAME"]
        .into_iter()
        .filter_map(env::var_os)
        .filter_map(|value| value.into_string().ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(unix)]
mod snapshot_signal {
    use super::SNAPSHOT_INTERRUPTED;
    use std::sync::atomic::Ordering;

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    const SIG_ERR: usize = usize::MAX;

    extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }

    pub(super) extern "C" fn mark_interrupted(_: i32) {
        SNAPSHOT_INTERRUPTED.store(true, Ordering::SeqCst);
    }

    pub(super) struct SignalGuard {
        previous_sigint: usize,
        previous_sigterm: usize,
    }

    impl SignalGuard {
        pub(super) fn install() -> Result<Self, String> {
            // SAFETY: `mark_interrupted` has the C signal-handler ABI and performs only an
            // async-signal-safe atomic store. The previous handlers are restored by Drop.
            let previous_sigint = unsafe { signal(SIGINT, mark_interrupted as *const () as usize) };
            if previous_sigint == SIG_ERR {
                return Err("无法安装 SIGINT snapshot 清理处理器".into());
            }
            // SAFETY: same handler contract as above, for SIGTERM.
            let previous_sigterm =
                unsafe { signal(SIGTERM, mark_interrupted as *const () as usize) };
            if previous_sigterm == SIG_ERR {
                // SAFETY: restoring the handler returned by the successful signal call is valid.
                unsafe {
                    signal(SIGINT, previous_sigint);
                }
                return Err("无法安装 SIGTERM snapshot 清理处理器".into());
            }
            Ok(Self {
                previous_sigint,
                previous_sigterm,
            })
        }
    }

    impl Drop for SignalGuard {
        fn drop(&mut self) {
            // SAFETY: both values were returned by successful signal calls in `install`.
            unsafe {
                signal(SIGINT, self.previous_sigint);
                signal(SIGTERM, self.previous_sigterm);
            }
        }
    }
}

#[cfg(not(unix))]
mod snapshot_signal {
    pub(super) struct SignalGuard;

    impl SignalGuard {
        pub(super) fn install() -> Result<Self, String> {
            Ok(Self)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{ExitStatus, Output};
    // These three are reached only from the unix-gated WAL-permission fixture, so on
    // Windows (where that test is not compiled) an ungated import is an unused import and
    // clippy `-D warnings` rejects it.
    #[cfg(unix)]
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agentlens_core::archive::NormalizedUsageRecord;
    #[cfg(unix)]
    use agentlens_core::fixture::FixtureGuard;
    use agentlens_core::fixture::{generate, Manifest};
    use agentlens_core::host::MachineIdentity;
    use agentlens_core::source::opencode::{SourceConnection as _, SqliteSourceConnection};
    use base64::Engine as _;

    use super::*;

    const TEST_MACHINE_ID: &str = "3f8a2c1d4e5b6079a1b2c3d4e5f60718";

    fn fixture_directory() -> (tempfile::TempDir, PathBuf, Manifest) {
        let temp = tempfile::tempdir().expect("create fixture parent");
        let directory = temp.path().join("fixture");
        let manifest = generate(&directory).expect("generate fixture");
        (temp, directory, manifest)
    }

    fn identity() -> MachineIdentity {
        MachineIdentity::from_machine_id(TEST_MACHINE_ID).expect("derive fixture machine identity")
    }

    fn request(data_dir: &Path, snapshot: bool) -> CollectRequest {
        CollectRequest {
            since: 0,
            data_dir: Some(data_dir.to_path_buf()),
            snapshot,
        }
    }

    fn abundant_space(_: &Path) -> CollectorResult<u64> {
        Ok(u64::MAX)
    }

    fn directory_entries(path: &Path) -> BTreeSet<OsString> {
        fs::read_dir(path)
            .expect("read directory")
            .map(|entry| entry.expect("read entry").file_name())
            .collect()
    }

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt as _;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(code as u32)
    }

    #[test]
    fn collector_fixture_counts_and_v1_ndjson_round_trip_match_manifest() {
        let (_temp, directory, manifest) = fixture_directory();
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let collection = collect_with_space_probe(
            &request(&directory, false),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &abundant_space,
        )
        .expect("collect fixture");

        assert_eq!(collection.meta.protocol_version, 1);
        assert_eq!(collection.meta.sources.len(), 1);
        let source = &collection.meta.sources[0];
        assert_eq!(source.source, "opencode");
        assert_eq!(source.eligible_count, manifest.eligible_assistant_count);
        assert_eq!(source.skipped_count, manifest.skipped_count);
        assert_eq!(collection.records.len() as u64, source.eligible_count);
        assert!(collection
            .records
            .iter()
            .all(|record| record.source == "opencode"));

        let mut ndjson = Vec::new();
        write_ndjson(&mut ndjson, &collection).expect("render NDJSON into buffer");
        let text = String::from_utf8(ndjson).expect("NDJSON is UTF-8");
        let mut lines = text.lines();
        let decoded_meta: CollectorMetaV1 =
            serde_json::from_str(lines.next().expect("meta line")).expect("decode meta line");
        assert_eq!(decoded_meta, collection.meta);
        let decoded_records = lines
            .map(|line| serde_json::from_str::<NormalizedUsageRecord>(line).expect("decode record"))
            .collect::<Vec<_>>();
        assert_eq!(decoded_records, collection.records);
    }

    #[test]
    fn collector_snapshot_matches_direct_scan_and_all_scan_connections_are_query_only() {
        let (_temp, directory, _manifest) = fixture_directory();
        let before = directory_entries(&directory);
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let direct = collect_with_space_probe(
            &request(&directory, false),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &abundant_space,
        )
        .expect("direct collect");
        let snapshotted = collect_with_space_probe(
            &request(&directory, true),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &abundant_space,
        )
        .expect("snapshot collect");

        assert_eq!(snapshotted.meta.sources, direct.meta.sources);
        assert_eq!(snapshotted.records, direct.records);
        assert_eq!(directory_entries(&directory), before);
        assert!(directory_entries(snapshot_dir.path()).is_empty());

        let database = directory.join("opencode.db");
        let source = SqliteSourceConnection::open(&database).expect("open source query-only");
        assert!(source.query_only().expect("source query_only"));
        drop(source);

        let target = snapshot_dir.path().join("query-only-check.db");
        let snapshot = snapshot_database_with(
            &database,
            &target,
            &abundant_space,
            &vacuum_into_read_only_source,
        )
        .expect("create query-only check snapshot");
        let scan_connection =
            SqliteSourceConnection::open(snapshot.path()).expect("open snapshot query-only");
        assert!(scan_connection.query_only().expect("snapshot query_only"));
        drop(scan_connection);
        drop(snapshot);
        assert!(!target.exists());
    }

    #[test]
    fn collector_snapshot_insufficient_space_is_exit_3_without_partial_file() {
        let (_temp, directory, _manifest) = fixture_directory();
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let error = collect_with_space_probe(
            &request(&directory, true),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &|_| Ok(0),
        )
        .expect_err("insufficient space must fail");

        assert_eq!(error.exit_code(), 3);
        assert!(error.to_string().contains("空间不足"));
        assert!(directory_entries(snapshot_dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn collector_wal_unreadable_is_exit_4_with_remediation_and_never_snapshots() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_temp, directory, _manifest) = fixture_directory();
        let guard = FixtureGuard::new(&directory).expect("create fixture WAL writer");
        let wal = PathBuf::from(format!("{}-wal", guard.db_path().display()));
        assert!(wal.is_file(), "fixture guard must retain a WAL sidecar");
        let original_mode = fs::metadata(&wal)
            .expect("WAL metadata")
            .permissions()
            .mode();
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o000)).expect("strip WAL access");
        let probes = AtomicUsize::new(0);
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let error = collect_with_space_probe(
            &request(&directory, true),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &|_| {
                probes.fetch_add(1, Ordering::SeqCst);
                Ok(u64::MAX)
            },
        )
        .expect_err("unreadable WAL must fail before snapshot");
        fs::set_permissions(&wal, fs::Permissions::from_mode(original_mode))
            .expect("restore WAL permissions");

        assert_eq!(error.exit_code(), 4);
        let message = error.to_string();
        assert!(message.contains("chmod"));
        assert!(message.contains("group"));
        assert!(message.contains("snapshot/VACUUM is not a fallback"));
        assert_eq!(probes.load(Ordering::SeqCst), 0);
        assert!(directory_entries(snapshot_dir.path()).is_empty());
    }

    #[test]
    fn collector_empty_or_file_data_dir_is_exit_2_with_chinese_hint() {
        let empty = tempfile::tempdir().expect("empty data dir");
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        for path in [
            empty.path().to_path_buf(),
            empty.path().join("not-a-directory.txt"),
        ] {
            if path.extension().is_some() {
                fs::write(&path, b"file").expect("create file data-dir probe");
            }
            let error = collect_with_space_probe(
                &request(&path, false),
                &identity(),
                "fixture-host",
                snapshot_dir.path(),
                &abundant_space,
            )
            .expect_err("missing data directory must fail");
            assert_eq!(error.exit_code(), 2);
            assert!(error.to_string().contains("数据目录"));
        }
    }

    #[test]
    fn collector_request_base64url_preserves_hostile_values_exactly() {
        let original = "-leading path with spaces, \"quotes\"\nand a newline";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "since": 123456789,
                "data_dir": original,
                "snapshot": true
            })
            .to_string(),
        );
        let args = vec![
            OsString::from("collect"),
            OsString::from("--since"),
            OsString::from("1"),
            OsString::from("--data-dir"),
            OsString::from("ignored"),
            OsString::from("--request-base64url"),
            OsString::from(encoded),
        ];
        let parsed = parse_collect_request(&args).expect("decode request payload");

        assert_eq!(parsed.since, 123456789);
        assert_eq!(parsed.data_dir.as_deref(), Some(Path::new(original)));
        assert!(parsed.snapshot);
    }

    #[test]
    fn collector_malformed_cli_inputs_are_rejected_readably() {
        let cases = [
            vec!["collect", "--request-base64url", "%%%"],
            vec!["collect", "--request-base64url", "bm90LWpzb24"],
            vec!["collect", "--unknown"],
            vec!["collect", "--since", "not-a-number"],
            vec!["collect", "--since", "-1"],
        ];
        for case in cases {
            let args = case.into_iter().map(OsString::from).collect::<Vec<_>>();
            let error = parse_collect_request(&args).expect_err("malformed input must fail");
            assert_eq!(error.exit_code(), 1);
            assert!(!error.to_string().trim().is_empty());
        }
    }

    /// 非 UTF-8 参数。两个平台的构造方式不同，被测分支是同一条，所以两侧都覆盖，
    /// 不留平台空档。
    #[cfg(unix)]
    fn non_utf8_arg() -> OsString {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(vec![0x63, 0xff, 0x6f])
    }

    #[cfg(windows)]
    fn non_utf8_arg() -> OsString {
        use std::os::windows::ffi::OsStringExt as _;
        // 未配对的高位代理项：合法的 UTF-16 存储，但不是合法 UTF-8。
        OsString::from_wide(&[0x0063, 0xD800, 0x006F])
    }

    /// 外层子命令派发。
    ///
    /// 已有用例都直接调 `parse_collect_request`，把这层整体跳过了，于是「远端主机上
    /// 到底怎么解释命令行」这段契约无人看守：派发错了，推到远端的采集器会打印用法而不是
    /// 采集，或者拒绝一次本来合法的调用——两种都表现为「远端没数据」，很难归因。
    #[test]
    fn collector_program_action_dispatch_covers_every_branch() {
        let action = |values: &[OsString]| parse_program_action(values);
        let owned = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();

        // 帮助与版本仅当它是唯一参数时生效。
        for flag in ["--help", "-h"] {
            assert!(matches!(
                action(&owned(&[flag])).expect("sole help flag"),
                ProgramAction::Help
            ));
        }
        for flag in ["--version", "-V"] {
            assert!(matches!(
                action(&owned(&[flag])).expect("sole version flag"),
                ProgramAction::Version
            ));
        }

        // 带了第二个参数就不再是帮助/版本，而是「未知子命令」。
        for extra in [vec!["--help", "collect"], vec!["--version", "collect"]] {
            let error = action(&owned(&extra)).expect_err("flag with extra argument must fail");
            assert_eq!(error.exit_code(), 1);
            assert!(error.to_string().contains("未知子命令或选项"));
        }

        // collect 转交内层解析器。
        let request = owned(&["collect", "--since", "7", "--data-dir", "/tmp/x"]);
        let parsed = action(&request).expect("collect delegates to the request parser");
        assert!(matches!(
            parsed,
            ProgramAction::Collect(CollectRequest {
                since: 7,
                data_dir: Some(path),
                snapshot: false,
            }) if path == Path::new("/tmp/x")
        ));

        let unknown = action(&owned(&["frobnicate"])).expect_err("unknown subcommand must fail");
        assert_eq!(unknown.exit_code(), 1);
        assert!(unknown.to_string().contains("未知子命令或选项"));

        let empty = action(&[]).expect_err("no arguments must fail");
        assert_eq!(empty.exit_code(), 1);
        assert!(empty.to_string().contains("缺少 collect 子命令"));

        let invalid = action(&[non_utf8_arg()]).expect_err("non-UTF-8 argument must fail");
        assert_eq!(invalid.exit_code(), 1);
        assert!(invalid.to_string().contains("有效的 UTF-8"));
    }

    #[test]
    fn collector_interrupted_snapshot_removes_partial_file() {
        let (_temp, directory, _manifest) = fixture_directory();
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let target = snapshot_dir.path().join("interrupted.db");
        let error = snapshot_database_with(
            &directory.join("opencode.db"),
            &target,
            &abundant_space,
            &|_, path| {
                fs::write(path, b"partial snapshot").expect("write injected partial snapshot");
                Err(CollectorError::Interrupted)
            },
        )
        .expect_err("injected interruption must fail");

        assert_eq!(error.exit_code(), 1);
        assert!(!target.exists());
        assert!(directory_entries(snapshot_dir.path()).is_empty());
    }

    #[test]
    fn collector_help_and_version_actions_finish_without_touching_a_data_source() {
        assert!(run_action(ProgramAction::Help).is_ok());
        assert!(run_action(ProgramAction::Version).is_ok());
    }

    #[test]
    fn collector_program_exit_code_reports_errors_and_keeps_success_silent() {
        let mut stderr = Vec::new();
        assert_eq!(program_exit_code(Ok(()), &mut stderr), 0);
        assert!(stderr.is_empty());

        let exit = program_exit_code(
            Err(CollectorError::NoDataDirectory("missing source".into())),
            &mut stderr,
        );
        assert_eq!(exit, 2);
        let message = String::from_utf8(stderr).expect("diagnostic is UTF-8");
        assert!(message.contains("missing source"));
        assert!(message.contains("--data-dir"));
    }

    #[test]
    fn collector_error_variants_keep_stable_exit_classes_and_actionable_messages() {
        let cases = [
            (
                CollectorError::Snapshot("snapshot detail".into()),
                1,
                "snapshot detail",
            ),
            (
                CollectorError::Identity("identity detail".into()),
                1,
                "identity detail",
            ),
            (
                CollectorError::Output("output detail".into()),
                1,
                "output detail",
            ),
            (CollectorError::Interrupted, 1, "已被中断"),
        ];

        for (error, expected_exit, expected_message) in cases {
            assert_eq!(error.exit_code(), expected_exit);
            assert!(error.to_string().contains(expected_message));
        }
    }

    #[test]
    fn collector_collect_parser_rejects_missing_duplicate_and_non_utf8_values_precisely() {
        let text_cases = [
            (vec!["--since", "1"], "缺少 collect 子命令"),
            (vec!["collect", "--since"], "--since 缺少值"),
            (vec!["collect", "--data-dir"], "--data-dir 缺少值"),
            (
                vec!["collect", "--request-base64url"],
                "--request-base64url 缺少值",
            ),
            (
                vec!["collect", "--since", "1", "--since", "2"],
                "--since 不能重复指定",
            ),
            (
                vec!["collect", "--data-dir", "a", "--data-dir", "b"],
                "--data-dir 不能重复指定",
            ),
            (
                vec![
                    "collect",
                    "--request-base64url",
                    "e30",
                    "--request-base64url",
                    "e30",
                ],
                "--request-base64url 不能重复指定",
            ),
            (
                vec!["collect", "--since", "1", "--snapshot", "--snapshot"],
                "--snapshot 不能重复指定",
            ),
        ];
        for (values, message) in text_cases {
            let error = parse_collect_request(&arguments(&values))
                .expect_err("invalid collect arguments must fail");
            assert!(error.to_string().contains(message));
        }

        let non_utf8_cases = [
            vec![OsString::from("collect"), non_utf8_arg()],
            vec![
                OsString::from("collect"),
                OsString::from("--since"),
                non_utf8_arg(),
            ],
            vec![
                OsString::from("collect"),
                OsString::from("--request-base64url"),
                non_utf8_arg(),
            ],
        ];
        for values in non_utf8_cases {
            let error =
                parse_collect_request(&values).expect_err("non-UTF-8 collect argument must fail");
            assert!(error.to_string().contains("有效的 UTF-8"));
        }
    }

    #[test]
    fn collector_encoded_request_validates_schema_since_and_precedence() {
        let encode = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        };
        let invalid_cases = [
            (
                encode(serde_json::json!({"since": 1, "unexpected": true})),
                "unknown field",
            ),
            (encode(serde_json::json!({"since": -1})), "不能为负数"),
            (
                encode(serde_json::json!({"snapshot": true})),
                "缺少 --since",
            ),
        ];
        for (payload, message) in invalid_cases {
            let error = parse_collect_request(&[
                OsString::from("collect"),
                OsString::from("--request-base64url"),
                OsString::from(payload),
            ])
            .expect_err("invalid encoded request must fail");
            assert!(error.to_string().contains(message));
        }

        let payload = encode(serde_json::json!({"since": 9, "snapshot": false}));
        let parsed = parse_collect_request(&arguments(&[
            "collect",
            "--since",
            "1",
            "--snapshot",
            "--request-base64url",
            &payload,
        ]))
        .expect("encoded fields override command-line fields");
        assert_eq!(parsed.since, 9);
        assert!(!parsed.snapshot);
    }

    #[test]
    fn collector_maps_discovery_failure_separately_from_other_source_failures() {
        let missing = map_source_error(OpenCodeError::DatabaseNotFound {
            probed_paths: vec![PathBuf::from("missing.db")],
        });
        assert_eq!(missing.exit_code(), 2);
        assert!(missing.to_string().contains("missing.db"));

        let unreadable = map_source_error(OpenCodeError::InvalidBatchSize);
        assert_eq!(unreadable.exit_code(), 4);
        assert!(unreadable.to_string().contains("batch_size"));
    }

    #[test]
    fn collector_scan_completion_rejects_interruption_partial_scan_and_wrong_source() {
        use agentlens_core::source::opencode::{ScanSkipReason, SkippedBreakdown};

        let complete = ScanResult {
            delivered_records: 0,
            delivered_batches: 0,
            eligible_count: 0,
            skipped_count: 0,
            skipped_breakdown: SkippedBreakdown::default(),
            observed_max_time_updated: Some(9),
            reached_eof: true,
            busy_retry_count: 0,
            last_success_utc: None,
            skip_reason: None,
        };
        let snapshot_request = CollectRequest {
            since: 0,
            data_dir: None,
            snapshot: true,
        };
        SNAPSHOT_INTERRUPTED.store(true, Ordering::SeqCst);
        let interrupted = validate_scan_completion(&snapshot_request, &complete, &[])
            .expect_err("interrupted snapshot must not publish partial data");
        SNAPSHOT_INTERRUPTED.store(false, Ordering::SeqCst);
        assert!(matches!(interrupted, CollectorError::Interrupted));

        let mut partial = complete.clone();
        partial.reached_eof = false;
        partial.skip_reason = Some(ScanSkipReason::Busy);
        let partial_error = validate_scan_completion(
            &CollectRequest {
                snapshot: false,
                ..snapshot_request.clone()
            },
            &partial,
            &[],
        )
        .expect_err("scan that did not reach EOF must fail");
        assert_eq!(partial_error.exit_code(), 4);
        assert!(partial_error.to_string().contains("Busy"));

        let (_temp, directory, _manifest) = fixture_directory();
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let collection = collect_with_space_probe(
            &request(&directory, false),
            &identity(),
            "fixture-host",
            snapshot_dir.path(),
            &abundant_space,
        )
        .expect("collect a valid record for source validation");
        let mut wrong_source = collection.records[0].clone();
        wrong_source.source = "not-opencode".into();
        let source_error =
            validate_scan_completion(&request(&directory, false), &complete, &[wrong_source])
                .expect_err("record source must match collector source");
        assert_eq!(source_error.exit_code(), 4);
        assert!(source_error.to_string().contains("错误的 source"));
    }

    #[test]
    fn collector_snapshot_refuses_overwrite_and_cleans_success_interruption_and_missing_files() {
        let (_temp, directory, _manifest) = fixture_directory();
        let database = directory.join(DATABASE_FILE);
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");

        let existing = snapshot_dir.path().join("existing.db");
        fs::write(&existing, b"keep me").expect("write existing target");
        let error = snapshot_database_with(
            &database,
            &existing,
            &abundant_space,
            &vacuum_into_read_only_source,
        )
        .expect_err("snapshot must not overwrite an existing target");
        assert_eq!(error.exit_code(), 1);
        assert!(error.to_string().contains("拒绝覆盖"));
        assert_eq!(
            fs::read(&existing).expect("read preserved target"),
            b"keep me"
        );

        let completed = snapshot_dir.path().join("completed.db");
        let snapshot =
            snapshot_database_with(&database, &completed, &abundant_space, &|source, target| {
                fs::copy(source, target)
                    .map(|_| ())
                    .map_err(|error| CollectorError::Snapshot(error.to_string()))
            })
            .expect("create injected snapshot");
        let debug = format!("{snapshot:?}");
        assert!(debug.contains("SnapshotDatabase"));
        assert!(debug.contains(&completed.to_string_lossy().into_owned()));
        drop(snapshot);
        assert!(!completed.exists());

        let interrupted = snapshot_dir.path().join("interrupted-after-vacuum.db");
        let error =
            snapshot_database_with(&database, &interrupted, &abundant_space, &|_, target| {
                fs::write(target, b"partial").expect("write partial snapshot");
                SNAPSHOT_INTERRUPTED.store(true, Ordering::SeqCst);
                Ok(())
            })
            .expect_err("interruption after vacuum must discard the snapshot");
        assert!(matches!(error, CollectorError::Interrupted));
        assert!(!interrupted.exists());

        let absent = snapshot_dir.path().join("already-absent.db");
        drop(SnapshotCleanup {
            path: absent.clone(),
        });
        assert!(!absent.exists());

        let directory_cleanup = snapshot_dir.path().join("directory-cleanup");
        fs::create_dir(&directory_cleanup).expect("create non-file cleanup target");
        drop(SnapshotCleanup {
            path: directory_cleanup.clone(),
        });
        assert!(directory_cleanup.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn collector_snapshot_rejects_non_utf8_target_and_formats_source_errors() {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

        let (_temp, directory, _manifest) = fixture_directory();
        let snapshot_dir = tempfile::tempdir().expect("snapshot tempdir");
        let mut target = snapshot_dir.path().as_os_str().as_bytes().to_vec();
        target.extend_from_slice(b"/snapshot-\xff.db");
        let target = PathBuf::from(OsString::from_vec(target));
        let error = vacuum_into_read_only_source(&directory.join(DATABASE_FILE), &target)
            .expect_err("non-UTF-8 target cannot be passed to SQLite");
        assert_eq!(error.exit_code(), 1);
        assert!(error.to_string().contains("不是有效 UTF-8"));

        let error = snapshot_source_error(Path::new("source.db"), rusqlite::Error::InvalidQuery);
        assert_eq!(error.exit_code(), 4);
        let message = error.to_string();
        assert!(message.contains("source.db"));
        assert!(message.contains("chmod"));
        assert!(message.contains("snapshot/VACUUM"));
    }

    #[test]
    fn collector_df_output_maps_process_failures_and_parses_only_the_last_data_line() {
        let path = Path::new("snapshot-dir");
        let spawn_error = free_space_from_df_output(
            path,
            Err(io::Error::new(io::ErrorKind::NotFound, "df missing")),
        )
        .expect_err("spawn failure must be reported");
        assert!(spawn_error.to_string().contains("df missing"));
        assert!(spawn_error.to_string().contains("snapshot-dir"));

        let status_error = free_space_from_df_output(
            path,
            Ok(Output {
                status: exit_status(7),
                stdout: Vec::new(),
                stderr: b"df denied\n".to_vec(),
            }),
        )
        .expect_err("non-zero df status must be reported");
        assert!(status_error.to_string().contains("df denied"));

        let available = free_space_from_df_output(
            path,
            Ok(Output {
                status: exit_status(0),
                stdout: b"Filesystem 1024-blocks Used Available Capacity Mounted\n/dev/x 100 58 42 58% /\n\n"
                    .to_vec(),
                stderr: Vec::new(),
            }),
        )
        .expect("parse valid df output");
        assert_eq!(available, 42 * 1024);

        let saturated = free_space_from_df_output(
            path,
            Ok(Output {
                status: exit_status(0),
                stdout: format!("/dev/x 1 1 {} 1% /\n", u64::MAX).into_bytes(),
                stderr: Vec::new(),
            }),
        )
        .expect("large df values saturate instead of wrapping");
        assert_eq!(saturated, u64::MAX);
    }

    #[test]
    fn collector_df_output_rejects_non_utf8_empty_short_and_non_numeric_data() {
        let cases = [
            (vec![0xff], "不是 UTF-8"),
            (Vec::new(), "没有输出"),
            (b"too short\n".to_vec(), "无法解析 df 输出"),
            (b"/dev/x 1 1 nope 1% /\n".to_vec(), "无法解析 df 可用空间"),
        ];
        for (stdout, expected) in cases {
            let error = free_space_from_df_output(
                Path::new("."),
                Ok(Output {
                    status: exit_status(0),
                    stdout,
                    stderr: Vec::new(),
                }),
            )
            .expect_err("malformed df output must fail");
            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn collector_local_hostname_always_returns_a_nonempty_trimmed_value() {
        let hostname = local_hostname();
        assert!(!hostname.is_empty());
        assert_eq!(hostname, hostname.trim());
    }

    #[cfg(unix)]
    #[test]
    fn collector_signal_handler_marks_snapshot_interrupted() {
        SNAPSHOT_INTERRUPTED.store(false, Ordering::SeqCst);
        snapshot_signal::mark_interrupted(2);
        assert!(snapshot_interrupted());
        SNAPSHOT_INTERRUPTED.store(false, Ordering::SeqCst);
    }
}
