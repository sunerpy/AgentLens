//! SSH 传输层：常量远端命令与载荷协议（todo 11）。
//!
//! 本模块经系统 `ssh` / `scp` 工作。远端命令是**常量字符串** `sh -s`，不做任何动态拼接。
//! 需要载荷的阶段在远端 argv 追加独立的 `-- <token>`，脚本从 `$1` 读取；OpenSSH 虽会把
//! 远端 argv 用空格连接成一个 shell 字符串，但 `validate_payload_token` 强制 token 只能包含
//! `[A-Za-z0-9_-]`，因此不可能表示空白、引号或 shell 元字符。
//!
//! stdin 只承载固定脚本正文（`set -eu`、`umask 077`、`LC_ALL=C`、
//! `trap 'rm -rf "$WORKDIR"' EXIT`），不再混入动态数据。shell 侧不解码也不解析 JSON，只把
//! `$AGENTLENS_PAYLOAD` 原样作为单个参数传给 collector 的 `--request-base64url`，由 collector
//! 在 Rust/serde 内完成 base64url 解码与 JSON 校验。
//!
//! 阶段划分：STAGE1 探测（uname -m / XDG_DATA_HOME / df / machine-id）→
//! STAGE2 `mkdir -p ~/.cache/agentlens` 并 `mktemp -d` 回显绝对路径（本地校验以 `/` 开头、无控制字符）→
//! STAGE3 `scp` 推送 collector 到该绝对路径（不在 trap 保护内，失败由 STAGE4 trap 或下次 GC 兜底）→
//! STAGE4 校验 sha256、执行 collect、stdout 回传 NDJSON、trap 清理；
//! 每次连接顺带 GC 超过 24h 的 `run.*`。
//!
//! STAGE1 的 machine-id **只在远端就地摘要**：脚本用
//! `printf '%s' "$(tr -d '[:space:]' < "$FILE")" | sha256sum` 去掉全部空白后再算 SHA-256，
//! 与本地 [`crate::host::MachineIdentity::from_machine_id`]（先 `trim` 再 SHA-256、小写 hex）
//! 逐字节一致；直接 `sha256sum < "$FILE"` 会把尾部换行算进摘要，故不可用。原文永不过网，
//! 回传的只有 [`MACHINE_ID_HASH_HEX_LENGTH`] 位小写十六进制摘要，且由 `parse_probe` 在核心侧
//! 校验格式，不把远端垃圾透传给界面。
//!
//! 认证：默认 agent / 密钥路径配 `-o BatchMode=yes`；钥匙串口令走非 BatchMode 分支加
//! `SSH_ASKPASS=<打包的 agentlens-askpass>`、`SSH_ASKPASS_REQUIRE=force`、`DISPLAY=:0` 占位、
//! 进程无 TTY。Windows 下 ssh/scp 定位顺序为 PATH → 显式设置 → `%WINDIR%\System32\OpenSSH`，
//! 且要求成对同目录。统一附加 `StrictHostKeyChecking=accept-new`、`ConnectTimeout=10`。
//!
//! `CommandRunner` trait 用于注入（prod 用 `std::process`，test 用 fake）。
//! 错误枚举附中文 remediation：ArchMismatch / NoWritableCache / TransferCorrupted /
//! AuthFailed / NoDataDir / WalUnreadable / ClientCancelled。启动时探测 `ssh -V`。

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::host::MACHINE_ID_HASH_HEX_LENGTH;

/// The only remote command passed to OpenSSH.
pub const REMOTE_COMMAND: &str = "sh -s";
/// Environment variable through which the bundled askpass helper finds its one-shot secret channel.
pub const ASKPASS_CHANNEL_ENV: &str = "AGENTLENS_ASKPASS_CHANNEL";
/// OpenSSH's TCP-connect phase limit. It does not cover authentication or remote execution.
pub const SSH_CONNECT_TIMEOUT_SECS: u64 = 10;
/// Whole-process wall limit for the STAGE1 connection probe.
///
/// Twice the TCP-connect limit leaves the same ten-second budget for authentication and the
/// four lightweight discovery commands after a slow connection succeeds.
pub const SSH_PROBE_WALL_TIMEOUT: Duration = Duration::from_secs(20);

const CHECKSUM_FILE_NAME: &str = "collector.sha256";
const TRANSFER_CORRUPTED_EXIT: i32 = 70;
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CONNECT_TIMEOUT_OPTION: &str = "ConnectTimeout=10";

const STAGE1_SCRIPT: &str = r#"set -eu
umask 077
LC_ALL=C
export LC_ALL
ARCH=$(uname -m)
XDG_VALUE=${XDG_DATA_HOME:-}
AVAILABLE_KIB=$(df -Pk "$HOME" | awk 'NR == 2 { print $4 }')
MACHINE_ID_SOURCE=
if [ -r /etc/machine-id ] && [ -n "$(tr -d '[:space:]' < /etc/machine-id)" ]; then
  MACHINE_ID_SOURCE=/etc/machine-id
elif [ -r /var/lib/dbus/machine-id ] && [ -n "$(tr -d '[:space:]' < /var/lib/dbus/machine-id)" ]; then
  MACHINE_ID_SOURCE=/var/lib/dbus/machine-id
fi
MACHINE_ID_HASH=
if [ -n "$MACHINE_ID_SOURCE" ]; then
  MACHINE_ID_HASH=$(printf '%s' "$(tr -d '[:space:]' < "$MACHINE_ID_SOURCE")" | sha256sum | cut -d' ' -f1)
fi
printf 'AGENTLENS_ARCH=%s\n' "$ARCH"
printf 'AGENTLENS_XDG_DATA_HOME=%s\n' "$XDG_VALUE"
printf 'AGENTLENS_AVAILABLE_KIB=%s\n' "$AVAILABLE_KIB"
printf 'AGENTLENS_MACHINE_ID_SOURCE=%s\n' "$MACHINE_ID_SOURCE"
printf 'AGENTLENS_MACHINE_ID_HASH=%s\n' "$MACHINE_ID_HASH"
"#;

const STAGE2_SCRIPT: &str = r#"AGENTLENS_PAYLOAD=$1
set -eu
umask 077
LC_ALL=C
export LC_ALL
mkdir -p "$HOME/.cache/agentlens"
WORKDIR=$(mktemp -d "$HOME/.cache/agentlens/run.XXXXXX")
printf '%s\n' "$AGENTLENS_PAYLOAD" > "$WORKDIR/request"
printf '%s\n' "$WORKDIR"
"#;

const STAGE4_SCRIPT: &str = r#"AGENTLENS_PAYLOAD=$1
set -eu
umask 077
LC_ALL=C
export LC_ALL
WORKDIR=
MATCH_COUNT=0
for REQUEST_FILE in "$HOME"/.cache/agentlens/run.*/request; do
  [ -f "$REQUEST_FILE" ] || continue
  CANDIDATE=
  IFS= read -r CANDIDATE < "$REQUEST_FILE" || true
  if [ "$CANDIDATE" = "$AGENTLENS_PAYLOAD" ]; then
    MATCH_COUNT=$((MATCH_COUNT + 1))
    WORKDIR=${REQUEST_FILE%/request}
  fi
done
if [ "$MATCH_COUNT" -ne 1 ]; then
  printf '%s\n' 'AgentLens request marker missing or ambiguous' >&2
  exit 71
fi
trap 'rm -rf "$WORKDIR"' EXIT
cd "$WORKDIR"
if ! sha256sum -c collector.sha256 >/dev/null 2>&1; then
  printf '%s\n' 'AGENTLENS_TRANSFER_CORRUPTED' >&2
  exit 70
fi
chmod 700 collector
PATH="$WORKDIR:$PATH"
export PATH
collector collect --request-base64url "$AGENTLENS_PAYLOAD"
"#;

const GC_SCRIPT: &str = r#"set -eu
umask 077
LC_ALL=C
export LC_ALL
if [ -d "$HOME/.cache/agentlens" ]; then
  find "$HOME/.cache/agentlens" -mindepth 1 -maxdepth 1 -type d -name 'run.*' -mmin +1440 -exec rm -rf -- {} +
fi
"#;

static CHECKSUM_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, SshError>;

/// A logical command boundary exposed to fake runners and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandStage {
    StartupProbe,
    Stage1,
    Stage2,
    Stage3,
    Stage4,
    Gc,
}

/// Complete process invocation. All values are argv/env entries; no local shell is involved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub stage: CommandStage,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub detached: bool,
}

/// Platform-neutral command result used by production and fake runners.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Injectable process boundary for every `ssh` and `scp` operation.
pub trait CommandRunner: Send + Sync {
    fn run(&self, command: &CommandSpec) -> io::Result<CommandOutput>;

    fn timeout(&self) -> Option<Duration> {
        None
    }

    fn run_with_cancel(
        &self,
        command: &CommandSpec,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CommandOutput> {
        if is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "SSH operation cancelled before process start",
            ))
        } else {
            self.run(command)
        }
    }
}

/// Production [`CommandRunner`] backed by [`std::process::Command`].
#[derive(Clone, Copy, Debug, Default)]
pub struct StdCommandRunner;

impl StdCommandRunner {
    pub fn with_timeout(timeout: Duration) -> TimedCommandRunner {
        TimedCommandRunner {
            timeout,
            started: Instant::now(),
        }
    }
}

impl CommandRunner for StdCommandRunner {
    fn run(&self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        run_process(spec, None, &|| false)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimedCommandRunner {
    timeout: Duration,
    started: Instant,
}

impl CommandRunner for TimedCommandRunner {
    fn run(&self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        run_process(spec, Some(self.remaining_timeout()?), &|| false)
    }

    fn run_with_cancel(
        &self,
        spec: &CommandSpec,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CommandOutput> {
        run_process(spec, Some(self.remaining_timeout()?), is_cancelled)
    }

    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }
}

impl TimedCommandRunner {
    fn remaining_timeout(&self) -> io::Result<Duration> {
        let remaining = self.timeout.saturating_sub(self.started.elapsed());
        if remaining.is_zero() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "operation exceeded the {} ms shared wall-clock limit before process start",
                    self.timeout.as_millis()
                ),
            ))
        } else {
            Ok(remaining)
        }
    }
}

fn run_process(
    spec: &CommandSpec,
    timeout: Option<Duration>,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<CommandOutput> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let process_tree = if timeout.is_some() {
        Some(ProcessTree::prepare(&mut command)?)
    } else {
        if spec.detached {
            configure_detached(&mut command);
        }
        None
    };
    let mut child = command.spawn()?;
    if let Some(tree) = &process_tree {
        if let Err(error) = tree.attach(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    if !spec.stdin.is_empty() {
        if let Err(error) = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("command stdin pipe is unavailable"))
            .and_then(|mut stdin| stdin.write_all(&spec.stdin))
        {
            terminate_child(&mut child, process_tree.as_ref())?;
            return Err(error);
        }
    }
    drop(child.stdin.take());

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("command stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("command stderr pipe is unavailable"))?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let started = Instant::now();
    let mut status = None;
    loop {
        if is_cancelled() {
            terminate_child(&mut child, process_tree.as_ref())?;
            join_reader(stdout_reader)?;
            join_reader(stderr_reader)?;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "SSH operation cancelled; process tree terminated",
            ));
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break;
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            let limit = timeout.expect("checked timeout");
            let cleanup_error = terminate_child(&mut child, process_tree.as_ref()).err();
            if cleanup_error.is_none() {
                join_reader(stdout_reader)?;
                join_reader(stderr_reader)?;
            }
            return Err(process_timeout_error(limit, cleanup_error));
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    }

    Ok(CommandOutput {
        status: status
            .expect("process and pipe readers finished")
            .code()
            .unwrap_or(-1),
        stdout: join_reader(stdout_reader)?,
        stderr: join_reader(stderr_reader)?,
    })
}

fn process_timeout_error(timeout: Duration, cleanup_error: Option<io::Error>) -> io::Error {
    let message = if let Some(cleanup_error) = cleanup_error {
        format!(
            "process exceeded the {} ms wall-clock limit; process tree cleanup also failed: {cleanup_error}",
            timeout.as_millis()
        )
    } else {
        format!(
            "process exceeded the {} ms wall-clock limit and its process tree was terminated",
            timeout.as_millis()
        )
    };
    io::Error::new(io::ErrorKind::TimedOut, message)
}

fn read_pipe(mut pipe: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| io::Error::other("command pipe reader panicked"))?
}

fn terminate_child(child: &mut Child, tree: Option<&ProcessTree>) -> io::Result<()> {
    if let Some(tree) = tree {
        tree.terminate(child)
    } else {
        child.kill()?;
        child.wait().map(|_| ())
    }
}

#[cfg(unix)]
struct ProcessTree;

#[cfg(unix)]
impl ProcessTree {
    fn prepare(command: &mut Command) -> io::Result<Self> {
        configure_detached(command);
        Ok(Self)
    }

    fn attach(&self, _child: &Child) -> io::Result<()> {
        Ok(())
    }

    fn terminate(&self, child: &mut Child) -> io::Result<()> {
        const ESRCH: i32 = 3;
        const SIGKILL: i32 = 9;

        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }

        let process_group = -i32::try_from(child.id())
            .map_err(|_| io::Error::other("child pid does not fit i32"))?;
        // SAFETY: the controlled child called `setsid` before exec, so its PID is also the
        // process-group id and a negative PID targets that whole isolated group.
        if unsafe { kill(process_group, SIGKILL) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ESRCH) {
                return Err(error);
            }
        }
        let _ = child.kill();
        child.wait().map(|_| ())
    }
}

#[cfg(windows)]
struct ProcessTree {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessTree {
    fn prepare(command: &mut Command) -> io::Result<Self> {
        use std::mem::size_of;
        use std::os::windows::process::CommandExt as _;
        use std::ptr;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);

        // SAFETY: null security/name pointers request a private job with default security.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` has the exact layout and byte length requested by this information class.
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            // SAFETY: `handle` was returned by CreateJobObjectW and is still owned here.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self { handle })
    }

    fn attach(&self, child: &Child) -> io::Result<()> {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        };
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        };

        // SAFETY: both handles are valid for the duration of the call; children inherit the job.
        if unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle().cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: the snapshot handle is checked and every opened Win32 handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = false;
        // SAFETY: `entry.dwSize` is initialized and `snapshot` is a valid thread snapshot.
        let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
        while has_entry {
            if entry.th32OwnerProcessID == child.id() {
                // SAFETY: the thread id came from the live snapshot; the returned handle is checked.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    // SAFETY: `thread` grants THREAD_SUSPEND_RESUME and is closed immediately after.
                    resumed = unsafe { ResumeThread(thread) } != u32::MAX;
                    unsafe { CloseHandle(thread) };
                    if resumed {
                        break;
                    }
                }
            }
            // SAFETY: `entry` remains initialized for iteration over the valid snapshot.
            has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
        }
        // SAFETY: `snapshot` is uniquely owned by this function.
        unsafe { CloseHandle(snapshot) };
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other(
                "unable to resume the job-assigned child process",
            ))
        }
    }

    fn terminate(&self, child: &mut Child) -> io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: this handle remains owned by `self`; termination covers every associated child.
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        child.wait().map(|_| ())
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: the job handle is uniquely owned. KILL_ON_JOB_CLOSE is a final safety net.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn prepare(_command: &mut Command) -> io::Result<Self> {
        Ok(Self)
    }

    fn attach(&self, _child: &Child) -> io::Result<()> {
        Ok(())
    }

    fn terminate(&self, child: &mut Child) -> io::Result<()> {
        child.kill()?;
        child.wait().map(|_| ())
    }
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    unsafe extern "C" {
        fn setsid() -> i32;
    }

    // SAFETY: `pre_exec` runs after fork and before exec. The closure calls only async-signal-safe
    // `setsid` and constructs an OS error if it fails; it captures no shared process state.
    unsafe {
        command.pre_exec(|| {
            if setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn configure_detached(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn configure_detached(_: &mut Command) {}

/// Located system OpenSSH client pair. Both paths must share one directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshTools {
    pub ssh: PathBuf,
    pub scp: PathBuf,
}

impl SshTools {
    pub fn new(ssh: impl Into<PathBuf>, scp: impl Into<PathBuf>) -> Result<Self> {
        let tools = Self {
            ssh: ssh.into(),
            scp: scp.into(),
        };
        if tools.ssh.parent() != tools.scp.parent() {
            return Err(SshError::InvalidInput {
                detail: "ssh 与 scp 必须来自同一目录".into(),
            });
        }
        Ok(tools)
    }
}

/// Resolve an OpenSSH pair in the fixed order: PATH, explicit setting, Windows system directory.
pub fn discover_ssh_tools(explicit: Option<&SshTools>) -> Result<SshTools> {
    if let Some(tools) = tools_on_path() {
        return Ok(tools);
    }
    if let Some(tools) = explicit {
        if tools.ssh.is_file() && tools.scp.is_file() {
            return SshTools::new(&tools.ssh, &tools.scp);
        }
    }
    #[cfg(windows)]
    if let Some(windir) = env::var_os("WINDIR") {
        let directory = PathBuf::from(windir).join("System32").join("OpenSSH");
        if let Some(tools) = tools_in_directory(&directory) {
            return Ok(tools);
        }
    }
    Err(SshError::SshUnavailable {
        detail: "PATH、显式设置及系统 OpenSSH 目录均未找到成对的 ssh/scp".into(),
    })
}

fn tools_on_path() -> Option<SshTools> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|directory| tools_in_directory(&directory))
}

fn tools_in_directory(directory: &Path) -> Option<SshTools> {
    #[cfg(windows)]
    let (ssh_name, scp_name) = ("ssh.exe", "scp.exe");
    #[cfg(not(windows))]
    let (ssh_name, scp_name) = ("ssh", "scp");
    let ssh = directory.join(ssh_name);
    let scp = directory.join(scp_name);
    if ssh.is_file() && scp.is_file() {
        SshTools::new(ssh, scp).ok()
    } else {
        None
    }
}

/// SSH authentication mode. Askpass secrets are obtained by the helper from a one-shot channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SshAuthentication {
    Batch {
        identity_file: Option<PathBuf>,
    },
    Askpass {
        executable: PathBuf,
        ipc_channel: OsString,
        identity_file: Option<PathBuf>,
    },
}

impl SshAuthentication {
    fn identity_file(&self) -> Option<&Path> {
        match self {
            Self::Batch { identity_file } | Self::Askpass { identity_file, .. } => {
                identity_file.as_deref()
            }
        }
    }

    fn apply(&self, args: &mut Vec<OsString>, env: &mut BTreeMap<OsString, OsString>) -> bool {
        if let Some(identity_file) = self.identity_file() {
            args.push("-i".into());
            args.push(identity_file.as_os_str().to_owned());
        }
        match self {
            Self::Batch { .. } => {
                args.push("-o".into());
                args.push("BatchMode=yes".into());
                false
            }
            Self::Askpass {
                executable,
                ipc_channel,
                ..
            } => {
                env.insert("SSH_ASKPASS".into(), executable.as_os_str().to_owned());
                env.insert("SSH_ASKPASS_REQUIRE".into(), "force".into());
                env.insert("DISPLAY".into(), ":0".into());
                env.insert(ASKPASS_CHANNEL_ENV.into(), ipc_channel.clone());
                true
            }
        }
    }
}

/// Per-architecture collector artifacts available for transfer.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollectorArtifacts {
    pub x86_64: Option<PathBuf>,
    pub aarch64: Option<PathBuf>,
}

impl CollectorArtifacts {
    pub fn with_x86_64(mut self, path: impl Into<PathBuf>) -> Self {
        self.x86_64 = Some(path.into());
        self
    }

    pub fn with_aarch64(mut self, path: impl Into<PathBuf>) -> Self {
        self.aarch64 = Some(path.into());
        self
    }

    fn artifact_for(&self, architecture: RemoteArchitecture) -> Option<&Path> {
        match architecture {
            RemoteArchitecture::X86_64 => self.x86_64.as_deref(),
            RemoteArchitecture::Aarch64 => self.aarch64.as_deref(),
        }
    }

    fn available_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.x86_64.is_some() {
            names.push("x86_64".into());
        }
        if self.aarch64.is_some() {
            names.push("aarch64".into());
        }
        names
    }
}

/// Collector request serialized as base64url JSON and consumed only by Rust/serde remotely.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CollectPayload {
    pub since: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    pub snapshot: bool,
}

/// Encode the exact collector request format (`URL_SAFE_NO_PAD`).
pub fn encode_collect_payload(payload: &CollectPayload) -> Result<String> {
    if payload.since < 0 {
        return Err(SshError::InvalidInput {
            detail: format!("since 不能为负数，收到 {}", payload.since),
        });
    }
    let json = serde_json::to_vec(payload).map_err(|error| SshError::InvalidInput {
        detail: format!("无法序列化采集请求：{error}"),
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// Decode and validate the Rust-side payload format without invoking a shell.
pub fn decode_collect_payload(encoded: &str) -> Result<CollectPayload> {
    validate_payload_token(encoded)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| SshError::InvalidInput {
            detail: format!("base64url 载荷解码失败：{error}"),
        })?;
    serde_json::from_slice(&decoded).map_err(|error| SshError::InvalidInput {
        detail: format!("载荷解码后不是合法 collect JSON：{error}"),
    })
}

fn validate_payload_token(payload: &str) -> Result<()> {
    if payload.is_empty()
        || !payload
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(SshError::InvalidInput {
            detail: "载荷必须是非空、无 padding 的 base64url token".into(),
        });
    }
    Ok(())
}

fn assemble_script(script: &str) -> Vec<u8> {
    let mut framed = Vec::with_capacity(script.len() + 1);
    framed.extend_from_slice(script.as_bytes());
    if !script.ends_with('\n') {
        framed.push(b'\n');
    }
    framed
}

/// Remote architecture selected by `uname -m`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteArchitecture {
    X86_64,
    Aarch64,
}

/// Parsed STAGE1 facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshProbe {
    pub architecture: RemoteArchitecture,
    pub xdg_data_home: Option<String>,
    pub available_kib: u64,
    pub machine_id_source: String,
    /// SHA-256 of the remote machine id, computed on the remote host.
    ///
    /// Always [`MACHINE_ID_HASH_HEX_LENGTH`] lowercase hex characters — `parse_probe` rejects
    /// anything else, so callers may feed this straight to
    /// [`crate::host::MachineIdentity::from_machine_id_hash`].
    pub machine_id_hash: String,
}

/// Input to one SSH collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshCollectRequest {
    pub ssh_target: String,
    pub since: i64,
    pub data_dir: Option<PathBuf>,
    pub snapshot: bool,
}

/// Validated collector output. The first line is a v1 metadata object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshCollection {
    pub probe: SshProbe,
    pub ndjson: Vec<u8>,
}

/// Typed SSH transport errors with UI-ready Chinese remediation.
#[derive(Debug, Error)]
pub enum SshError {
    #[error("远端架构 {remote_arch:?} 没有匹配的 collector 产物（可用：{available:?}）")]
    ArchMismatch {
        remote_arch: String,
        available: Vec<String>,
    },
    #[error("远端缓存目录不可写：{detail}")]
    NoWritableCache { detail: String },
    #[error("collector 传输完整性校验失败：{detail}")]
    TransferCorrupted { detail: String },
    #[error("SSH 认证失败：{detail}")]
    AuthFailed { detail: String },
    #[error("远端 OpenCode 数据目录不可用：{detail}")]
    NoDataDir { detail: String },
    #[error("远端 OpenCode 数据库或 WAL/SHM 不可读：{detail}")]
    WalUnreadable { detail: String },
    #[error("客户端已取消 SSH 采集")]
    ClientCancelled,
    #[error("{stage:?} 超过 {timeout_ms} 毫秒硬超时：{detail}")]
    TimedOut {
        stage: CommandStage,
        timeout_ms: u128,
        detail: String,
    },
    #[error("SSH 传输已降级停用：{detail}")]
    SshUnavailable { detail: String },
    #[error("SSH 输入无效：{detail}")]
    InvalidInput { detail: String },
    #[error("{stage:?} 返回无效响应：{detail}")]
    InvalidResponse { stage: CommandStage, detail: String },
    #[error("{stage:?} 进程调用失败：{detail}")]
    Runner { stage: CommandStage, detail: String },
}

impl SshError {
    /// Human-actionable remediation. Every branch intentionally contains Chinese guidance.
    pub const fn remediation(&self) -> &'static str {
        match self {
            Self::ArchMismatch { .. } => "请安装与远端 uname -m 匹配的 collector 产物后重试。",
            Self::NoWritableCache { .. } => {
                "请确认远端用户可创建 ~/.cache/agentlens，且该文件系统空间充足。"
            }
            Self::TransferCorrupted { .. } => {
                "请重试传输；若持续失败，请检查 ssh/scp 路径和远端磁盘完整性。"
            }
            Self::AuthFailed { .. } => "请检查 SSH 用户、密钥、agent 或钥匙串口令后重试。",
            Self::NoDataDir { .. } => "请设置正确的远端 data-dir，使其包含可读的 opencode.db。",
            Self::WalUnreadable { .. } => {
                "请用 chmod 或所属组授予数据库及 WAL/SHM 只读权限；不要用 snapshot 绕过权限。"
            }
            Self::ClientCancelled => {
                "可安全重新发起采集；遗留 run.* 会在下次连接的 24 小时 GC 中清理。"
            }
            Self::TimedOut { .. } => {
                "连接测试已终止 SSH 进程树；请检查网络、代理、远端 sshd 与认证交互后重试。"
            }
            Self::SshUnavailable { .. } => {
                "请安装 OpenSSH 客户端，或配置同一目录下的 ssh 与 scp 路径。"
            }
            Self::InvalidInput { .. } => "请修正 SSH 目标、游标或 UTF-8 数据目录后重试。",
            Self::InvalidResponse { .. } => {
                "请检查远端 shell、collector 版本及输出是否符合 AgentLens v1 协议。"
            }
            Self::Runner { .. } => "请查看命令 stderr，修复远端环境或连接问题后重试。",
        }
    }
}

/// Four-stage SSH collector transport with a per-connection stale-directory GC.
pub struct SshTransport<R> {
    runner: R,
    tools: SshTools,
    authentication: SshAuthentication,
    artifacts: CollectorArtifacts,
}

impl<R: CommandRunner> SshTransport<R> {
    /// Construct the transport and immediately prove `ssh -V` is runnable.
    pub fn new(
        runner: R,
        tools: SshTools,
        authentication: SshAuthentication,
        artifacts: CollectorArtifacts,
    ) -> Result<Self> {
        Self::new_with_cancel(runner, tools, authentication, artifacts, &|| false)
    }

    fn new_with_cancel(
        runner: R,
        tools: SshTools,
        authentication: SshAuthentication,
        artifacts: CollectorArtifacts,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        let transport = Self {
            runner,
            tools,
            authentication,
            artifacts,
        };
        transport.probe_ssh_binary(is_cancelled)?;
        Ok(transport)
    }

    /// Discover tools in the mandated order, then perform the startup probe.
    pub fn discover(
        runner: R,
        explicit: Option<&SshTools>,
        authentication: SshAuthentication,
        artifacts: CollectorArtifacts,
    ) -> Result<Self> {
        Self::discover_with_cancel(runner, explicit, authentication, artifacts, &|| false)
    }

    fn discover_with_cancel(
        runner: R,
        explicit: Option<&SshTools>,
        authentication: SshAuthentication,
        artifacts: CollectorArtifacts,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        Self::new_with_cancel(
            runner,
            discover_ssh_tools(explicit)?,
            authentication,
            artifacts,
            is_cancelled,
        )
    }

    pub fn collect(&self, request: &SshCollectRequest) -> Result<SshCollection> {
        self.collect_with_cancel(request, &|| false)
    }

    pub fn probe_connection(&self, target: &str) -> Result<SshProbe> {
        self.probe_connection_with_cancel(target, &|| false)
    }

    pub fn probe_connection_with_cancel(
        &self,
        target: &str,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<SshProbe> {
        validate_ssh_target(target)?;
        ensure_not_cancelled(is_cancelled)?;
        let output = self.run_ssh_command_with_cancel(
            CommandStage::Stage1,
            target,
            None,
            STAGE1_SCRIPT,
            is_cancelled,
        )?;
        ensure_success(CommandStage::Stage1, &output)?;
        ensure_not_cancelled(is_cancelled)?;
        parse_probe(&output.stdout, &self.artifacts)
    }

    /// Collect with a deterministic cancellation seam checked between process stages.
    pub fn collect_with_cancel(
        &self,
        request: &SshCollectRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<SshCollection> {
        validate_ssh_target(&request.ssh_target)?;
        let data_dir = request
            .data_dir
            .as_deref()
            .map(path_to_utf8)
            .transpose()?
            .map(str::to_owned);
        let payload = encode_collect_payload(&CollectPayload {
            since: request.since,
            data_dir,
            snapshot: request.snapshot,
        })?;
        ensure_not_cancelled(is_cancelled)?;

        let stage1 =
            self.run_ssh_without_payload(CommandStage::Stage1, &request.ssh_target, STAGE1_SCRIPT)?;
        ensure_success(CommandStage::Stage1, &stage1)?;
        let probe = parse_probe(&stage1.stdout, &self.artifacts)?;
        let artifact = self
            .artifacts
            .artifact_for(probe.architecture)
            .ok_or_else(|| SshError::ArchMismatch {
                remote_arch: architecture_name(probe.architecture).into(),
                available: self.artifacts.available_names(),
            })?;
        ensure_not_cancelled(is_cancelled)?;

        let stage2 = self.run_ssh(
            CommandStage::Stage2,
            &request.ssh_target,
            &payload,
            STAGE2_SCRIPT,
        )?;
        if stage2.status != 0 {
            return Err(map_unsuccessful(CommandStage::Stage2, &stage2));
        }
        let workdir = parse_workdir(&stage2.stdout)?;
        ensure_not_cancelled(is_cancelled)?;

        let digest = sha256_file(artifact)?;
        let checksum = ChecksumManifest::create(&digest)?;
        self.run_scp(
            &request.ssh_target,
            artifact,
            &format!("{workdir}/collector"),
        )?;
        self.run_scp(
            &request.ssh_target,
            checksum.path(),
            &format!("{workdir}/{CHECKSUM_FILE_NAME}"),
        )?;
        ensure_not_cancelled(is_cancelled)?;

        let stage4 = self.run_ssh(
            CommandStage::Stage4,
            &request.ssh_target,
            &payload,
            STAGE4_SCRIPT,
        )?;
        if stage4.status != 0 {
            return Err(map_unsuccessful(CommandStage::Stage4, &stage4));
        }
        validate_ndjson(&stage4.stdout)?;
        ensure_not_cancelled(is_cancelled)?;

        let gc = self.run_ssh_without_payload(CommandStage::Gc, &request.ssh_target, GC_SCRIPT)?;
        ensure_success(CommandStage::Gc, &gc)?;
        Ok(SshCollection {
            probe,
            ndjson: stage4.stdout,
        })
    }

    fn probe_ssh_binary(&self, is_cancelled: &dyn Fn() -> bool) -> Result<()> {
        let spec = CommandSpec {
            stage: CommandStage::StartupProbe,
            program: self.tools.ssh.clone(),
            args: vec!["-V".into()],
            env: BTreeMap::new(),
            stdin: Vec::new(),
            detached: false,
        };
        let output = self
            .runner
            .run_with_cancel(&spec, is_cancelled)
            .map_err(|error| {
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
                ) {
                    map_process_error(CommandStage::StartupProbe, error, self.runner.timeout())
                } else {
                    SshError::SshUnavailable {
                        detail: format!("执行 {} -V 失败：{error}", self.tools.ssh.display()),
                    }
                }
            })?;
        if output.status != 0 {
            return Err(SshError::SshUnavailable {
                detail: output_detail(&output),
            });
        }
        Ok(())
    }

    fn run_ssh(
        &self,
        stage: CommandStage,
        target: &str,
        payload: &str,
        script: &str,
    ) -> Result<CommandOutput> {
        self.run_ssh_command(stage, target, Some(payload), script)
    }

    fn run_ssh_without_payload(
        &self,
        stage: CommandStage,
        target: &str,
        script: &str,
    ) -> Result<CommandOutput> {
        self.run_ssh_command(stage, target, None, script)
    }

    fn run_ssh_command(
        &self,
        stage: CommandStage,
        target: &str,
        payload: Option<&str>,
        script: &str,
    ) -> Result<CommandOutput> {
        self.run_ssh_command_with_cancel(stage, target, payload, script, &|| false)
    }

    fn run_ssh_command_with_cancel(
        &self,
        stage: CommandStage,
        target: &str,
        payload: Option<&str>,
        script: &str,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<CommandOutput> {
        let mut args = vec![
            "-T".into(),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            CONNECT_TIMEOUT_OPTION.into(),
        ];
        let mut command_env = BTreeMap::new();
        let detached = self.authentication.apply(&mut args, &mut command_env);
        args.push("--".into());
        args.push(target.into());
        args.push(REMOTE_COMMAND.into());
        if let Some(payload) = payload {
            validate_payload_token(payload)?;
            args.push("--".into());
            args.push(payload.into());
        }
        let spec = CommandSpec {
            stage,
            program: self.tools.ssh.clone(),
            args,
            env: command_env,
            stdin: assemble_script(script),
            detached,
        };
        let output = self
            .runner
            .run_with_cancel(&spec, is_cancelled)
            .map_err(|error| map_process_error(stage, error, self.runner.timeout()))?;
        if is_auth_failure(&output) {
            return Err(SshError::AuthFailed {
                detail: output_detail(&output),
            });
        }
        Ok(output)
    }

    fn run_scp(&self, target: &str, local: &Path, remote_path: &str) -> Result<()> {
        let mut args = vec![
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
        ];
        let mut command_env = BTreeMap::new();
        let detached = self.authentication.apply(&mut args, &mut command_env);
        args.push("--".into());
        args.push(local.as_os_str().to_owned());
        args.push(format!("{target}:{remote_path}").into());
        let spec = CommandSpec {
            stage: CommandStage::Stage3,
            program: self.tools.scp.clone(),
            args,
            env: command_env,
            stdin: Vec::new(),
            detached,
        };
        let output = self.runner.run(&spec).map_err(|error| SshError::Runner {
            stage: CommandStage::Stage3,
            detail: error.to_string(),
        })?;
        if is_auth_failure(&output) {
            return Err(SshError::AuthFailed {
                detail: output_detail(&output),
            });
        }
        if output.status != 0 {
            return Err(SshError::TransferCorrupted {
                detail: output_detail(&output),
            });
        }
        Ok(())
    }
}

impl SshTransport<TimedCommandRunner> {
    pub fn probe_connection_with_timeout(
        _runner: StdCommandRunner,
        explicit: Option<&SshTools>,
        authentication: SshAuthentication,
        artifacts: CollectorArtifacts,
        target: &str,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<SshProbe> {
        let transport = Self::discover_with_cancel(
            StdCommandRunner::with_timeout(timeout),
            explicit,
            authentication,
            artifacts,
            is_cancelled,
        )
        .map_err(|error| attach_timeout(error, timeout))?;
        transport
            .probe_connection_with_cancel(target, is_cancelled)
            .map_err(|error| attach_timeout(error, timeout))
    }
}

fn map_process_error(stage: CommandStage, error: io::Error, timeout: Option<Duration>) -> SshError {
    match error.kind() {
        io::ErrorKind::Interrupted => SshError::ClientCancelled,
        io::ErrorKind::TimedOut => SshError::TimedOut {
            stage,
            timeout_ms: timeout.map_or(0, |duration| duration.as_millis()),
            detail: error.to_string(),
        },
        _ => SshError::Runner {
            stage,
            detail: error.to_string(),
        },
    }
}

fn attach_timeout(error: SshError, timeout: Duration) -> SshError {
    match error {
        SshError::TimedOut { stage, detail, .. } => SshError::TimedOut {
            stage,
            timeout_ms: timeout.as_millis(),
            detail,
        },
        other => other,
    }
}

fn path_to_utf8(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| SshError::InvalidInput {
        detail: format!("data-dir 不是有效 UTF-8：{}", path.display()),
    })
}

fn validate_ssh_target(target: &str) -> Result<()> {
    if target.is_empty()
        || target.starts_with('-')
        || target.chars().any(|character| {
            character.is_control() || character.is_whitespace() || matches!(character, '\'' | '"')
        })
    {
        return Err(SshError::InvalidInput {
            detail: format!("ssh_target 非法或可能被解释为选项：{target:?}"),
        });
    }
    Ok(())
}

fn ensure_not_cancelled(is_cancelled: &dyn Fn() -> bool) -> Result<()> {
    if is_cancelled() {
        Err(SshError::ClientCancelled)
    } else {
        Ok(())
    }
}

fn parse_probe(bytes: &[u8], artifacts: &CollectorArtifacts) -> Result<SshProbe> {
    let text = std::str::from_utf8(bytes).map_err(|error| SshError::InvalidResponse {
        stage: CommandStage::Stage1,
        detail: format!("探测输出不是 UTF-8：{error}"),
    })?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| SshError::InvalidResponse {
                stage: CommandStage::Stage1,
                detail: format!("探测行缺少 '='：{line:?}"),
            })?;
        if value.chars().any(char::is_control) {
            return Err(SshError::InvalidResponse {
                stage: CommandStage::Stage1,
                detail: format!("探测值含控制字符：{key}"),
            });
        }
        values.insert(key, value);
    }
    let remote_arch = required_probe_value(&values, "AGENTLENS_ARCH")?;
    let architecture = match remote_arch {
        "x86_64" | "amd64" => RemoteArchitecture::X86_64,
        "aarch64" | "arm64" => RemoteArchitecture::Aarch64,
        unsupported => {
            return Err(SshError::ArchMismatch {
                remote_arch: unsupported.into(),
                available: artifacts.available_names(),
            });
        }
    };
    if artifacts.artifact_for(architecture).is_none() {
        return Err(SshError::ArchMismatch {
            remote_arch: remote_arch.into(),
            available: artifacts.available_names(),
        });
    }
    let xdg = required_probe_value(&values, "AGENTLENS_XDG_DATA_HOME")?;
    let available_kib = required_probe_value(&values, "AGENTLENS_AVAILABLE_KIB")?
        .parse::<u64>()
        .map_err(|error| SshError::InvalidResponse {
            stage: CommandStage::Stage1,
            detail: format!("df 可用空间不是整数：{error}"),
        })?;
    let machine_id_source = required_probe_value(&values, "AGENTLENS_MACHINE_ID_SOURCE")?;
    if machine_id_source.is_empty() {
        return Err(SshError::InvalidResponse {
            stage: CommandStage::Stage1,
            detail: "远端 /etc/machine-id 与 /var/lib/dbus/machine-id 均不可读或为空".into(),
        });
    }
    let machine_id_hash = required_probe_value(&values, "AGENTLENS_MACHINE_ID_HASH")?;
    if !is_machine_id_hash(machine_id_hash) {
        return Err(SshError::InvalidResponse {
            stage: CommandStage::Stage1,
            detail: format!(
                "远端 machine-id 摘要不是 {MACHINE_ID_HASH_HEX_LENGTH} 位小写十六进制：{machine_id_hash:?}"
            ),
        });
    }
    Ok(SshProbe {
        architecture,
        xdg_data_home: (!xdg.is_empty()).then(|| xdg.to_owned()),
        available_kib,
        machine_id_source: machine_id_source.to_owned(),
        machine_id_hash: machine_id_hash.to_owned(),
    })
}

/// Uppercase hex is rejected on purpose: [`crate::host::MachineIdentity::from_machine_id_hash`]
/// accepts lowercase only, so accepting it here would just move the failure to the UI.
fn is_machine_id_hash(value: &str) -> bool {
    value.len() == MACHINE_ID_HASH_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn required_probe_value<'a>(values: &'a BTreeMap<&str, &str>, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .copied()
        .ok_or_else(|| SshError::InvalidResponse {
            stage: CommandStage::Stage1,
            detail: format!("探测输出缺少 {key}"),
        })
}

fn parse_workdir(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|error| SshError::NoWritableCache {
        detail: format!("mktemp 输出不是 UTF-8：{error}"),
    })?;
    let path = text.trim_end_matches(['\r', '\n']);
    if path.is_empty()
        || !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.lines().count() != 1
    {
        return Err(SshError::NoWritableCache {
            detail: format!("mktemp 未返回合法绝对路径：{text:?}"),
        });
    }
    Ok(path.to_owned())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|error| SshError::InvalidInput {
        detail: format!("无法读取 collector 产物 {}：{error}", path.display()),
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

struct ChecksumManifest {
    path: PathBuf,
}

impl ChecksumManifest {
    fn create(digest: &str) -> Result<Self> {
        let sequence = CHECKSUM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            ".agentlens-collector-sha256-{}-{sequence}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|error| SshError::Runner {
            stage: CommandStage::Stage3,
            detail: format!("无法创建临时 sha256 清单 {}：{error}", path.display()),
        })?;
        if let Err(error) = writeln!(file, "{digest}  collector") {
            let _ = fs::remove_file(&path);
            return Err(SshError::Runner {
                stage: CommandStage::Stage3,
                detail: format!("无法写入临时 sha256 清单 {}：{error}", path.display()),
            });
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ChecksumManifest {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn ensure_success(stage: CommandStage, output: &CommandOutput) -> Result<()> {
    if output.status == 0 {
        Ok(())
    } else {
        Err(map_unsuccessful(stage, output))
    }
}

fn map_unsuccessful(stage: CommandStage, output: &CommandOutput) -> SshError {
    let detail = output_detail(output);
    match (stage, output.status) {
        (CommandStage::Stage2, _) | (CommandStage::Stage4, 3) => {
            SshError::NoWritableCache { detail }
        }
        (CommandStage::Stage4, TRANSFER_CORRUPTED_EXIT) => SshError::TransferCorrupted { detail },
        (CommandStage::Stage4, 2) => SshError::NoDataDir { detail },
        (CommandStage::Stage4, 4) => SshError::WalUnreadable { detail },
        _ => SshError::Runner { stage, detail },
    }
}

fn is_auth_failure(output: &CommandOutput) -> bool {
    if output.status != 255 {
        return false;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    stderr.contains("permission denied")
        || stderr.contains("authentication failed")
        || stderr.contains("no supported authentication methods")
}

fn output_detail(output: &CommandOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if detail.is_empty() {
        format!("exit {} 且无输出", output.status)
    } else {
        format!("exit {}: {detail}", output.status)
    }
}

fn architecture_name(architecture: RemoteArchitecture) -> &'static str {
    match architecture {
        RemoteArchitecture::X86_64 => "x86_64",
        RemoteArchitecture::Aarch64 => "aarch64",
    }
}

fn validate_ndjson(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Err(SshError::InvalidResponse {
            stage: CommandStage::Stage4,
            detail: "collector exit 0 但 stdout 为空，缺少 meta 行".into(),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|error| SshError::InvalidResponse {
        stage: CommandStage::Stage4,
        detail: format!("collector stdout 不是 UTF-8：{error}"),
    })?;
    let mut lines = text.lines();
    let meta_line = lines.next().ok_or_else(|| SshError::InvalidResponse {
        stage: CommandStage::Stage4,
        detail: "缺少 meta 行".into(),
    })?;
    let meta: serde_json::Value =
        serde_json::from_str(meta_line).map_err(|error| SshError::InvalidResponse {
            stage: CommandStage::Stage4,
            detail: format!("meta 行不是 JSON：{error}"),
        })?;
    if meta
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || meta
            .get("machine_id_hash")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|hash| hash.len() != 64)
        || meta
            .get("sources")
            .and_then(serde_json::Value::as_array)
            .is_none()
    {
        return Err(SshError::InvalidResponse {
            stage: CommandStage::Stage4,
            detail: "meta 行缺少 v1 protocol_version、machine_id_hash 或 sources".into(),
        });
    }
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line).map_err(|error| {
            SshError::InvalidResponse {
                stage: CommandStage::Stage4,
                detail: format!("record 行 {} 不是 JSON：{error}", index + 2),
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    #[cfg(not(unix))]
    use std::io;
    #[cfg(unix)]
    use std::io::{self, Write as _};
    use std::path::Path;
    use std::path::PathBuf;
    #[cfg(any(unix, windows))]
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use base64::Engine as _;
    #[cfg(unix)]
    use sha2::{Digest as _, Sha256};

    use super::*;

    const PROBE_MACHINE_ID_HASH: &str =
        "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
    const STAGE1_DIGEST_EXPR: &str =
        r#"printf '%s' "$(tr -d '[:space:]' < "$MACHINE_ID_SOURCE")" | sha256sum | cut -d' ' -f1"#;
    const PROBE_X86_64: &str = "AGENTLENS_ARCH=x86_64\n\
AGENTLENS_XDG_DATA_HOME=/home/test/.local/share\n\
AGENTLENS_AVAILABLE_KIB=1048576\n\
AGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\n\
AGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n";
    const REMOTE_RUN_DIR: &str = "/home/test/.cache/agentlens/run.A1b2C3";
    const META_LINE: &str = "{\"protocol_version\":1,\"machine_id_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"hostname\":\"fixture\",\"collector_version\":\"0.1.0\",\"sources\":[{\"source\":\"opencode\",\"data_dir\":\"/data\",\"scan_window\":{\"since\":0,\"cutoff\":0},\"eligible_count\":0,\"skipped_count\":0}]}\n";

    struct FakeResponse {
        expected_stage: CommandStage,
        result: io::Result<CommandOutput>,
    }

    #[derive(Clone, Default)]
    struct FakeRunner {
        responses: Arc<Mutex<VecDeque<FakeResponse>>>,
        commands: Arc<Mutex<Vec<CommandSpec>>>,
        stage3_calls: Arc<AtomicUsize>,
    }

    impl FakeRunner {
        fn push(&self, stage: CommandStage, status: i32, stdout: &str, stderr: &str) {
            self.responses
                .lock()
                .expect("response lock")
                .push_back(FakeResponse {
                    expected_stage: stage,
                    result: Ok(CommandOutput {
                        status,
                        stdout: stdout.as_bytes().to_vec(),
                        stderr: stderr.as_bytes().to_vec(),
                    }),
                });
        }

        fn push_error(&self, stage: CommandStage, error: io::Error) {
            self.responses
                .lock()
                .expect("response lock")
                .push_back(FakeResponse {
                    expected_stage: stage,
                    result: Err(error),
                });
        }

        fn commands(&self) -> Vec<CommandSpec> {
            self.commands.lock().expect("command lock").clone()
        }

        fn stage3_calls(&self) -> usize {
            self.stage3_calls.load(Ordering::SeqCst)
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, command: &CommandSpec) -> io::Result<CommandOutput> {
            self.commands
                .lock()
                .expect("command lock")
                .push(command.clone());
            if command.stage == CommandStage::Stage3 {
                self.stage3_calls.fetch_add(1, Ordering::SeqCst);
            }
            let response = self
                .responses
                .lock()
                .expect("response lock")
                .pop_front()
                .expect("unexpected command without fake response");
            assert_eq!(command.stage, response.expected_stage);
            response.result
        }
    }

    fn artifact() -> (tempfile::TempDir, CollectorArtifacts) {
        let temp = tempfile::tempdir().expect("collector tempdir");
        let collector = temp.path().join("agentlens-collector");
        fs::write(&collector, b"fixture collector bytes").expect("write collector artifact");
        (temp, CollectorArtifacts::default().with_x86_64(collector))
    }

    fn tools() -> SshTools {
        SshTools::new("/usr/bin/ssh", "/usr/bin/scp").expect("valid paired tools")
    }

    fn request() -> SshCollectRequest {
        SshCollectRequest {
            ssh_target: "test@example.invalid".into(),
            since: 0,
            data_dir: None,
            snapshot: false,
        }
    }

    fn push_startup(runner: &FakeRunner) {
        runner.push(CommandStage::StartupProbe, 0, "", "OpenSSH_9.9p1 fixture");
    }

    fn push_successful_collection(runner: &FakeRunner) {
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, &format!("{REMOTE_RUN_DIR}\n"), "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage4, 0, META_LINE, "");
        runner.push(CommandStage::Gc, 0, "", "");
    }

    fn transport(
        runner: &FakeRunner,
        artifacts: CollectorArtifacts,
    ) -> Result<SshTransport<FakeRunner>> {
        SshTransport::new(
            runner.clone(),
            tools(),
            SshAuthentication::Batch {
                identity_file: None,
            },
            artifacts,
        )
    }

    fn assert_chinese_remediation(error: &SshError) {
        let remediation = error.remediation();
        assert!(!remediation.trim().is_empty());
        assert!(remediation
            .chars()
            .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character)));
    }

    #[test]
    fn ssh_tool_pairs_artifacts_and_probe_aliases_preserve_platform_contracts() {
        let error = SshTools::new("/opt/a/ssh", "/opt/b/scp")
            .expect_err("ssh and scp from different directories must be rejected");
        assert!(matches!(error, SshError::InvalidInput { .. }));

        let temp = tempfile::tempdir().expect("tool tempdir");
        let ssh_name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
        let scp_name = if cfg!(windows) { "scp.exe" } else { "scp" };
        fs::write(temp.path().join(ssh_name), b"fixture ssh").expect("write ssh fixture");
        fs::write(temp.path().join(scp_name), b"fixture scp").expect("write scp fixture");
        let paired = tools_in_directory(temp.path()).expect("complete pair is discoverable");
        assert_eq!(paired.ssh, temp.path().join(ssh_name));
        assert_eq!(paired.scp, temp.path().join(scp_name));
        fs::remove_file(temp.path().join(scp_name)).expect("remove scp fixture");
        assert!(tools_in_directory(temp.path()).is_none());

        let artifacts = CollectorArtifacts::default()
            .with_x86_64("/fixtures/collector-x86_64")
            .with_aarch64("/fixtures/collector-aarch64");
        let probe = parse_probe(
            b"AGENTLENS_ARCH=arm64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=42\nAGENTLENS_MACHINE_ID_SOURCE=/var/lib/dbus/machine-id\nAGENTLENS_MACHINE_ID_HASH=0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0\n",
            &artifacts,
        )
        .expect("arm64 alias with matching artifact");
        assert_eq!(probe.architecture, RemoteArchitecture::Aarch64);
        assert_eq!(probe.xdg_data_home, None);
        assert_eq!(probe.available_kib, 42);
        assert_eq!(probe.machine_id_source, "/var/lib/dbus/machine-id");
        assert_eq!(
            probe.machine_id_hash,
            "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0"
        );
        assert_eq!(architecture_name(probe.architecture), "aarch64");
        assert_eq!(architecture_name(RemoteArchitecture::X86_64), "x86_64");
        assert_eq!(
            artifacts.available_names(),
            vec!["x86_64".to_owned(), "aarch64".to_owned()]
        );
    }

    #[test]
    fn ssh_probe_parser_rejects_each_corrupt_or_incomplete_fact() {
        type ProbeErrorMatcher = fn(&SshError) -> bool;

        let artifacts = CollectorArtifacts::default().with_x86_64("/fixtures/collector");
        let cases: &[(&[u8], ProbeErrorMatcher)] = &[
            (&[0xff], |error| {
                matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("UTF-8"))
            }),
            (
                b"AGENTLENS_ARCH=x86_64\0\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("控制字符")),
            ),
            (
                b"AGENTLENS_ARCH=riscv64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n",
                |error| matches!(error, SshError::ArchMismatch { remote_arch, .. } if remote_arch == "riscv64"),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=many\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("不是整数")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=\nAGENTLENS_MACHINE_ID_HASH=\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("machine-id")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("缺少 AGENTLENS_MACHINE_ID_SOURCE")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("缺少 AGENTLENS_MACHINE_ID_HASH")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=/etc/machine-id\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("64 位小写十六进制")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("64 位小写十六进制")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4E5F60718293A4B5C6D7E8F90\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("64 位小写十六进制")),
            ),
            (
                b"AGENTLENS_ARCH=x86_64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=g1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n",
                |error| matches!(error, SshError::InvalidResponse { detail, .. } if detail.contains("64 位小写十六进制")),
            ),
        ];

        for (bytes, expected) in cases {
            let error = parse_probe(bytes, &artifacts).expect_err("invalid probe must fail");
            assert!(expected(&error), "unexpected probe error: {error:?}");
        }

        let missing_artifact = parse_probe(PROBE_X86_64.as_bytes(), &CollectorArtifacts::default())
            .expect_err("known architecture without artifact must fail");
        assert!(matches!(missing_artifact, SshError::ArchMismatch { .. }));
    }

    #[test]
    fn ssh_probe_reports_remote_machine_id_digest_and_never_the_raw_value() {
        let artifacts = CollectorArtifacts::default().with_x86_64("/fixtures/collector");
        let probe =
            parse_probe(PROBE_X86_64.as_bytes(), &artifacts).expect("complete probe parses");
        assert_eq!(probe.machine_id_hash, PROBE_MACHINE_ID_HASH);
        assert_eq!(probe.machine_id_hash.len(), MACHINE_ID_HASH_HEX_LENGTH);
        assert!(crate::host::MachineIdentity::from_machine_id_hash(&probe.machine_id_hash).is_ok());

        assert!(STAGE1_SCRIPT.contains(STAGE1_DIGEST_EXPR));
        assert!(STAGE1_SCRIPT.contains("if [ -n \"$MACHINE_ID_SOURCE\" ]; then"));
        assert!(
            STAGE1_SCRIPT.contains("printf 'AGENTLENS_MACHINE_ID_HASH=%s\\n' \"$MACHINE_ID_HASH\"")
        );
        assert!(!STAGE1_SCRIPT.contains("sha256sum < "));
        assert!(!STAGE1_SCRIPT.contains("AGENTLENS_MACHINE_ID="));
    }

    #[cfg(unix)]
    #[test]
    fn ssh_stage1_digest_expression_matches_local_machine_id_hashing() {
        let temp = tempfile::tempdir().expect("digest tempdir");
        let path = temp.path().join("machine-id");
        fs::write(&path, b"3f2a1b0c9d8e7f6055443322110aabbc\n").expect("write machine-id fixture");

        let digest = Command::new("sh")
            .arg("-eu")
            .arg("-c")
            .arg(format!("MACHINE_ID_SOURCE=$1\n{STAGE1_DIGEST_EXPR}\n"))
            .arg("sh")
            .arg(&path)
            .output()
            .expect("shell digest runs");
        assert!(digest.status.success(), "digest stderr: {digest:?}");
        let shell_hash = String::from_utf8(digest.stdout).expect("digest is UTF-8");

        let mut hasher = Sha256::new();
        hasher.update(b"3f2a1b0c9d8e7f6055443322110aabbc");
        assert_eq!(shell_hash.trim_end(), hex::encode(hasher.finalize()));

        let blank = Command::new("sh")
            .arg("-eu")
            .arg("-c")
            .arg(format!(
                "MACHINE_ID_SOURCE=\nMACHINE_ID_HASH=\nif [ -n \"$MACHINE_ID_SOURCE\" ]; then\n  MACHINE_ID_HASH=$({STAGE1_DIGEST_EXPR})\nfi\nprintf 'AGENTLENS_MACHINE_ID_HASH=%s\\n' \"$MACHINE_ID_HASH\"\n"
            ))
            .output()
            .expect("blank-source guard runs");
        assert!(blank.status.success(), "blank-source stderr: {blank:?}");
        assert_eq!(blank.stdout, b"AGENTLENS_MACHINE_ID_HASH=\n");
    }

    #[test]
    fn ssh_payload_workdir_and_ndjson_validation_rejects_malformed_bytes() {
        let negative = encode_collect_payload(&CollectPayload {
            since: -1,
            data_dir: None,
            snapshot: false,
        })
        .expect_err("negative cursor is invalid");
        assert!(matches!(negative, SshError::InvalidInput { .. }));

        let malformed_base64 = decode_collect_payload("A").expect_err("invalid base64 length");
        assert!(matches!(
            malformed_base64,
            SshError::InvalidInput { ref detail } if detail.contains("base64url")
        ));
        assert_eq!(assemble_script("printf fixture"), b"printf fixture\n");

        let bad_workdir = parse_workdir(&[0xff]).expect_err("workdir must be UTF-8");
        assert!(matches!(bad_workdir, SshError::NoWritableCache { .. }));
        let missing_artifact = sha256_file(Path::new("/definitely/absent/agentlens-collector"))
            .expect_err("missing collector cannot be hashed");
        assert!(matches!(missing_artifact, SshError::InvalidInput { .. }));

        for (bytes, detail) in [
            (vec![0xff], "UTF-8"),
            (b"not-json\n".to_vec(), "meta 行不是 JSON"),
            (b"{}\n".to_vec(), "meta 行缺少 v1"),
            (format!("{META_LINE}not-json\n").into_bytes(), "record 行 2"),
        ] {
            let error = validate_ndjson(&bytes).expect_err("malformed NDJSON must fail");
            assert!(
                matches!(error, SshError::InvalidResponse { detail: ref actual, .. } if actual.contains(detail)),
                "unexpected NDJSON error for {detail}: {error:?}"
            );
        }

        let with_blank_record = format!("{META_LINE}\n{{\"messageId\":\"ok\"}}\n");
        validate_ndjson(with_blank_record.as_bytes()).expect("blank record lines are ignored");
    }

    #[test]
    fn ssh_startup_stage_transfer_and_gc_failures_keep_typed_stage_context() {
        let runner = FakeRunner::default();
        runner.push(CommandStage::StartupProbe, 9, "", "unsupported client");
        let (_temp, artifacts) = artifact();
        let error = match transport(&runner, artifacts) {
            Ok(_) => panic!("non-zero ssh -V must disable the transport"),
            Err(error) => error,
        };
        assert!(matches!(error, SshError::SshUnavailable { .. }));

        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 9, "", "probe exploded");
        let (_temp, artifacts) = artifact();
        let stage1_transport = transport(&runner, artifacts).expect("construct transport");
        let error = stage1_transport
            .collect(&request())
            .expect_err("stage1 non-zero must fail");
        assert!(matches!(
            error,
            SshError::Runner {
                stage: CommandStage::Stage1,
                ..
            }
        ));

        for (stage3, expected) in [
            (Err(io::Error::other("scp spawn failed")), "runner"),
            (
                Ok(CommandOutput {
                    status: 255,
                    stdout: Vec::new(),
                    stderr: b"Authentication failed".to_vec(),
                }),
                "auth",
            ),
            (
                Ok(CommandOutput {
                    status: 1,
                    stdout: Vec::new(),
                    stderr: b"disk full".to_vec(),
                }),
                "transfer",
            ),
        ] {
            let runner = FakeRunner::default();
            push_startup(&runner);
            runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
            runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
            runner
                .responses
                .lock()
                .expect("response lock")
                .push_back(FakeResponse {
                    expected_stage: CommandStage::Stage3,
                    result: stage3,
                });
            let (_temp, artifacts) = artifact();
            let transport = transport(&runner, artifacts).expect("construct transport");
            let error = transport.collect(&request()).expect_err("stage3 must fail");
            match expected {
                "runner" => assert!(matches!(
                    error,
                    SshError::Runner {
                        stage: CommandStage::Stage3,
                        ..
                    }
                )),
                "auth" => assert!(matches!(error, SshError::AuthFailed { .. })),
                "transfer" => assert!(matches!(error, SshError::TransferCorrupted { .. })),
                _ => unreachable!(),
            }
        }

        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage4, 9, "", "collector failed");
        let (_temp, artifacts) = artifact();
        let stage4_transport = transport(&runner, artifacts).expect("construct transport");
        let error = stage4_transport
            .collect(&request())
            .expect_err("generic stage4 failure");
        assert!(matches!(
            error,
            SshError::Runner {
                stage: CommandStage::Stage4,
                ..
            }
        ));

        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage4, 0, META_LINE, "");
        runner.push(CommandStage::Gc, 9, "", "find failed");
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");
        let error = transport
            .collect(&request())
            .expect_err("GC failure must be visible");
        assert!(matches!(
            error,
            SshError::Runner {
                stage: CommandStage::Gc,
                ..
            }
        ));
    }

    #[test]
    fn ssh_cancellation_timeout_and_runner_errors_map_without_losing_context() {
        let runner = FakeRunner::default();
        let command = CommandSpec {
            stage: CommandStage::Stage1,
            program: PathBuf::from("unused-while-cancelled"),
            args: Vec::new(),
            env: BTreeMap::new(),
            stdin: Vec::new(),
            detached: false,
        };
        let error = runner
            .run_with_cancel(&command, &|| true)
            .expect_err("pre-start cancellation must not invoke the runner");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(runner.commands().is_empty());

        let timeout = Duration::from_millis(321);
        let timed_out = map_process_error(
            CommandStage::Stage2,
            io::Error::new(io::ErrorKind::TimedOut, "deadline"),
            Some(timeout),
        );
        assert!(matches!(
            timed_out,
            SshError::TimedOut {
                stage: CommandStage::Stage2,
                timeout_ms: 321,
                ..
            }
        ));
        let cancelled = map_process_error(
            CommandStage::Stage4,
            io::Error::new(io::ErrorKind::Interrupted, "cancelled"),
            None,
        );
        assert!(matches!(cancelled, SshError::ClientCancelled));
        let runner_error =
            map_process_error(CommandStage::Gc, io::Error::other("spawn failed"), None);
        assert!(matches!(
            runner_error,
            SshError::Runner {
                stage: CommandStage::Gc,
                ..
            }
        ));

        let attached = attach_timeout(
            SshError::TimedOut {
                stage: CommandStage::StartupProbe,
                timeout_ms: 0,
                detail: "shared deadline".into(),
            },
            timeout,
        );
        assert!(matches!(
            attached,
            SshError::TimedOut {
                timeout_ms: 321,
                ..
            }
        ));
        let unchanged = attach_timeout(SshError::ClientCancelled, timeout);
        assert!(matches!(unchanged, SshError::ClientCancelled));

        let blank = output_detail(&CommandOutput {
            status: 17,
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        assert_eq!(blank, "exit 17 且无输出");
        assert!(is_auth_failure(&CommandOutput {
            status: 255,
            stdout: Vec::new(),
            stderr: b"No supported authentication methods available".to_vec(),
        }));
        assert!(!is_auth_failure(&CommandOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: b"Permission denied".to_vec(),
        }));

        for error in [
            SshError::TimedOut {
                stage: CommandStage::Stage1,
                timeout_ms: 1,
                detail: "timeout".into(),
            },
            SshError::SshUnavailable {
                detail: "missing".into(),
            },
            SshError::InvalidInput {
                detail: "bad input".into(),
            },
            SshError::InvalidResponse {
                stage: CommandStage::Stage4,
                detail: "bad output".into(),
            },
            SshError::Runner {
                stage: CommandStage::Stage3,
                detail: "spawn".into(),
            },
        ] {
            assert_chinese_remediation(&error);
        }
    }

    #[cfg(unix)]
    fn local_shell_spec(script: &str) -> CommandSpec {
        CommandSpec {
            stage: CommandStage::StartupProbe,
            program: PathBuf::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from(script)],
            env: BTreeMap::new(),
            stdin: Vec::new(),
            detached: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn std_runner_times_out_a_stalled_process_without_pipe_deadlock() {
        let runner = StdCommandRunner::with_timeout(Duration::from_millis(100));
        let spec = local_shell_spec("dd if=/dev/zero bs=65536 count=4 2>/dev/null; sleep 30");

        let error = runner.run(&spec).expect_err("stalled child must time out");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn timeout_error_preserves_timeout_kind_when_process_cleanup_fails() {
        let error = process_timeout_error(
            Duration::from_secs(1),
            Some(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "process cleanup denied",
            )),
        );

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("process cleanup denied"));
    }

    #[cfg(unix)]
    #[test]
    fn std_runner_without_timeout_frames_stdin_and_detaches_successfully() {
        let mut spec = local_shell_spec("IFS= read -r line; printf 'seen:%s' \"$line\"");
        spec.stdin = b"payload line\n".to_vec();
        spec.detached = true;

        let output = StdCommandRunner
            .run(&spec)
            .expect("unbounded detached command must complete");

        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, b"seen:payload line");
        assert!(output.stderr.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn timed_runner_with_exhausted_budget_refuses_to_spawn() {
        let runner = StdCommandRunner::with_timeout(Duration::ZERO);
        let error = runner
            .run(&local_shell_spec("printf should-not-run"))
            .expect_err("zero shared budget must fail before spawn");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("before process start"));
    }

    #[cfg(unix)]
    #[test]
    fn timed_runner_shares_one_deadline_across_processes() {
        let mut runner = StdCommandRunner::with_timeout(Duration::from_secs(10));

        runner
            .run(&local_shell_spec("printf first"))
            .expect("first process fits the shared budget");
        runner.started = runner
            .started
            .checked_sub(runner.timeout)
            .expect("the monotonic clock supports a ten-second test rewind");
        let error = runner
            .run(&local_shell_spec("printf second"))
            .expect_err("second process must not receive a fresh timeout budget");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("before process start"));
    }

    #[cfg(unix)]
    #[test]
    fn std_runner_observes_cancellation_while_a_process_is_running() {
        let runner = StdCommandRunner::with_timeout(Duration::from_secs(30));
        let spec = local_shell_spec("sleep 30");
        let checks = AtomicUsize::new(0);

        let error = runner
            .run_with_cancel(&spec, &|| checks.fetch_add(1, Ordering::SeqCst) >= 2)
            .expect_err("running child must observe cancellation");

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }

    #[cfg(unix)]
    #[test]
    fn std_runner_timeout_terminates_descendants_before_they_escape() {
        let temp = tempfile::tempdir().expect("marker tempdir");
        let marker = temp.path().join("descendant-escaped");
        let script = format!("(sleep 1; printf leaked > '{}') & wait", marker.display());
        let runner = StdCommandRunner::with_timeout(Duration::from_millis(100));

        let error = runner
            .run(&local_shell_spec(&script))
            .expect_err("process group must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        std::thread::sleep(Duration::from_millis(1_100));

        assert!(
            !marker.exists(),
            "a descendant survived process-group termination"
        );
    }

    #[cfg(unix)]
    #[test]
    fn std_runner_timeout_terminates_descendants_after_the_parent_exits() {
        let temp = tempfile::tempdir().expect("marker tempdir");
        let marker = temp.path().join("descendant-escaped");
        let child_pid = temp.path().join("child.pid");
        let script = format!(
            "(sleep 1; printf leaked > '{}') & child=$!; printf '%s' \"$child\" > '{}'; exit 0",
            marker.display(),
            child_pid.display()
        );
        let runner = StdCommandRunner::with_timeout(Duration::from_millis(100));

        let error = runner
            .run(&local_shell_spec(&script))
            .expect_err("inherited pipes must keep the escaped descendant inside the deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let pid = fs::read_to_string(&child_pid)
            .expect("parent must publish its descendant pid before exiting")
            .parse::<u32>()
            .expect("descendant pid must be numeric");
        assert_process_exits(pid, 1);
        std::thread::sleep(Duration::from_millis(1_100));

        assert!(
            !marker.exists(),
            "a descendant survived after its immediate parent exited"
        );
    }

    fn assert_remote_ssh_commands_keep_constant_command_and_payload_argv(
        commands: &[CommandSpec],
        expected_payload: &str,
    ) {
        for command in commands.iter().filter(|command| {
            matches!(
                command.stage,
                CommandStage::Stage1
                    | CommandStage::Stage2
                    | CommandStage::Stage4
                    | CommandStage::Gc
            )
        }) {
            let args = command
                .args
                .iter()
                .map(|arg| arg.to_str().expect("SSH argv must be UTF-8 in this test"))
                .collect::<Vec<_>>();
            match command.stage {
                CommandStage::Stage2 | CommandStage::Stage4 => assert!(
                    args.ends_with(&[REMOTE_COMMAND, "--", expected_payload]),
                    "payload command must end with constant command, separator, and token: {args:?}"
                ),
                CommandStage::Stage1 | CommandStage::Gc => assert!(
                    args.ends_with(&[REMOTE_COMMAND]),
                    "payload-free command must end with the constant command: {args:?}"
                ),
                CommandStage::StartupProbe | CommandStage::Stage3 => unreachable!(),
            }
            assert!(!args
                .iter()
                .any(|arg| { arg.starts_with(REMOTE_COMMAND) && *arg != REMOTE_COMMAND }));
        }
    }

    #[test]
    fn ssh_happy_path_uses_exact_stage_sequence_constant_command_and_timeout() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        push_successful_collection(&runner);
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        let collection = transport
            .collect(&request())
            .expect("collect over fake SSH");

        assert_eq!(collection.ndjson, META_LINE.as_bytes());
        let commands = runner.commands();
        let mut stages = Vec::new();
        for stage in commands
            .iter()
            .map(|command| command.stage)
            .filter(|stage| *stage != CommandStage::StartupProbe)
        {
            if stages.last() != Some(&stage) {
                stages.push(stage);
            }
        }
        assert_eq!(
            stages,
            vec![
                CommandStage::Stage1,
                CommandStage::Stage2,
                CommandStage::Stage3,
                CommandStage::Stage4,
                CommandStage::Gc,
            ]
        );
        let expected_payload = encode_collect_payload(&CollectPayload {
            since: 0,
            data_dir: None,
            snapshot: false,
        })
        .expect("encode expected request");
        assert_remote_ssh_commands_keep_constant_command_and_payload_argv(
            &commands,
            &expected_payload,
        );
        for command in &commands[1..] {
            let args = command
                .args
                .iter()
                .filter_map(|value| value.to_str())
                .collect::<Vec<_>>();
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-o", "ConnectTimeout=10"]));
        }
        assert!(String::from_utf8_lossy(
            &commands
                .iter()
                .find(|command| command.stage == CommandStage::Gc)
                .expect("GC command")
                .stdin
        )
        .contains("run.*"));
    }

    #[test]
    fn ssh_all_seven_failure_modes_have_typed_variants_and_chinese_remediation() {
        struct Case {
            name: &'static str,
            stage1: (i32, &'static str, &'static str),
            stage2: Option<(i32, &'static str, &'static str)>,
            stage4: Option<(i32, &'static str, &'static str)>,
            expected: fn(&SshError) -> bool,
        }

        let cases = [
            Case {
                name: "ArchMismatch",
                stage1: (
                    0,
                    "AGENTLENS_ARCH=aarch64\nAGENTLENS_XDG_DATA_HOME=\nAGENTLENS_AVAILABLE_KIB=1\nAGENTLENS_MACHINE_ID_SOURCE=/etc/machine-id\nAGENTLENS_MACHINE_ID_HASH=a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90\n",
                    "",
                ),
                stage2: None,
                stage4: None,
                expected: |error| matches!(error, SshError::ArchMismatch { .. }),
            },
            Case {
                name: "NoWritableCache",
                stage1: (0, PROBE_X86_64, ""),
                stage2: Some((1, "", "mkdir: Permission denied")),
                stage4: None,
                expected: |error| matches!(error, SshError::NoWritableCache { .. }),
            },
            Case {
                name: "TransferCorrupted",
                stage1: (0, PROBE_X86_64, ""),
                stage2: Some((0, REMOTE_RUN_DIR, "")),
                stage4: Some((70, "", "AGENTLENS_TRANSFER_CORRUPTED")),
                expected: |error| matches!(error, SshError::TransferCorrupted { .. }),
            },
            Case {
                name: "AuthFailed",
                stage1: (255, "", "Permission denied (publickey)"),
                stage2: None,
                stage4: None,
                expected: |error| matches!(error, SshError::AuthFailed { .. }),
            },
            Case {
                name: "NoDataDir",
                stage1: (0, PROBE_X86_64, ""),
                stage2: Some((0, REMOTE_RUN_DIR, "")),
                stage4: Some((2, "", "未找到可用的 OpenCode 数据目录")),
                expected: |error| matches!(error, SshError::NoDataDir { .. }),
            },
            Case {
                name: "WalUnreadable",
                stage1: (0, PROBE_X86_64, ""),
                stage2: Some((0, REMOTE_RUN_DIR, "")),
                stage4: Some((4, "", "WAL/SHM Permission denied")),
                expected: |error| matches!(error, SshError::WalUnreadable { .. }),
            },
        ];

        for case in cases {
            let runner = FakeRunner::default();
            push_startup(&runner);
            runner.push(
                CommandStage::Stage1,
                case.stage1.0,
                case.stage1.1,
                case.stage1.2,
            );
            if let Some(stage2) = case.stage2 {
                runner.push(CommandStage::Stage2, stage2.0, stage2.1, stage2.2);
                if stage2.0 == 0 {
                    runner.push(CommandStage::Stage3, 0, "", "");
                    runner.push(CommandStage::Stage3, 0, "", "");
                }
            }
            if let Some(stage4) = case.stage4 {
                runner.push(CommandStage::Stage4, stage4.0, stage4.1, stage4.2);
            }
            let (_temp, artifacts) = artifact();
            let transport = transport(&runner, artifacts).expect("construct transport");
            let error = transport.collect(&request()).expect_err(case.name);
            assert!((case.expected)(&error), "{} returned {error:?}", case.name);
            assert_chinese_remediation(&error);
        }

        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");
        let error = transport
            .collect_with_cancel(&request(), &|| runner.stage3_calls() >= 2)
            .expect_err("cancellation after transfer must stop before execution");
        assert!(matches!(error, SshError::ClientCancelled));
        assert_chinese_remediation(&error);
    }

    #[test]
    fn ssh_sha256_mismatch_maps_to_transfer_corrupted_before_ndjson_is_accepted() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(
            CommandStage::Stage4,
            70,
            "",
            "AGENTLENS_TRANSFER_CORRUPTED: collector: FAILED",
        );
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        let error = transport
            .collect(&request())
            .expect_err("bad digest must fail");
        assert!(matches!(error, SshError::TransferCorrupted { .. }));
    }

    #[test]
    fn ssh_cancellation_is_reclaimed_by_gc_on_next_connection() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        push_successful_collection(&runner);
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        let first = transport.collect_with_cancel(&request(), &|| runner.stage3_calls() >= 2);
        assert!(matches!(first, Err(SshError::ClientCancelled)));
        transport
            .collect_with_cancel(&request(), &|| false)
            .expect("next collection runs GC");

        let commands = runner.commands();
        let gc = commands
            .iter()
            .rfind(|command| command.stage == CommandStage::Gc)
            .expect("next connection GC command");
        let script = String::from_utf8_lossy(&gc.stdin);
        assert!(script.contains("run.*"));
        assert!(script.contains("1440"));
    }

    #[test]
    fn ssh_askpass_sets_force_environment_detaches_and_omits_batch_mode() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        let (_temp, artifacts) = artifact();
        let transport = SshTransport::new(
            runner.clone(),
            tools(),
            SshAuthentication::Askpass {
                executable: PathBuf::from("/opt/agentlens/agentlens-askpass"),
                ipc_channel: "agentlens-once-42".into(),
                identity_file: Some(PathBuf::from("/keys/id_ed25519")),
            },
            artifacts,
        )
        .expect("construct askpass transport");

        runner.push(CommandStage::Stage2, 1, "", "cache denied");

        let error = transport
            .collect(&request())
            .expect_err("injected cache failure stops after askpass probe");
        assert!(matches!(error, SshError::NoWritableCache { .. }));
        let command = runner
            .commands()
            .into_iter()
            .find(|command| command.stage == CommandStage::Stage1)
            .expect("askpass SSH command");
        assert_eq!(
            command
                .env
                .get(std::ffi::OsStr::new("SSH_ASKPASS"))
                .and_then(|value| value.to_str()),
            Some("/opt/agentlens/agentlens-askpass")
        );
        assert_eq!(
            command
                .env
                .get(std::ffi::OsStr::new("SSH_ASKPASS_REQUIRE"))
                .and_then(|value| value.to_str()),
            Some("force")
        );
        assert_eq!(
            command
                .env
                .get(std::ffi::OsStr::new("DISPLAY"))
                .and_then(|value| value.to_str()),
            Some(":0")
        );
        assert_eq!(
            command
                .env
                .get(std::ffi::OsStr::new("AGENTLENS_ASKPASS_CHANNEL"))
                .and_then(|value| value.to_str()),
            Some("agentlens-once-42")
        );
        assert!(command.detached);
        assert!(!command
            .args
            .iter()
            .filter_map(|arg| arg.to_str())
            .any(|arg| arg.contains("BatchMode")));
    }

    #[test]
    fn ssh_mktemp_echo_rejects_empty_relative_and_control_paths() {
        for invalid in ["", "relative/run.123456", "/tmp/run.bad\npath"] {
            let runner = FakeRunner::default();
            push_startup(&runner);
            runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
            runner.push(CommandStage::Stage2, 0, invalid, "");
            let (_temp, artifacts) = artifact();
            let transport = transport(&runner, artifacts).expect("construct transport");

            let error = transport
                .collect(&request())
                .expect_err("invalid mktemp output must fail");
            assert!(matches!(error, SshError::NoWritableCache { .. }));
        }
    }

    #[test]
    fn ssh_missing_binary_probe_reports_clear_degraded_mode_error() {
        let runner = FakeRunner::default();
        runner.push_error(
            CommandStage::StartupProbe,
            io::Error::new(io::ErrorKind::NotFound, "ssh not found"),
        );
        let (_temp, artifacts) = artifact();

        let error = match transport(&runner, artifacts) {
            Ok(_) => panic!("missing ssh must disable transport"),
            Err(error) => error,
        };
        assert!(matches!(error, SshError::SshUnavailable { .. }));
        assert!(error.to_string().contains("降级"));
        assert_chinese_remediation(&error);
    }

    #[test]
    fn ssh_malformed_remote_probe_is_typed_and_does_not_poison_the_next_probe() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, "malformed remote output", "");
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        let error = transport
            .probe_connection("fixture.invalid")
            .expect_err("malformed remote output must fail");
        assert!(matches!(
            error,
            SshError::InvalidResponse {
                stage: CommandStage::Stage1,
                ..
            }
        ));
        assert_chinese_remediation(&error);

        transport
            .probe_connection("fixture.invalid")
            .expect("the next probe must not inherit stale output or failure state");
        assert_eq!(
            runner
                .commands()
                .iter()
                .filter(|command| command.stage == CommandStage::Stage1)
                .count(),
            2
        );
    }

    #[test]
    fn ssh_malformed_inputs_are_rejected_before_remote_execution() {
        for token in ["", "YWJj=", "YW Jj"] {
            assert!(matches!(
                decode_collect_payload(token),
                Err(SshError::InvalidInput { .. })
            ));
        }
        let not_json = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not-json");
        assert!(matches!(
            decode_collect_payload(&not_json),
            Err(SshError::InvalidInput { .. })
        ));

        let runner = FakeRunner::default();
        push_startup(&runner);
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");
        let mut invalid_target = request();
        invalid_target.ssh_target = "-oProxyCommand=evil".into();
        assert!(matches!(
            transport.collect(&invalid_target),
            Err(SshError::InvalidInput { .. })
        ));
        assert_eq!(runner.commands().len(), 1, "only startup probe may run");
    }

    #[test]
    fn ssh_invalid_payload_tokens_are_rejected_before_spawning_remote_process() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        for token in ["", "YWJj=", "YW Jj"] {
            let error = transport
                .run_ssh(
                    CommandStage::Stage2,
                    "test@example.invalid",
                    token,
                    STAGE2_SCRIPT,
                )
                .expect_err("invalid token must be rejected");
            assert!(matches!(error, SshError::InvalidInput { .. }));
            assert_eq!(
                runner.commands().len(),
                1,
                "invalid token {token:?} spawned a remote process"
            );
        }
    }

    /// Fake `ssh` that exits 0 for `-V` and otherwise forks a long-lived descendant, publishes
    /// its pid and hangs waiting on it — the shape needed to prove a timeout kills the whole
    /// process group. Kept as a helper so the calibration below and the test under it agree on
    /// the exact fixture, byte for byte.
    #[cfg(unix)]
    fn write_hanging_fake_ssh(dir: &Path) -> (SshTools, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;

        let ssh = dir.join("ssh");
        let scp = dir.join("scp");
        let child_pid = dir.join("child.pid");
        let script = format!(
            "#!/bin/sh\nif [ \"${{1-}}\" = \"-V\" ]; then exit 0; fi\nsleep 60 &\nchild=$!\nprintf '%s\\n' \"$child\" > '{}'\nwait \"$child\"\n",
            child_pid.display()
        );
        fs::write(&ssh, script).expect("write fake ssh");
        fs::write(&scp, "#!/bin/sh\nexit 0\n").expect("write fake scp");
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).expect("chmod fake ssh");
        fs::set_permissions(&scp, fs::Permissions::from_mode(0o700)).expect("chmod fake scp");
        (
            SshTools::new(&ssh, &scp).expect("paired fake tools"),
            child_pid,
        )
    }

    /// A pid file is written by `printf` in another process, so "the file exists" does not imply
    /// "the file holds a complete number". Only a parsed pid counts as published.
    #[cfg(unix)]
    fn read_published_pid(path: &Path) -> Option<u32> {
        fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
    }

    /// Measure, on THIS host, the two process startups the shared probe deadline has to span: the
    /// `ssh -V` startup probe inside [`SshTransport::new`], and the fixture reaching its pid
    /// publication (`exec /bin/sh` + `fork` + `printf`). Measured, not guessed: the Linux CI host
    /// does both in ~22 ms while the macOS CodeBuild fleet needs 100-300 ms, which is what made a
    /// hard-coded 250 ms budget a coin flip there.
    #[cfg(unix)]
    fn measure_probe_startup_cost(
        tools: &SshTools,
        artifacts: &CollectorArtifacts,
        child_pid: &Path,
    ) -> Duration {
        use std::thread;

        let mut worst = Duration::ZERO;
        for sample in 1..=3 {
            let _ = fs::remove_file(child_pid);
            let startup_probe = Instant::now();
            SshTransport::new(
                StdCommandRunner,
                tools.clone(),
                SshAuthentication::Batch {
                    identity_file: None,
                },
                artifacts.clone(),
            )
            .expect("unbounded calibration transport");
            let startup_probe = startup_probe.elapsed();

            let publish = Instant::now();
            let mut child = Command::new(&tools.ssh)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn calibration fixture");
            let pid = loop {
                if let Some(pid) = read_published_pid(child_pid) {
                    break pid;
                }
                assert!(
                    publish.elapsed() < Duration::from_secs(30),
                    "calibration sample {sample}: the fake ssh never published a child pid even \
                     without any deadline"
                );
                thread::sleep(Duration::from_millis(1));
            };
            worst = worst.max(startup_probe + publish.elapsed());
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(child_pid);
        }
        worst
    }

    #[cfg(unix)]
    #[test]
    fn ssh_probe_timeout_is_typed_kills_the_process_group_and_leaves_no_stale_state() {
        use std::thread;

        let temp = tempfile::tempdir().expect("fake ssh tempdir");
        let (tools, child_pid) = write_hanging_fake_ssh(temp.path());
        let (_artifact_dir, artifacts) = artifact();

        // The contract under test is "the timeout is typed AND the process group it kills was
        // real", so the fixture must have forked and published its descendant before the deadline
        // fires. A hard-coded 250 ms budget made that a race: the budget is shared with the
        // `ssh -V` startup probe inside `SshTransport::new` (one deadline across processes, see
        // `timed_runner_shares_one_deadline_across_processes`), and on the macOS CI fleet that
        // startup probe alone was measured at 89-278 ms — when it eats the whole budget the Stage1
        // process is refused before spawn, so no pid is ever published and the process-group half
        // of this test cannot be checked at all. The budget is therefore derived from a measured
        // same-host cost of both startups, with a 5x margin, and floored so a fast host still
        // leaves a wide absolute window.
        let startup_cost = measure_probe_startup_cost(&tools, &artifacts, &child_pid);
        let probe_budget =
            (startup_cost * 5).clamp(Duration::from_secs(1), Duration::from_secs(10));
        // Bounded, and still far below the fixture's 60 s hang: a deadline that failed to fire
        // is caught, while a slow-but-working host is not.
        let test_allowance = probe_budget + Duration::from_secs(8);

        for attempt in 1..=2 {
            assert!(
                read_published_pid(&child_pid).is_none(),
                "attempt {attempt} started with a stale pid file"
            );
            // Observe the publication while the process tree is still alive instead of reading the
            // file after the kill: a pid seen here is positive proof the deadline hit a live tree.
            let watch_path = child_pid.clone();
            let watcher = thread::spawn(move || {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(30) {
                    if let Some(pid) = read_published_pid(&watch_path) {
                        return Some(pid);
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                None
            });

            let started = Instant::now();
            let transport = SshTransport::new(
                StdCommandRunner::with_timeout(probe_budget),
                tools.clone(),
                SshAuthentication::Batch {
                    identity_file: None,
                },
                artifacts.clone(),
            )
            .expect("construct bounded probe transport");
            let error = transport
                .probe_connection_with_cancel("fixture.invalid", &|| false)
                .expect_err("a hanging probe must hit its wall-clock deadline");

            assert!(
                started.elapsed() < test_allowance,
                "attempt {attempt} blocked past the bounded test allowance of {} ms",
                test_allowance.as_millis()
            );
            assert!(
                matches!(
                    error,
                    SshError::TimedOut {
                        stage: CommandStage::Stage1,
                        ..
                    }
                ),
                "attempt {attempt} returned the wrong typed error: {error:?}"
            );
            let pid = watcher
                .join()
                .expect("pid watcher thread")
                .unwrap_or_else(|| {
                    panic!(
                        "attempt {attempt}: fake ssh never published a child pid within the {} ms \
                         probe budget (measured host startup cost {} us) — the process-group \
                         contract was never exercised",
                        probe_budget.as_millis(),
                        startup_cost.as_micros()
                    )
                });
            assert_process_exits(pid, attempt);
            fs::remove_file(&child_pid).expect("remove prior attempt pid");
        }
    }

    #[cfg(unix)]
    fn assert_process_exits(pid: u32, attempt: usize) {
        use std::thread;
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let alive = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "attempt {attempt} left child process {pid} alive after timeout"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    #[test]
    fn ssh_probe_timeout_is_typed_kills_the_windows_job_and_leaves_no_stale_state() {
        use std::time::{Duration, Instant};

        const CHILD_PID_ENV: &str = "AGENTLENS_WINDOWS_JOB_CHILD_PID";
        const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
        const TEST_ALLOWANCE: Duration = Duration::from_secs(15);

        if let Some(child_pid) = env::var_os(CHILD_PID_ENV) {
            let mut child = Command::new("ping.exe")
                .args(["-n", "61", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn Windows job descendant");
            fs::write(PathBuf::from(child_pid), child.id().to_string())
                .expect("publish Windows job descendant pid");
            child.wait().expect("wait for Windows job descendant");
            return;
        }

        let temp = tempfile::tempdir().expect("fake ssh tempdir");
        let ssh = temp.path().join("ssh.cmd");
        let scp = temp.path().join("scp.cmd");
        let child_pid = temp.path().join("child.pid");
        let test_exe = env::current_exe().expect("resolve current Windows test executable");
        fs::write(
            &ssh,
            format!(
                "@echo off\r\nif \"%~1\"==\"-V\" exit /b 0\r\nset \"{CHILD_PID_ENV}={}\"\r\n\"{}\" --exact transport::ssh::tests::ssh_probe_timeout_is_typed_kills_the_windows_job_and_leaves_no_stale_state --nocapture\r\n",
                child_pid.display(),
                test_exe.display()
            ),
        )
        .expect("write fake ssh");
        fs::write(&scp, "@echo off\r\nexit /b 0\r\n").expect("write fake scp");
        let tools = SshTools::new(&ssh, &scp).expect("paired fake tools");
        let (_artifact_dir, artifacts) = artifact();

        for attempt in 1..=2 {
            let started = Instant::now();
            let transport = SshTransport::new(
                // Re-enter this test in fixture mode so the job owns a known descendant without
                // depending on environment-sensitive PowerShell process startup.
                StdCommandRunner::with_timeout(PROBE_TIMEOUT),
                tools.clone(),
                SshAuthentication::Batch {
                    identity_file: None,
                },
                artifacts.clone(),
            )
            .expect("construct bounded probe transport");
            let error = transport
                .probe_connection_with_cancel("fixture.invalid", &|| false)
                .expect_err("a hanging probe must hit its wall-clock deadline");

            assert!(
                started.elapsed() < TEST_ALLOWANCE,
                "attempt {attempt} blocked past the bounded test allowance"
            );
            assert!(
                matches!(
                    error,
                    SshError::TimedOut {
                        stage: CommandStage::Stage1,
                        ..
                    }
                ),
                "attempt {attempt} returned the wrong typed error: {error:?}"
            );
            let pid = fs::read_to_string(&child_pid)
                .expect("fake ssh must publish its child pid")
                .trim()
                .parse::<u32>()
                .expect("child pid must be numeric");
            assert_windows_process_exits(pid, attempt);
            fs::remove_file(&child_pid).expect("remove prior attempt pid");
        }
    }

    #[cfg(windows)]
    fn assert_windows_process_exits(pid: u32, attempt: usize) {
        use std::thread;
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .expect("query child process");
            let alive = String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""));
            if !alive {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "attempt {attempt} left child process {pid} alive after timeout"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(unix)]
    #[test]
    fn ssh_non_utf8_data_dir_is_rejected_without_lossy_encoding() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let runner = FakeRunner::default();
        push_startup(&runner);
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");
        let mut invalid = request();
        invalid.data_dir = Some(PathBuf::from(OsString::from_vec(vec![b'/', 0xff])));

        assert!(matches!(
            transport.collect(&invalid),
            Err(SshError::InvalidInput { .. })
        ));
        assert_eq!(runner.commands().len(), 1, "only startup probe may run");
    }

    #[test]
    fn ssh_exit_zero_with_empty_stdout_is_not_success() {
        let runner = FakeRunner::default();
        push_startup(&runner);
        runner.push(CommandStage::Stage1, 0, PROBE_X86_64, "");
        runner.push(CommandStage::Stage2, 0, REMOTE_RUN_DIR, "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage3, 0, "", "");
        runner.push(CommandStage::Stage4, 0, "", "");
        let (_temp, artifacts) = artifact();
        let transport = transport(&runner, artifacts).expect("construct transport");

        let error = transport
            .collect(&request())
            .expect_err("missing meta line must fail");
        assert!(matches!(error, SshError::InvalidResponse { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn ssh_real_sh_framing_preserves_hostile_payload_as_one_inert_argument() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("framing tempdir");
        let home = temp.path().join("home");
        let workdir = home.join(".cache/agentlens/run.Framed1");
        fs::create_dir_all(&workdir).expect("create remote workdir fixture");
        let hostile = "-leading path with spaces, \"quotes\"\nand ; rm -rf / $(id) `id`";
        let payload = encode_collect_payload(&CollectPayload {
            since: 123_456_789,
            data_dir: Some(hostile.into()),
            snapshot: true,
        })
        .expect("encode hostile request");
        fs::write(workdir.join("request"), format!("{payload}\n")).expect("write request marker");
        let collector = workdir.join("collector");
        fs::write(
            &collector,
            b"#!/bin/sh\nprintf 'ARGC=%s\\n' \"$#\"\ni=1\nfor arg in \"$@\"; do\n  printf 'ARG%s=%s\\n' \"$i\" \"$arg\"\n  i=$((i + 1))\ndone\n",
        )
        .expect("write stub collector");
        fs::set_permissions(&collector, fs::Permissions::from_mode(0o700))
            .expect("chmod stub collector");
        let digest = hex::encode(Sha256::digest(fs::read(&collector).expect("read stub")));
        fs::write(
            workdir.join(CHECKSUM_FILE_NAME),
            format!("{digest}  collector\n"),
        )
        .expect("write checksum manifest");

        let framed = assemble_script(STAGE4_SCRIPT);
        let shell = env::var_os("AGENTLENS_TEST_SH").unwrap_or_else(|| "/bin/sh".into());
        let mut child = Command::new(&shell)
            .arg("-s")
            .arg("--")
            .arg(&payload)
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| {
                panic!("spawn test shell {}: {error}", shell.to_string_lossy())
            });
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(&framed)
            .expect("pipe framing");
        let output = child.wait_with_output().expect("wait for real shell");
        let stdout = String::from_utf8(output.stdout).expect("stub stdout UTF-8");
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("RAW_STUB_ARGV:\n{stdout}");
        assert!(output.status.success(), "real sh failed: {stderr}");
        assert!(stdout.contains("ARGC=3\n"));
        assert!(stdout.contains("ARG1=collect\n"));
        assert!(stdout.contains("ARG2=--request-base64url\n"));
        assert!(stdout.contains(&format!("ARG3={payload}\n")));
        let decoded = decode_collect_payload(&payload).expect("Rust-side payload decode");
        eprintln!("SCRIPT_FIRST_THREE:");
        for line in String::from_utf8_lossy(&framed).lines().take(3) {
            eprintln!("{line}");
        }
        eprintln!("TEST_SHELL={}", shell.to_string_lossy());
        eprintln!("ENCODED_TOKEN={payload}");
        eprintln!("DECODED_DATA_DIR={:?}", decoded.data_dir);
        eprintln!(
            "HOSTILE_ROUND_TRIP_MATCH={}",
            decoded.data_dir.as_deref() == Some(hostile)
        );
        assert_eq!(decoded.data_dir.as_deref(), Some(hostile));
        assert!(!temp.path().join("command-injection-marker").exists());
        assert!(
            !workdir.exists(),
            "STAGE4 trap must remove its run directory"
        );
        assert_eq!(REMOTE_COMMAND, "sh -s");
        assert_eq!(framed, STAGE4_SCRIPT.as_bytes());

        let text = String::from_utf8(framed).expect("framing UTF-8");
        let first_three = text.lines().take(3).collect::<Vec<_>>();
        assert_eq!(
            first_three,
            vec!["AGENTLENS_PAYLOAD=$1", "set -eu", "umask 077"]
        );
    }

    // Linux-only, NOT `cfg(unix)`: the fixture ages the run directory with GNU coreutils
    // `touch -d "2 days ago"`. BSD `touch` (macOS) rejects that relative-date syntax, and
    // BusyBox `touch` (Alpine) would reject it identically, so `cfg(unix)` overstated where
    // the harness can run. The GC script under test is POSIX sh and is not Linux-specific;
    // only this way of aging a directory is.
    #[cfg(target_os = "linux")]
    #[test]
    fn ssh_local_gc_reclaims_interrupted_run_and_consecutive_runs_leave_no_stale_state() {
        let temp = tempfile::tempdir().expect("GC tempdir");
        let home = temp.path().join("home");
        fs::create_dir_all(&home).expect("create HOME");
        let payload = encode_collect_payload(&CollectPayload {
            since: 1,
            data_dir: None,
            snapshot: false,
        })
        .expect("encode GC request");

        let first = run_local_script(&home, Some(&payload), STAGE2_SCRIPT);
        assert!(first.status.success(), "STAGE2 must create run dir");
        let interrupted =
            PathBuf::from(String::from_utf8(first.stdout).expect("path UTF-8").trim());
        assert!(interrupted.is_dir());
        let touch = Command::new("touch")
            .args(["-d", "2 days ago", "--"])
            .arg(&interrupted)
            .status()
            .expect("age interrupted directory deterministically");
        assert!(touch.success());
        let gc = run_local_script(&home, None, GC_SCRIPT);
        assert!(gc.status.success());
        assert!(
            !interrupted.exists(),
            "next connection GC must reclaim interruption"
        );

        for since in [2, 3] {
            let payload = encode_collect_payload(&CollectPayload {
                since,
                data_dir: None,
                snapshot: false,
            })
            .expect("encode consecutive request");
            let stage2 = run_local_script(&home, Some(&payload), STAGE2_SCRIPT);
            assert!(stage2.status.success());
            let workdir =
                PathBuf::from(String::from_utf8(stage2.stdout).expect("path UTF-8").trim());
            install_success_stub(&workdir);
            let stage4 = run_local_script(&home, Some(&payload), STAGE4_SCRIPT);
            assert!(stage4.status.success());
            assert!(!workdir.exists());
        }
        let cache = home.join(".cache/agentlens");
        let remaining = fs::read_dir(cache)
            .expect("read cache")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("run."))
            .count();
        assert_eq!(remaining, 0);
    }

    // Gated exactly like its only caller, the Linux-only local GC fixture.
    #[cfg(target_os = "linux")]
    fn install_success_stub(workdir: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        let collector = workdir.join("collector");
        fs::write(
            &collector,
            format!("#!/bin/sh\nprintf '%s' '{META_LINE}'\n"),
        )
        .expect("write success stub");
        fs::set_permissions(&collector, fs::Permissions::from_mode(0o700))
            .expect("chmod success stub");
        let digest = hex::encode(Sha256::digest(fs::read(&collector).expect("read stub")));
        fs::write(
            workdir.join(CHECKSUM_FILE_NAME),
            format!("{digest}  collector\n"),
        )
        .expect("write checksum manifest");
    }

    // Gated exactly like its only caller, the Linux-only local GC fixture.
    #[cfg(target_os = "linux")]
    fn run_local_script(home: &Path, payload: Option<&str>, script: &str) -> std::process::Output {
        let framed = assemble_script(script);
        let mut command = Command::new("/bin/sh");
        command.arg("-s");
        if let Some(payload) = payload {
            command.arg("--").arg(payload);
        }
        let mut child = command
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn local shell");
        child
            .stdin
            .take()
            .expect("local shell stdin")
            .write_all(&framed)
            .expect("write local script");
        child.wait_with_output().expect("wait local shell")
    }
}
