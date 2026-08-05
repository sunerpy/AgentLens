# AgentLens

<!--
  NOTE: The badge URLs below name the real repository, sunerpy/AgentLens, but
  that repository does not exist yet: this checkout still has no git remote, so
  the badges 404 until it is created.
  GitHub Actions has never executed for this repo: `.github/workflows/ci.yml`
  is authored and actionlint-clean, but unrun. The CI badge is therefore
  expected to render as "no status" -- it does not indicate a passing build.
-->

[![CI](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml/badge.svg)](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/sunerpy/AgentLens/branch/main/graph/badge.svg)](https://codecov.io/gh/sunerpy/AgentLens)
![version](https://img.shields.io/badge/version-0.1.0-blue)
![platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)
![license](https://img.shields.io/badge/license-MIT-green)

**English** · [简体中文](docs/readme/README.zh.md)

A desktop dashboard for AI coding-agent token usage. AgentLens collects usage
records from your local machine and from any number of remote SSH hosts into one
local SQLite archive, then slices them by timezone, agent, model and project for
trend, drill-down and detail analysis.

## Table of Contents

- [Highlights](#highlights)
- [Install](#install)
- [Quick Start](#quick-start)
- [Architecture](#architecture)
- [Testing and Quality](#testing-and-quality)
- [Development](#development)
- [Documentation](#documentation)
- [Status and Limitations](#status-and-limitations)
- [License](#license)

## Highlights

- **The archive is authoritative history, never pruned.** Source-log rotation,
  deleted backups or a wiped remote data directory cannot make an archived
  record disappear.
- **Remote collection is read-only.** A statically linked musl collector is
  pushed to the remote, sha256-verified, executed in place and cleaned up on
  exit. It never writes to the remote tool's data.
- **Credentials live in the OS keychain** (Linux Secret Service, Windows
  Credential Manager). No passwords in config files, none returned over IPC.
- **Calendar bucketing is implemented in Rust.** The frontend deliberately
  carries no `date-fns` / `dayjs` / `moment`, so there is only one timezone
  engine and no second implementation to disagree with it.
- **Typed IPC.** TypeScript contracts are generated from the Rust types by
  `ts-rs`, so the boundary cannot silently drift.

## Install

Prebuilt packages: `.deb` (Linux x86_64), NSIS installer (Windows x64), `.dmg`
(macOS aarch64). The one-line installers detect the platform, **verify the
SHA-256 against the published manifest**, and never escalate privileges on their
own.

```sh
curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
```

Prefer not to pipe to a shell? Download from the release page and verify by hand:

```sh
sha256sum -c sha256sums-linux.txt
sudo apt install ./AgentLens_0.1.0_amd64.deb
```

Both URLs 404 today, because the repository has not been created yet; see
[Status and Limitations](#status-and-limitations). Full instructions, the
environment overrides, the installed file layout and source builds:
[docs/installation.md](docs/installation.md).

## Quick Start

1. Install, then launch **AgentLens**.
2. Open Host Management. The local machine self-registers on first open, with no
   configuration.
3. Add an SSH host, run Test Connection, copy the reported machine-id hash back
   into the form, then save.
4. Press refresh on the host card to collect.

Step by step, including the machine-id de-duplication rule and how the collector
is transported: [docs/remote-hosts.md](docs/remote-hosts.md).

Where the archive, price overrides and secrets live on each platform:
[docs/data-storage.md](docs/data-storage.md).

## Architecture

| Component | Path | Role |
| --- | --- | --- |
| Core crate | `crates/agentlens-core` | Archive, parsing, aggregation, SSH transport |
| Remote collector | `crates/agentlens-collector` | Static musl single file, pushed on demand |
| Askpass helper | `crates/agentlens-askpass` | `SSH_ASKPASS` peer, shipped in the package |
| Desktop shell | `src-tauri/` | Tauri 2 host, IPC commands, tray |
| Frontend | `frontend/` | React 18.3.1 + Vite 8 + Tailwind v4 |

The archive is SQLite with de-duplication and per-source watermarks. The SSH
transport uses a constant remote command with the payload passed as a positional
argument. Details: [docs/architecture.md](docs/architecture.md).

## Testing and Quality

| Tier | Count | Command |
| --- | --- | --- |
| Rust workspace | 182 passed / 0 failed / 18 ignored | `make test` |
| Vitest unit | 268 cases across 15 specs | `make test-unit` |
| Playwright component | 58 specs, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |
| Line coverage | >= 75% enforced floor; 76.92% (12025/15633) at HEAD on local Linux, 79.56% (11252/14143) on the CodeBuild Linux runner | `make coverage-gate` |

All three platforms build green on AWS CodeBuild in `us-east-2` against the
post-H4b code: Linux `d2edbcdd` (a 5,709,438-byte `AgentLens_0.1.0_amd64.deb`,
182 passed / 0 failed / 18 ignored), Windows `39f89617` (a 4,142,828-byte
`AgentLens_0.1.0_x64-setup.exe`, 170 passed / 0 failed / 8 ignored) and macOS
`82b4d172` (a 5,862,574-byte `AgentLens_0.1.0_aarch64.dmg`, 180 passed / 0
failed / 10 ignored). Linux and Windows read the `13:17:14Z` source zip while
macOS read a later `14:55:03Z` one, with only documentation commits in between,
so the product code is the same even though the zip is not literally the same
file. The three totals are not supposed to agree: `#[cfg(unix)]`-gated tests do
not compile on Windows and `#[cfg(target_os = "linux")]`-gated ones do not
compile on macOS, so on those platforms the tests are absent rather than
ignored. `.omo/evidence/aws-aw5-test-matrix.md` reconciles the counts and the
`cfg`-gate inventory for an earlier round of builds in the earlier `us-west-2`
region. A green build proves the defect did not manifest; nobody has launched the
installers.

## Development

```sh
make help          # list every target
make dev           # Tauri dev mode
make fmt           # format Rust + frontend
make lint          # cargo fmt/clippy + frontend lint/typecheck + copy gate
make test          # cargo test --workspace
make test-unit     # vitest
make test-e2e      # Playwright component QA (mocked IPC)
make test-e2e-real # WebdriverIO against the real WebView
make coverage-gate # coverage with the 75% floor enforced
make dist          # build artifacts/dist/
```

More, including the `dist` targets and the AWS CodeBuild path:
[docs/development.md](docs/development.md).

## Documentation

- [Installation](docs/installation.md)
- [Repository metadata and the pending-remote checklist](docs/repo-metadata.md)
- [Adding remote hosts](docs/remote-hosts.md)
- [Data storage and settings](docs/data-storage.md)
- [Architecture](docs/architecture.md)
- [Development and build](docs/development.md)
- [Remote Source API v1](docs/remote-source-api.md)
- Adapter contracts: [Codex](docs/adapters/codex.md) and
  [Claude Code](docs/adapters/claude-code.md)

## Status and Limitations

- Version `0.1.0`. The single source of truth is `[workspace.package].version`
  in the root `Cargo.toml`; every crate and `src-tauri` inherit it,
  `tauri.conf.json` no longer declares a version, and `make dist-version` echoes
  the resolved value.
- **OpenCode is the only implemented adapter.** The Codex and Claude Code
  documents describe reserved contracts, not working collection.
- Remote hosts are Linux hosts regardless of which platform manages them, so the
  shipped collectors are Linux static binaries.
- **Windows 11 Snap Layouts are lost.** The title bar is drawn by the app on
  Windows and Linux, so hovering the maximize button no longer opens the Windows
  11 layout picker. Aero Snap, the resize borders, the drop shadow and the
  rounded corners all still work. This is an accepted degradation blocked
  upstream on WebView2, not a pending fix; the reasoning is in
  [docs/architecture.md](docs/architecture.md#accepted-degradation-windows-11-snap-layouts).
- **There is no git remote yet.** Every `github.com/sunerpy/AgentLens` URL on
  this page therefore 404s: the badges, and the two installer one-liners. The
  owner is decided and substituted everywhere; what is missing is the repository
  itself. The installers are shellcheck- / parser-clean and were exercised
  against a local source, but have never fetched a real release. Full inventory
  of what that blocks, plus the paste-ready `gh` commands for the repository
  description and topics: [docs/repo-metadata.md](docs/repo-metadata.md).

## License

[MIT](LICENSE) © 2026 sunerpy. The same declaration appears as
`license = "MIT"` in the root `Cargo.toml` (inherited by every crate) and as
`"license": "MIT"` in `frontend/package.json`.
