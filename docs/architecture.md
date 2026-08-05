# Architecture

[← README](../README.md) · [简体中文](readme/architecture.zh.md)

## Components

| Component | Path | Role |
| --- | --- | --- |
| Core crate | `crates/agentlens-core` | Archive, parsing, aggregation, SSH transport |
| Remote collector | `crates/agentlens-collector` | Headless static musl binary, shipped in the package and pushed to remotes on demand |
| Askpass helper | `crates/agentlens-askpass` | `SSH_ASKPASS` peer, shipped in the package |
| Desktop shell | `src-tauri/` | Tauri 2 host, IPC command layer, tray |
| Frontend | `frontend/` | React 18.3.1, Vite 8, Tailwind v4 |

## Archive

SQLite. Ingestion de-duplicates records and keeps a per-source watermark, so a
re-scan of an unchanged source is cheap and re-ingesting an overlapping window
cannot double-count. The archive is treated as authoritative history and is never
pruned; see [data-storage.md](data-storage.md).

## SSH transport

The remote command is **constant**, with the request payload passed as a single
positional argument. That keeps the remote invocation free of shell interpolation
and makes the contract inspectable: the same command string is used for every
call, and only the argument varies.

The collector binary is transported per refresh, sha256-verified on the remote,
executed in place and cleaned up on exit. Remote work is a read-only scan.

## IPC layer

Tauri commands form the only boundary between the frontend and the core. The
TypeScript contracts are generated from the Rust types by `ts-rs`, so a change to
a Rust struct that the frontend consumes shows up as a TypeScript type error
rather than as a silent runtime mismatch. Payloads are single camelCase objects,
and each wrapper's key set is asserted in the unit tests.

## Timezone and calendar bucketing

Calendar bucketing lives in Rust. The frontend intentionally carries **no**
`date-fns`, `dayjs` or `moment`: two implementations of week boundaries and
timezone offsets is a defect generator, so there is exactly one, on the Rust
side, and the frontend renders what it is given.

## Adapters

`OpenCode` is the only implemented adapter. [Codex](adapters/codex.md) and
[Claude Code](adapters/claude-code.md) are documented **reserved contracts**;
neither collects data today.

## Credentials

Host passwords and key passphrases go to the OS keychain (Linux Secret Service,
Windows Credential Manager). They are never written to a config file and are
never returned to the UI over IPC.

## Window chrome

The title bar is drawn by React on Windows and Linux, and left native on macOS.
The split is a **build-time configuration merge**, not a runtime call: Tauri
merges `tauri.macos.conf.json` over `tauri.conf.json` when it targets macOS, so
`src-tauri/src/**` contains no `set_decorations` and was not touched for this.

| Config file | Applies to | Window settings |
| --- | --- | --- |
| `src-tauri/tauri.conf.json` | Windows, Linux | `decorations: false`, `shadow: true` |
| `src-tauri/tauri.macos.conf.json` | macOS only | `decorations: true`, `titleBarStyle: "Overlay"`, `hiddenTitle: true`, `trafficLightPosition: { x: 20, y: 18 }` |

`titleBarStyle` is a macOS-only setting, which is why one shared `decorations`
value cannot serve all three platforms: `Overlay` keeps the native traffic
lights while the web content extends under them, and there is no Windows or
Linux equivalent.

The merge is RFC 7396, so **arrays are replaced whole, not merged per element**.
`app.windows` is an array, therefore the macOS override has to restate every
geometry field or a macOS build silently falls back to Tauri's default window
size. `windowConfig.test.ts` asserts that both files declare identical geometry.

The React side lives in `frontend/src/app/titlebar/`:

| File | Role |
| --- | --- |
| `TitleBar.tsx` | The drawn bar: drag region, minimize / maximize / close buttons |
| `useWindowChrome.ts` | Subscribes to window state (maximized, focused) and exposes it to the bar |
| `windowControls.ts` | Thin wrappers over the Tauri window API used by the buttons |
| `platform.ts` | `detectPlatform(userAgent)`, so the bar renders only where it should |

Platform detection reads the user agent rather than adding
`@tauri-apps/plugin-os`: the plugin would need an npm package, a Cargo crate, a
builder call and a capability entry to answer a question the user agent already
answers, and it is not injected under Vitest or Playwright, where `platform()`
would throw.

### Accepted degradation: Windows 11 Snap Layouts

On Windows 11, **hovering the maximize button no longer opens the fly-out layout
picker.** This is a real regression against a natively decorated window, and it
is accepted rather than pending.

The cause is upstream and has no clean workaround. The layout picker can only be
opened by responding to the `NC_HITTEST` message, and WebView2 does not send
those messages for clicks inside the webview
([tauri#4531](https://github.com/tauri-apps/tauri/issues/4531)). It is blocked
on WebView2 gaining Window Controls Overlay support.

What still works on Windows:

- Aero Snap — drag to a screen edge, and the `Win` + arrow shortcuts.
- Resize borders and hit-testing, via the transparent overlay window Tauri
  registers over the webview.
- The window drop shadow and the Windows 11 rounded corners. Both depend on
  `shadow: true`, which is declared explicitly for exactly that reason:
  `shadow: false` would drop the corners along with the shadow.

`tauri-plugin-decorum` advertises Snap Layouts support and was rejected. Its
implementation synthesizes a `Win` + `Z` keypress through `enigo` instead of
answering `NC_HITTEST`, so the fly-out is mispositioned when the resize-border
overlay window is present, and the plugin has had no release since 2024-09.
`trafficLightPosition` is a first-party Tauri setting and covers the other
feature it was considered for.

### Icons

`src-tauri/icons/` holds the 16 canonical desktop icon files, now carrying the
AgentLens mark instead of the Tauri placeholder. `bundle.icon` references five
of them, unchanged.

The pipeline is hand-authored SVG → `cairosvg` rasterization → a quantitative
audit → `tauri icon`; no image generation is involved. Both inputs are committed,
so the set is reproducible: the candidate sources are
`assets/brand/candidate-{a..d}-*.svg` and the audit is
`assets/brand/icon_audit.py`. One file is not byte-deterministic —
`icon.icns` regenerates to a different sha256 from the same source each time,
at a constant length of 195,992 bytes.
