//! `agentlens-askpass`：OpenSSH `SSH_ASKPASS` 一次性口令助手（todo 20）。
//!
//! 调用契约由 todo 11 的 [`agentlens_core::transport::ssh`] 冻结，本二进制只是它的对端：
//! SSH 的钥匙串分支会设置 `SSH_ASKPASS=<本二进制>`、`SSH_ASKPASS_REQUIRE=force`、
//! `DISPLAY=:0` 占位、`AGENTLENS_ASKPASS_CHANNEL=<一次性通道路径>`，并把 ssh 进程从 TTY
//! 脱离（Linux `setsid`）。OpenSSH 随后以「提示语」为唯一参数执行本二进制，并从其
//! **stdout 读一行**当作口令。
//!
//! 因此这里只做四件事：
//!
//! 1. 从 `AGENTLENS_ASKPASS_CHANNEL` 取通道路径（普通临时文件或命名管道 FIFO 都支持）；
//! 2. 读完整通道内容后**立刻 unlink**，使同一口令不可能被读第二次（这就是「一次性」）；
//! 3. 去掉尾部换行后把字节原样写 stdout，补一个 `\n`；
//! 4. 退出。OpenSSH 传入的提示语一律忽略，绝不回显到任何流；口令也绝不写 stderr。
//!
//! 刻意零依赖（`[dependencies]` 为空）：这个进程短暂持有明文口令，依赖面越小越好。

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// 一次性口令通道的环境变量名，必须与 `agentlens_core::transport::ssh::ASKPASS_CHANNEL_ENV` 一致。
const CHANNEL_ENV: &str = "AGENTLENS_ASKPASS_CHANNEL";

const USAGE: &str = "用法: agentlens-askpass [提示语]\n\
     由 OpenSSH 经 SSH_ASKPASS 调用；口令取自环境变量 AGENTLENS_ASKPASS_CHANNEL \
     指向的一次性通道（临时文件或 FIFO），读取后立即删除。\n\
     选项: --help|-h 显示帮助；--version|-V 显示版本。";

/// 参数错误。
const EXIT_USAGE: u8 = 1;
/// 通道未设置、不存在或不可读。
const EXIT_CHANNEL_UNAVAILABLE: u8 = 2;
/// 通道存在但内容为空（去掉尾部换行后无字节）。
const EXIT_CHANNEL_EMPTY: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Help,
    Version,
    /// 正常路径：读通道并输出口令。
    Emit,
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let stdout = io::stdout();
    let stderr = io::stderr();
    run_with_io(
        &args,
        env::var_os(CHANNEL_ENV),
        &mut stdout.lock(),
        &mut stderr.lock(),
    )
}

fn run_with_io(
    args: &[OsString],
    channel: Option<OsString>,
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
) -> ExitCode {
    match classify(args) {
        Some(Mode::Help) => {
            writeln!(stdout, "{USAGE}").expect("write askpass help");
            ExitCode::SUCCESS
        }
        Some(Mode::Version) => {
            writeln!(stdout, "agentlens-askpass {}", env!("CARGO_PKG_VERSION"))
                .expect("write askpass version");
            ExitCode::SUCCESS
        }
        Some(Mode::Emit) => emit_to(channel, stdout, stderr),
        None => {
            writeln!(stderr, "参数错误：本助手最多接受一个提示语参数。\n{USAGE}")
                .expect("write askpass usage error");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// 归类参数。`None` 表示参数错误。
///
/// OpenSSH 只会传一个提示语，所以「零参数」和「单个非选项参数」都是正常路径；
/// `--help`/`--version` 仅当它是唯一参数时生效（与 `agentlens-collector` 同约定）。
fn classify(args: &[OsString]) -> Option<Mode> {
    match args {
        [] => Some(Mode::Emit),
        [single] => {
            let text = single.to_string_lossy();
            match text.as_ref() {
                "--help" | "-h" => Some(Mode::Help),
                "--version" | "-V" => Some(Mode::Version),
                other if other.starts_with('-') => None,
                _ => Some(Mode::Emit),
            }
        }
        _ => None,
    }
}

fn emit_to(
    channel: Option<OsString>,
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
) -> ExitCode {
    let Some(channel) = channel.filter(|value| !value.is_empty()) else {
        let _ = writeln!(
            stderr,
            "环境变量 {CHANNEL_ENV} 未设置或为空：本助手只能由 AgentLens 经 SSH_ASKPASS 调用。"
        );
        return ExitCode::from(EXIT_CHANNEL_UNAVAILABLE);
    };
    let path = PathBuf::from(channel);
    let secret = match read_channel_once(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = writeln!(stderr, "无法读取一次性口令通道 {}：{error}", path.display());
            return ExitCode::from(EXIT_CHANNEL_UNAVAILABLE);
        }
    };
    let trimmed = trim_trailing_newline(&secret);
    if trimmed.is_empty() {
        let _ = writeln!(
            stderr,
            "一次性口令通道 {} 为空，未向 ssh 输出任何内容。",
            path.display()
        );
        return ExitCode::from(EXIT_CHANNEL_EMPTY);
    }
    match write_secret_line(stdout, trimmed) {
        Ok(()) => ExitCode::SUCCESS,
        // ssh 提前关闭管道时按成功退出：口令已经交付，与 agentlens-collector 的 stdout 约定一致。
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(stderr, "写出口令失败：{error}");
            ExitCode::from(EXIT_CHANNEL_UNAVAILABLE)
        }
    }
}

/// 读取通道全部字节，随后立即 unlink，实现「一次性」。
///
/// 普通文件与 FIFO 都走这条路径：对 FIFO，`fs::read` 会阻塞到写端关闭。
/// unlink 失败不影响本次交付（口令已在内存里），但会在 stderr 提示一次性保证被削弱。
fn read_channel_once(path: &Path) -> io::Result<Vec<u8>> {
    read_channel_once_with(path, |channel| fs::remove_file(channel))
}

fn read_channel_once_with(
    path: &Path,
    remove: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if let Err(error) = remove(path) {
        eprintln!(
            "警告：一次性口令通道 {} 删除失败（{error}），请手动清理。",
            path.display()
        );
    }
    Ok(bytes)
}

/// 只去掉**尾部**的 `\r`/`\n`：口令的前导与内部空白都是有效字节，不能 trim。
fn trim_trailing_newline(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &bytes[..end]
}

fn write_secret_line(mut writer: impl io::Write, secret: &[u8]) -> io::Result<()> {
    writer.write_all(secret)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    /// 唯一临时目录，不引入 `tempfile`（本 crate 刻意零依赖，含 dev-dependencies）。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "agentlens-askpass-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn child(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    struct FailingWriter(io::ErrorKind);

    impl io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "injected writer failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn classify_treats_no_argument_and_prompt_as_emit() {
        assert_eq!(classify(&args(&[])), Some(Mode::Emit));
        // OpenSSH 实际传入的形态。
        assert_eq!(
            classify(&args(&["user@example.com's password:"])),
            Some(Mode::Emit)
        );
    }

    #[test]
    fn classify_recognizes_help_and_version_only_as_sole_argument() {
        assert_eq!(classify(&args(&["--help"])), Some(Mode::Help));
        assert_eq!(classify(&args(&["-h"])), Some(Mode::Help));
        assert_eq!(classify(&args(&["--version"])), Some(Mode::Version));
        assert_eq!(classify(&args(&["-V"])), Some(Mode::Version));
        assert_eq!(classify(&args(&["--help", "--version"])), None);
    }

    #[test]
    fn classify_rejects_unknown_flags_and_extra_arguments() {
        assert_eq!(classify(&args(&["--channel"])), None);
        assert_eq!(classify(&args(&["prompt", "extra"])), None);
    }

    #[test]
    fn run_with_io_renders_help_version_usage_error_and_emit_results_to_the_correct_stream() {
        let cases = [
            (vec!["--help"], format!("{USAGE}\n")),
            (
                vec!["--version"],
                format!("agentlens-askpass {}\n", env!("CARGO_PKG_VERSION")),
            ),
        ];
        for (values, expected) in cases {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = run_with_io(&args(&values), None, &mut stdout, &mut stderr);
            assert_eq!(exit, ExitCode::SUCCESS);
            assert_eq!(stdout, expected.as_bytes());
            assert!(stderr.is_empty());
        }

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with_io(&args(&["prompt", "extra"]), None, &mut stdout, &mut stderr);
        assert_eq!(exit, ExitCode::from(EXIT_USAGE));
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr)
            .expect("usage diagnostic is UTF-8")
            .contains("最多接受一个提示语参数"));

        let temp = TempDir::new();
        let channel = temp.child("run-channel");
        fs::write(&channel, b"through-dispatch\n").expect("write run channel");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with_io(
            &args(&["Password:"]),
            Some(channel.into_os_string()),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::SUCCESS);
        assert_eq!(stdout, b"through-dispatch\n");
        assert!(stderr.is_empty());
    }

    #[test]
    fn trim_trailing_newline_keeps_inner_and_leading_whitespace() {
        assert_eq!(trim_trailing_newline(b"secret\n"), b"secret");
        assert_eq!(trim_trailing_newline(b"secret\r\n"), b"secret");
        assert_eq!(trim_trailing_newline(b"secret\n\n\r\n"), b"secret");
        assert_eq!(trim_trailing_newline(b"  pa ss  "), b"  pa ss  ");
        assert_eq!(trim_trailing_newline(b"\n\n"), b"");
        assert_eq!(trim_trailing_newline(b""), b"");
    }

    #[test]
    fn read_channel_once_unlinks_so_second_read_fails() {
        let temp = TempDir::new();
        let channel = temp.child("channel");
        fs::write(&channel, b"one-shot-secret\n").expect("write channel");

        let first = read_channel_once(&channel).expect("first read succeeds");
        assert_eq!(first, b"one-shot-secret\n");
        assert!(
            !channel.exists(),
            "channel must be unlinked after the single read"
        );

        let second = read_channel_once(&channel).expect_err("second read must fail");
        assert_eq!(second.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn read_channel_once_preserves_bytes_verbatim() {
        let temp = TempDir::new();
        let channel = temp.child("channel");
        // 含空格、引号、`$(...)`、反引号与换行：字节必须逐字保真，绝不做 shell 解释。
        let payload = b"  pa$(touch /tmp/never)ss `id` \"q\" \n";
        fs::write(&channel, payload).expect("write channel");

        let bytes = read_channel_once(&channel).expect("read channel");
        assert_eq!(bytes, payload);
        assert_eq!(
            trim_trailing_newline(&bytes),
            b"  pa$(touch /tmp/never)ss `id` \"q\" "
        );
    }

    #[test]
    fn read_channel_once_returns_the_secret_when_unlink_fails() {
        let temp = TempDir::new();
        let channel = temp.child("channel");
        fs::write(&channel, b"still-delivered").expect("write channel");

        let bytes = read_channel_once_with(&channel, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected unlink failure",
            ))
        })
        .expect("unlink failure must not discard an already-read secret");

        assert_eq!(bytes, b"still-delivered");
        assert!(
            channel.exists(),
            "injected remover leaves the channel in place"
        );
    }

    #[test]
    fn emit_to_delivers_exactly_one_normalized_line_and_consumes_the_channel() {
        let temp = TempDir::new();
        let channel = temp.child("channel");
        fs::write(&channel, b"  exact\0secret  \r\n").expect("write channel");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = emit_to(
            Some(channel.clone().into_os_string()),
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit, ExitCode::SUCCESS);
        assert_eq!(stdout, b"  exact\0secret  \n");
        assert!(stderr.is_empty());
        assert!(
            !channel.exists(),
            "successful delivery must consume channel"
        );
    }

    #[test]
    fn emit_to_distinguishes_missing_unreadable_and_empty_channels_without_output() {
        let temp = TempDir::new();
        let missing = temp.child("missing");

        for channel in [None, Some(OsString::new())] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = emit_to(channel, &mut stdout, &mut stderr);
            assert_eq!(exit, ExitCode::from(EXIT_CHANNEL_UNAVAILABLE));
            assert!(stdout.is_empty());
            assert!(String::from_utf8(stderr)
                .expect("diagnostic is UTF-8")
                .contains(CHANNEL_ENV));
        }

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = emit_to(
            Some(missing.clone().into_os_string()),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::from(EXIT_CHANNEL_UNAVAILABLE));
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr)
            .expect("diagnostic is UTF-8")
            .contains(&missing.to_string_lossy().into_owned()));

        let empty = temp.child("empty");
        fs::write(&empty, b"\r\n\n").expect("write empty channel");
        let mut stderr = Vec::new();
        let exit = emit_to(
            Some(empty.clone().into_os_string()),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, ExitCode::from(EXIT_CHANNEL_EMPTY));
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr)
            .expect("diagnostic is UTF-8")
            .contains("为空"));
        assert!(!empty.exists(), "empty channel must still be consumed");
    }

    #[test]
    fn emit_to_treats_broken_pipe_as_delivered_but_reports_other_write_failures() {
        for (kind, expected, diagnostic) in [
            (io::ErrorKind::BrokenPipe, ExitCode::SUCCESS, false),
            (
                io::ErrorKind::PermissionDenied,
                ExitCode::from(EXIT_CHANNEL_UNAVAILABLE),
                true,
            ),
        ] {
            let temp = TempDir::new();
            let channel = temp.child("channel");
            fs::write(&channel, b"secret").expect("write channel");
            let mut stdout = FailingWriter(kind);
            let mut stderr = Vec::new();

            let exit = emit_to(
                Some(channel.clone().into_os_string()),
                &mut stdout,
                &mut stderr,
            );

            assert_eq!(exit, expected);
            assert_eq!(!stderr.is_empty(), diagnostic);
            assert!(
                !channel.exists(),
                "write failure occurs after channel consumption"
            );
        }
    }

    #[test]
    fn write_secret_line_propagates_failures_from_secret_newline_and_flush() {
        #[derive(Default)]
        struct ScriptedWriter {
            calls: usize,
            fail_on: usize,
            bytes: Vec<u8>,
        }

        impl io::Write for ScriptedWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.calls += 1;
                if self.calls == self.fail_on {
                    return Err(io::Error::other("injected write failure"));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                self.calls += 1;
                if self.calls == self.fail_on {
                    Err(io::Error::other("injected flush failure"))
                } else {
                    Ok(())
                }
            }
        }

        for fail_on in 1..=3 {
            let mut writer = ScriptedWriter {
                fail_on,
                ..ScriptedWriter::default()
            };
            let error = write_secret_line(&mut writer, b"secret")
                .expect_err("each output stage must propagate its failure");
            assert_eq!(error.kind(), io::ErrorKind::Other);
        }

        let mut writer = ScriptedWriter {
            fail_on: 4,
            ..ScriptedWriter::default()
        };
        write_secret_line(&mut writer, b"secret").expect("all output stages succeed");
        assert_eq!(writer.bytes, b"secret\n");
    }
}
