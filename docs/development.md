# Development and build

[← README](readme/README.en.md) · [简体中文](readme/development.zh.md)

## Everyday targets

```sh
make help          # list every target
make dev           # Tauri dev mode
make fmt           # format Rust + frontend
make fmt-check     # verify formatting without writing
make lint          # cargo fmt/clippy + frontend lint/typecheck + copy gate
make test          # cargo test --workspace
make test-unit     # vitest
make coverage      # coverage report into artifacts/coverage/
make coverage-gate # coverage with the 90% floor enforced
make hooks         # install the pre-commit / pre-push hooks
```

## Frontend scripts

Run from `frontend/`, or through the Makefile targets above.

```
dev  build  lint  format  format:check  preview  typecheck
test:unit  test:unit:coverage  test:e2e  test:e2e:real  check:i18n
```

## Test tiers

| Tier | Count | Command |
| --- | --- | --- |
| Rust workspace | 414 passed / 0 failed / 21 ignored | `make test` |
| Vitest unit | 497 cases across 26 specs | `make test-unit` |
| Playwright component | 126 cases across 12 spec files, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |

The enforced line-coverage floor is 90%, declared as `COVERAGE_MIN := 90` in the
Makefile. The `:=` is deliberate: it overrides a same-named environment variable,
so the floor cannot be silently lowered, while a command-line
`make coverage-gate COVERAGE_MIN=...` still wins. `make coverage-gate` reports
92.57% (53101/57363 lines) at HEAD on local Linux. The measured percentage is
per-runner because llvm-cov's line base differs by environment; the floor, not the
number, is the durable guarantee. The margin stays modest because the
lowest-covered Rust modules are the Tauri runtime wiring (`state.rs`, `tray.rs`),
which needs a real `AppHandle` and event loop; that gap is structural rather than
an omission.

The gate reads the same `artifacts/coverage/lcov.info` bytes that Codecov
consumes, so the local number and the dashboard number cannot drift apart.

## Packaging

```sh
make dist            # deb + both collector architectures + sha256sums.txt
make dist-all        # same, but fails when the aarch64 collector is missing
make dist-version    # echo the resolved version
make dist-verify     # verify the staged artifacts
make dist-clean      # clean artifacts/dist/
```

Prerequisites and the aarch64 fallback behaviour are documented in
[installation.md](installation.md#from-source).

## AWS CodeBuild

```sh
make aws-source-upload
make aws-build-linux
make aws-build-windows
make aws-build-macos
make aws-status
make aws-logs
```

All three platforms are proven green on `us-east-2` CodeBuild against the
post-H4b code: Linux `d2edbcdd` (a 5,709,438-byte `AgentLens_0.1.0_amd64.deb`,
182 passed / 0 failed / 18 ignored), Windows `39f89617` (a 4,142,828-byte
`AgentLens_0.1.0_x64-setup.exe`, 170 passed / 0 failed / 8 ignored) and macOS
`82b4d172` (a real 5,862,574-byte `AgentLens_0.1.0_aarch64.dmg`, 180 passed / 0
failed / 10 ignored). Linux and Windows read the `13:17:14Z` source zip while
macOS read a later `14:55:03Z` one, and only documentation commits sit between
them, so the product code is the same without the zip being literally the same
file. The totals are not meant to match: `#[cfg(unix)]`-gated tests do not
compile on Windows and `#[cfg(target_os = "linux")]`-gated ones do not compile on
macOS, so on those platforms the tests are absent rather than ignored. Each build
succeeding means the defect did not manifest, not that anyone has launched the
installers; `.omo/evidence/aws-aw5-test-matrix.md` holds the per-platform
reconciliation for an earlier round of builds in the earlier `us-west-2` region.
The buildspecs and their prose live under `.aws/`; see
[.aws/README.md](../.aws/README.md).

## Real-machine acceptance

A green build only proves the defect did not manifest. Windows is the one platform
that has also been accepted on real hardware: on EC2 Windows Server the package
was actually installed and the app actually launched, and the GUI run
(`h7-20260805T123646Z`) passed all 25 machine-decidable assertions — client area
exactly 1180x780, no native title bar, a 900x600 minimum track size, a real
SendInput drag with zero drift, and the close button honouring
`prevent_close + hide` instead of exiting the app.

`install.ps1` has a separate end-to-end run at 38/38
(`installps1-20260805T111723Z`), which surfaced and fixed a real defect:
`Start-Process -PassThru -Wait` waits on the whole process tree, and the NSIS
finish page ticks "run AgentLens" by default, so the script never returned;
switching to `ProcessStartInfo` + `WaitForExit()` made it exit cleanly.

**The Linux and macOS packages have still never been launched on a real machine.**

## Continuous integration

`.github/workflows/ci.yml` runs on GitHub Actions and is green on `main`. Besides
formatting, clippy and the test tiers, it enforces two gates worth knowing about
before you push:

- **ts-rs zero drift.** The job wipes `frontend/src/generated/`, re-exports every
  DTO via
  `cargo test -p agentlens-tauri --features ts-export bindings_export`, then fails
  when a path-scoped `git status` shows a diff. Hand-edited bindings fail here.
- **Coverage.** `make coverage-gate` runs on Linux only, and only Linux uploads to
  Codecov — uploading from all three platforms would count the same code three
  times.

`.github/workflows/pr-title.yml` enforces conventional PR titles. That is not
cosmetic: the repository squash-merges, so release-please only sees the squash
subject, and a PR titled `chore:` would swallow the `feat:` / `fix:` commits on its
branch and stop version bumps with no error.

release-please keeps a release PR open on `main`. No release has been published
yet, so the version badge renders as "no status" and the installer download paths
have never fetched a real asset.

## Versioning

The single source of truth is `[workspace.package].version` in the root
`Cargo.toml`. Every crate and `src-tauri` inherit it, `tauri.conf.json` no longer
declares a version of its own, and `make dist-version` echoes the resolved value.
