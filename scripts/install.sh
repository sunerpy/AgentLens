#!/usr/bin/env bash
# =============================================================================
# AgentLens installer for Linux and macOS.
#
# What it does, in order:
#   1. Detect OS + CPU architecture and map them to the one release asset that
#      actually exists for that pair. Refuse loudly on any other pair instead of
#      downloading an artifact for the wrong machine.
#   2. Resolve the version (AGENTLENS_VERSION override, else the GitHub
#      "latest release" API).
#   3. Download the per-platform sha256sums-<os>.txt manifest and read the exact
#      asset name + expected digest out of it.
#   4. Download the asset and VERIFY its SHA-256 against that digest. A mismatch
#      is a hard failure; the file is deleted and nothing is installed.
#   5. Print the install command. It does NOT silently escalate to sudo --
#      set AGENTLENS_INSTALL=1 to have the script run the platform installer,
#      and it will say so before invoking sudo.
#
# Why the asset name comes from the manifest instead of being built by hand:
# the release workflow is the only authority on what was actually produced
# (tauri's arch token differs per platform: amd64 / x64 / aarch64). Reading the
# manifest means the script cannot drift from the workflow, and the digest we
# verify against arrives in the same fetch.
#
# NOTE ON THE DEFAULT REPO: AGENTLENS_REPO below names the real repository,
# sunerpy/AgentLens. That repository has not been created yet, so every release
# URL derived from it 404s until it exists -- there is nothing left to
# substitute, only a remote left to create. Export AGENTLENS_REPO to point at a
# fork or a mirror.
# The download path has never fetched a real GitHub release, but it HAS been
# exercised end to end against the real CodeBuild artifacts and the real
# sha256sums-<os>.txt manifests (the 5,659,402-byte .deb and the 5,640,264-byte
# .dmg), including a one-byte-tamper rejection:
# .omo/evidence/install-scripts-real-artifacts.md.
#
# Bash, not POSIX sh: the project's convention is `set -euo pipefail`, and
# `pipefail` plus `${var:-}` handling here are bash features. Both supported
# platforms ship bash.
# =============================================================================
set -euo pipefail

readonly PROGRAM='AgentLens'
readonly DEFAULT_REPO='sunerpy/AgentLens'

# --- configuration (all overridable from the environment) --------------------
REPO="${AGENTLENS_REPO:-$DEFAULT_REPO}"
VERSION="${AGENTLENS_VERSION:-}"
# Test / mirror seam. When set, the GitHub release URL is not used at all, so
# the script can be exercised end to end against a local file:// directory. A
# plaintext http:// source needs AGENTLENS_ALLOW_INSECURE_URL=1 as well.
BASE_URL="${AGENTLENS_BASE_URL:-}"
API_URL="${AGENTLENS_API_URL:-}"
ALLOW_INSECURE="${AGENTLENS_ALLOW_INSECURE_URL:-0}"
DOWNLOAD_DIR="${AGENTLENS_DOWNLOAD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/agentlens/downloads}"
DO_INSTALL="${AGENTLENS_INSTALL:-0}"
DRY_RUN="${AGENTLENS_DRY_RUN:-0}"
# Diagnostic overrides for the platform probe: they make the OS/arch mapping
# inspectable without the matching hardware (and are how the arch matrix in
# .omo/evidence/wd-repo-metadata.md was produced).
UNAME_S_OVERRIDE="${AGENTLENS_UNAME_S:-}"
UNAME_M_OVERRIDE="${AGENTLENS_UNAME_M:-}"

log() { printf '%s\n' "$*" >&2; }
die() {
	printf 'error: %s\n' "$1" >&2
	shift || true
	for line in "$@"; do printf '       %s\n' "$line" >&2; done
	exit 1
}

usage() {
	cat <<'EOF'
AgentLens installer (Linux, macOS)

  curl -fsSL https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.sh | bash

Environment:
  AGENTLENS_REPO          GitHub owner/repo (default: sunerpy/AgentLens)
  AGENTLENS_VERSION       exact version, e.g. 0.1.0 (default: latest release)
  AGENTLENS_BASE_URL      override the asset base URL (mirror / local testing);
                          https:// and file:// only, unless the next one is set
  AGENTLENS_ALLOW_INSECURE_URL=1
                          permit a plaintext http:// source. Then the digest
                          check proves nothing: the same host serves both the
                          package and the manifest. Sources you control only.
  AGENTLENS_API_URL       override the "latest release" API URL
  AGENTLENS_DOWNLOAD_DIR  where to put the downloaded package
                          (default: ${XDG_CACHE_HOME:-$HOME/.cache}/agentlens/downloads)
  AGENTLENS_INSTALL=1     run the platform installer after verifying
                          (announces before it uses sudo)
  AGENTLENS_DRY_RUN=1     print the resolved plan and exit without downloading

Flags: -h | --help
EOF
}

# --- prerequisites ----------------------------------------------------------
DOWNLOADER=''
select_downloader() {
	if command -v curl >/dev/null 2>&1; then
		DOWNLOADER='curl'
	elif command -v wget >/dev/null 2>&1; then
		DOWNLOADER='wget'
	else
		die 'neither curl nor wget is available' \
			'install one of them and re-run'
	fi
}

# The transport allowlist is enforced here, on the URL, instead of being left to
# the downloader's own flags. `curl --proto` restricts curl only, so a wget-only
# host used to accept the very same http:// URL without complaint. That gap is
# not cosmetic: over plaintext http whoever answers supplies BOTH the package and
# the sha256sums manifest it is compared against, so the verification below
# proves nothing. Checking the URL keeps the posture identical for curl and wget.
assert_url_scheme() {
	local url="$1"
	case "$url" in
	https://* | file://*) return 0 ;;
	esac
	if [[ $ALLOW_INSECURE == '1' ]]; then
		log "WARNING: unauthenticated transport, digests are not trustworthy: ${url}"
		log 'WARNING: AGENTLENS_ALLOW_INSECURE_URL=1 -- whoever serves this URL can'
		log 'WARNING: swap the package AND the manifest digest together.'
		return 0
	fi
	die "refusing to fetch over a non-https URL: ${url}" \
		'Only https:// (and file:// for local testing) are allowed by default.' \
		'Over plaintext http an attacker can substitute the package AND the' \
		'sha256sums manifest it is verified against, so the checksum below would' \
		'prove nothing.' \
		'For a source you control, set AGENTLENS_ALLOW_INSECURE_URL=1.'
}

# fetch <url> <dest-file>
fetch() {
	local url="$1" dest="$2"
	assert_url_scheme "$url"
	local protos='=https,file'
	[[ $ALLOW_INSECURE == '1' ]] && protos='=https,http,file'
	case "$DOWNLOADER" in
	curl) curl --proto "$protos" --tlsv1.2 -fsSL "$url" -o "$dest" ;;
	wget) wget -q -O "$dest" "$url" ;;
	esac
}

SHA_CMD=()
select_sha_tool() {
	if command -v sha256sum >/dev/null 2>&1; then
		SHA_CMD=(sha256sum)
	elif command -v shasum >/dev/null 2>&1; then
		SHA_CMD=(shasum -a 256)
	else
		die 'no SHA-256 tool found (need sha256sum or shasum)' \
			'refusing to install an unverified package'
	fi
}

sha256_of() {
	"${SHA_CMD[@]}" "$1" | awk '{print $1}' | tr '[:upper:]' '[:lower:]'
}

# --- platform detection -----------------------------------------------------
OS='' ARCH='' RAW_S='' RAW_M=''
detect_platform() {
	RAW_S="${UNAME_S_OVERRIDE:-$(uname -s)}"
	RAW_M="${UNAME_M_OVERRIDE:-$(uname -m)}"

	case "$RAW_S" in
	Linux) OS='linux' ;;
	Darwin) OS='macos' ;;
	MINGW* | MSYS* | CYGWIN* | Windows_NT)
		die "this script does not install on Windows (detected ${RAW_S})" \
			'use scripts/install.ps1 from PowerShell instead'
		;;
	*) OS='' ;;
	esac

	case "$RAW_M" in
	x86_64 | amd64) ARCH='x86_64' ;;
	aarch64 | arm64) ARCH='aarch64' ;;
	*) ARCH='' ;;
	esac

	if [[ -z $OS || -z $ARCH ]]; then
		die "unsupported platform: uname -s='${RAW_S}' uname -m='${RAW_M}'" \
			'AgentLens publishes packages only for Linux x86_64 and macOS aarch64.' \
			'Build from source instead: see docs/installation.md ("From source").'
	fi
}

# --- artifact selection -----------------------------------------------------
# Mapping is derived from .github/workflows/release.yml, which is the only
# authority on which assets a release carries:
#   linux-deb    -> AgentLens_<v>_amd64.deb        + sha256sums-linux.txt
#                   (plus the two standalone musl collectors)
#   windows-nsis -> AgentLens_<v>_x64-setup.exe    + sha256sums-windows.txt
#   macos-dmg    -> AgentLens_<v>_aarch64.dmg      + sha256sums-macos.txt
# There is deliberately NO arm64 .deb and NO x86_64 .dmg, so those two pairs
# must fail with an explanation rather than fall back to the wrong file.
SUMS_NAME='' ARTIFACT_SUFFIX='' ARTIFACT_KIND=''
select_artifact() {
	case "${OS}:${ARCH}" in
	linux:x86_64)
		SUMS_NAME='sha256sums-linux.txt'
		ARTIFACT_SUFFIX='_amd64.deb'
		ARTIFACT_KIND='deb'
		;;
	linux:aarch64)
		die 'no arm64 Linux package is published' \
			'The release builds the .deb on an x86_64 runner only. What IS published' \
			'for aarch64 is the standalone remote collector' \
			'(agentlens-collector-aarch64-unknown-linux-musl), which is the sidecar' \
			'pushed to remote hosts, not the desktop app.' \
			'To get the desktop app on arm64 Linux, build from source:' \
			'docs/installation.md ("From source").'
		;;
	macos:aarch64)
		SUMS_NAME='sha256sums-macos.txt'
		ARTIFACT_SUFFIX='_aarch64.dmg'
		ARTIFACT_KIND='dmg'
		;;
	macos:x86_64)
		die 'no Intel (x86_64) macOS package is published' \
			'The release builds a single unsigned aarch64 .dmg; there is no universal' \
			'binary and no x86_64 slice. Build from source on this machine instead:' \
			'docs/installation.md ("From source").'
		;;
	*)
		die "unsupported platform pair: ${OS}/${ARCH}"
		;;
	esac
}

# --- version resolution -----------------------------------------------------
# VERSION becomes part of a URL and of a local filename, so it is validated
# against a strict semver shape BEFORE any interpolation. This is what stops
# path-traversal input such as ../../etc/passwd from producing a traversing
# URL or writing outside the download directory.
validate_version() {
	local v="$1"
	if [[ ! $v =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
		die "refusing version '${v}': not a plain semver version" \
			'Expected something like 0.1.0 (optionally 0.1.0-rc.1).' \
			'A version is interpolated into a URL and a filename, so anything' \
			'containing a path separator or traversal is rejected outright.'
	fi
}

validate_repo() {
	if [[ ! $REPO =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]]; then
		die "refusing AGENTLENS_REPO='${REPO}': expected owner/repo"
	fi
	# '.' is inside the class above, so the shape check alone accepted '../..'
	# and interpolated it into the release URL. GitHub has no such owner or repo.
	local owner="${REPO%%/*}" name="${REPO##*/}"
	if [[ $owner == '.' || $owner == '..' || $name == '.' || $name == '..' ]]; then
		die "refusing AGENTLENS_REPO='${REPO}': '.' and '..' are not owner or repo names"
	fi
}

resolve_version() {
	if [[ -n $VERSION ]]; then
		validate_version "$VERSION"
		return
	fi
	if [[ -n $BASE_URL ]]; then
		die 'AGENTLENS_BASE_URL is set but AGENTLENS_VERSION is not' \
			'A custom base URL has no releases API to query, so the version' \
			'must be given explicitly.'
	fi
	local api="${API_URL:-https://api.github.com/repos/${REPO}/releases/latest}"
	local body
	body="$(mktemp)"
	if ! fetch "$api" "$body"; then
		rm -f "$body"
		die "could not query the latest release for ${REPO}" \
			"tried: ${api}" \
			'If the repository has no releases yet (or no remote exists yet),' \
			'pin one explicitly: AGENTLENS_VERSION=0.1.0'
	fi
	# tag_name is v-prefixed by release-please's tag_pattern (^v[0-9]+...).
	VERSION="$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' "$body" | head -1)"
	rm -f "$body"
	[[ -n $VERSION ]] || die "no tag_name in the release API response from ${api}"
	validate_version "$VERSION"
	log "resolved latest release: ${VERSION}"
}

# --- manifest parsing -------------------------------------------------------
ASSET_NAME='' ASSET_SHA=''
read_manifest() {
	local sums_url="${BASE_URL}/${SUMS_NAME}"
	local sums_file="${DOWNLOAD_DIR}/${SUMS_NAME}"

	log "manifest: ${sums_url}"
	fetch "$sums_url" "$sums_file" ||
		die "could not download ${SUMS_NAME}" \
			"tried: ${sums_url}" \
			'Without the manifest there is no digest to verify against, and this' \
			'installer will not install an unverified package.'

	# Manifest shape (all three platforms): optional '#' comment lines, then
	# "<64 hex>  <bare filename>". Filenames are bare basenames by construction
	# (each platform runs its checksum tool from inside the upload directory).
	local hash name matches=0
	while read -r hash name _rest; do
		[[ -z ${hash:-} ]] && continue
		[[ $hash == \#* ]] && continue
		[[ -z ${name:-} ]] && continue
		[[ $name != *"$ARTIFACT_SUFFIX" ]] && continue
		matches=$((matches + 1))
		ASSET_NAME="$name"
		ASSET_SHA="$(printf '%s' "$hash" | tr '[:upper:]' '[:lower:]')"
	done <"$sums_file"

	if ((matches == 0)); then
		die "no asset ending in '${ARTIFACT_SUFFIX}' is listed in ${SUMS_NAME}" \
			"manifest contents:" \
			"$(sed 's/^/         /' "$sums_file")"
	fi
	if ((matches > 1)); then
		die "${SUMS_NAME} lists ${matches} assets ending in '${ARTIFACT_SUFFIX}'" \
			'Refusing to guess which one is meant.'
	fi

	# The manifest is remote content, so its filename field is untrusted: it is
	# joined into both a URL and a local path below. Anything that is not a bare
	# basename is rejected.
	case "$ASSET_NAME" in
	*/* | *\\* | .. | .)
		die "manifest entry '${ASSET_NAME}' is not a bare filename" \
			'Refusing to use a path from a downloaded manifest.'
		;;
	esac
	if [[ ! $ASSET_SHA =~ ^[0-9a-f]{64}$ ]]; then
		die "manifest digest for '${ASSET_NAME}' is not a SHA-256 hex string: '${ASSET_SHA}'"
	fi
}

# --- download + verify ------------------------------------------------------
ASSET_PATH=''
download_and_verify() {
	local url="${BASE_URL}/${ASSET_NAME}"
	ASSET_PATH="${DOWNLOAD_DIR}/${ASSET_NAME}"

	log "download: ${url}"
	fetch "$url" "$ASSET_PATH" ||
		die "download failed: ${url}"

	local actual
	actual="$(sha256_of "$ASSET_PATH")"
	if [[ $actual != "$ASSET_SHA" ]]; then
		rm -f "$ASSET_PATH"
		die "SHA-256 MISMATCH for ${ASSET_NAME} -- refusing to install" \
			"expected ${ASSET_SHA}" \
			"actual   ${actual}" \
			'The downloaded file has been deleted. Either the download was' \
			'corrupted or the artifact does not match the published manifest.' \
			'Do not install this file.'
	fi
	log "sha256 ok: ${ASSET_NAME}"
}

# --- install / hand off -----------------------------------------------------
install_or_report() {
	local cmd
	case "$ARTIFACT_KIND" in
	deb) cmd=(sudo apt install "./${ASSET_NAME}") ;;
	dmg) cmd=(open "./${ASSET_NAME}") ;;
	*) die "internal error: unknown artifact kind '${ARTIFACT_KIND}'" ;;
	esac

	printf '\n%s %s verified at:\n  %s\n\n' "$PROGRAM" "$VERSION" "$ASSET_PATH" >&2

	if [[ $DO_INSTALL != '1' ]]; then
		{
			printf 'Not installed yet -- this installer does not escalate privileges on its own.\n'
			printf 'Run:\n\n  cd %q && %s\n\n' "$DOWNLOAD_DIR" "${cmd[*]}"
			if [[ $ARTIFACT_KIND == 'deb' ]]; then
				printf 'That command needs root (it is a system package). Re-run this script with\n'
				printf 'AGENTLENS_INSTALL=1 to have it invoke sudo for you.\n'
			else
				printf 'Then drag %s to Applications. Re-run with AGENTLENS_INSTALL=1 to have\n' "$PROGRAM"
				printf 'this script open the disk image for you.\n'
			fi
			printf 'Installed file layout: docs/installation.md\n'
		} >&2
		return 0
	fi

	if [[ $ARTIFACT_KIND == 'deb' ]]; then
		log 'AGENTLENS_INSTALL=1: about to run the following command, which uses sudo'
		log "  ${cmd[*]}"
	else
		log "AGENTLENS_INSTALL=1: opening ${ASSET_NAME}"
	fi
	(cd "$DOWNLOAD_DIR" && "${cmd[@]}")
}

print_plan() {
	cat >&2 <<EOF
plan:
  repo          ${REPO}
  version       ${VERSION}
  detected      uname -s='${RAW_S}' uname -m='${RAW_M}' -> ${OS}/${ARCH}
  manifest      ${BASE_URL}/${SUMS_NAME}
  asset match   *${ARTIFACT_SUFFIX} (kind: ${ARTIFACT_KIND})
  download dir  ${DOWNLOAD_DIR}
EOF
}

main() {
	case "${1:-}" in
	-h | --help)
		usage
		exit 0
		;;
	'') ;;
	*) die "unknown argument: $1" 'run with --help for usage' ;;
	esac

	select_downloader
	select_sha_tool
	validate_repo
	detect_platform
	select_artifact
	resolve_version

	if [[ -z $BASE_URL ]]; then
		BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"
	fi

	mkdir -p "$DOWNLOAD_DIR"

	if [[ $DRY_RUN == '1' ]]; then
		print_plan
		log 'AGENTLENS_DRY_RUN=1: stopping before download'
		exit 0
	fi

	print_plan
	read_manifest
	download_and_verify
	install_or_report
}

main "$@"
