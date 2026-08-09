# For AI coding assistants

Read these three before changing code in this repository, in this order:

1. [Per-source measurement semantics](measurement.md) (Chinese) — how the four
   sources disagree on what one record represents. Required before touching any
   aggregation, or you will add per-message and per-session counts together.
2. [Architecture](architecture.md) — archive tables, pricing resolution, the IPC
   boundary and the window-chrome trade-offs.
3. [Development and build](development.md) — every `make` target and the
   verification path.

## Hard constraints

- **Time handling lives in Rust only.** Do not add `date-fns` / `dayjs` /
  `moment` to `frontend/`; a second timezone implementation is a defect
  generator.
- **The TypeScript contracts are generated.** Never hand-edit
  `frontend/src/generated/`; after changing a Rust DTO, re-export with
  `cargo test -p agentlens-tauri --features ts-export bindings_export`. CI has a
  zero-drift gate that fails when the two disagree.
- **Run `make lint` and `make test` before handing work over.** The coverage
  floor is 90% and `make coverage-gate` enforces it.

## Things that surprise newcomers

- Only OpenCode is collected by default. `hosts.enabled_sources` defaults to
  `'opencode'`, so "installed it but see no data" is usually an unticked source
  on the host card, not a parsing bug.
- Hermes is a per-session source. Its usage lands in `session_record_count`, not
  `message_count`; adding the two is wrong.
- The repository carries no real fixture. Every automated test runs on synthetic
  data, so the measured numbers in the docs cannot be reproduced in CI.
- The Linux and macOS packages have never been launched on a real machine. Only
  Windows has been through real-machine acceptance — see
  [Development and build](development.md).
