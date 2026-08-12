#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
readonly REPO_ROOT
readonly GENERATOR="${REPO_ROOT}/scripts/ci/generate-latest-json.mjs"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentlens-updater-manifest-XXXXXX")"
readonly WORK
trap 'rm -rf -- "$WORK"' EXIT

readonly ASSETS="${WORK}/assets"
mkdir -p -- "$ASSETS"
printf 'nsis updater' >"${ASSETS}/AgentLens_0.0.5_x64-setup.exe"
printf 'msi updater' >"${ASSETS}/AgentLens_0.0.5_x64_en-US.msi"
printf 'deb updater' >"${ASSETS}/AgentLens_0.0.5_amd64.deb"
printf 'mac updater' >"${ASSETS}/AgentLens.app.tar.gz"
for asset in \
  AgentLens_0.0.5_x64-setup.exe \
  AgentLens_0.0.5_x64_en-US.msi \
  AgentLens_0.0.5_amd64.deb \
  AgentLens.app.tar.gz
do
  printf 'signature-for-%s\n' "$asset" >"${ASSETS}/${asset}.sig"
done
printf 'Signed updater release\nSecond line\n' >"${WORK}/notes.md"

generate() {
  local output="$1"
  node "$GENERATOR" \
    --assets "$ASSETS" \
    --version 0.0.5 \
    --tag v0.0.5 \
    --repository sunerpy/AgentLens \
    --notes "${WORK}/notes.md" \
    --pub-date 2026-08-12T03:00:00Z \
    --output "$output"
}

generate "${WORK}/latest.json"
jq -e '
  .version == "0.0.5" and
  .pub_date == "2026-08-12T03:00:00Z" and
  (.platforms | keys) == [
    "darwin-aarch64",
    "linux-x86_64-deb",
    "windows-x86_64-msi",
    "windows-x86_64-nsis"
  ] and
  (.platforms[] | .signature != "" and (.url | startswith("https://github.com/sunerpy/AgentLens/releases/download/v0.0.5/")))
' "${WORK}/latest.json" >/dev/null
printf '[PASS] complete assets produce strict four-platform latest.json\n'

readonly MISSING_SIG="AgentLens_0.0.5_x64_en-US.msi.sig"
rm -- "${ASSETS}/${MISSING_SIG}"
set +e
negative_output="$(generate "${WORK}/latest-negative.json" 2>&1)"
negative_rc=$?
set -e
printf '%s\n' "$negative_output"
if [[ $negative_rc -eq 0 ]]; then
  printf '[FAIL] missing signature unexpectedly succeeded\n' >&2
  exit 1
fi
if [[ $negative_output != *"missing signature ${MISSING_SIG}"* ]]; then
  printf '[FAIL] failure did not name missing signature %s\n' "$MISSING_SIG" >&2
  exit 1
fi
printf '[PASS] missing signature fails and names %s\n' "$MISSING_SIG"
