fn main() {
    delay_load_comctl32_on_windows_msvc();
    tauri_build::build()
}

/// Delay-load `comctl32.dll` on Windows so a binary WITHOUT the application manifest can
/// still load.
///
/// `tauri-runtime-wry` and `muda` call `TaskDialogIndirect`, which exists only in
/// **Common-Controls v6**. `C:\Windows\System32\comctl32.dll` is the legacy v5.82 build and
/// does not export it; v6 lives in WinSxS and is selected per process by an application
/// manifest declaring a dependency on `Microsoft.Windows.Common-Controls 6.0.0.0`.
///
/// `tauri_build::build()` embeds exactly that manifest, but through `embed_resource`, which
/// emits `cargo:rustc-link-arg-bins` -- bin targets ONLY. So the `cargo test` harness
/// executable for this crate has no manifest, binds `TaskDialogIndirect` against v5.82, and
/// the loader kills it before `main` with `STATUS_ENTRYPOINT_NOT_FOUND` (`0xc0000139`,
/// surfaced by cargo as exit `-1073741511`). Measured on CodeBuild
/// `aws/codebuild/windows-base:2022-1.0` by diffing the exe's import table against each
/// DLL's exports: `comctl32.dll` was the only DLL missing an imported symbol. It is not
/// image-specific -- every Windows host resolves `comctl32.dll` to v5.82 without a manifest.
///
/// A load-time failure cannot be gated per test: all 32 of this crate's tests silently never
/// ran. Delay-loading moves resolution to the first actual call, so:
/// - the test harness, which never opens a task dialog, loads and runs every test;
/// - the shipped binary keeps its v6 manifest, so the delayed `LoadLibrary("comctl32.dll")`
///   resolves through the process activation context to the same v6 module as before.
///
/// Plain `cargo:rustc-link-arg` is deliberate: it reaches every linkable unit (bin, cdylib
/// AND the test harness), while the `-bins` / `-tests` variants each cover only some.
fn delay_load_comctl32_on_windows_msvc() {
    let is_windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !is_windows_msvc {
        return;
    }
    // delayimp.lib provides the helper stub the linker calls on first use.
    println!("cargo:rustc-link-arg=delayimp.lib");
    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
}
