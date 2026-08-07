// 防止 Windows release 下弹出额外的控制台窗口，请勿删除。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = agentlens_tauri_lib::run() {
        // Not `eprintln!`: under `windows_subsystem = "windows"` the standard error handle can
        // be NULL, and `eprintln!` panics when the write fails — turning a clean "startup
        // failed" exit into a crash. This failure happens before the Tauri app handle exists,
        // so there is no `app_log_dir()` to log to yet; a best-effort stderr write is all
        // that is available, and it must not be able to panic.
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            format!("AgentLens startup failed: {error}\n").as_bytes(),
        );
        std::process::exit(1);
    }
}
