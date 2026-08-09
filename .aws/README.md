# AgentLens on AWS CodeBuild

> **`<aws-account-id>` throughout this document is a placeholder.** Substitute
> your own 12-digit AWS account id; every bucket name and role ARN below is
> scoped to it. To print yours:
>
> ```sh
> aws sts get-caller-identity --profile <profile> --region <region> \
>   --query Account --output text
> ```
>
> The `Makefile` and `scripts/qa/*.sh` do not need it spelled out: they resolve
> it from that same call and derive the bucket name from it.

CodeBuild is the **proving ground** for AgentLens artifacts, not an alternative
to GitHub Actions. The order is deliberate:

1. Build and verify all three platforms here.
2. **Only after all three go green**, write `.github/workflows/*` by mechanically
   translating the steps proven here.

Every gate in these buildspecs is therefore a plain invocation of a Makefile /
npm / cargo target that also exists locally, so `run: make lint` in a future
workflow is a 1:1 copy. Do not inline CodeBuild-only logic into a gate — put it
in the `Makefile` instead, where both CI systems can reach it.

---

## The buildspec ⊇ workflow parity contract

Step 1 above only means something if the buildspec is a **superset** of the
workflow on every gate the workflow runs. It was not, for six commits:
`linux.yml` ran `npm --prefix frontend run test:unit`, `windows.yml` and
`macos.yml` had **zero** occurrences of it, while `.github/workflows/ci.yml`
runs the frontend gates on all three platforms. So `ci.yml` contained steps no
buildspec had ever executed — "proven source" was false for those steps.

The rule that closes it, and must not be quietly broken again:

> **Every gate `ci.yml` runs on platform P must also run in the buildspec for
> platform P** — or the buildspec must say, in a comment at the omission site,
> *why* it cannot.

Currently justified omissions (all three also Linux-only in `ci.yml`, so parity
holds):

| Gate | Linux | Windows | macOS | Why not everywhere |
|---|---|---|---|---|
| `format:check` / `lint` / `typecheck` | ✅ via `make lint` | ✅ bare npm | ✅ bare npm | — |
| `check-i18n.mjs` | ✅ via `make lint` | ✅ | ✅ | — |
| `test:unit` (Vitest) | ✅ | ✅ | ✅ | — |
| `npm run build` | ✅ | ✅ | ✅ | Tauri `beforeBuildCommand` needs it anyway |
| `cargo fmt` / `clippy -D warnings` / `cargo test --workspace` | ✅ | ✅ | ✅ | — |
| oxfmt (YAML/JSON/Markdown, inside `make fmt-check`) | ✅ | ❌ | ❌ | Platform-neutral bytes; a 2nd and 3rd opinion adds no signal. Linux-only in `ci.yml` too |
| `make coverage-gate` | ✅ | ❌ | ❌ | One coverage number per commit or Codecov double-counts. Linux-only in `ci.yml` too |
| `make dist-all` | ✅ | ❌ | ❌ | Linux packaging; the others ship NSIS / dmg |
| ts-rs bindings drift | ✅ **real, proven red** | ❌ | ❌ | The S3 zip now carries `.git/`, so the drift half genuinely runs on CodeBuild (see below). Not duplicated on the other two: the export half needs a `--features ts-export` rebuild of `agentlens-tauri`, the priciest thing to repeat on the two slowest platforms, and drift is a property of the commit, not of the OS |
| Playwright `test:e2e` (56 specs) | ✅ | ❌ | ❌ | Component-level, mocked IPC — platform-neutral by construction. `ci.yml` runs it **nowhere**, so this is buildspec ⊃ workflow, the safe direction |
| wdio `test:e2e:real` (8 specs) | ❌ | ❌ | ❌ | Needs the real Tauri WebView, an X display **and the operator's own OpenCode DB**. Measured, not assumed: `buildspec/linux-e2e-real-probe.yml` |
| any `make` target | ✅ | ❌ | ✅ | GNU make is not guaranteed on `windows-base:2022-1.0`; `ci.yml` also drops to bare npm/cargo there |

Windows and macOS use bare `npm`/`cargo`, not `make lint`, for the same reason
`ci.yml` does: `make lint` would drag oxfmt in with it.

---

## Two traps that will cost you an hour if you skip them

### 1. The ambient region points at the wrong AWS partition

This shell exports `AWS_REGION=cn-northwest-1` and
`AWS_DEFAULT_REGION=cn-northwest-1`, left over from a China profile. So:

```console
$ aws sts get-caller-identity --profile us
An error occurred (InvalidClientTokenId) when calling the GetCallerIdentity
operation: The security token included in the request is invalid.

$ aws sts get-caller-identity --profile us --region us-west-2
{ "Account": "<aws-account-id>", "Arn": "arn:aws:iam::<aws-account-id>:user/sunerpy" }
```

The failure reads like broken credentials; it is actually the `us` profile's
credentials being presented to a China-partition endpoint. **Pass `--region <the
region you mean>` on every single AWS command.** Never rely on the ambient
region.

**This includes the S3 calls that look region-agnostic.** Measured:
`aws s3api get-bucket-location --bucket ... --profile us` without `--region`
returns `InvalidAccessKeyId`, because the request still resolves against the
China endpoint. `s3api create-bucket` outside `us-east-1` additionally needs
`--create-bucket-configuration LocationConstraint=<region>`, or the bucket lands
in the wrong place — and CodeBuild's S3 source **must** be in the build's own
region.

This trap also bites *inside* the Makefile: `AWS_REGION ?= us-west-2` silently
does nothing, because Make treats an environment variable as already defined.
That is why the `Makefile` uses `AWS_REGION := us-west-2` (a makefile assignment
outranks the environment, while a command-line `make ... AWS_REGION=x` still
wins). The measured symptom of getting this wrong was an S3 upload failing with
`InvalidAccessKeyId: The AWS Access Key Id you provided does not exist in our
records`.

`S3_BUCKET` is now derived from `AWS_REGION`, because S3 buckets are regional
and the source zip has to sit in the build's region:

```sh
make aws-source-upload                      # us-west-2 -> agentlens-build-<aws-account-id>
make aws-source-upload AWS_REGION=us-east-2  # us-east-2 -> agentlens-build-use2-<aws-account-id>
```

An unmapped region falls back to `agentlens-build-<region-without-dashes>-<account>`,
so adding a region needs no recipe edit. CodeBuild project names are unique
*per region*, so `agentlens-linux` exists independently in both regions and the
two never collide.

### 1b. The shell here is zsh, which does not word-split unquoted variables

```console
$ R="--region us-east-2 --profile us"
$ aws codebuild batch-get-fleets --names test-mac-us-east-2 $R
An error occurred (InvalidInputException) ... Invalid characters in fleet name.
```

zsh passes `$R` as **one** argument, which `--names` then swallows. Under bash
the same line works. Write the flags out in full, or use `${=R}`.

### 2. Source type is S3, because this repo has no git remote

`git remote -v` is empty. There is nothing for CodeBuild to clone, so we package
the working tree and build from a zip:

```sh
make aws-source-upload   # -> s3://agentlens-build-<aws-account-id>/source/agentlens-src.zip
```

This is precisely what makes "verify before creating GitHub workflows" possible.
The zip excludes `target/`, `frontend/node_modules/`, `frontend/dist/`,
`artifacts/` and `.omo/`.

### 2b. `.git/` is REQUIRED source-package content, not baggage

The zip used to exclude `.git/` as well (~900 KB, ~206 files). That quietly
destroyed a gate. `linux.yml`'s ts-rs drift check is:

```sh
cargo test -p agentlens-tauri --features ts-export bindings_export   # re-export
git status --porcelain -uall -- frontend/src/generated/              # compare
```

With no `.git/` there is no index to compare against, so `git status` was
*structurally* empty and the second line could never fire. The gate meant only
"the export did not crash" — it stayed green no matter how far the committed
`.ts` bindings had drifted from the Rust DTOs. That is not a degraded gate, it is
a decorative one, and the old note here calling it "acceptable" was wrong.

So `make aws-source-upload` now ships `.git/` and **refuses to upload** unless
`.git/HEAD`, `.git/index` and at least one `.git/objects/` entry are in the
archive. All three are load-bearing: HEAD+refs resolve the commit, `index` is the
baseline `git status` diffs the worktree against, and the objects are what
`git diff` reads to print the real difference on the failure path.

Cost: the zip goes from ~900 KB / ~206 entries to ~5 MB / ~1436 entries (this
repo's `.git` is all loose objects, no packs), which is a few extra seconds in
`aws s3 cp` and in `DOWNLOAD_SOURCE`. Verified end to end on us-east-2 — the gate
was made to go **red** on a deliberately mismatched binding and green again on
the restored one; the log lines are in
`.omo/evidence/aws-use2-e2e-drift.md`. The same commit also dropped the
`2>/dev/null || true` that used to wrap the `git status` call, since with `.git/`
guaranteed present a git error must be a hard failure rather than a silent
"no drift".

---

## What each buildspec gates

| File | Project | Gates |
|---|---|---|
| `buildspec/linux.yml` | `agentlens-linux` | `make lint` (rustfmt + oxfmt + clippy `-D warnings` + frontend `format:check`/`lint`/`typecheck` + `check-i18n.mjs`) → `cargo test --workspace` → frontend `test:unit` → ts-rs `bindings_export` drift → `make test-e2e` (Playwright, 56 specs) → `make coverage-gate` (floor 75 %, emits `artifacts/coverage/lcov.info` for Codecov) → `make dist-all` (`.deb` + both musl collectors + `sha256sums.txt`) |
| `buildspec/windows.yml` | `agentlens-windows` | `cargo fmt --check` → `cargo clippy -D warnings` → `cargo test --workspace` → frontend build → sidecar staging → NSIS installer via `cargo tauri build --bundles nsis --config src-tauri/tauri.bundle.windows.json` |
| `buildspec/macos.yml` | `agentlens-macos` | `cargo fmt --check` → `cargo clippy -D warnings` → `cargo test --workspace` → frontend build → sidecar staging → `.app`/`.dmg` via `cargo tauri build --bundles app,dmg --target aarch64-apple-darwin --config src-tauri/tauri.bundle.macos.json` |

Linux owns the platform-neutral gates (frontend lint/typecheck/i18n, coverage).
Windows and macOS deliberately do not duplicate them — that only lengthens slow
builds. Both still run `npm run build`, because Tauri's `beforeBuildCommand`
needs `frontend/dist`.

`linux.yml` uses `make dist-all`, not `make dist`. `dist` degrades to a
single-arch bundle when the aarch64 musl cross compiler is missing; `dist-all`
makes that a hard failure. Since the buildspec installs zig precisely so the
aarch64 collector *can* be built, the degrading variant would let a silently
single-arch bundle be reported as green.

### E2E: tier 1 is gated, tier 2 is measured and honestly excluded

- **Tier 1 — Playwright, 56 specs, `make test-e2e`** is gate `[5/7]` in
  `linux.yml`. Headless Chromium against `vite dev` with the IPC layer mocked, so
  no X server is involved. `playwright install --with-deps chromium` runs in
  `pre_build`: `standard:7.0` ships neither the browser nor the libs it links
  (`libnss3`, `libasound2`, `libatk-bridge2.0`…), and without `--with-deps`
  Chromium dies with a bare "error while loading shared libraries" that reads
  like a Playwright bug. `playwright.config.ts` owns the `webServer` lifecycle,
  so nothing is backgrounded by hand in the buildspec.
- **Tier 2 — wdio, 8 specs, `make test-e2e-real`** is **not** gated, but it has
  now actually been run: `buildspec/linux-e2e-real-probe.yml` (Xvfb +
  `webkit2gtk-driver` + `tauri-driver` + a real `target/debug` binary) measured

  ```text
  Spec Files:  5 passed, 3 failed, 8 total (100% completed) in 00:06:40
  ```

  The interesting half is what did **not** fail. The `POST /session` hang that
  blocks this suite on the local Arch box does not reproduce in the container:
  sessions are created in ~1.1 s, the real WebView boots as `wry 0.55.1 linux`,
  real IPC answers, and 00-smoke / 30-geometry / 40-scope / 60-threshold /
  90-tray pass against the real driver. All 3 failures reduce to one runtime
  fact — `OpenCode database was not found; probed paths: [...]`, so nothing is
  archived and `expect(archived).toBeGreaterThan(0)` fails. Tier 2 is therefore
  blocked on **test data**, not on the driver, and the remaining work is a
  synthetic archive fixture — which must not be faked just to turn a build
  green. Full log lines: `.omo/evidence/aws-use2-e2e-drift.md`.
- **Ignored Rust tests** — 18 of the tests are `#[ignore]` because they need the
  operator's live ~43 GB archive DB or a real OS keyring/GUI. Plain
  `cargo test --workspace` skips them, which is what we want. **Never add
  `--ignored` / `--include-ignored` to a CI gate.**
- **Code signing** — macOS bundles are unsigned. Notarisation would need an
  Apple Developer cert in Secrets Manager plus `codesign`/`notarytool` steps.

---

## Ownership

| Surface | Owner |
|---|---|
| `buildspec/*.yml`, `Makefile` `aws-*` targets, the S3 bucket | shared prep (this doc) |
| `agentlens-linux` project + iterating `linux.yml` | Linux worker |
| `agentlens-windows` project + iterating `windows.yml` | Windows worker |
| `agentlens-macos` project + `src-tauri/tauri.bundle.macos.json` + iterating `macos.yml` | macOS worker |

`src-tauri/tauri.bundle.macos.json` **now exists** (added at `d718260`, 415 B).
The "does not exist yet" note that used to live here is obsolete; `macos.yml`
still fails fast with an actionable message if the file goes missing, and its
header comment still documents the expected `externalBin` / `resources` shape.

Do not put account IDs, bucket names, or role ARNs inside a buildspec. They
belong in the `Makefile` and in the `create-project` call below.

---

## Reusable AWS assets — do not create new ones

The account and the service role are global; everything else is per region.

| Asset | `us-west-2` (original) | `us-east-2` (added) |
|---|---|---|
| Profile / account | `us` → `<aws-account-id>` | same |
| Service role | `arn:aws:iam::<aws-account-id>:role/workkit-codebuild-role` | same role, global |
| Source + artifact bucket | `agentlens-build-<aws-account-id>` | `agentlens-build-use2-<aws-account-id>` |
| MAC_ARM fleet | `workkit-macos-fleet`, `test-mac-us-west-2` | `test-mac-us-east-2` |
| Linux compute | `BUILD_GENERAL1_2XLARGE`, timeout 90 | `BUILD_GENERAL1_LARGE`, timeout 180 |
| Windows compute | `BUILD_GENERAL1_2XLARGE`, timeout 120 | `BUILD_GENERAL1_MEDIUM`, timeout 240 |
| macOS compute | `BUILD_GENERAL1_LARGE`, timeout 120 | `BUILD_GENERAL1_LARGE`, timeout 90 |

The us-east-2 timeouts are derived from the measured us-west-2 wall clocks
scaled for the smaller compute, not guessed:

- **Linux 180.** 718 s at 2XLARGE (72 vCPU). LARGE is 8 vCPU, and the two
  heaviest gates (`coverage-gate` re-instruments the whole workspace,
  `dist-all` links three release targets) are compile-bound, so a 3-5x factor is
  expected. 180 leaves headroom without letting a genuine hang run for hours.
- **Windows 240.** 24 m 21 s / 26 m 15 s at 2XLARGE, of which ~7 m was the VS2022
  BuildTools install (I/O bound, does not shrink with vCPUs) and ~17 m was
  compile. MEDIUM is 4 vCPU, so the compile part can plausibly be 4-6x, plus the
  frontend gates this file newly added. 240 is roughly 2x the worst realistic
  case.
- **macOS 90.** Measured 176-211 s hot, 239 s install cold, plus up to 852 s of
  fleet queueing. 90 is already an order of magnitude of headroom; the fleet's
  `baseCapacity: 1` queue, not the build, is what makes macOS feel slow.

**Reuse the fleet — never create one.** `test-mac-us-east-2` has
`baseCapacity: 1`, exactly like the two us-west-2 fleets, so **one macOS build at
a time account-wide per fleet**. `agentlens-macos` queues behind any sibling
macOS project (`tradepilot-macos`, `godot-mcp-macos`, `up-to-down-macos`,
`web-test-genie-macos` all share it). Check before starting, and do not read the
queue delay as a hang:

```sh
aws codebuild list-builds-for-project --project-name tradepilot-macos \
  --region us-east-2 --profile us --max-items 1
```

Windows runs **on demand at any compute type — no fleet needed**. The original
note here claimed only `2XLARGE` was on-demand; the sibling `codegraph-rs-windows`
in us-east-2 runs `BUILD_GENERAL1_MEDIUM` with `environment.fleet == null`, and
`agentlens-windows` in us-east-2 does the same, so that claim was wrong.

### The role's S3 policy is per bucket — a new region needs one line added

`workkit-codebuild-role` carries a per-project inline policy `agentlens-s3`
whose `Resource` list was scoped to the us-west-2 bucket only. A build from a new
bucket therefore dies in `DOWNLOAD_SOURCE`, 2 seconds in, with a message that
names the missing action exactly:

```text
error while downloading key source/agentlens-src.zip, error: AccessDenied:
User: .../AWSCodeBuild-<build-uuid> is not authorized to perform: s3:GetObject
on resource: "arn:aws:s3:::agentlens-build-use2-<aws-account-id>/source/agentlens-src.zip"
because no identity-based policy allows the s3:GetObject action
```

The fix is **additive** — append the new bucket to the same inline policy, never
create a role and never remove the us-west-2 grant:

```sh
aws iam get-role-policy --role-name workkit-codebuild-role \
  --policy-name agentlens-s3 --profile us --region us-east-2
# add "arn:aws:s3:::<new-bucket>" and "<new-bucket>/*" to Resource, then
aws iam put-role-policy --role-name workkit-codebuild-role \
  --policy-name agentlens-s3 --policy-document file://agentlens-s3.json \
  --profile us --region us-east-2
```

---

## Creating the projects

Each platform worker runs one of these. Upload the source first
(`make aws-source-upload`).

The three blocks below are the **us-west-2** originals, kept verbatim as the
historical record. For **us-east-2** the shape is identical with four
substitutions — `--region us-east-2`, bucket `agentlens-build-use2-<aws-account-id>`,
the compute types and timeouts from the asset table above, and the
`test-mac-us-east-2` fleet ARN:

```sh
aws codebuild batch-get-fleets --names test-mac-us-east-2 \
  --region us-east-2 --profile us --query 'fleets[0].arn' --output text
# arn:aws:codebuild:us-east-2:<aws-account-id>:fleet/test-mac-us-east-2:9405d674-...
```

Do not paste that ARN from here — fleets can be recreated, so resolve it each
time. Note the zsh word-splitting trap in section 1b before factoring these
long command lines into shell variables.

### Linux

```sh
aws codebuild create-project \
  --name agentlens-linux \
  --region us-west-2 --profile us \
  --service-role arn:aws:iam::<aws-account-id>:role/workkit-codebuild-role \
  --source '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>/source/agentlens-src.zip",
    "buildspec": ".aws/buildspec/linux.yml"
  }' \
  --artifacts '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>",
    "path": "artifacts",
    "namespaceType": "BUILD_ID",
    "name": "agentlens-linux",
    "packaging": "ZIP"
  }' \
  --environment '{
    "type": "LINUX_CONTAINER",
    "image": "aws/codebuild/standard:7.0",
    "computeType": "BUILD_GENERAL1_2XLARGE",
    "privilegedMode": false
  }' \
  --timeout-in-minutes 90
```

### Windows

```sh
aws codebuild create-project \
  --name agentlens-windows \
  --region us-west-2 --profile us \
  --service-role arn:aws:iam::<aws-account-id>:role/workkit-codebuild-role \
  --source '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>/source/agentlens-src.zip",
    "buildspec": ".aws/buildspec/windows.yml"
  }' \
  --artifacts '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>",
    "path": "artifacts",
    "namespaceType": "BUILD_ID",
    "name": "agentlens-windows",
    "packaging": "ZIP"
  }' \
  --environment '{
    "type": "WINDOWS_SERVER_2022_CONTAINER",
    "image": "aws/codebuild/windows-base:2022-1.0",
    "computeType": "BUILD_GENERAL1_2XLARGE",
    "privilegedMode": false
  }' \
  --timeout-in-minutes 120
```

### macOS

Fleet-backed — macOS has no on-demand compute. Use the full fleet ARN:

```sh
aws codebuild create-project \
  --name agentlens-macos \
  --region us-west-2 --profile us \
  --service-role arn:aws:iam::<aws-account-id>:role/workkit-codebuild-role \
  --source '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>/source/agentlens-src.zip",
    "buildspec": ".aws/buildspec/macos.yml"
  }' \
  --artifacts '{
    "type": "S3",
    "location": "agentlens-build-<aws-account-id>",
    "path": "artifacts",
    "namespaceType": "BUILD_ID",
    "name": "agentlens-macos",
    "packaging": "ZIP"
  }' \
  --environment '{
    "type": "MAC_ARM",
    "image": "aws/codebuild/macos-arm-base:15",
    "computeType": "BUILD_GENERAL1_LARGE",
    "privilegedMode": false,
    "fleet": {
      "fleetArn": "arn:aws:codebuild:us-west-2:<aws-account-id>:fleet/workkit-macos-fleet:ad4ede23-8de6-402b-a83b-f3f7ea8815e4"
    }
  }' \
  --timeout-in-minutes 120
```

Resolve the fleet ARN yourself rather than trusting the id above, since fleets
can be recreated:

```sh
aws codebuild batch-get-fleets --names workkit-macos-fleet \
  --region us-west-2 --profile us --query 'fleets[0].arn' --output text
```

---

## Day-to-day

```sh
make aws-source-upload        # re-package + upload the working tree
make aws-build-linux          # start a build, prints the build id
make aws-build-windows
make aws-build-macos          # serialize — fleet baseCapacity is 1
make aws-status               # latest build per platform
make aws-logs BUILD_ID=agentlens-linux:xxxxxxxx           # fetch logs
make aws-logs BUILD_ID=agentlens-linux:xxxxxxxx FOLLOW=1  # follow
```

Append `AWS_REGION=us-east-2` to any of them to drive the other region; the
bucket follows automatically and every target echoes the region it resolved, so
a mis-typed region is visible on line one instead of surfacing as a confusing
`InvalidAccessKeyId` later:

```sh
make aws-source-upload AWS_REGION=us-east-2
make aws-build-linux   AWS_REGION=us-east-2
make aws-status        AWS_REGION=us-east-2
```

Re-upload the source after every code change — CodeBuild reads the zip, not your
working tree. The bucket is versioned, so each upload creates a new version and
you can tell from `LastModified` whether a build used the source you meant.

To point everything at a different account, override the variables; no recipe
edits needed:

```sh
make aws-source-upload AWS_PROFILE=other AWS_ACCOUNT=123456789012 \
  S3_BUCKET=agentlens-build-123456789012
```

---

## Platform gotchas already encoded in the buildspecs

Read these before "fixing" something that is already handled.

### Linux (`standard:7.0`, Ubuntu 22.04)

- Evict the stale cargo registry (`rm -rf $HOME/.cargo/registry
  $HOME/.cargo/config.toml`) before installing a fresh rustup — the preinstalled
  toolchain lags.
- Tauri 2 needs `libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`,
  `libjavascriptcoregtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `patchelf`,
  and `libayatana-appindicator3-dev` (the last one because `src-tauri` enables
  the `tray-icon` feature; without it the bundler panics
  "Can't detect any appindicator library").
- `rusqlite` uses the `bundled` feature, so it *compiles* `sqlite3.c` and every
  musl target needs a musl-ABI C compiler: `musl-tools` for x86_64, and zig for
  aarch64. `aarch64-linux-gnu-gcc` is the **wrong ABI** and will not work. The
  Makefile already generates the `zig cc -target aarch64-linux-musl` wrapper,
  strips the `--target=` argument cc-rs injects, and sets
  `-C link-self-contained=no` (otherwise `ld.lld: duplicate symbol: _start`).
  The buildspec only has to make `zig` resolvable.
- Node is installed from a pinned tarball: the frontend is on Vite 8 / Vitest 4 /
  TypeScript 6, which need Node ≥ 20.19.
- `cargo tauri build` **must run from the repo root** — from `frontend/` it
  panics "Couldn't recognize the current folder as a Tauri project".

### Windows (`windows-base:2022-1.0`)

- **Each buildspec list item runs in its own PowerShell process**, so
  `$env:PATH` does not survive to the next item → everything lives in one
  multi-line block.
- **Use a single phase.** Between phases CodeBuild serializes the PowerShell
  session and restores it with `Set-Variable`, which throws "Cannot overwrite
  variable ExecutionContext because it is read-only".
- The preinstalled Rust is a **Chocolatey shim** in
  `C:\ProgramData\chocolatey\bin`. Deleting the user `.cargo`/`.rustup` dirs does
  not remove it, and filtering PATH on `\.cargo`/`\.rustup` does not drop the
  chocolatey segment. Delete the shim exes *and* filter `chocolatey` out of PATH.
- The image ships **VS Build Tools 2019 without the C++ workload**, so there is
  no MSVC `link.exe` and the only `link` on PATH is GNU coreutils at
  `C:\tools\msys64\usr\bin` ("link: extra operand"). Install
  `visualstudio2022buildtools` + `visualstudio2022-workload-vctools`, then
  `Enter-VsDevShell`, and drop `msys64` / `Git\usr\bin` / `mingw` from PATH.
- Enable long paths for git and the kernel; Tauri/MSVC paths exceed `MAX_PATH`.
- Use `Start-Process -Wait -PassThru` for `rustup-init` — direct invocation
  reports non-zero because of an MSVC warning.
- The collectors in the Windows installer are Linux musl binaries and are
  declared as `resources`, not `externalBin` (`externalBin` would append `.exe`,
  and the app looks for the un-suffixed name). Windows has no musl cc, so they
  are cross-compiled with `cargo-zigbuild`.

### macOS (`macos-arm-base:15`)

- Fleet required; `baseCapacity: 1` means serialize.
- brew-installed node is **not** on PATH: `export
  PATH="/opt/homebrew/opt/node@22/bin:$PATH"`.
- Target is `aarch64-apple-darwin` only, not universal — the fleet is
  capacity-1 and account-serialized, so a universal build doubles everyone's
  wait, and the bundled sidecars are Linux musl regardless of host arch. To
  switch: add `rustup target add x86_64-apple-darwin` and change the `--target`
  to `universal-apple-darwin`.
- Same musl-cross story as Windows: `cargo-zigbuild` for the collectors.
