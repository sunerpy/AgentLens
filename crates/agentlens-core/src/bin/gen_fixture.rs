//! `gen-fixture` 二进制入口：生成合成 opencode 数据集与 `manifest.json`。

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match parse_out_dir(env::args_os().skip(1)) {
        Ok(Some(out_dir)) => match agentlens_core::fixture::generate(&out_dir) {
            Ok(manifest) => {
                println!(
                    "generated fixture at {}: {} messages, {} eligible, {} skipped",
                    out_dir.display(),
                    manifest.total_message_rows,
                    manifest.eligible_assistant_count,
                    manifest.skipped_count
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("gen-fixture: {error}");
                ExitCode::from(1)
            }
        },
        Ok(None) => {
            println!("Usage: gen-fixture --out <directory>");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("gen-fixture: {error}\nUsage: gen-fixture --out <directory>");
            ExitCode::from(2)
        }
    }
}

fn parse_out_dir(
    mut arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<Option<PathBuf>, String> {
    let mut out_dir = None;
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            if out_dir.is_some() || arguments.next().is_some() {
                return Err("--help cannot be combined with other arguments".to_string());
            }
            return Ok(None);
        }
        if argument != "--out" {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()));
        }
        if out_dir.is_some() {
            return Err("--out may only be provided once".to_string());
        }
        let value = arguments
            .next()
            .ok_or_else(|| "--out requires a directory path".to_string())?;
        if value.is_empty() {
            return Err("--out requires a non-empty directory path".to_string());
        }
        out_dir = Some(PathBuf::from(value));
    }
    out_dir
        .map(Some)
        .ok_or_else(|| "missing required --out <directory>".to_string())
}
