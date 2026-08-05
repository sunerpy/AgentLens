//! 真实库只读烟测与性能门槛（todo 21）。
//!
//! 本套件默认 `#[ignore]`，只在显式请求时运行：
//!
//! ```text
//! cargo test --release -p agentlens-core -- --ignored real_db_
//! ```
//!
//! 三条硬约束：
//!
//! 1. **只读**。源库（`/config/.local/share/opencode/opencode.db`，实测 ~43 GB）永不被写入。
//!    连接一律 `mode=ro` + `PRAGMA query_only=ON`；SQLite 自管的 `-wal`/`-shm` 句柄是唯一
//!    允许的写句柄，这是 WAL 模式的固有行为，也是计划文档化的唯一豁免。
//! 2. **基线动态化**。源库正被 OpenCode 持续写入，因此不硬编码任何行数、SHA 或时间戳。
//!    基线在**同一个 WAL 读快照内**取得：`SnapshotSourceConnection` 先 `BEGIN DEFERRED`，
//!    再用同一连接读 eligible 计数、去重 `message_id` 计数与完整 id 集合，随后把**这同一个
//!    快照**流给扫描器。因此「归档行数 == 该快照 eligible」是精确等式，不是竞态断言。
//! 3. **legacy 回填全程关闭**。只测 live DB 路径：`LocalHostSource` 的定时轮次恒为
//!    `Origin::Live`（`INCREMENTAL_ORIGIN`），本文件不调用任何 `backfill_legacy*` /
//!    `import_backup_databases`，也不读 `tokens.total` 或 `session.tokens_*` 预聚合列。
//!
//! 归档一律写进 `tempfile::tempdir()`，测试结束即删除。

#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

use agentlens_core::archive::Archive;
use agentlens_core::host::{HostRecord, HostRegistry, MachineIdentity};
use agentlens_core::hostsource::{
    set_archive_busy_timeout, HostSource, LocalHostSource, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS,
};
use agentlens_core::ingest::{read_cursor, IngestRound, OPENCODE_SOURCE};
use agentlens_core::source::opencode::{
    discover_database_path, scan_connection, OpenCodeError, ScanRequest, ScanSkipReason, SinkError,
    SourceConnection, SourceMessageRow, SqliteSourceConnection, StreamError, OVERLAP_WINDOW_MS,
};

/// 首轮全量回填预算（计划门槛）。实测全表 ~23 s，留约 13x 余量。
const FIRST_ROUND_BUDGET: Duration = Duration::from_secs(300);

/// 稳态增量轮预算（计划门槛）。实测 24 h 窗口 ~0.9 s，留约 33x 余量。
const INCREMENTAL_BUDGET: Duration = Duration::from_secs(30);

/// 与 `parse_message` 的 eligible 判定逐字等价的 SQL 谓词：
/// JSON 可解析、`role='assistant'`、`tokens` 是 object。
const ELIGIBLE_PREDICATE: &str = "json_valid(data) \
AND json_extract(data,'$.role')='assistant' AND json_type(data,'$.tokens')='object'";

/// 生产扫描器使用的逐字 SQL（`source::opencode` 私有常量的复刻，无 `ORDER BY`/`LIMIT`）。
const MESSAGE_QUERY: &str =
    "SELECT id, session_id, time_created, time_updated, data FROM message WHERE time_updated >= ?1";

// ---------------------------------------------------------------------------
// 单快照源连接：让「基线」与「被扫描的行」来自同一个 WAL 读快照
// ---------------------------------------------------------------------------

/// 同一 WAL 读快照内既提供动态基线、又提供扫描行的只读 [`SourceConnection`]。
///
/// 源库正被 OpenCode 追加写入并就地 bump `time_updated`，所以任何「先在连接 A 数一次、
/// 再用连接 B 扫描」的写法都必然漂移（实测同一个 `sqlite3` 进程内三条独立 SELECT 就已经
/// 给出 155042 / 155046 / 155047 三个不同的数）。这里改为：`BEGIN DEFERRED` 之后的**第一条
/// 读语句**钉住快照，此后同一事务内的全部读——包括交给扫描器的行流——都看到同一份数据。
struct SnapshotSourceConnection {
    connection: Connection,
    path: PathBuf,
    rows_streamed: u64,
    /// `Some(n)` 时在第 n 行后主动中断，用于验证整轮回滚。
    interrupt_after_rows: Option<u64>,
}

/// 同一快照内取得的动态基线。
#[derive(Debug)]
struct SnapshotBaseline {
    eligible_count: u64,
    distinct_message_ids: u64,
    /// eligible 行的 `max(time_updated)`。
    max_eligible_time_updated: i64,
    /// **全部**行的 `max(time_updated)`：扫描器的 `observed_max_time_updated` 在 parse 之前
    /// 逐行更新，因此 watermark 对齐的是这个值，而不是 eligible 子集的最大值。
    max_row_time_updated: i64,
    ids: BTreeSet<String>,
}

impl SnapshotSourceConnection {
    /// 以 `mode=ro` + `query_only=ON` 打开源库，并立即进入延迟读事务。
    fn open(path: &Path) -> Self {
        let uri = format!("file:{}?mode=ro", path.display());
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("以 mode=ro 打开真实源库");
        connection
            .pragma_update(None, "query_only", true)
            .expect("启用 PRAGMA query_only");
        connection
            .busy_timeout(Duration::from_millis(5_000))
            .expect("设置 busy_timeout");
        connection
            .execute_batch("BEGIN DEFERRED")
            .expect("开启只读快照事务");
        Self {
            connection,
            path: path.to_path_buf(),
            rows_streamed: 0,
            interrupt_after_rows: None,
        }
    }

    /// 在快照内读取 eligible 计数、去重 `message_id` 计数与完整 id 集合。
    ///
    /// 这是本套件唯一的基线来源，全部数字都是运行时测得，无任何硬编码等式。
    fn baseline(&self) -> SnapshotBaseline {
        let summary_sql = format!(
            "SELECT count(*), count(DISTINCT id), coalesce(max(time_updated), 0) \
             FROM message WHERE {ELIGIBLE_PREDICATE}"
        );
        let (eligible_count, distinct_message_ids, max_eligible_time_updated) = self
            .connection
            .query_row(&summary_sql, [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .expect("读取快照内 eligible 基线");
        let max_row_time_updated = self
            .connection
            .query_row(
                "SELECT coalesce(max(time_updated), 0) FROM message",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("读取快照内全部行的 max(time_updated)");

        let ids_sql = format!("SELECT id FROM message WHERE {ELIGIBLE_PREDICATE}");
        let mut statement = self.connection.prepare(&ids_sql).expect("准备 id 集合查询");
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("读取快照内 eligible id 集合")
            .map(|id| id.expect("解码 message_id"))
            .collect::<BTreeSet<String>>();

        SnapshotBaseline {
            eligible_count: eligible_count.unsigned_abs(),
            distinct_message_ids: distinct_message_ids.unsigned_abs(),
            max_eligible_time_updated,
            max_row_time_updated,
            ids,
        }
    }

    /// 在第 `rows` 行之后以 [`StreamError::Interrupted`] 中断，模拟被打断的一轮。
    fn interrupt_after(mut self, rows: u64) -> Self {
        self.interrupt_after_rows = Some(rows);
        self
    }

    /// 结束读事务。只读事务用 `COMMIT` 与 `ROLLBACK` 等价，这里不产生任何写入。
    fn finish(self) {
        self.connection
            .execute_batch("COMMIT")
            .expect("结束只读快照事务");
    }
}

impl SourceConnection for SnapshotSourceConnection {
    fn query_only(&self) -> rusqlite::Result<bool> {
        self.connection
            .pragma_query_value(None, "query_only", |row| row.get::<_, i32>(0))
            .map(|value| value == 1)
    }

    fn stream_messages(
        &mut self,
        window_start: i64,
        visitor: &mut dyn FnMut(SourceMessageRow) -> Result<(), StreamError>,
    ) -> Result<(), StreamError> {
        let mut statement = self.connection.prepare(MESSAGE_QUERY)?;
        let mut rows = statement.query([window_start])?;
        while let Some(row) = rows.next()? {
            if let Some(limit) = self.interrupt_after_rows {
                if self.rows_streamed >= limit {
                    return Err(StreamError::Interrupted(format!(
                        "注入中断：已流出 {} 行（源库 {}）",
                        self.rows_streamed,
                        self.path.display()
                    )));
                }
            }
            self.rows_streamed += 1;
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

// ---------------------------------------------------------------------------
// 进程侧只读审计
// ---------------------------------------------------------------------------

/// 本进程持有的一个与源库相关的文件句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenHandle {
    path: String,
    /// `lsof` 的 `r`（只读）或 `/proc` fdinfo 的 `O_RDONLY`。
    read_only: bool,
    /// 原始模式标记，进 evidence 用。
    mode: String,
}

/// 审计方式，写进报告以便复现。
#[derive(Debug, PartialEq, Eq)]
enum AuditMethod {
    Lsof,
    ProcFd,
}

/// 用 `lsof -w -p <pid>` 审计本进程句柄。
///
/// PID 取自 [`std::process::id`]——**绝不能**用子 shell 的 `$$`，那取到的是 shell 自己的 PID。
fn lsof_handles(pid: u32, database: &Path) -> Option<Vec<OpenHandle>> {
    let output = Command::new("lsof")
        .args(["-w", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return None;
    }
    let prefix = database.to_string_lossy().into_owned();
    let mut handles = Vec::new();
    for line in stdout.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 9 {
            continue;
        }
        let name = fields[8];
        if !name.starts_with(&prefix) {
            continue;
        }
        let fd = fields[3];
        let mode = fd
            .chars()
            .last()
            .filter(|character| matches!(character, 'r' | 'w' | 'u'))
            .map_or_else(|| fd.to_owned(), |character| character.to_string());
        handles.push(OpenHandle {
            path: name.to_owned(),
            read_only: mode == "r",
            mode,
        });
    }
    Some(handles)
}

/// `lsof` 缺失时的回退：解析 `/proc/self/fd` 与 `/proc/self/fdinfo` 的访问模式位。
fn proc_fd_handles(database: &Path) -> Vec<OpenHandle> {
    let prefix = database.to_string_lossy().into_owned();
    let mut handles = Vec::new();
    for entry in fs::read_dir("/proc/self/fd").expect("读取 /proc/self/fd") {
        let entry = entry.expect("读取 fd 条目");
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let target_text = target.to_string_lossy().into_owned();
        if !target_text.starts_with(&prefix) {
            continue;
        }
        let fdinfo = fs::read_to_string(Path::new("/proc/self/fdinfo").join(entry.file_name()))
            .expect("读取 fdinfo");
        let flags = fdinfo
            .lines()
            .find_map(|line| line.strip_prefix("flags:\t"))
            .map(str::trim)
            .and_then(|value| u32::from_str_radix(value, 8).ok())
            .expect("解析 fd flags");
        let read_only = flags & 0b11 == 0;
        handles.push(OpenHandle {
            path: target_text,
            read_only,
            mode: if read_only {
                "r".to_owned()
            } else {
                "w/u".to_owned()
            },
        });
    }
    handles
}

/// 优先 `lsof`，缺失时回退 `/proc/self/fd`，并返回实际使用的方法。
fn audit_handles(database: &Path) -> (AuditMethod, Vec<OpenHandle>) {
    let pid = std::process::id();
    match lsof_handles(pid, database) {
        Some(handles) => (AuditMethod::Lsof, handles),
        None => (AuditMethod::ProcFd, proc_fd_handles(database)),
    }
}

/// SQLite 自管 sidecar 路径，唯一允许出现写模式句柄的两个文件。
fn sidecar(database: &Path, suffix: &str) -> String {
    format!("{}{suffix}", database.display())
}

/// 源库所在目录的顶层条目快照。
fn directory_entries(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .expect("读取源库目录")
        .map(|entry| {
            entry
                .expect("读取源库目录条目")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// 断言除 SQLite 管理的 `-shm`/`-wal` 外没有新建文件。
fn assert_no_new_source_files(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Vec<String> {
    let unexpected = after
        .difference(before)
        .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "源库目录出现了非 SQLite 管理的新文件：{unexpected:?}"
    );
    unexpected
}

// ---------------------------------------------------------------------------
// 归档侧脚手架
// ---------------------------------------------------------------------------

/// 临时归档 + 已注册的本机 host，全部落在 tempdir。
struct Harness {
    archive: Archive,
    host_id: String,
    _temp: tempfile::TempDir,
}

impl Harness {
    fn new(label: &str) -> Self {
        let temp = tempfile::tempdir().expect("创建归档 tempdir");
        let archive = Archive::open_in_data_dir(temp.path()).expect("打开临时归档");
        set_archive_busy_timeout(&archive, DEFAULT_ARCHIVE_BUSY_TIMEOUT_MS)
            .expect("配置归档 busy_timeout");
        let identity = MachineIdentity::from_machine_id(label).expect("构造 QA machine identity");
        let host = HostRecord::local("本机（real_db QA）", &identity);
        HostRegistry::new(archive.connection())
            .insert(&host)
            .expect("注册 host 行");
        let host_id = host.host_id().to_owned();
        Self {
            archive,
            host_id,
            _temp: temp,
        }
    }

    fn archived_ids(&self) -> BTreeSet<String> {
        let mut statement = self
            .archive
            .connection()
            .prepare("SELECT message_id FROM usage_record WHERE host_id = ?1 AND source = ?2")
            .expect("准备归档 message_id 查询");
        statement
            .query_map(rusqlite::params![&self.host_id, OPENCODE_SOURCE], |row| {
                row.get::<_, String>(0)
            })
            .expect("读取归档 message_id")
            .map(|id| id.expect("解码归档 message_id"))
            .collect()
    }

    fn archived_rows(&self) -> u64 {
        self.archive
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_record WHERE host_id = ?1 AND source = ?2",
                rusqlite::params![&self.host_id, OPENCODE_SOURCE],
                |row| row.get::<_, i64>(0),
            )
            .expect("统计归档行数")
            .unsigned_abs()
    }

    fn cursor(&self) -> Option<i64> {
        read_cursor(self.archive.connection(), &self.host_id).expect("读取 live cursor")
    }

    /// 显式关闭归档并删除 tempdir，打印清理凭据。
    fn close(self) {
        let directory = self._temp.path().to_path_buf();
        drop(self.archive);
        self._temp.close().expect("删除归档 tempdir");
        assert!(!directory.exists(), "归档 tempdir 必须已被删除");
        println!("清理凭据       : removed {}", directory.display());
    }
}

/// 外部 `sqlite3` 二进制的四数对账（`ATTACH ... mode=ro`，外部进程同样无法写源库）。
#[derive(Debug)]
struct ExternalReconciliation {
    source_eligible: u64,
    archived: u64,
    archived_not_in_source: u64,
    source_not_archived: u64,
}

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
    let output = Command::new("sqlite3")
        .arg(archive_path)
        .arg(&script)
        .output()
        .expect("调用外部 sqlite3 二进制");
    assert!(
        output.status.success(),
        "外部 sqlite3 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let fields = text
        .trim()
        .split('|')
        .map(|field| field.parse::<u64>().expect("解析 sqlite3 计数"))
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 4, "外部 sqlite3 输出异常：{text:?}");
    ExternalReconciliation {
        source_eligible: fields[0],
        archived: fields[1],
        archived_not_in_source: fields[2],
        source_not_archived: fields[3],
    }
}

/// 定位真实源库；本机没有时明确跳过而不是假绿。
fn locate_source_database() -> Option<PathBuf> {
    match discover_database_path() {
        Ok(path) => Some(path),
        Err(error) => {
            println!("SKIP：本机没有可发现的 OpenCode 数据库：{error}");
            None
        }
    }
}

fn source_size_bytes(path: &Path) -> u64 {
    fs::metadata(path).expect("stat 源库").len()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 打印计时报告并断言预算。余量同时以倍数与秒数给出。
fn assert_within_budget(label: &str, elapsed: Duration, budget: Duration) {
    let headroom = budget.as_secs_f64() / elapsed.as_secs_f64();
    println!(
        "计时门槛 {label:<12} elapsed={:.3}s budget={:.3}s headroom={headroom:.1}x (剩余 {:.3}s)",
        elapsed.as_secs_f64(),
        budget.as_secs_f64(),
        budget.as_secs_f64() - elapsed.as_secs_f64()
    );
    assert!(
        elapsed < budget,
        "{label} 超出性能门槛：elapsed={:.3}s >= budget={:.3}s",
        elapsed.as_secs_f64(),
        budget.as_secs_f64()
    );
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 首轮全量回填：同一快照内取动态基线，断言归档 == 该快照 eligible，且 < 300 s。
#[test]
#[ignore = "真实库烟测：显式 cargo test --release -p agentlens-core -- --ignored real_db_ 运行"]
fn real_db_first_round_matches_snapshot_eligible_within_backfill_budget() {
    let Some(database) = locate_source_database() else {
        return;
    };
    let directory = database.parent().expect("源库父目录");
    let entries_before = directory_entries(directory);
    let size_before = source_size_bytes(&database);
    println!("源库路径       : {}", database.display());
    println!("源库大小       : {size_before} bytes（mode=ro，query_only=ON，全程未写）");

    let mut harness = Harness::new("agentlens-real-db-first-round");
    println!("临时归档       : {}", harness.archive.path().display());
    println!("host_id        : {}", harness.host_id);
    assert_eq!(harness.cursor(), None, "新建归档不应带有 live cursor");

    // 基线与被扫描的行来自同一个 WAL 读快照，因此下面的等式是精确的，不是竞态断言。
    let mut source = SnapshotSourceConnection::open(&database);
    assert!(
        source.query_only().expect("读取 query_only"),
        "源连接必须 query_only=1"
    );
    let baseline = source.baseline();
    println!(
        "动态基线       : eligible={} distinct_message_id={} max_time_updated(eligible)={} max_time_updated(all_rows)={}",
        baseline.eligible_count,
        baseline.distinct_message_ids,
        baseline.max_eligible_time_updated,
        baseline.max_row_time_updated
    );
    assert_eq!(
        baseline.eligible_count, baseline.distinct_message_ids,
        "eligible 集合内 message_id 必须已去重"
    );
    assert_eq!(
        u64::try_from(baseline.ids.len()).expect("id 集合大小"),
        baseline.eligible_count,
        "id 集合大小必须等于快照 eligible 计数"
    );
    assert!(
        baseline.eligible_count > 0,
        "真实库应至少有一条 eligible 行"
    );

    let local = LocalHostSource::with_database(harness.host_id.as_str(), &database);
    let started = Instant::now();
    let outcome = local
        .collect_from_connection(&mut harness.archive, &mut source, now_ms())
        .expect("首轮全量回填");
    let elapsed = started.elapsed();
    source.finish();

    println!(
        "首轮结果       : reached_eof={} eligible={} skipped={} received={} changed={} committed={} cursor={:?}",
        outcome.reached_eof,
        outcome.eligible_count,
        outcome.skipped_count,
        outcome.stats.received_records,
        outcome.stats.changed_records,
        outcome.stats.committed,
        outcome.stats.cursor_time_updated
    );
    assert!(outcome.is_success(), "首轮必须 EOF 且提交");
    assert_within_budget("首轮全量回填", elapsed, FIRST_ROUND_BUDGET);

    // 归档行数 == 该快照 eligible（精确等式）。
    let archived = harness.archived_rows();
    assert_eq!(
        archived, baseline.eligible_count,
        "归档行数必须等于同一快照的 eligible 计数"
    );
    assert_eq!(
        outcome.eligible_count, baseline.eligible_count,
        "扫描器 eligible 必须等于同一快照的 eligible 计数"
    );
    // 集合相等比计数相等更强：证明是同一批 message_id，而不是数量凑巧一致。
    assert_eq!(
        harness.archived_ids(),
        baseline.ids,
        "归档 message_id 集合必须与快照 eligible 集合完全一致"
    );
    assert_eq!(
        outcome.stats.cursor_time_updated,
        Some(baseline.max_row_time_updated),
        "cursor 必须推进到快照内全部行的 max(time_updated)"
    );
    assert!(
        baseline.max_row_time_updated >= baseline.max_eligible_time_updated,
        "全部行的 max(time_updated) 不可能小于 eligible 子集的最大值"
    );
    println!(
        "对账（快照内） : archived={archived} snapshot_eligible={} 集合相等=是",
        baseline.eligible_count
    );

    // 外部 sqlite3 交叉核对。此处对的是**当下**的源库快照（进程外，必然更新），
    // 所以只能断言方向性事实：我方从不凭空造行；扫描后新增的行以差额呈现。
    let external = external_reconciliation(harness.archive.path(), &database, &harness.host_id);
    println!(
        "对账（外部）   : archived={} source_eligible={} archived_not_in_source={} source_not_archived={}",
        external.archived,
        external.source_eligible,
        external.archived_not_in_source,
        external.source_not_archived
    );
    assert_eq!(
        external.archived, archived,
        "外部二进制必须看到同样的归档行数"
    );
    assert_eq!(
        external.archived_not_in_source, 0,
        "归档中不得存在源库没有的 message_id"
    );
    assert_eq!(
        external.archived + external.source_not_archived,
        external.source_eligible,
        "归档行 + 扫描快照后新增行 必须与当下源库 eligible 对账"
    );
    println!(
        "结论           : {} 已归档 + {} 扫描快照后新增 = 当下 {} eligible",
        external.archived, external.source_not_archived, external.source_eligible
    );

    let entries_after = directory_entries(directory);
    let unexpected = assert_no_new_source_files(&entries_before, &entries_after);
    println!(
        "只读收尾       : size_before={size_before} size_after={} unexpected_files={unexpected:?}",
        source_size_bytes(&database)
    );
    harness.close();
}

/// 稳态增量轮：先用有界一轮建立 cursor，再走生产路径 `collect_incremental`，断言 < 30 s。
#[test]
#[ignore = "真实库烟测：显式 cargo test --release -p agentlens-core -- --ignored real_db_ 运行"]
fn real_db_incremental_round_meets_steady_state_budget() {
    let Some(database) = locate_source_database() else {
        return;
    };
    let mut harness = Harness::new("agentlens-real-db-incremental");
    let source = LocalHostSource::with_database(harness.host_id.as_str(), &database);

    // 播种轮：watermark=now → 窗口为 now-24h，等价于生产稳态的查询范围。
    let seed_request = ScanRequest::live(harness.host_id.as_str(), Some(now_ms()));
    let seed_started = Instant::now();
    let (seed_scan, seed_stats) = run_direct_round(&mut harness, &database, &seed_request);
    let seed_elapsed = seed_started.elapsed();
    println!(
        "播种轮         : window_start={} reached_eof={} eligible={} skipped={} committed={} cursor={:?} elapsed={:.3}s",
        seed_request.window_start(),
        seed_scan.reached_eof,
        seed_scan.eligible_count,
        seed_scan.skipped_count,
        seed_stats.committed,
        seed_stats.cursor_time_updated,
        seed_elapsed.as_secs_f64()
    );
    assert!(
        seed_scan.reached_eof && seed_stats.committed,
        "播种轮必须 EOF 且提交"
    );
    let seeded_cursor = harness
        .cursor()
        .expect("播种轮必须写入 live cursor（源库近 24h 应有活动）");

    // 稳态轮走生产路径：自行打开 mode=ro 连接，窗口取 cursor-24h。
    let started = Instant::now();
    let outcome = source
        .collect_incremental(&mut harness.archive, now_ms())
        .expect("稳态增量轮");
    let elapsed = started.elapsed();
    println!(
        "稳态增量轮     : window_start={} reached_eof={} eligible={} skipped={} changed={} cursor={:?} elapsed={:.3}s",
        seeded_cursor - OVERLAP_WINDOW_MS,
        outcome.reached_eof,
        outcome.eligible_count,
        outcome.skipped_count,
        outcome.stats.changed_records,
        outcome.stats.cursor_time_updated,
        elapsed.as_secs_f64()
    );
    assert!(outcome.is_success(), "稳态增量轮必须 EOF 且提交");
    assert_within_budget("稳态增量轮", elapsed, INCREMENTAL_BUDGET);

    let advanced = harness.cursor().expect("稳态轮后仍应有 cursor");
    assert!(advanced >= seeded_cursor, "增量轮不得让 watermark 回退");

    // 增量轮同样不得凭空造行。
    let external = external_reconciliation(harness.archive.path(), &database, &harness.host_id);
    println!(
        "对账（外部）   : archived={} source_eligible={} archived_not_in_source={} source_not_archived={}",
        external.archived,
        external.source_eligible,
        external.archived_not_in_source,
        external.source_not_archived
    );
    assert_eq!(
        external.archived_not_in_source, 0,
        "增量轮不得引入源库没有的 message_id"
    );
    assert_eq!(
        external.archived + external.source_not_archived,
        external.source_eligible
    );
    println!(
        "cursor 推进    : {seeded_cursor} -> {advanced}（+{}ms）",
        advanced - seeded_cursor
    );
    harness.close();
}

/// 进程侧只读铁证：`query_only=1`、连接仍打开时 `lsof -p <std::process::id()>` 无写句柄、
/// 目录内除 `-shm`/`-wal` 外零新建文件。
#[test]
#[ignore = "真实库烟测：显式 cargo test --release -p agentlens-core -- --ignored real_db_ 运行"]
fn real_db_read_only_process_audit_reports_no_write_handles() {
    let Some(database) = locate_source_database() else {
        return;
    };
    let directory = database.parent().expect("源库父目录");
    let entries_before = directory_entries(directory);
    let size_before = source_size_bytes(&database);

    let mut connection = SqliteSourceConnection::open(&database).expect("打开真实源库");
    assert!(
        connection.query_only().expect("读取 query_only"),
        "PRAGMA query_only 必须为 1"
    );

    // 先真正跑一小段扫描，确保 fd 已经被 SQLite 打开并使用过。
    let request = ScanRequest::live("host-real-db-audit", Some(now_ms()));
    let scan = scan_connection(&mut connection, &request, |_batch| Ok(())).expect("有界只读扫描");
    assert!(scan.reached_eof, "审计用的有界扫描应到达 EOF");

    // PID 取自本进程，绝不使用子 shell 的 $$。审计在源连接仍打开时进行。
    let pid = std::process::id();
    let (method, handles) = audit_handles(&database);
    println!("审计方式       : {method:?}（pid={pid}，源连接仍打开）");
    for handle in &handles {
        println!("句柄           : mode={} path={}", handle.mode, handle.path);
    }
    assert!(!handles.is_empty(), "源连接打开时必须能观察到句柄");

    let main_path = database.to_string_lossy().into_owned();
    let wal = sidecar(&database, "-wal");
    let shm = sidecar(&database, "-shm");
    assert!(
        handles
            .iter()
            .any(|handle| handle.path == main_path && handle.read_only),
        "主库句柄必须是只读：{handles:?}"
    );
    assert!(
        !handles
            .iter()
            .any(|handle| handle.path == main_path && !handle.read_only),
        "主库不得存在任何写模式（w/u）句柄：{handles:?}"
    );
    let writable = handles
        .iter()
        .filter(|handle| !handle.read_only)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        writable
            .iter()
            .all(|handle| handle.path == wal || handle.path == shm),
        "只允许 SQLite 自管的 -wal/-shm 出现写句柄：{writable:?}"
    );
    println!(
        "写模式句柄     : {:?}（WAL 模式固有行为，计划文档化的唯一豁免）",
        writable
            .iter()
            .map(|handle| &handle.path)
            .collect::<Vec<_>>()
    );

    drop(connection);
    let entries_after = directory_entries(directory);
    let unexpected = assert_no_new_source_files(&entries_before, &entries_after);
    let size_after = source_size_bytes(&database);
    println!(
        "目录审计       : before={} after={} unexpected={unexpected:?}",
        entries_before.len(),
        entries_after.len()
    );
    println!(
        "源库尺寸       : before={size_before} after={size_after}（差值由 OpenCode 自身写入产生，非我方）"
    );
}

/// 被打断的一轮：整轮回滚，cursor 不前进，重跑可完成。
#[test]
#[ignore = "真实库烟测：显式 cargo test --release -p agentlens-core -- --ignored real_db_ 运行"]
fn real_db_interrupted_round_keeps_cursor_and_reruns_clean() {
    let Some(database) = locate_source_database() else {
        return;
    };
    let mut harness = Harness::new("agentlens-real-db-interrupt");
    assert_eq!(harness.cursor(), None, "起点必须没有 cursor");

    // (1) 源侧中断：流出 10 行后抛 StreamError::Interrupted。
    let local = LocalHostSource::with_database(harness.host_id.as_str(), &database);
    let mut interrupted_source = SnapshotSourceConnection::open(&database).interrupt_after(10);
    let outcome = local
        .collect_from_connection(&mut harness.archive, &mut interrupted_source, now_ms())
        .expect("源侧中断应作为非 EOF 轮次返回，而不是 panic");
    interrupted_source.finish();
    println!(
        "源侧中断轮     : reached_eof={} committed={} received={} cursor={:?}",
        outcome.reached_eof,
        outcome.stats.committed,
        outcome.stats.received_records,
        outcome.stats.cursor_time_updated
    );
    assert!(!outcome.reached_eof, "中断轮不得报告 EOF");
    assert!(!outcome.stats.committed, "中断轮必须整轮回滚");
    assert_eq!(harness.archived_rows(), 0, "中断轮不得留下半轮数据");
    assert_eq!(harness.cursor(), None, "中断轮不得推进 cursor");

    // (2) sink 侧中断：走 todo 6 的 IngestRound 接线，让 sink 在第二个批次失败。
    let request = ScanRequest::live(harness.host_id.as_str(), Some(now_ms()));
    let mut source = SqliteSourceConnection::open(&database).expect("打开真实源库");
    let mut round = IngestRound::begin(
        harness.archive.connection_mut(),
        harness.host_id.clone(),
        request.origin,
    )
    .expect("开启中断实验轮");
    let mut batches = 0_u64;
    let scan = scan_connection(&mut source, &request, |batch| {
        batches += 1;
        if batches >= 2 {
            return Err(SinkError::new("注入 sink 失败：验证整轮回滚"));
        }
        round
            .ingest_batch(batch)
            .map_err(|error| SinkError::new(error.to_string()))
    })
    .expect("sink 中断应返回非 EOF 的 ScanResult");
    let stats = round.finish(&scan).expect("结束中断实验轮");
    println!(
        "sink 中断轮    : reached_eof={} skip_reason={:?} committed={} received={} observed_max={:?}",
        scan.reached_eof, scan.skip_reason, stats.committed, stats.received_records,
        scan.observed_max_time_updated
    );
    if batches >= 2 {
        assert!(!scan.reached_eof, "sink 中断不得报告 EOF");
        assert!(
            matches!(scan.skip_reason, Some(ScanSkipReason::Interrupted(_))),
            "sink 中断必须记录 Interrupted 原因"
        );
        assert!(!stats.committed, "sink 中断必须整轮回滚");
        assert_eq!(
            scan.observed_max_time_updated, None,
            "中断轮必须隐藏 partial max"
        );
    } else {
        println!(
            "说明：近 24h 的 eligible 行不足两个批次，sink 中断分支未触发（源侧中断已覆盖语义）"
        );
    }
    assert_eq!(harness.archived_rows(), 0, "两次中断后归档仍必须为空");
    assert_eq!(harness.cursor(), None, "两次中断后 cursor 仍必须为 None");

    // (3) 重跑同样的有界轮次：必须干净完成并推进 cursor。
    let rerun_request = ScanRequest::live(harness.host_id.as_str(), Some(now_ms()));
    let (rerun_scan, rerun_stats) = run_direct_round(&mut harness, &database, &rerun_request);
    println!(
        "重跑轮         : reached_eof={} eligible={} committed={} changed={} cursor={:?}",
        rerun_scan.reached_eof,
        rerun_scan.eligible_count,
        rerun_stats.committed,
        rerun_stats.changed_records,
        rerun_stats.cursor_time_updated
    );
    assert!(rerun_scan.reached_eof, "重跑必须到达 EOF");
    assert!(rerun_stats.committed, "重跑必须提交");
    assert_eq!(
        harness.archived_rows(),
        rerun_scan.eligible_count,
        "重跑后归档行数必须等于本轮 eligible"
    );
    assert!(harness.cursor().is_some(), "重跑后必须写入 cursor");
    harness.close();
}

/// 坏路径：不存在的路径与目录路径都必须给出可读的中文/结构化错误，且不 panic。
#[test]
#[ignore = "真实库烟测：显式 cargo test --release -p agentlens-core -- --ignored real_db_ 运行"]
fn real_db_malformed_source_paths_report_readable_errors() {
    let temp = tempfile::tempdir().expect("创建坏路径 tempdir");
    let missing = temp.path().join("no-such-opencode.db");
    let directory = temp.path().to_path_buf();

    for (label, path) in [("不存在的路径", &missing), ("目录路径", &directory)] {
        let error = SqliteSourceConnection::open(path).expect_err("坏路径必须失败");
        let message = error.to_string();
        println!("{label:<12} -> {message}");
        assert!(
            matches!(&error, OpenCodeError::DatabaseNotFound { probed_paths } if probed_paths == &vec![path.clone()]),
            "{label} 应报告 DatabaseNotFound 并携带探测路径：{error:?}"
        );
        assert!(
            message.contains(&path.display().to_string()),
            "{label} 的错误文案必须包含路径：{message}"
        );

        let probe_error = LocalHostSource::with_database("host-real-db-malformed", path)
            .probe()
            .expect_err("probe 坏路径必须失败");
        println!("{label:<12} probe -> {probe_error}");
        assert!(
            probe_error
                .to_string()
                .contains("OpenCode database was not found"),
            "probe 错误应透传 DatabaseNotFound：{probe_error}"
        );
    }
}

/// 直接按 todo 6 的接线跑一轮（scan_connection + IngestRound），返回扫描与入库统计。
fn run_direct_round(
    harness: &mut Harness,
    database: &Path,
    request: &ScanRequest,
) -> (
    agentlens_core::source::opencode::ScanResult,
    agentlens_core::ingest::IngestStats,
) {
    let mut source = SqliteSourceConnection::open(database).expect("打开真实源库");
    let mut round = IngestRound::begin(
        harness.archive.connection_mut(),
        request.host_id.clone(),
        request.origin,
    )
    .expect("开启入库轮次");
    let scan = scan_connection(&mut source, request, |batch| {
        round
            .ingest_batch(batch)
            .map_err(|error| SinkError::new(error.to_string()))
    })
    .expect("执行只读扫描");
    let stats = round.finish(&scan).expect("结束入库轮次");
    (scan, stats)
}
