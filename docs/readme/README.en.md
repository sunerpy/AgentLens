# AgentLens

[![CI](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml/badge.svg)](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/sunerpy/AgentLens/branch/main/graph/badge.svg)](https://codecov.io/gh/sunerpy/AgentLens)
![version](https://img.shields.io/github/v/release/sunerpy/AgentLens?sort=semver)
![platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)
![license](https://img.shields.io/badge/license-MIT-green)

[简体中文](../../README.md) · **English**

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
sudo apt install ./AgentLens_*_amd64.deb
```

Full instructions, the environment overrides, the installed file layout and
source builds: [../installation.md](../installation.md).

## Quick Start

1. Install, then launch **AgentLens**.
2. Open Host Management. The local machine self-registers on first open, with no
   configuration.
3. Add an SSH host and run Test Connection. The machine-id hash is filled in from
   the probe result and turns read-only, so just save.
4. Press refresh on the host card to collect.

Step by step, including the machine-id de-duplication rule and how the collector
is transported: [../remote-hosts.md](../remote-hosts.md).

Where the archive, price overrides and secrets live on each platform:
[../data-storage.md](../data-storage.md).

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
argument. Details: [../architecture.md](../architecture.md).

## Testing and Quality

| Tier | Count | Command |
| --- | --- | --- |
| Rust workspace | green (`cargo test --workspace`) | `make test` |
| Vitest unit | 268 cases across 15 specs | `make test-unit` |
| Playwright component | 58 specs, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |
| Line coverage | 91.63% measured across the workspace (15894/17346); the floor is enforced by `make coverage-gate` | `make coverage-gate` |

The GitHub Actions matrix (ubuntu / windows / macos) is green on `main`. An
earlier round of the post-H4b code also built green on AWS CodeBuild in
`us-east-2` on all three platforms: Linux `d2edbcdd` (a 5,709,438-byte
`AgentLens_0.1.0_amd64.deb`, 182 passed / 0 failed / 18 ignored), Windows
`39f89617` (a 4,142,828-byte `AgentLens_0.1.0_x64-setup.exe`, 170 passed / 0
failed / 8 ignored) and macOS `82b4d172` (a 5,862,574-byte
`AgentLens_0.1.0_aarch64.dmg`, 180 passed / 0 failed / 10 ignored). The three
totals are not supposed to agree: `#[cfg(unix)]`-gated tests do not compile on
Windows and `#[cfg(target_os = "linux")]`-gated ones do not compile on macOS, so
on those platforms the tests are absent rather than ignored. A green build proves
the defect did not manifest, not that the product works, so Windows got a
real-machine acceptance run: on EC2 Windows Server the package was actually
installed and the app actually launched, and the GUI run
(`h7-20260805T123646Z`) passed all 25 machine-decidable assertions — client area
exactly 1180x780, no native title bar, a 900x600 minimum track size, a real
SendInput drag with zero drift, and the close button honouring
`prevent_close + hide` instead of exiting the app. `install.ps1` has a separate
end-to-end run at 38/38 (`installps1-20260805T111723Z`), which surfaced and fixed
a real defect: `Start-Process -PassThru -Wait` waits on the whole process tree,
and the NSIS finish page ticks "run AgentLens" by default, so the script never
returned; switching to `ProcessStartInfo` + `WaitForExit()` made it exit cleanly.
**The Linux and macOS packages have still never been launched on a real
machine.**

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
make coverage-gate # coverage with the floor enforced
make dist          # build artifacts/dist/
```

More, including the `dist` targets and the AWS CodeBuild path:
[../development.md](../development.md).

## Documentation

- [Installation](../installation.md)
- [Repository metadata](../repo-metadata.md)
- [Adding remote hosts](../remote-hosts.md)
- [Data storage and settings](../data-storage.md)
- [Architecture](../architecture.md)
- [Development and build](../development.md)
- [Remote Source API v1](../remote-source-api.md)
- Adapter contracts: [Codex](../adapters/codex.md) and
  [Claude Code](../adapters/claude-code.md)

## Status and Limitations

- The single source of truth for the version is `[workspace.package].version` in
  the root `Cargo.toml`; every crate and `src-tauri` inherit it,
  `tauri.conf.json` no longer declares a version, and `make dist-version` echoes
  the resolved value. For the published version, read the badge above or the
  release page.
- **OpenCode is the only implemented adapter.** The Codex and Claude Code
  documents describe reserved contracts, not working collection.
- Remote hosts are Linux hosts regardless of which platform manages them, so the
  shipped collectors are Linux static binaries.
- **Windows 11 Snap Layouts are lost.** The title bar is drawn by the app on
  Windows and Linux, so hovering the maximize button no longer opens the Windows
  11 layout picker. Aero Snap, the resize borders, the drop shadow and the
  rounded corners all still work. This is an accepted degradation blocked
  upstream on WebView2, not a pending fix; the reasoning is in
  [../architecture.md](../architecture.md#accepted-degradation-windows-11-snap-layouts).
- **No release published yet.** The installer scripts live in the repository, but
  the release page carries no artifacts, so the download step cannot fetch a
  package yet. `install.ps1` has been verified end-to-end on a real Windows
  machine at 38/38, covering a real NSIS cancel code, a checksum rejection and a
  non-HTTPS rejection; `install.sh` has only had shellcheck and a local-source
  exercise. **Neither script has ever fetched a real GitHub Release**, because
  there is none yet. Repository description and topics:
  [../repo-metadata.md](../repo-metadata.md).

## License

[MIT](../../LICENSE) © 2026 sunerpy. The same declaration appears as
`license = "MIT"` in the root `Cargo.toml` (inherited by every crate) and as
`"license": "MIT"` in `frontend/package.json`.
