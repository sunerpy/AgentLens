# Installation

[← README](readme/README.en.md) · [简体中文](readme/installation.zh.md)

## One-line installer

The scripts detect your OS and CPU architecture, pick the one release asset that
exists for that pair, **verify its SHA-256 against the published
`sha256sums-<os>.txt`**, and then hand the package over. They do not escalate
privileges on their own: they print the install command and only run it when you
set `AGENTLENS_INSTALL=1`.

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
```

| Variable | Effect |
| --- | --- |
| `AGENTLENS_VERSION` | Install an exact version (`0.1.0`) instead of the latest release |
| `AGENTLENS_REPO` | GitHub `owner/repo` to download from |
| `AGENTLENS_DOWNLOAD_DIR` | Where the package is written |
| `AGENTLENS_INSTALL=1` | Run the platform installer after verifying (announces before using sudo) |
| `AGENTLENS_DRY_RUN=1` | Print the resolved plan and stop before downloading |

Published assets, per platform: Linux x86_64 gets `.deb`, Windows x64 gets the
NSIS `-setup.exe`, macOS aarch64 gets `.dmg`. There is no arm64 `.deb`, no 32-bit
or arm64 Windows build, and no Intel `.dmg`; on those the script fails with an
explanation and points at [From source](#from-source) rather than downloading the
wrong file.

If you would rather not pipe a script to a shell, every method below works
standalone. Download from the release page and follow the per-platform section.

> The repository sunerpy/AgentLens exists and is public, so both
> `raw.githubusercontent.com` URLs above do fetch the scripts. **No release has
> been published yet**, though, so the scripts run but cannot download an asset:
> both have been verified against a local source only and have never fetched a real
> release. See [repo-metadata.md](repo-metadata.md) for what is still unproven.

## Linux (deb)

Download `AgentLens_<version>_amd64.deb` and `sha256sums-linux.txt` from the
release page, verify, then install. Substitute the version you downloaded for
`<version>`.

```sh
sha256sum -c sha256sums-linux.txt    # verify integrity first
sudo apt install ./AgentLens_<version>_amd64.deb
```

Executables land in `/usr/bin/`:

| Path | Purpose |
| --- | --- |
| `/usr/bin/agentlens-tauri` | Desktop application (the menu entry is named AgentLens) |
| `/usr/bin/agentlens-askpass` | SSH password helper, invoked by the app through `SSH_ASKPASS` |
| `/usr/bin/agentlens-collector-x86_64-unknown-linux-musl` | x86_64 remote collector |
| `/usr/bin/agentlens-collector-aarch64-unknown-linux-musl` | aarch64 remote collector |

## Windows (NSIS)

Run the NSIS installer (`*-setup.exe`) from the release page. Besides the main
program the install directory contains `agentlens-askpass.exe`, both musl
collectors from the table above, and a `collectors.sha256` manifest. Remote hosts
managed from Windows are still Linux hosts, so the collectors remain Linux static
binaries.

## macOS (dmg)

Open the `.dmg` from the release page and drag AgentLens to Applications. The
macOS bundle is built on AWS CodeBuild for `aarch64` (build `82b4d172` produced a
5,862,574-byte `AgentLens_0.1.0_aarch64.dmg`).

## From source

```sh
make dist        # deb, both collector architectures and sha256sums.txt into artifacts/dist/
make dist-all    # same, but fails hard when the aarch64 collector is missing (release use)
```

Requirements: `rustup target add x86_64-unknown-linux-musl
aarch64-unknown-linux-musl`, `musl-gcc`, and an aarch64 musl C cross compiler
(`aarch64-linux-musl-gcc`, or install `zig` and the Makefile wraps it as
`zig cc -target aarch64-linux-musl`).

Without the aarch64 toolchain `make dist` prints a loud warning, emits only the
x86_64 collector, and records the aarch64 absence at the head of
`sha256sums.txt`. It does not fabricate an artifact.

## Next

- [Adding remote hosts](remote-hosts.md)
- [Data storage and settings](data-storage.md)
