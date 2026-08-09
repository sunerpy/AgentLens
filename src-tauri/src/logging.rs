//! Structured logging, the log-tail read path, and the leak-free diagnostics snapshot.
//!
//! Why this file exists at all: `main.rs` declares `windows_subsystem = "windows"` for release
//! builds, so a shipped AgentLens has **no console**. Every `eprintln!` the shell used to emit
//! was therefore written to a handle nobody can read — the errors were real, the destination
//! was not. Diagnostics have to land in a file the user can open.
//!
//! Three deliberate choices:
//!
//! - **A byte-capped writer instead of `tracing-appender`.** `tracing-appender`'s rolling file
//!   appender caps the *number* of files (`max_log_files`), not their size, so a single chatty
//!   day still has no ceiling. [`LOG_MAX_TOTAL_BYTES`] is a real ceiling: at most
//!   [`LOG_RETAINED_FILES`] files of [`LOG_MAX_FILE_BYTES`] each.
//! - **JSON lines.** The viewer filters by level. Splitting a human-readable line with a regex
//!   breaks the first time a message contains the delimiter; one self-describing JSON object per
//!   line cannot be mis-split.
//! - **Writes never panic.** On Windows GUI subsystem `eprintln!` panics when the standard
//!   handle is NULL, which is exactly why `tracing-subscriber`'s own `log_internal_errors` is
//!   turned off here and why [`RollingWriter`] swallows I/O errors instead of unwrapping. A
//!   failed log write must never take the app down; the whole point is that logging is the
//!   safety net, not a new failure mode.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt::format::Writer as FmtWriter;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

/// Name of the live log file. Rotated generations get a `.1`, `.2`, … suffix.
pub const LOG_FILE_NAME: &str = "agentlens.log";
/// Rotation threshold for the live file.
pub const LOG_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Live file plus rotated generations kept on disk.
pub const LOG_RETAINED_FILES: usize = 3;
/// Hard ceiling on the space logging may ever occupy.
pub const LOG_MAX_TOTAL_BYTES: u64 = LOG_MAX_FILE_BYTES * LOG_RETAINED_FILES as u64;
/// Upper bound on entries a single [`read_recent`] call may return.
pub const LOG_TAIL_MAX_LIMIT: usize = 2000;
/// Default when the caller does not ask for a specific count.
pub const LOG_TAIL_DEFAULT_LIMIT: usize = 500;

/// Level filter applied when `RUST_LOG` is absent.
const DEFAULT_FILTER: &str = "info";

/// Severity of one log record, mirroring `tracing::Level`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "ERROR" => Some(Self::Error),
            "WARN" => Some(Self::Warn),
            "INFO" => Some(Self::Info),
            "DEBUG" => Some(Self::Debug),
            "TRACE" => Some(Self::Trace),
            _ => None,
        }
    }
}

/// One parsed log record, as the viewer renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Local-time RFC 3339 stamp, offset included, exactly as written.
    pub timestamp: String,
    pub level: LogLevel,
    /// Emitting module path, e.g. `agentlens_tauri_lib::tray`.
    pub target: String,
    pub message: String,
}

/// Result of a log-tail read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    /// Directory holding the log files, so the view can offer "reveal in file manager".
    pub directory: String,
    /// Newest first.
    pub entries: Vec<LogEntry>,
    /// True when the log directory exists but holds no parsable record yet.
    pub empty: bool,
}

/// Environment facts safe to publish in a public bug report.
///
/// Every field here is a build-time or platform constant. There is deliberately no hostname,
/// user name, machine-id hash, archive path, host address or credential: this struct is what
/// gets pasted into a GitHub issue body, so anything identifying must not be able to reach it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    /// WebView2 / WebKitGTK version, `None` when the runtime cannot report it.
    pub webview_version: Option<String>,
}

/// Collects the publishable environment facts.
pub fn diagnostics_snapshot() -> DiagnosticsReport {
    DiagnosticsReport {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        webview_version: webview_version(),
    }
}

#[cfg(not(test))]
fn webview_version() -> Option<String> {
    tauri::webview_version().ok()
}

/// Headless CI has no WebView runtime at all, and probing for one there would make the
/// snapshot test's result depend on the machine rather than on the code.
#[cfg(test)]
fn webview_version() -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Byte-capped rolling writer
// ---------------------------------------------------------------------------

/// Append-only writer that rotates on [`LOG_MAX_FILE_BYTES`] and keeps
/// [`LOG_RETAINED_FILES`] generations.
#[derive(Debug)]
pub struct RollingWriter {
    directory: PathBuf,
    max_file_bytes: u64,
    retained_files: usize,
    file: Option<File>,
    written_bytes: u64,
}

impl RollingWriter {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self::with_limits(directory, LOG_MAX_FILE_BYTES, LOG_RETAINED_FILES)
    }

    pub fn with_limits(
        directory: impl Into<PathBuf>,
        max_file_bytes: u64,
        retained_files: usize,
    ) -> Self {
        Self {
            directory: directory.into(),
            max_file_bytes: max_file_bytes.max(1),
            // Zero retained files would mean "log nowhere", which silently reintroduces the
            // very problem this module exists to fix.
            retained_files: retained_files.max(1),
            file: None,
            written_bytes: 0,
        }
    }

    fn live_path(&self) -> PathBuf {
        self.directory.join(LOG_FILE_NAME)
    }

    fn rotated_path(&self, generation: usize) -> PathBuf {
        self.directory.join(format!("{LOG_FILE_NAME}.{generation}"))
    }

    /// Shifts `agentlens.log` to `.1`, `.1` to `.2`, … and drops whatever falls off the end.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        self.written_bytes = 0;
        for generation in (1..self.retained_files).rev() {
            let source = if generation == 1 {
                self.live_path()
            } else {
                self.rotated_path(generation - 1)
            };
            if !source.exists() {
                continue;
            }
            let target = self.rotated_path(generation);
            if generation == self.retained_files - 1 {
                let _ = fs::remove_file(&target);
            }
            fs::rename(&source, &target)?;
        }
        if self.retained_files == 1 {
            let _ = fs::remove_file(self.live_path());
        }
        Ok(())
    }

    fn open_live(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let path = self.live_path();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.written_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    /// Appends one already-formatted record.
    ///
    /// `tracing_subscriber::fmt` emits each event with a single `write_all` of the complete
    /// line, so treating one call as one record is exact rather than a guess.
    pub fn append(&mut self, record: &[u8]) -> io::Result<()> {
        if self.file.is_none() {
            self.open_live()?;
        }
        if self.written_bytes > 0
            && self.written_bytes.saturating_add(record.len() as u64) > self.max_file_bytes
        {
            self.rotate()?;
            self.open_live()?;
        }
        let Some(file) = self.file.as_mut() else {
            return Err(io::Error::other("log file is not open"));
        };
        file.write_all(record)?;
        file.flush()?;
        self.written_bytes = self.written_bytes.saturating_add(record.len() as u64);
        Ok(())
    }
}

/// `MakeWriter` adapter handing the fmt layer a shared [`RollingWriter`].
#[derive(Clone, Debug)]
pub struct LogSink {
    writer: Arc<Mutex<RollingWriter>>,
}

impl LogSink {
    pub fn new(writer: RollingWriter) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }
}

/// Discards the record on a poisoned lock or an I/O failure.
///
/// A log write is best-effort by construction: propagating the error has nowhere to go (the
/// caller is `tracing`, mid-event) and panicking would turn "the disk is full" into "the app
/// crashed".
impl Write for LogSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.append(buffer);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogSink {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// Subscriber installation
// ---------------------------------------------------------------------------

/// Local wall-clock timer.
///
/// The default `SystemTime` timer stamps UTC. Everything else the user sees in AgentLens is
/// bucketed in their own timezone, so a UTC log would be the one surface where "when did this
/// happen" needs mental arithmetic.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, writer: &mut FmtWriter<'_>) -> std::fmt::Result {
        write!(writer, "{}", local_timestamp())
    }
}

fn local_timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        .to_string()
}

static LOG_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Directory logging was initialised with, if [`init`] has run.
pub fn log_directory() -> Option<&'static Path> {
    LOG_DIRECTORY.get().map(PathBuf::as_path)
}

/// Installs the process-wide subscriber writing JSON lines under `directory`.
///
/// Idempotent and never fatal: a second call, or a failure to set the global default, leaves
/// the first subscriber in place and returns `false`. The desktop shell must start even when
/// logging cannot.
pub fn init(directory: impl Into<PathBuf>) -> bool {
    let directory = directory.into();
    if fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let sink = LogSink::new(RollingWriter::new(directory.clone()));
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let installed = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_span_list(false)
        .with_timer(LocalTimer)
        .with_target(true)
        .with_ansi(false)
        // Its failure path is `eprintln!`, which panics under `windows_subsystem = "windows"`.
        .log_internal_errors(false)
        .with_env_filter(filter)
        .with_writer(sink)
        .try_init()
        .is_ok();
    if installed {
        let _ = LOG_DIRECTORY.set(directory);
    }
    installed
}

// ---------------------------------------------------------------------------
// Read path
// ---------------------------------------------------------------------------

/// Shape `tracing_subscriber`'s JSON formatter writes with `flatten_event(true)`.
#[derive(Deserialize)]
struct RawRecord {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    message: String,
}

/// Parses one written line, or `None` for anything that is not a log record.
///
/// Blank lines and truncated tails are expected, not exceptional: a rotation or a crash can
/// leave a half-written last line, and one unparsable line must not hide the rest of the file.
pub fn parse_log_line(line: &str) -> Option<LogEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw: RawRecord = serde_json::from_str(trimmed).ok()?;
    Some(LogEntry {
        timestamp: raw.timestamp,
        level: LogLevel::parse(&raw.level)?,
        target: raw.target,
        message: raw.message,
    })
}

fn read_entries(path: &Path) -> Vec<LogEntry> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| parse_log_line(&line))
        .collect()
}

/// Reads up to `limit` newest records, newest first.
///
/// Walks the live file first and only reaches back into rotated generations when the live file
/// holds fewer than `limit` records — a full 2 MiB generation is tens of thousands of lines, so
/// the common case touches exactly one file.
pub fn read_recent(directory: &Path, limit: usize) -> LogTail {
    let limit = limit.clamp(1, LOG_TAIL_MAX_LIMIT);
    let mut newest_first: Vec<LogEntry> = Vec::new();
    for generation in 0..LOG_RETAINED_FILES {
        if newest_first.len() >= limit {
            break;
        }
        let path = if generation == 0 {
            directory.join(LOG_FILE_NAME)
        } else {
            directory.join(format!("{LOG_FILE_NAME}.{generation}"))
        };
        let mut entries = read_entries(&path);
        entries.reverse();
        newest_first.extend(entries);
    }
    newest_first.truncate(limit);
    LogTail {
        directory: directory.display().to_string(),
        empty: newest_first.is_empty(),
        entries: newest_first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lines(writer: &mut RollingWriter, count: usize, level: &str) {
        for index in 0..count {
            let line = format!(
                "{{\"timestamp\":\"2026-08-07T10:00:0{}.000+08:00\",\"level\":\"{level}\",\"target\":\"t\",\"message\":\"m{index}\"}}\n",
                index % 10
            );
            writer.append(line.as_bytes()).expect("append log record");
        }
    }

    #[test]
    fn log_level_parses_every_tracing_level_case_insensitively() {
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse(" warn "), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("Debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("tRaCe"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("fatal"), None);
        assert_eq!(LogLevel::parse(""), None);
    }

    #[test]
    fn log_levels_order_from_most_to_least_severe() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
    }

    #[test]
    fn parse_log_line_reads_a_flattened_json_record() {
        let entry = parse_log_line(
            "{\"timestamp\":\"2026-08-07T10:11:12.345+08:00\",\"level\":\"WARN\",\"target\":\"agentlens_tauri_lib::tray\",\"message\":\"archive unavailable\"}",
        )
        .expect("parse a well-formed record");
        assert_eq!(entry.timestamp, "2026-08-07T10:11:12.345+08:00");
        assert_eq!(entry.level, LogLevel::Warn);
        assert_eq!(entry.target, "agentlens_tauri_lib::tray");
        assert_eq!(entry.message, "archive unavailable");
    }

    #[test]
    fn parse_log_line_rejects_noise_instead_of_inventing_a_record() {
        assert!(parse_log_line("").is_none());
        assert!(parse_log_line("   \n").is_none());
        assert!(parse_log_line("not json at all").is_none());
        // A rotation or a crash can leave the last line half-written.
        assert!(parse_log_line("{\"timestamp\":\"2026-08-07T10:00:00").is_none());
        // Valid JSON, unknown level: dropping it beats displaying a record with a guessed level.
        assert!(parse_log_line("{\"level\":\"FATAL\",\"message\":\"m\"}").is_none());
    }

    #[test]
    fn parse_log_line_tolerates_missing_optional_fields() {
        let entry =
            parse_log_line("{\"level\":\"INFO\",\"message\":\"m\"}").expect("parse minimal record");
        assert_eq!(entry.timestamp, "");
        assert_eq!(entry.target, "");
        assert_eq!(entry.message, "m");
    }

    #[test]
    fn rolling_writer_creates_the_directory_it_was_pointed_at() {
        let temp = tempfile::tempdir().expect("temp dir");
        let nested = temp.path().join("logs").join("deeper");
        let mut writer = RollingWriter::new(&nested);
        write_lines(&mut writer, 1, "INFO");
        assert!(nested.join(LOG_FILE_NAME).exists());
    }

    #[test]
    fn rolling_writer_rotates_once_the_live_file_would_exceed_the_cap() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 200, 3);
        write_lines(&mut writer, 12, "INFO");

        assert!(temp.path().join(LOG_FILE_NAME).exists());
        assert!(temp.path().join(format!("{LOG_FILE_NAME}.1")).exists());
        for entry in fs::read_dir(temp.path()).expect("read log dir") {
            let path = entry.expect("dir entry").path();
            let size = fs::metadata(&path).expect("metadata").len();
            // One oversized record still gets written whole rather than truncated, so the
            // ceiling is "cap plus at most one record", never "cap times files written".
            assert!(size <= 200 + 128, "{path:?} grew to {size} bytes");
        }
    }

    #[test]
    fn rolling_writer_never_keeps_more_generations_than_configured() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 200, 3);
        write_lines(&mut writer, 200, "INFO");

        let files = fs::read_dir(temp.path())
            .expect("read log dir")
            .filter_map(Result::ok)
            .count();
        assert_eq!(files, 3, "retention must be a hard ceiling, not a hint");
        assert!(!temp.path().join(format!("{LOG_FILE_NAME}.3")).exists());
    }

    #[test]
    fn rolling_writer_with_a_single_generation_discards_the_old_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 120, 1);
        write_lines(&mut writer, 20, "INFO");

        let files = fs::read_dir(temp.path())
            .expect("read log dir")
            .filter_map(Result::ok)
            .count();
        assert_eq!(files, 1);
        assert!(!temp.path().join(format!("{LOG_FILE_NAME}.1")).exists());
    }

    #[test]
    fn rolling_writer_clamps_nonsense_limits_instead_of_logging_nowhere() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 0, 0);
        write_lines(&mut writer, 3, "INFO");
        assert!(temp.path().join(LOG_FILE_NAME).exists());
    }

    #[test]
    fn rolling_writer_resumes_an_existing_file_rather_than_truncating_it() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut first = RollingWriter::with_limits(temp.path(), 10_000, 3);
        write_lines(&mut first, 3, "INFO");
        drop(first);

        let mut second = RollingWriter::with_limits(temp.path(), 10_000, 3);
        write_lines(&mut second, 2, "WARN");

        let tail = read_recent(temp.path(), 100);
        assert_eq!(tail.entries.len(), 5);
    }

    #[test]
    fn log_sink_swallows_write_failures_so_a_bad_disk_cannot_crash_the_app() {
        let temp = tempfile::tempdir().expect("temp dir");
        // A regular file where the log directory should be makes every open fail.
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, b"x").expect("create blocking file");
        let mut sink = LogSink::new(RollingWriter::new(&blocked));

        let written = sink
            .write(b"{\"level\":\"INFO\"}\n")
            .expect("write reports success");
        assert_eq!(written, 17);
        sink.flush().expect("flush is a no-op");
    }

    #[test]
    fn read_recent_returns_newest_first_and_reaches_into_rotated_generations() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 400, 3);
        write_lines(&mut writer, 12, "INFO");

        let tail = read_recent(temp.path(), 100);
        assert!(!tail.empty);
        assert_eq!(tail.directory, temp.path().display().to_string());
        assert!(
            tail.entries.len() > 3,
            "a rotated generation must still be readable, got {}",
            tail.entries.len()
        );
        assert_eq!(tail.entries.first().expect("newest entry").message, "m11");
    }

    #[test]
    fn read_recent_clamps_the_limit_at_both_ends() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut writer = RollingWriter::with_limits(temp.path(), 10_000, 3);
        write_lines(&mut writer, 10, "INFO");

        assert_eq!(read_recent(temp.path(), 0).entries.len(), 1);
        assert_eq!(read_recent(temp.path(), 3).entries.len(), 3);
        assert_eq!(
            read_recent(temp.path(), LOG_TAIL_MAX_LIMIT * 10)
                .entries
                .len(),
            10
        );
    }

    #[test]
    fn read_recent_reports_empty_for_a_directory_with_no_records() {
        let temp = tempfile::tempdir().expect("temp dir");
        let tail = read_recent(temp.path(), 50);
        assert!(tail.empty);
        assert!(tail.entries.is_empty());

        fs::write(temp.path().join(LOG_FILE_NAME), b"garbage\n\n").expect("write junk");
        let tail = read_recent(temp.path(), 50);
        assert!(tail.empty, "unparsable content is not a record");
    }

    #[test]
    fn read_recent_skips_a_missing_directory_without_failing() {
        let temp = tempfile::tempdir().expect("temp dir");
        let tail = read_recent(&temp.path().join("never-created"), 50);
        assert!(tail.empty);
    }

    #[test]
    fn local_timestamp_is_rfc3339_with_a_local_offset() {
        let stamp = local_timestamp();
        assert_eq!(stamp.len(), 29, "unexpected shape: {stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert_eq!(&stamp[19..20], ".");
        let offset = &stamp[23..];
        assert!(
            (offset.starts_with('+') || offset.starts_with('-')) && offset[3..4] == *":",
            "offset must be ±HH:MM, got {offset}"
        );
    }

    #[test]
    fn local_timer_writes_the_same_stamp_shape_the_parser_expects() {
        let mut buffer = String::new();
        let mut writer = FmtWriter::new(&mut buffer);
        LocalTimer.format_time(&mut writer).expect("format time");
        assert_eq!(buffer.len(), 29, "unexpected shape: {buffer}");
    }

    /// The whole feature's load-bearing claim: a real `tracing::error!` reaches the file, and
    /// the read path parses it back with the right level. Adding `tracing` proves nothing on
    /// its own — this asserts the round trip.
    #[test]
    fn a_real_tracing_event_round_trips_through_the_file_at_its_own_level() {
        use tracing::subscriber::with_default;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::Registry;

        let temp = tempfile::tempdir().expect("temp dir");
        let sink = LogSink::new(RollingWriter::new(temp.path()));
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_span_list(false)
            .with_timer(LocalTimer)
            .with_ansi(false)
            .log_internal_errors(false)
            .with_writer(sink);

        with_default(Registry::default().with(layer), || {
            tracing::error!("hosts_delete failed: host disappeared");
            tracing::warn!(host_id = "local", "refresh interval clamped");
            tracing::info!("tray icon installed");
        });

        let tail = read_recent(temp.path(), 50);
        assert_eq!(tail.entries.len(), 3, "every event must reach the file");

        let newest = tail.entries.first().expect("newest entry");
        assert_eq!(newest.level, LogLevel::Info);
        assert_eq!(newest.message, "tray icon installed");

        let error = tail
            .entries
            .iter()
            .find(|entry| entry.level == LogLevel::Error)
            .expect("the error event must be recorded at ERROR, not downgraded");
        assert_eq!(error.message, "hosts_delete failed: host disappeared");
        assert!(
            error.target.starts_with("agentlens_tauri_lib"),
            "target should name the emitting module, got {}",
            error.target
        );
        assert!(
            !error.timestamp.is_empty(),
            "a record with no timestamp is unusable in the viewer"
        );

        assert!(tail
            .entries
            .iter()
            .any(|entry| entry.level == LogLevel::Warn));
    }

    #[test]
    fn init_is_idempotent_and_reports_whether_it_installed_the_subscriber() {
        let temp = tempfile::tempdir().expect("temp dir");
        let first = init(temp.path().join("logs"));
        let second = init(temp.path().join("other"));
        assert!(
            !(first && second),
            "a second install must not replace the first subscriber"
        );
        if first {
            assert_eq!(log_directory(), Some(temp.path().join("logs").as_path()));
        }
    }

    #[test]
    fn init_refuses_a_directory_it_cannot_create_instead_of_panicking() {
        let temp = tempfile::tempdir().expect("temp dir");
        let blocked = temp.path().join("file");
        fs::write(&blocked, b"x").expect("create blocking file");
        assert!(!init(blocked.join("logs")));
    }

    #[test]
    fn total_byte_ceiling_is_the_product_of_the_two_limits() {
        assert_eq!(
            LOG_MAX_TOTAL_BYTES,
            LOG_MAX_FILE_BYTES * LOG_RETAINED_FILES as u64
        );
        assert_eq!(LOG_MAX_TOTAL_BYTES, 6 * 1024 * 1024);
    }

    #[test]
    fn diagnostics_snapshot_carries_no_identifying_information() {
        let report = diagnostics_snapshot();
        assert_eq!(report.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.arch, std::env::consts::ARCH);

        // Serialise and scan: a future field cannot smuggle in identifying data without
        // failing here, which is stronger than asserting on today's field list.
        let serialised = serde_json::to_string(&report).expect("serialize report");
        let mut forbidden: Vec<String> = Vec::new();
        for key in [
            "HOME",
            "USER",
            "USERNAME",
            "USERPROFILE",
            "HOSTNAME",
            "LOGNAME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                let value = value.to_string_lossy().to_string();
                if value.len() > 2 {
                    forbidden.push(value);
                }
            }
        }
        if let Ok(hostname) = fs::read_to_string("/etc/hostname") {
            let hostname = hostname.trim().to_owned();
            if hostname.len() > 2 {
                forbidden.push(hostname);
            }
        }
        for needle in forbidden {
            assert!(
                !serialised.contains(&needle),
                "diagnostics leaked {needle:?}: {serialised}"
            );
        }
        assert!(!serialised.contains('/') || !serialised.contains("home"));
    }
}
