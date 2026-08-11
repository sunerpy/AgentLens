#!/usr/bin/env bash
# =============================================================================
# Hermetic local test for the Windows DUAL-FORMAT release contract.
#
# Windows publishes two installers as of the MSI addition: the NSIS
# `*-setup.exe` and the WiX `*.msi`. Both must appear in
# sha256sums-windows.txt, because scripts/install.ps1 -- and any user running
# `sha256sum -c` -- can only verify an asset that carries a digest there.
#
# WHY THIS TEST EXISTS
#   The manifest is produced on a Windows runner, so before this file the only
#   way to see what it would contain was to cut a release. That is far too late
#   to discover that a newly added asset has no digest: nothing about that
#   failure is visible in a build's exit code or logs.
#
# WHAT IT ACTUALLY EXERCISES (no re-implementation, no mocks of our own code)
#   1. scripts/ci/windows-collect-assets.ps1 -- the SHIPPED collector, the exact
#      file both .github/workflows/release.yml and ci.yml invoke -- against a
#      fake bundle tree. Happy path plus both negative paths.
#   2. scripts/install.ps1 -- the SHIPPED installer bootstrap, byte for byte --
#      against the manifest that step 1 produced, converted to CRLF because that
#      is what `Set-Content -Encoding ascii` emits on Windows. This proves the
#      new `.msi` line does not confuse the `-setup.exe` selection.
#
#   Neither script is modified or paraphrased here. A test that re-implements
#   the logic it is checking proves only that the copy agrees with itself.
#
# REQUIREMENTS
#   pwsh (PowerShell 7) and python3. Both are hard requirements: a test that
#   silently skips is worse than no test, because it reads as evidence.
#   AWS is NOT required -- unlike scripts/qa/install-ps1-verify.sh, which drives
#   a real EC2 Windows instance and covers the install step this one cannot.
#
# USAGE
#   scripts/qa/windows-dual-format-manifest.sh
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
readonly REPO_ROOT
readonly COLLECTOR="${REPO_ROOT}/scripts/ci/windows-collect-assets.ps1"
readonly INSTALL_PS1="${REPO_ROOT}/scripts/install.ps1"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentlens-winmanifest-XXXXXX")"
readonly WORK
HTTP_PID=''

cleanup() {
	if [[ -n $HTTP_PID ]] && kill -0 "$HTTP_PID" 2>/dev/null; then
		kill "$HTTP_PID" 2>/dev/null || true
		wait "$HTTP_PID" 2>/dev/null || true
	fi
	rm -rf -- "$WORK"
}
trap cleanup EXIT

PASS=0
FAIL=0
# Set by case_both_formats; consumed by case_install_ps1. A global rather than a
# stdout return value because these functions also log to stdout.
MANIFEST_PATH=''

log() { printf '%s\n' "$*"; }

check() {
	local name="$1" ok="$2" detail="${3:-}"
	if [[ $ok == 'yes' ]]; then
		PASS=$((PASS + 1))
		printf '  [PASS] %s\n' "$name"
	else
		FAIL=$((FAIL + 1))
		printf '  [FAIL] %s\n' "$name"
		[[ -n $detail ]] && printf '         %s\n' "$detail"
	fi
}

require_tools() {
	local tool
	for tool in pwsh python3; do
		command -v "$tool" >/dev/null 2>&1 || {
			printf 'ERROR: required tool not found: %s\n' "$tool" >&2
			printf '       This test does not degrade to a skip. Install it and re-run.\n' >&2
			exit 1
		}
	done
	[[ -f $COLLECTOR ]] || {
		printf 'ERROR: missing artifact under test: %s\n' "$COLLECTOR" >&2
		exit 1
	}
	[[ -f $INSTALL_PS1 ]] || {
		printf 'ERROR: missing artifact under test: %s\n' "$INSTALL_PS1" >&2
		exit 1
	}
	log "pwsh:        $(pwsh --version)"
	log "collector:   ${COLLECTOR}"
	log "install.ps1: ${INSTALL_PS1}"
}

# Fake bundle tree shaped exactly like a native Windows `cargo tauri build`
# output: target\release\bundle\{nsis,msi}\... . Sizes are the real orders of
# magnitude so the byte counts in the log are not misleading.
make_bundle() {
	local root="$1" want_nsis="$2" want_msi="$3"
	rm -rf -- "$root"
	mkdir -p -- "${root}/nsis" "${root}/msi"
	if [[ $want_nsis == 'yes' ]]; then
		head -c 4104407 /dev/urandom >"${root}/nsis/AgentLens_0.0.3_x64-setup.exe"
	fi
	if [[ $want_msi == 'yes' ]]; then
		head -c 5210112 /dev/urandom >"${root}/msi/AgentLens_0.0.3_x64_en-US.msi"
	fi
}

run_collector() {
	local cwd="$1" dest="$2" manifest="$3"
	(cd -- "$cwd" && pwsh -NoProfile -File "$COLLECTOR" \
		-BundleRoot bundle -Destination "$dest" -ManifestName "$manifest")
}

# --- case 1: both formats present ------------------------------------------
case_both_formats() {
	log ''
	log '=== case 1: both installers present -> both hashed ==='
	local cwd="${WORK}/both"
	mkdir -p -- "$cwd"
	make_bundle "${cwd}/bundle" yes yes
	local out rc=0
	out="$(run_collector "$cwd" upload sha256sums-windows.txt 2>&1)" || rc=$?
	printf '%s\n' "$out" | sed 's/^/  | /'
	check 'collector exits 0' "$([[ $rc -eq 0 ]] && echo yes || echo no)" "exit=${rc}"

	local manifest="${cwd}/upload/sha256sums-windows.txt"
	local lines
	lines="$(wc -l <"$manifest" | tr -d ' ')"
	check 'manifest has exactly 2 lines' \
		"$([[ $lines -eq 2 ]] && echo yes || echo no)" "lines=${lines}"
	check 'manifest covers the NSIS -setup.exe' \
		"$(grep -qc -- '-setup\.exe$' "$manifest" && echo yes || echo no)"
	check 'manifest covers the MSI' \
		"$(grep -qc -- '\.msi$' "$manifest" && echo yes || echo no)"

	# The digests must be the real ones. A manifest full of correctly formatted
	# but wrong hashes would satisfy every check above and fail every real
	# download, so verify it the way a user would.
	local verify_rc=0
	(cd -- "${cwd}/upload" && sha256sum -c --strict sha256sums-windows.txt) >"${WORK}/sha-c.log" 2>&1 ||
		verify_rc=$?
	sed 's/^/  | /' "${WORK}/sha-c.log"
	check 'sha256sum -c --strict accepts the manifest' \
		"$([[ $verify_rc -eq 0 ]] && echo yes || echo no)" "exit=${verify_rc}"

	MANIFEST_PATH="$manifest"
}

# --- case 2 and 3: one format missing --------------------------------------
# The point of the whole change: a Windows release that quietly carries only one
# of its two installers must be impossible. Each direction is checked, because a
# guard that only looks for the NEW format still lets the old one disappear.
case_missing_format() {
	local label="$1" want_nsis="$2" want_msi="$3" expect="$4"
	log ''
	log "=== ${label} ==="
	local cwd="${WORK}/${5}"
	mkdir -p -- "$cwd"
	make_bundle "${cwd}/bundle" "$want_nsis" "$want_msi"
	local out rc=0
	out="$(run_collector "$cwd" upload sha256sums-windows.txt 2>&1)" || rc=$?
	printf '%s\n' "$out" | sed 's/^/  | /'
	check "${label}: collector FAILS (non-zero exit)" \
		"$([[ $rc -ne 0 ]] && echo yes || echo no)" "exit=${rc}"
	check "${label}: names the missing format (${expect})" \
		"$(printf '%s' "$out" | grep -q "$expect" && echo yes || echo no)"
	check "${label}: no manifest was written" \
		"$([[ ! -f "${cwd}/upload/sha256sums-windows.txt" ]] && echo yes || echo no)"
}

# --- case 4: install.ps1 against the real dual-format manifest -------------
# install.ps1 selects its asset by suffix (Read-Manifest -Suffix '-setup.exe')
# and REFUSES to guess when more than one line matches. The new .msi line must
# therefore be inert to it. Served over a local HTTP server because
# Invoke-WebRequest cannot fetch file:// URIs.
case_install_ps1() {
	local manifest="$1"
	log ''
	log '=== case 4: install.ps1 parses the dual-format manifest (CRLF) ==='
	local serve="${WORK}/serve"
	mkdir -p -- "$serve"
	cp -- "$(dirname -- "$manifest")"/* "$serve/"

	# Windows Set-Content -Encoding ascii writes CRLF; pwsh on Linux writes LF.
	# Convert so the bytes install.ps1 sees are the bytes a real release serves.
	python3 - "$serve/sha256sums-windows.txt" <<'PYEOF'
import sys
path = sys.argv[1]
with open(path, "rb") as fh:
    raw = fh.read()
raw = raw.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
with open(path, "wb") as fh:
    fh.write(raw)
PYEOF
	log '  manifest bytes as served (od -c, proving CRLF):'
	od -c "$serve/sha256sums-windows.txt" | sed 's/^/  | /'

	local port
	port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
	(cd -- "$serve" && exec python3 -m http.server "$port" --bind 127.0.0.1) \
		>"${WORK}/http.log" 2>&1 &
	HTTP_PID=$!
	local attempt
	for attempt in $(seq 1 50); do
		if python3 -c "
import socket, sys
s = socket.socket()
s.settimeout(0.2)
sys.exit(0 if s.connect_ex(('127.0.0.1', ${port})) == 0 else 1)
" 2>/dev/null; then break; fi
		sleep 0.1
	done

	local dl="${WORK}/downloads" out rc=0
	rm -rf -- "$dl"
	out="$(
		AGENTLENS_BASE_URL="http://127.0.0.1:${port}" \
			AGENTLENS_ALLOW_INSECURE_URL=1 \
			AGENTLENS_VERSION=0.0.3 \
			AGENTLENS_ARCH=AMD64 \
			AGENTLENS_DOWNLOAD_DIR="$dl" \
			pwsh -NoProfile -File "$INSTALL_PS1" 2>&1
	)" || rc=$?
	printf '%s\n' "$out" | sed 's/^/  | /'

	check 'install.ps1 exits 0 against the dual-format manifest' \
		"$([[ $rc -eq 0 ]] && echo yes || echo no)" "exit=${rc}"
	check 'install.ps1 selected the NSIS -setup.exe' \
		"$(printf '%s' "$out" | grep -q 'sha256 ok: AgentLens_0.0.3_x64-setup.exe' && echo yes || echo no)"
	check 'install.ps1 did NOT pick the .msi' \
		"$(printf '%s' "$out" | grep -q 'sha256 ok: .*\.msi' && echo no || echo yes)"
	check 'install.ps1 did not hit the "lists N assets" ambiguity guard' \
		"$(printf '%s' "$out" | grep -q 'lists .* assets ending in' && echo no || echo yes)"
	check 'only the selected asset was downloaded' \
		"$([[ -f "${dl}/AgentLens_0.0.3_x64-setup.exe" && ! -f "${dl}/AgentLens_0.0.3_x64_en-US.msi" ]] && echo yes || echo no)" \
		"$(ls -1 "$dl" 2>/dev/null | tr '\n' ' ')"
}

main() {
	log 'AgentLens Windows dual-format manifest contract'
	log "work dir: ${WORK}"
	require_tools

	case_both_formats
	case_missing_format 'case 2: MSI missing' yes no 'MSI installer not produced' missing-msi
	case_missing_format 'case 3: NSIS missing' no yes 'NSIS installer not produced' missing-nsis
	case_install_ps1 "$MANIFEST_PATH"

	log ''
	log '======================================================================'
	printf 'checks: %s pass / %s fail\n' "$PASS" "$FAIL"
	if [[ $FAIL -ne 0 ]]; then
		log 'VERDICT: FAIL'
		exit 1
	fi
	log 'VERDICT: PASS'
}

main "$@"
