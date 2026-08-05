# Development and build

[← README](../README.md) · [简体中文](readme/development.zh.md)

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
make coverage-gate # coverage with the 75% floor enforced
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
| Rust workspace | 182 passed / 0 failed / 18 ignored | `make test` |
| Vitest unit | 268 cases across 15 specs | `make test-unit` |
| Playwright component | 58 specs, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |

The enforced line-coverage floor is 75%, and that floor is the durable guarantee.
The measured percentage is per-runner: `make coverage-gate` reports 76.92%
(12025/15633 lines) at HEAD on local Linux, and 79.56% (11252/14143) on the
CodeBuild Linux runner, because llvm-cov's line base differs by environment. The
local margin above the floor is 1.92pp, a shade wider than the previous round's
1.85pp, so nothing regressed. The margin stays thin because the lowest-covered
Rust modules are the Tauri runtime wiring (`state.rs`, `tray.rs`), which needs a
real `AppHandle` and event loop; that gap is structural rather than an omission.

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

## Continuous integration

`.github/workflows/ci.yml` is authored and `actionlint`-clean, but it has **never
executed** — this repository has no git remote yet, so GitHub Actions has had
nothing to run. Treat the CI badge in the README as "no status" until a remote
exists.

## Versioning

The single source of truth is `[workspace.package].version` in the root
`Cargo.toml`. Every crate and `src-tauri` inherit it, `tauri.conf.json` no longer
declares a version of its own, and `make dist-version` echoes the resolved value.
