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
- [For AI coding assistants](#for-ai-coding-assistants)
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
- **Calendar bucketing has one implementation, and it is in Rust.** The frontend
  deliberately carries no `date-fns` / `dayjs` / `moment`. Every raw epoch is
  bucketed in the report timezone, and labels the backend already formatted are
  not converted a second time in the frontend.
- **Typed IPC.** TypeScript contracts are generated from the Rust types by
  `ts-rs`, so the boundary cannot silently drift.
- **Pricing falls back across providers.** When the same model is reached through
  different gateways, the price entry usually only exists under the owning
  provider, so matching is allowed to fall back across providers. Measured over
  251737 records, the priceable share went from 0.1% to 99.4%. Manual overrides
  stay strict: they match `(provider, model)` exactly and never bleed across
  providers. Rules: [../measurement.md](../measurement.md) (Chinese).

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

> Note: **no release has been published yet**, so the download step cannot fetch
> a package. See [Status and Limitations](#status-and-limitations).

Full instructions, the environment overrides, the installed file layout and
source builds: [../installation.md](../installation.md).

## Quick Start

1. Install, then launch **AgentLens**.
2. Open Host Management. The local machine self-registers on first open, with no
   configuration.
3. Add an SSH host and run Test Connection. The machine-id hash is filled in from
   the probe result and turns read-only, so just save.
4. **Tick the sources to collect on the host card.** Enabling lives on the host
   card, not in Settings; only OpenCode is on by default, so Claude Code, Codex
   and Hermes each have to be ticked.
5. Press refresh on the host card to collect. Both the local machine and remote
   hosts can be switched to automatic; remotes use their own interval, and the
   floor for both is 600 seconds.

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
| Rust workspace | 426 cases | `make test` |
| Vitest unit | 560 cases | `make test-unit` |
| Playwright component | 151 cases, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |
| Line coverage | 92.72% measured; the 90% floor is enforced by `make coverage-gate` | `make coverage-gate` |

The GitHub Actions matrix (ubuntu / windows / macos) is green on `main`, and all
three platforms have produced real installers on AWS CodeBuild. But **a green
build proves the defect did not manifest, not that the product works**, so Windows
got a separate real-machine acceptance run: on EC2 Windows Server the package was
actually installed and the app actually launched, and all 25 machine-decidable
GUI assertions passed.

How the coverage floor is enforced, the per-platform build IDs, artifact byte
counts, and the acceptance assertions one by one:
[../development.md](../development.md).

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
- [Per-source measurement semantics](../measurement.md) (Chinese, like the
  adapter contracts)
- Adapter contracts: [Codex](../adapters/codex.md),
  [Claude Code](../adapters/claude-code.md) and [Hermes](../adapters/hermes.md)

The measurement page and the adapter contracts are Chinese-only on purpose: they
are field-by-field semantic contracts, and a translated copy that drifts is worse
than no copy at all.

## For AI coding assistants

Before changing code in this repository, read these three in order:

1. [Per-source measurement semantics](../measurement.md) (Chinese) — how the four
   sources disagree on what one record represents. Required before touching any
   aggregation, or you will add per-message and per-session counts together.
2. [Architecture](../architecture.md) — archive tables, pricing resolution, the
   IPC boundary and the window-chrome trade-offs.
3. [Development and build](../development.md) — every `make` target and the
   verification path.

Three hard constraints:

- **Time handling lives in Rust only.** Do not add `date-fns` / `dayjs` /
  `moment` to `frontend/`; a second timezone implementation is a defect
  generator.
- **The TypeScript contracts are generated.** Never hand-edit
  `frontend/src/generated/`; after changing a Rust DTO, re-export with
  `cargo test -p agentlens-tauri --features ts-export bindings_export`. CI has a
  zero-drift gate that fails when the two disagree.
- **Run `make lint` and `make test` before handing work over.** The coverage
  floor is 90% and `make coverage-gate` enforces it.

## Status and Limitations

- **No release published yet.** The installer scripts live in the repository, but
  the release page carries no artifacts, so neither script has ever fetched a
  real GitHub Release. What is verified and what is still missing:
  [../repo-metadata.md](../repo-metadata.md).
- **The Linux and macOS packages have still never been launched on a real
  machine.** Only Windows has been through real-machine acceptance.
- **Four adapters are implemented (OpenCode, Claude Code, Codex, Hermes), but
  only OpenCode is enabled by default.** The `hosts.enabled_sources` column
  defaults to `'opencode'`; the other three must be ticked explicitly on the host
  card, which is the usual reason for "installed it but see no data".
- **Hermes is a per-session source, and its usage never lands in the message
  count.** Its tokens only exist on the `sessions` table, so each session is
  normalised into one session-level record: usage counts toward tokens and cost,
  but the record lands in `session_record_count` rather than `message_count`, and
  adding the two together is wrong. Semantics:
  [../measurement.md](../measurement.md) (Chinese).
- **The repository carries no real fixture.** Claude Code, Codex and Hermes have
  each been reconciled against real local data, but that data is not committed,
  so every automated test runs on synthetic fixtures. The measured reconciliation
  numbers are recorded in [../measurement.md](../measurement.md) and
  [../adapters/](../adapters/).
- Remote hosts are Linux hosts regardless of which platform manages them, so the
  shipped collectors are Linux static binaries.

The remaining deliberate trade-offs — the single source of truth for the version,
how `granularity` landed in the schema, the models that still resolve no cost,
Codex records all being labelled `openai` so the gateway is lost, `.jsonl.zst`
being unsupported, local Ollama models never being priced, and the Windows 11
Snap Layouts degradation — are documented with their measured numbers in
[Architecture](../architecture.md),
[measurement semantics](../measurement.md) and the
[adapter contracts](../adapters/).

## License

[MIT](../../LICENSE) © 2026 sunerpy
