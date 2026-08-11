# AgentLens

[![CI](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml/badge.svg)](https://github.com/sunerpy/AgentLens/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/sunerpy/AgentLens/branch/main/graph/badge.svg)](https://codecov.io/gh/sunerpy/AgentLens)
![version](https://img.shields.io/github/v/release/sunerpy/AgentLens?sort=semver)
![platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey)
![license](https://img.shields.io/badge/license-MIT-green)

[简体中文](../../README.md) · **English**

A desktop dashboard for AI coding-agent token usage. AgentLens pulls OpenCode,
Claude Code, Codex and Hermes records off your own machine and off any number of
remote SSH hosts into one local SQLite archive, then slices them by timezone,
agent, model and project, down to a single record if you want. Only OpenCode is
collected out of the box. The other three get ticked on per host.

## Table of Contents

- [Screenshots](#screenshots)
- [Highlights](#highlights)
- [Install](#install)
- [Quick Start](#quick-start)
- [Architecture](#architecture)
- [Testing and Quality](#testing-and-quality)
- [Development](#development)
- [Documentation](#documentation)
- [License](#license)

## Screenshots

The three-state rail sits on the left. On the right, top to bottom: range and
granularity, the four token buckets, the cost card, the usage trend chart. The
thing worth explaining here is why the cost card shows one number and not three.
What it shows is a local estimate, this machine's price table times billable
tokens. The alternatives aren't comparable: records carrying an upstream amount
were priced by someone else's table, and records whose model is missing from the
catalog don't even have a complete base. Both sit behind a fold, and neither is
folded into the estimate. The one figure on the card that's safe to compare is the
price per million billable tokens, and the line next to it, how many records the
estimate covers, is there to tell you how much to trust it. In the chart, hatched
spans are buckets with no data coverage and tinted spans are partial coverage.
Neither is a zero.

![Overview page](../../assets/screenshots/overview.png)

The same trend chart grouped by model, in the Deep Ocean theme. Grouping offers
none / model / agent / tool, six themes switch in place from the header, and
neither is buried in Settings. The rail expands, collapses to a 64px icon strip,
or hides entirely.

![Overview grouped by model, Deep Ocean theme](../../assets/screenshots/overview-by-model-dark.png)

Usage analysis is a source → agent → model drilldown sharing one range-and-timezone
state with the overview. The part that needs explaining is a row with no price: it
gets a "cost missing" marker, not a 0. A 0 would read as free. Shares are computed
against the token total of their own level too, never borrowed from a level above.

![Three-level drilldown on the usage analysis page](../../assets/screenshots/usage-drilldown.png)

Hosts puts the local machine and the SSH remotes side by side. The source toggles
live on each host card rather than in Settings, because whether a source is on for
one host and off for another are two separate questions. Only OpenCode starts
ticked.

![Host management page](../../assets/screenshots/hosts.png)

## Highlights

- **The archive is authoritative history, never pruned.** Rotate the source logs,
  delete the backups, wipe the remote data directory: what's archived is still
  there.
- Remote collection is read-only start to finish. A statically linked musl
  collector gets pushed to the remote, sha256-verified, run in place, and cleans
  itself up on exit. It touches none of the remote tool's data.
- Credentials go into the OS keychain and nowhere else: Secret Service on Linux,
  Credential Manager on Windows. No passwords in config files, none handed back
  over IPC.
- **Calendar bucketing has one implementation, and it lives in Rust.** The
  frontend ships no date library at all. No `date-fns`, no `dayjs`, no `moment`.
  Raw epochs are bucketed in the report timezone on the backend, so the labels the
  frontend receives are already final and never get converted into some other
  timezone on the way to the screen.
- TypeScript contracts are generated from the Rust types by `ts-rs` rather than
  hand-written, so the boundary can't silently drift.
- **Pricing is allowed to fall back across providers.** When one model is reached
  through several gateways, its price entry usually only exists under the owning
  provider, and strict `(provider, model)` matching leaves large stretches
  unpriced. Measured over 251737 records, the priceable share went from 0.1% to
  99.4%. Manual overrides are the exception: still exact, still no bleed.
  Rules: [../measurement.md](../measurement.md) (Chinese).

## Install

Three prebuilt packages: `.deb` (Linux x86_64), an NSIS installer (Windows x64),
`.dmg` (macOS aarch64). The one-line installers work out the platform themselves,
**verify the SHA-256 against the published manifest**, and never escalate
privileges on their own.

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

The archive is SQLite, with de-duplication and per-source watermarks. On the SSH
side the remote command is constant; the only thing that varies is the payload,
passed in as a positional argument, so the command itself is never assembled from
strings. Details: [../architecture.md](../architecture.md).

## Testing and Quality

| Tier | Count | Command |
| --- | --- | --- |
| Rust workspace | 426 cases | `make test` |
| Vitest unit | 560 cases | `make test-unit` |
| Playwright component | 151 cases, mocked IPC | `make test-e2e` |
| WebdriverIO | 8 specs, real Tauri WebView against a 155k-row archive | `make test-e2e-real` |
| Line coverage | 92.72% measured; the 90% floor is enforced by `make coverage-gate` | `make coverage-gate` |

The GitHub Actions matrix (ubuntu / windows / macos) is green on `main`, and all
three platforms have produced real installers on AWS CodeBuild. **A green build
proves the defect did not manifest, not that the product works.** Windows got a
separate real-machine acceptance run on top of it: on EC2 Windows Server the
package was installed, the app launched, and all 25 machine-decidable GUI
assertions passed. Linux and macOS have only got as far as producing packages.
Neither has been through the same launch-on-real-hardware check.

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
- [For AI coding assistants](../ai-assistants.md)
- [Remote Source API v1](../remote-source-api.md)
- [Per-source measurement semantics](../measurement.md) (Chinese, like the
  adapter contracts)
- Adapter contracts: [Codex](../adapters/codex.md),
  [Claude Code](../adapters/claude-code.md) and [Hermes](../adapters/hermes.md)

The measurement page and the adapter contracts are Chinese-only on purpose: they
are field-by-field semantic contracts, and a translated copy that drifts is worse
than no copy at all.

## License

[MIT](../../LICENSE) © 2026 sunerpy
