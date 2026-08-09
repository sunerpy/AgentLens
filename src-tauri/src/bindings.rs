#[cfg(all(test, feature = "ts-export"))]
use std::path::Path;

#[cfg(all(test, feature = "ts-export"))]
use ts_rs::TS;

#[cfg(all(test, feature = "ts-export"))]
use crate::contract::{
    AggregateFilters, AppSettings, BreakdownDimensions, BreakdownRow, CostTotals, CoverageNote,
    CoverageShortfall, CoverageStatus, DateRange, DetailCost, DetailFilters, Granularity, Host,
    HostCreateInput, HostKind, HostUpdateInput, IpcError, IpcErrorCode, MessageFilters,
    MessagePage, MessageRow, ObservedModelPrice, PriceCatalog, PriceEntry, PriceMatchKind,
    PriceTable, RefreshEvent, SeriesGroup, SeriesGroupDimension, SeriesPoint, SeriesQueryResult,
    SourceState, SourceStatus, Summary, TimeBucket, TokenValues, TriggerMode, TriggerRefreshResult,
    WeekStart,
};
#[cfg(all(test, feature = "ts-export"))]
use crate::credentials::{
    CredentialKind, CredentialRef, CredentialStatus, LocalIdentity, SshProbeInput, SshProbeResult,
};
#[cfg(all(test, feature = "ts-export"))]
use crate::logging::{DiagnosticsReport, LogEntry, LogLevel, LogTail};

#[cfg(all(test, feature = "ts-export"))]
pub(crate) fn export_all(output_dir: &Path) -> Result<(), ts_rs::ExportError> {
    export::<IpcErrorCode>(output_dir)?;
    export::<IpcError>(output_dir)?;
    export::<WeekStart>(output_dir)?;
    export::<DateRange>(output_dir)?;
    export::<Granularity>(output_dir)?;
    export::<AggregateFilters>(output_dir)?;
    export::<BreakdownDimensions>(output_dir)?;
    export::<DetailFilters>(output_dir)?;
    export::<MessageFilters>(output_dir)?;
    export::<TokenValues>(output_dir)?;
    export::<CostTotals>(output_dir)?;
    export::<CoverageStatus>(output_dir)?;
    export::<CoverageShortfall>(output_dir)?;
    export::<CoverageNote>(output_dir)?;
    export::<TimeBucket>(output_dir)?;
    export::<SeriesPoint>(output_dir)?;
    export::<SeriesGroupDimension>(output_dir)?;
    export::<SeriesGroup>(output_dir)?;
    export::<SeriesQueryResult>(output_dir)?;
    export::<Summary>(output_dir)?;
    export::<BreakdownRow>(output_dir)?;
    export::<DetailCost>(output_dir)?;
    export::<MessageRow>(output_dir)?;
    export::<MessagePage>(output_dir)?;
    export::<HostKind>(output_dir)?;
    export::<Host>(output_dir)?;
    export::<HostCreateInput>(output_dir)?;
    export::<HostUpdateInput>(output_dir)?;
    export::<TriggerMode>(output_dir)?;
    export::<SourceState>(output_dir)?;
    export::<SourceStatus>(output_dir)?;
    export::<RefreshEvent>(output_dir)?;
    export::<TriggerRefreshResult>(output_dir)?;
    export::<AppSettings>(output_dir)?;
    export::<PriceEntry>(output_dir)?;
    export::<PriceTable>(output_dir)?;
    export::<PriceMatchKind>(output_dir)?;
    export::<ObservedModelPrice>(output_dir)?;
    export::<PriceCatalog>(output_dir)?;
    export::<CredentialKind>(output_dir)?;
    export::<CredentialRef>(output_dir)?;
    export::<CredentialStatus>(output_dir)?;
    export::<SshProbeInput>(output_dir)?;
    export::<SshProbeResult>(output_dir)?;
    export::<LocalIdentity>(output_dir)?;
    export::<LogLevel>(output_dir)?;
    export::<LogEntry>(output_dir)?;
    export::<LogTail>(output_dir)?;
    export::<DiagnosticsReport>(output_dir)?;
    Ok(())
}

#[cfg(all(test, feature = "ts-export"))]
fn export<T: TS + 'static>(output_dir: &Path) -> Result<(), ts_rs::ExportError> {
    T::export_all_to(output_dir)
}

#[cfg(all(test, feature = "ts-export"))]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    const TYPE_NAMES: [&str; 49] = [
        "IpcErrorCode",
        "IpcError",
        "WeekStart",
        "DateRange",
        "Granularity",
        "AggregateFilters",
        "BreakdownDimensions",
        "DetailFilters",
        "MessageFilters",
        "TokenValues",
        "CostTotals",
        "CoverageStatus",
        "CoverageShortfall",
        "CoverageNote",
        "TimeBucket",
        "SeriesPoint",
        "SeriesGroupDimension",
        "SeriesGroup",
        "SeriesQueryResult",
        "Summary",
        "BreakdownRow",
        "DetailCost",
        "MessageRow",
        "MessagePage",
        "HostKind",
        "Host",
        "HostCreateInput",
        "HostUpdateInput",
        "TriggerMode",
        "SourceState",
        "SourceStatus",
        "RefreshEvent",
        "TriggerRefreshResult",
        "AppSettings",
        "PriceEntry",
        "PriceTable",
        "PriceMatchKind",
        "ObservedModelPrice",
        "PriceCatalog",
        "CredentialKind",
        "CredentialRef",
        "CredentialStatus",
        "SshProbeInput",
        "SshProbeResult",
        "LocalIdentity",
        "LogLevel",
        "LogEntry",
        "LogTail",
        "DiagnosticsReport",
    ];

    #[test]
    fn bindings_export() {
        let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("frontend");
        let output_dir = frontend.join("src/generated");
        if output_dir.exists() {
            fs::remove_dir_all(&output_dir).expect("clear generated bindings");
        }
        fs::create_dir_all(&output_dir).expect("create generated bindings directory");

        export_all(&output_dir).expect("export all TypeScript DTO bindings");
        let index = TYPE_NAMES
            .iter()
            .map(|name| format!("export type {{ {name} }} from \"./{name}\";"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            output_dir.join("index.ts"),
            format!(
                "// Generated by `cargo test --features ts-export bindings_export`.\n{index}\n"
            ),
        )
        .expect("write generated bindings index");

        run(
            &frontend,
            node_package_runner(),
            &["prettier", "--write", "src/generated"],
        );
        run(&frontend, node_package_runner(), TYPECHECK_ARGUMENTS);
    }

    /// `frontend/tsconfig.json` is a solution-style config: `files: []` plus
    /// `references`. A bare `tsc --noEmit` therefore has an EMPTY input set — it
    /// checks zero files and exits 0, so the gate stays green even when the
    /// frontend has real type errors. Only build mode (`-b`) walks
    /// `references`, and `-b` alone is incremental: with a fresh `.tsbuildinfo`
    /// it silently skips the check. `--force` makes every run a full check, so a
    /// correctness gate never depends on file mtimes.
    const TYPECHECK_ARGUMENTS: &[&str] = &["tsc", "-b", "--force"];

    #[cfg(windows)]
    fn node_package_runner() -> &'static str {
        "npx.cmd"
    }

    #[cfg(not(windows))]
    fn node_package_runner() -> &'static str {
        "npx"
    }

    #[test]
    fn node_package_runner_uses_the_platform_executable_shim() {
        #[cfg(windows)]
        assert_eq!(node_package_runner(), "npx.cmd");

        #[cfg(not(windows))]
        assert_eq!(node_package_runner(), "npx");
    }

    #[test]
    fn typecheck_arguments_use_forced_build_mode() {
        assert!(TYPECHECK_ARGUMENTS.contains(&"-b"));
        assert!(TYPECHECK_ARGUMENTS.contains(&"--force"));
        assert!(!TYPECHECK_ARGUMENTS.contains(&"--noEmit"));
    }

    fn run(current_dir: &Path, program: &str, arguments: &[&str]) {
        let output = Command::new(program)
            .args(arguments)
            .current_dir(current_dir)
            .output()
            .unwrap_or_else(|error| panic!("run {program} {arguments:?}: {error}"));
        assert!(
            output.status.success(),
            "{program} {arguments:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
