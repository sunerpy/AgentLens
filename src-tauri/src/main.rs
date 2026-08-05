// 防止 Windows release 下弹出额外的控制台窗口，请勿删除。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = agentlens_tauri_lib::run() {
        eprintln!("AgentLens startup failed: {error}");
        std::process::exit(1);
    }
}
