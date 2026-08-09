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
- **Calendar bucketing is implemented in Rust.** The frontend deliberately
  carries no `date-fns` / `dayjs` / `moment`, so there is only one timezone
  engine and no second implementation to disagree with it. Every raw epoch is
  bucketed in the report timezone, and labels the backend already formatted are
  not converted a second time in the frontend.
- **Typed IPC.** TypeScript contracts are generated from the Rust types by
  `ts-rs`, so the boundary cannot silently drift.
- **Pricing falls back across providers.** When the same model is reached through
  different gateways, the price entry usually only exists under the owning
  provider, so matching is allowed to fall back across providers and strips the
  reasoning-effort suffixes (`max` / `thinking` / `fast` and five more). Measured
  over 251737 records, the priceable share went from 0.1% to 99.4%. Manual price
  overrides stay strict: they match `(provider, model)` exactly and never bleed
  across providers.

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
| Rust workspace | 414 passed / 0 failed / 21 ignored | `make test` |
| Vitest unit | 497 cases across 26 files | `make test-unit` |
| Playwright component | 126 cases across 12 spec files, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |
| Line coverage | 92.57% measured across the workspace (53101/57363); the 90% floor is enforced by `make coverage-gate` | `make coverage-gate` |

The hard floor is `COVERAGE_MIN := 90`, declared in the Makefile with `:=` so a
same-named environment variable cannot silently lower it. The measured 92.57% is
flat against last round's 92.61% while the denominator grew from 43674 to 57363
lines: this round's pricing fallback, granularity aggregation and theme layer all
arrived with tests, rather than the ratio being held up by a shrinking
denominator.

The GitHub Actions matrix (ubuntu / windows / macos) is green on `main`, and all
three platforms have produced real installers on AWS CodeBuild. But **a green
build proves the defect did not manifest, not that the product works**, so Windows
got a separate real-machine acceptance run: on EC2 Windows Server the package was
actually installed and the app actually launched, all 25 machine-decidable GUI
assertions passed, and `install.ps1` has its own end-to-end run at 38/38.
**The Linux and macOS packages have still never been launched on a real
machine.**

Per-platform build IDs, artifact byte counts, why the per-platform test totals
differ, and the acceptance assertions one by one:
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

- The single source of truth for the version is `[workspace.package].version` in
  the root `Cargo.toml`; every crate and `src-tauri` inherit it,
  `tauri.conf.json` no longer declares a version, and `make dist-version` echoes
  the resolved value. For the published version, read the badge above or the
  release page.
- **There are four implemented adapters: OpenCode, Claude Code, Codex and
  Hermes.** All four run through both paths, local and remote (collector
  `--source`), one host can collect several sources at once, and
  `SUPPORTED_SOURCES` now holds four entries. The `hosts.enabled_sources` default
  is `'opencode'`, so an upgraded host only collects OpenCode until the other
  three are **enabled explicitly**.
- **Three per-message sources plus one per-session source, and that split changes
  the message count you see.** OpenCode, Claude Code and Codex hang tokens off
  individual messages. Hermes does not: its `messages.token_count` is NULL
  throughout and the five buckets only exist on the `sessions` table (measured: 0
  of 158 message rows carry a token count, while 9 session rows sum to 2038297).
  That is a property of the source, not a defect, so the archive gained a
  `granularity` column (`'message'` / `'session'`) and Hermes normalises each
  session into one session-level record. Aggregation splits accordingly:
  `message_count` counts only per-message records, `session_record_count` counts
  only per-session ones, while the five token buckets, cost and
  `active_session_count` sum across both. **Hermes usage lands in tokens and cost
  but never in the message count**; it lands in the session-record count instead,
  and adding the two counts together is wrong. `granularity` is declared straight
  into the `migration_v1` baseline schema with `DEFAULT 'message'`, together with
  this round's four composite indexes, and `LATEST_SCHEMA_VERSION` is still 3 —
  the project is not in production, so a v4 migration was deliberately skipped.
  **Development archives created before that change are incompatible with the
  current baseline**, but they do not have to be deleted by hand: opening an
  archive verifies the table-column fingerprint, and a mismatch triggers a
  `VACUUM INTO` backup to `archive.db.backup-<timestamp>.db` followed by a rebuild
  on the current baseline.
- **Six models, 1617 records, still resolve no cost.** They are
  `claude-haiku-4-5` (1510), `antigravity-gemini-3.1-pro` (75),
  `claude-sonnet-4-5` (20), `gpt-5.6` (9), `big-pickle` (2) and `auto` (1): the
  pricing catalog has no entry for them, so they land in `unavailable` rather than
  being estimated as 0. Settings can supply a price by hand.
- **Claude Code, Codex and Hermes have all been reconciled against real data, but
  the repository carries no real fixture.** Claude Code was run over 645 jsonl
  files / 5222 lines under a real `~/.claude/projects`: all 17 `messageId` values
  hit, and the five buckets summing to 404254 matched an independent extraction
  bucket by bucket. Codex produced 20252 events from 220 rollouts, with the five
  buckets matching an independent extraction exactly and pricing resolving for
  19952/20252. Hermes produced 9 session-level records summing to 2038297 with
  `skipped=0`, identical to an independent SQL query. All three reconciliations
  ran on private local data that is not committed, so every automated test in the
  repository still runs on synthetic data (14 for Claude Code, 5 for Codex, 6
  `#[test]` for Hermes).
- **Every Codex record is labelled `openai`, and the gateway it came through is
  lost.** `provider_id` is taken from the namespace of `turn_context.model`, so all
  20252 measured Codex records carry provider `openai` even though 17317 of them
  actually arrived via amazon-bedrock. This is deliberate: the pricing catalog is
  organised by the owning provider, so reading `session_meta.model_provider` (the
  forwarding channel) would leave 85% of the records permanently un-costed. It
  costs two things — the channel dimension is invisible in the archive, and
  Bedrock's rates differ from direct access (measured 5.5/27.5 against 5.0/25.0
  USD per Mtok), so this slice of the cost estimate carries a systematic bias.
- **Codex does not support `.jsonl.zst`.** The contract calls for streaming
  decompression, but there were zero zst files locally and the workspace has no
  zstd dependency, so this round deliberately skips such files whole and counts
  them. That is a scope limit, not a defect.
- **Hermes local Ollama models have no price.** Provider normalisation writes
  `ollama`, `pricing_catalog.json` has no such provider, and all three pricing
  match tiers require the provider to be equal, so local models are never
  estimated. Measured: 6 ollama records resolved no price while 3 anthropic
  records resolved normally. That is the correct outcome — it keeps local models
  from picking up cloud prices.
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
