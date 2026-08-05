#!/usr/bin/env bash
# =============================================================================
# Run scripts/install.ps1 -- the SHIPPED Windows installer bootstrap -- on a real
# Windows instance against the real NSIS installer, and collect the evidence.
#
# WHAT THIS IS FOR
#   scripts/install.ps1 is parser-clean and was exercised against a local HTTP
#   source under pwsh 7 on Linux, but had never executed on Windows: the
#   Start-Process / .ExitCode branch at install.ps1:362-369 had never run against
#   a real installer. This driver closes that gap and reports what happened.
#
# WHAT IT DELIBERATELY DOES NOT DO
#   * It does NOT modify scripts/install.ps1. That file is the artifact under
#     test; it is staged byte-for-byte and its SHA-256 is recorded on both sides.
#   * It NEVER creates, stops, reboots or terminates an AWS resource. There is no
#     ec2 run-instances and no ec2 terminate-instances call anywhere in this file
#     -- deliberately, so no verb of this script can destroy the QA box. Lifecycle
#     stays with scripts/qa/ec2-windows-gui-qa.sh, which owns it.
#   * It contains no executable `trap`. A trap firing mid-teardown is how a
#     partial state gets reported as a clean one.
#
# WHERE THE WORK HAPPENS
#   SSM runs as SYSTEM in session 0, and the NSIS installer is installMode
#   currentUser, so a session-0 install lands in the SERVICE profile. The in-guest
#   companion therefore re-executes itself inside the interactive logon session
#   via a -LogonType Interactive scheduled task and refuses to fall back to
#   session 0. See scripts/qa/install-ps1-verify.ps1.
#
# USAGE
#   scripts/qa/install-ps1-verify.sh verify
#   scripts/qa/install-ps1-verify.sh collect <run-id>
#   scripts/qa/install-ps1-verify.sh help
# =============================================================================

set -euo pipefail

readonly PROGRAM='AgentLens install.ps1 Windows verification'

readonly AWS_ARGS=(--region us-east-2 --profile us)
readonly AWS_REGION_LITERAL='us-east-2'
readonly AWS_PROFILE_LITERAL='us'

# The scratch root is fixed because the workstation's / filled to 100% once and
# every temporary file in this run has to land off the root filesystem.
readonly SCRATCH='/config/workspace/.scratch/agentlens'
readonly STATE_FILE="${SCRATCH}/h7-instance.json"

# No account id or bucket name is written down: this repository is public. The
# bucket is derived from the caller's own account at run time, and the resolution
# is LAZY -- 'help' and 'collect' must not need STS. See resolve_bucket.
BUCKET="${AGENTLENS_QA_BUCKET:-}"
AWS_ACCOUNT_ID="${AGENTLENS_QA_ACCOUNT:-}"
# The real CodeBuild Windows artifact: a zip whose root holds
# AgentLens_0.1.0_x64-setup.exe.
INSTALLER_KEY="${AGENTLENS_QA_KEY:-artifacts/39f89617-585d-443c-a7fb-031a1d9f60ee/agentlens-windows}"
EXPECTED_SHA256="${AGENTLENS_QA_SHA256:-ad6accacfb9b69b9fd05545e89ecad8dd122d460191134d0749cb9e6220d360d}"
OUT_PREFIX="${AGENTLENS_INSTALLPS1_OUT_PREFIX:-qa/installps1}"

readonly CMD_ATTEMPTS=270
readonly CMD_SLEEP=10

INSTANCE_ID=''
RUN_ID=''
OUT_DIR=''
COMMAND_ID=''
COMMAND_STATUS=''
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
readonly REPO_ROOT
readonly INSTALL_PS1="${REPO_ROOT}/scripts/install.ps1"
readonly GUEST_PS1="${SCRIPT_DIR}/install-ps1-verify.ps1"

log() { printf '%s\n' "$*" >&2; }

die() {
	log "ERROR: $1"
	shift || true
	local line
	for line in "$@"; do log "       ${line}"; done
	exit 1
}

usage() {
	cat <<EOF
${PROGRAM}

Verbs:
  verify            Stage scripts/install.ps1 and the in-guest driver, run them
                    on the existing Windows QA instance in its INTERACTIVE
                    session, and download the artifacts.
  collect <run-id>  Re-download the artifacts of an earlier run.
  help              This text.

This script never creates, stops, reboots or terminates an instance. Instance
lifecycle belongs to scripts/qa/ec2-windows-gui-qa.sh.

Environment overrides:
  AGENTLENS_QA_INSTANCE_ID          instance to use (default: read from
                                    ${STATE_FILE})
  AGENTLENS_QA_BUCKET               S3 bucket. Default: derived at run time as
                                    agentlens-build-use2-<account>, where
                                    <account> comes from sts get-caller-identity.
  AGENTLENS_QA_ACCOUNT              skip the sts lookup and use this account id
  AGENTLENS_QA_KEY                  installer object key
  AGENTLENS_QA_SHA256               expected installer SHA-256
  AGENTLENS_INSTALLPS1_OUT_PREFIX   S3 prefix for artifacts (default: ${OUT_PREFIX})
EOF
}

require_tools() {
	local tool
	for tool in aws python3; do
		command -v "$tool" >/dev/null 2>&1 ||
			die "required tool not found: ${tool}"
	done
}

# Lazy and memoised, matching scripts/qa/ec2-windows-gui-qa.sh: the derivation
# mirrors the Makefile's S3_BUCKET_us-east-2, which is the authority on where
# CodeBuild writes its artifacts.
resolve_bucket() {
	if [[ -n $BUCKET ]]; then
		log "bucket (from AGENTLENS_QA_BUCKET): ${BUCKET}"
		return 0
	fi
	if [[ -z $AWS_ACCOUNT_ID ]]; then
		AWS_ACCOUNT_ID="$(aws sts get-caller-identity "${AWS_ARGS[@]}" \
			--query Account --output text 2>/dev/null || printf '')"
	fi
	if [[ -z $AWS_ACCOUNT_ID || $AWS_ACCOUNT_ID == 'None' ]]; then
		die 'could not determine the AWS account id' \
			"tried: aws sts get-caller-identity ${AWS_ARGS[*]} --query Account" \
			'The bucket name is derived from it, so there is no fallback. Supply either' \
			'explicitly:' \
			'  AGENTLENS_QA_ACCOUNT=<12 digits> ...' \
			'  AGENTLENS_QA_BUCKET=<bucket> ...'
	fi
	BUCKET="agentlens-build-use2-${AWS_ACCOUNT_ID}"
	log "bucket (derived for ${AWS_REGION_LITERAL} from account ${AWS_ACCOUNT_ID}): ${BUCKET}"
}

# Resolves the instance WITHOUT creating anything. An absent instance is a hard
# stop with the command that would create one, never an implicit provision.
resolve_instance() {
	if [[ -n ${AGENTLENS_QA_INSTANCE_ID:-} ]]; then
		INSTANCE_ID="$AGENTLENS_QA_INSTANCE_ID"
		log "instance from AGENTLENS_QA_INSTANCE_ID: ${INSTANCE_ID}"
		return 0
	fi
	if [[ -f $STATE_FILE ]]; then
		INSTANCE_ID="$(python3 -c '
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("instance_id", ""))
except Exception:
    print("")
' "$STATE_FILE" 2>/dev/null || printf '')"
		if [[ -n $INSTANCE_ID ]]; then
			log "instance from the id cache: ${INSTANCE_ID}"
			return 0
		fi
	fi
	return 1
}

require_instance() {
	resolve_instance ||
		die 'no Windows QA instance id available' \
			'This script operates on an existing instance and will not create one.' \
			'Provision one with the harness that owns the lifecycle:' \
			'  AGENTLENS_QA_INSTANCE_PROFILE=<name> scripts/qa/ec2-windows-gui-qa.sh provision' \
			'or point this run at an instance explicitly:' \
			'  AGENTLENS_QA_INSTANCE_ID=i-... scripts/qa/install-ps1-verify.sh verify'

	local state
	state="$(aws ec2 describe-instances "${AWS_ARGS[@]}" \
		--instance-ids "$INSTANCE_ID" \
		--query 'Reservations[0].Instances[0].State.Name' \
		--output text 2>/dev/null || printf 'unknown')"
	log "instance state: ${state}"
	[[ $state == 'running' ]] ||
		die "instance ${INSTANCE_ID} is '${state}', not running" \
			'Start it with the harness that owns the lifecycle:' \
			'  scripts/qa/ec2-windows-gui-qa.sh start'

	local ping
	ping="$(aws ssm describe-instance-information "${AWS_ARGS[@]}" \
		--filters "Key=InstanceIds,Values=${INSTANCE_ID}" \
		--query 'InstanceInformationList[0].PingStatus' \
		--output text 2>/dev/null || printf 'None')"
	log "SSM PingStatus: ${ping}"
	[[ $ping == 'Online' ]] ||
		die "the SSM agent on ${INSTANCE_ID} is '${ping}', not Online" \
			'Without SSM there is no way to run anything in the guest.'
}

# The interactive session is a PRECONDITION, not something this script creates.
# A session-0 install would land in the service profile and measure the wrong
# thing, so an absent interactive session is a hard stop with the fix.
require_interactive_session() {
	local doc="${SCRATCH}/installps1-session-probe-${RUN_ID}.json"
	python3 - "$doc" <<'PYEOF'
import json, sys

commands = [
    "$ErrorActionPreference='Continue'",
    "foreach ($p in (Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | "
    "Where-Object { $_.Name -eq 'explorer.exe' })) { "
    "$o = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction SilentlyContinue; "
    "$w = ''; if ($o -and $o.ReturnValue -eq 0) { $w = '' + $o.Domain + [char]92 + $o.User }; "
    "if ($p.SessionId -ne 0 -and $w) { Write-Output ('INTERACTIVE session=' + $p.SessionId + ' user=' + $w) } }",
]
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump({"commands": commands, "executionTimeout": ["300"]}, fh)
PYEOF
	local out
	out="$(ssm_run_document "$doc" 'install.ps1 verify: interactive session probe' 24 || printf '')"
	printf '%s\n' "$out" | sed 's/^/  /' >&2
	printf '%s\n' "$out" | grep -q '^INTERACTIVE session=[1-9]' ||
		die 'the guest has no interactive logon session' \
			'The NSIS installer is installMode currentUser, so a session-0 run installs' \
			'into the service profile and would measure something no user experiences.' \
			'Establish one first (autologon + reboot), then re-run:' \
			'  scripts/qa/ec2-windows-gui-qa.sh interactive'
}

# Runs one AWS-RunPowerShellScript document and echoes StandardOutputContent on
# stdout. The document is passed as file://... because the --parameters shorthand
# parser eats backslashes and these documents carry Windows paths.
ssm_run_document() {
	local doc="$1" comment="$2" attempts="${3:-40}"
	local cmd_id status attempt
	cmd_id="$(aws ssm send-command "${AWS_ARGS[@]}" \
		--document-name AWS-RunPowerShellScript \
		--instance-ids "$INSTANCE_ID" \
		--comment "$comment" \
		--parameters "file://${doc}" \
		--query 'Command.CommandId' --output text 2>/dev/null || printf '')"
	if [[ -z $cmd_id || $cmd_id == 'None' ]]; then
		log "WARNING: send-command failed for: ${comment}"
		return 1
	fi
	status='Pending'
	for ((attempt = 1; attempt <= attempts; attempt++)); do
		status="$(aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
			--query Status --output text 2>/dev/null || printf 'Pending')"
		case "$status" in
		Success | Failed | Cancelled | TimedOut) break ;;
		esac
		sleep 5
	done
	aws ssm get-command-invocation "${AWS_ARGS[@]}" \
		--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
		--query StandardOutputContent --output text 2>/dev/null | tr -d '\r' || true
	if [[ $status != 'Success' ]]; then
		log "WARNING: ${comment} ended as ${status} (command ${cmd_id})"
		aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
			--query StandardErrorContent --output text >&2 2>/dev/null || true
		return 1
	fi
	return 0
}

# Both files are staged into this run's own prefix, so the exact bytes that ran
# are parked next to the results they produced. install.ps1 goes up VERBATIM.
stage_scripts() {
	[[ -f $INSTALL_PS1 ]] || die "the artifact under test is missing: ${INSTALL_PS1}"
	[[ -f $GUEST_PS1 ]] || die "the in-guest driver is missing: ${GUEST_PS1}"
	local sha
	sha="$(sha256sum "$INSTALL_PS1" | cut -d' ' -f1)"
	log "install.ps1 under test: ${INSTALL_PS1}"
	log "  sha256 ${sha}"
	log "  bytes  $(wc -c <"$INSTALL_PS1")"
	printf '%s\n' "$sha" >"${OUT_DIR}/install.ps1.sha256.local"
	cp -- "$INSTALL_PS1" "${OUT_DIR}/install.ps1.asrun"

	log "staging s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/install.ps1"
	aws s3 cp "${AWS_ARGS[@]}" "$INSTALL_PS1" \
		"s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/install.ps1" >&2 ||
		die "could not stage ${INSTALL_PS1}" \
			"the caller needs s3:PutObject on ${OUT_PREFIX}/ in bucket ${BUCKET}"
	log "staging s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/install-ps1-verify.ps1"
	aws s3 cp "${AWS_ARGS[@]}" "$GUEST_PS1" \
		"s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/install-ps1-verify.ps1" >&2 ||
		die "could not stage ${GUEST_PS1}"
}

# The bootstrap fetches both staged scripts with the preinstalled AWSPowerShell
# module. The bare Windows_Server-2022-English-Full-Base AMI has NO aws CLI:
# neither C:\Program Files\Amazon\AWSCLIV2\aws.exe nor ...\AWSCLI\bin\aws.exe
# exists, so `aws s3 cp` in a bootstrap dies with CommandNotFoundException
# before anything else runs.
send_verify_command() {
	local params_file="${SCRATCH}/installps1-params-${RUN_ID}.json"
	ALV_BUCKET="$BUCKET" ALV_KEY="$INSTALLER_KEY" ALV_OUT_PREFIX="$OUT_PREFIX" \
		ALV_SHA256="$EXPECTED_SHA256" ALV_RUN_ID="$RUN_ID" \
		ALV_REGION="$AWS_REGION_LITERAL" ALV_PARAMS_FILE="$params_file" \
		python3 - <<'PYEOF'
import json, os

bucket = os.environ["ALV_BUCKET"]
prefix = os.environ["ALV_OUT_PREFIX"]
run_id = os.environ["ALV_RUN_ID"]
region = os.environ["ALV_REGION"]

work = r"C:\agentlens-installps1"
driver = work + r"\install-ps1-verify.ps1"
artifact = work + r"\install.ps1"


def pslit(value):
    return "'%s'" % value.replace("'", "''")


commands = [
    "$env:ALV_BUCKET=%s" % pslit(bucket),
    "$env:ALV_KEY=%s" % pslit(os.environ["ALV_KEY"]),
    "$env:ALV_OUT_PREFIX=%s" % pslit(prefix),
    "$env:ALV_SHA256=%s" % pslit(os.environ["ALV_SHA256"]),
    "$env:ALV_RUN_ID=%s" % pslit(run_id),
    "$env:ALV_REGION=%s" % pslit(region),
    "$ErrorActionPreference='Stop'",
    "New-Item -ItemType Directory -Force -Path %s | Out-Null" % pslit(work),
    "foreach ($m in @('AWSPowerShell','AWSPowerShell.NetCore','AWS.Tools.S3')) "
    "{ if (Get-Module -ListAvailable -Name $m) "
    "{ Import-Module $m -ErrorAction SilentlyContinue; break } }",
    "if (-not (Get-Command Read-S3Object -ErrorAction SilentlyContinue)) { "
    "Write-Error 'Read-S3Object is unavailable: AWSPowerShell did not import, and "
    "this AMI has no aws CLI, so the staged scripts cannot be fetched.'; exit 91 }",
    "Read-S3Object -BucketName %s -Key %s -File %s -Region %s | Out-Null"
    % (pslit(bucket), pslit("%s/%s/install-ps1-verify.ps1" % (prefix, run_id)),
       pslit(driver), pslit(region)),
    "Read-S3Object -BucketName %s -Key %s -File %s -Region %s | Out-Null"
    % (pslit(bucket), pslit("%s/%s/install.ps1" % (prefix, run_id)),
       pslit(artifact), pslit(region)),
    "foreach ($f in @(%s, %s)) { if (-not (Test-Path -LiteralPath $f) -or "
    "((Get-Item -LiteralPath $f).Length -eq 0)) { Write-Error ('staged script did not "
    "arrive: ' + $f); exit 90 } }" % (pslit(driver), pslit(artifact)),
    "Write-Host ('install.ps1 as staged in the guest: ' + "
    "(Get-FileHash -LiteralPath %s -Algorithm SHA256).Hash.ToLowerInvariant())" % pslit(artifact),
    "powershell -NoProfile -ExecutionPolicy Bypass -File %s" % pslit(driver),
    # Propagate the child's exit code so the SSM invocation status reflects the
    # verdict instead of always reporting Success.
    "if ($null -eq $LASTEXITCODE) { exit 0 } else { exit $LASTEXITCODE }",
]

doc = {"commands": commands, "executionTimeout": ["3600"]}
with open(os.environ["ALV_PARAMS_FILE"], "w", encoding="utf-8") as fh:
    json.dump(doc, fh)
PYEOF
	[[ -s $params_file ]] || die "failed to build the SSM parameters file: ${params_file}"

	log 'sending AWS-RunPowerShellScript'
	COMMAND_ID="$(aws ssm send-command "${AWS_ARGS[@]}" \
		--document-name AWS-RunPowerShellScript \
		--instance-ids "$INSTANCE_ID" \
		--comment "AgentLens install.ps1 verification ${RUN_ID}" \
		--parameters "file://${params_file}" \
		--query 'Command.CommandId' --output text)"
	[[ -n $COMMAND_ID && $COMMAND_ID != 'None' ]] ||
		die 'send-command returned no CommandId'
	log "command id: ${COMMAND_ID}"
}

wait_for_command() {
	local attempt status
	for ((attempt = 1; attempt <= CMD_ATTEMPTS; attempt++)); do
		status="$(aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$COMMAND_ID" --instance-id "$INSTANCE_ID" \
			--query Status --output text 2>/dev/null || printf 'Pending')"
		case "$status" in
		Success | Failed | Cancelled | TimedOut)
			COMMAND_STATUS="$status"
			log "command ${COMMAND_ID} finished: ${status} (~$((attempt * CMD_SLEEP))s)"
			break
			;;
		esac
		if ((attempt % 6 == 0)); then
			log "  command still ${status} (${attempt}/${CMD_ATTEMPTS})"
		fi
		sleep "$CMD_SLEEP"
	done
	if [[ -z $COMMAND_STATUS ]]; then
		COMMAND_STATUS='PollTimeout'
		log "WARNING: gave up polling after $((CMD_ATTEMPTS * CMD_SLEEP))s"
	fi

	log '--- inline StandardOutputContent (SSM truncates this at ~2500 chars) ---'
	aws ssm get-command-invocation "${AWS_ARGS[@]}" \
		--command-id "$COMMAND_ID" --instance-id "$INSTANCE_ID" \
		--query StandardOutputContent --output text 2>/dev/null | tr -d '\r' >&2 || true
	log '--- inline StandardErrorContent ---'
	aws ssm get-command-invocation "${AWS_ARGS[@]}" \
		--command-id "$COMMAND_ID" --instance-id "$INSTANCE_ID" \
		--query StandardErrorContent --output text 2>/dev/null | tr -d '\r' >&2 || true
	log '--- end inline output (the full logs are in the artifacts below) ---'
}

collect_artifacts() {
	local run="$1" dest="$2"
	mkdir -p -- "$dest"
	log "downloading s3://${BUCKET}/${OUT_PREFIX}/${run}/ -> ${dest}"
	aws s3 cp "${AWS_ARGS[@]}" --recursive \
		"s3://${BUCKET}/${OUT_PREFIX}/${run}/" "$dest" >&2 ||
		log 'WARNING: nothing could be downloaded from the run prefix'
	local f
	for f in "$dest"/*; do
		[[ -e $f ]] || continue
		log "  $(basename -- "$f") ($(wc -c <"$f") bytes)"
	done
}

report() {
	local results="${OUT_DIR}/results.json"
	if [[ ! -s $results ]]; then
		log ''
		log 'NO results.json WAS PRODUCED. Nothing about install.ps1 is proven by this'
		log 'run. The supervisor log and the inline SSM output above are the only'
		log 'evidence of how far it got.'
		return 1
	fi
	log ''
	log '=============================== SUMMARY ==============================='
	RESULTS="$results" python3 - <<'PYEOF' >&2
import json, os

doc = json.load(open(os.environ["RESULTS"], encoding="utf-8"))
print("run                : %s" % doc.get("run_id"))
print("artifact under test: %s" % doc.get("artifact_under_test"))
print("install.ps1 sha256 : %s" % doc.get("install_ps1_sha256"))
print("installer sha256   : %s" % doc.get("installer_sha256"))
diag = doc.get("diagnostics") or {}
print("session id         : %s (%s)" % (diag.get("session_id"), diag.get("whoami")))
print("verdict            : %s  (%s pass / %s fail)"
      % (doc.get("verdict"), doc.get("checks_pass"), doc.get("checks_fail")))
if doc.get("note"):
    print("note               : %s" % doc["note"])
print("")
print("scenarios:")
for s in doc.get("scenarios") or []:
    print("  %-32s exit=%-5s timedOut=%-5s elapsed=%s/%ss wizard=%s uncheckRun=%s appAliveAtExit=%s"
          % (s.get("name"), s.get("exit_code"), s.get("timed_out"),
             s.get("elapsed_s"), s.get("timeout_budget"), s.get("wizard_mode"),
             s.get("uncheck_run"), s.get("app_alive_at_exit")))
    for t in s.get("timeline") or []:
        print("        %s" % t)
print("")
print("checks:")
for c in doc.get("checks") or []:
    print("  [%s] %s / %s" % (c.get("verdict"), c.get("scenario"), c.get("name")))
    print("        expected: %s" % c.get("expected"))
    print("        observed: %s" % c.get("observed"))
fails = [c for c in (doc.get("checks") or []) if c.get("verdict") == "FAIL"]
print("")
if fails:
    print("FAILED CHECKS: %d" % len(fails))
    for c in fails:
        print("  %s / %s" % (c.get("scenario"), c.get("name")))
else:
    print("no failed checks")
PYEOF
	log '======================================================================='
	local verdict
	verdict="$(RESULTS="$results" python3 -c '
import json, os
print(json.load(open(os.environ["RESULTS"], encoding="utf-8")).get("verdict", "UNKNOWN"))
' 2>/dev/null || printf 'UNKNOWN')"
	[[ $verdict == 'PASS' ]]
}

cmd_verify() {
	require_tools
	RUN_ID="installps1-$(date -u +%Y%m%dT%H%M%SZ)"
	OUT_DIR="${SCRATCH}/${RUN_ID}"
	mkdir -p -- "$OUT_DIR"
	log "${PROGRAM}"
	log "run id: ${RUN_ID}"
	log "artifacts: ${OUT_DIR}"
	log "region ${AWS_REGION_LITERAL} profile ${AWS_PROFILE_LITERAL}"
	resolve_bucket
	require_instance
	require_interactive_session
	stage_scripts
	send_verify_command
	wait_for_command
	collect_artifacts "$RUN_ID" "$OUT_DIR"
	log "SSM invocation status: ${COMMAND_STATUS}"
	report
}

cmd_collect() {
	require_tools
	local run="${1:-}"
	[[ -n $run ]] || die 'collect needs a run id' 'scripts/qa/install-ps1-verify.sh collect installps1-...'
	RUN_ID="$run"
	OUT_DIR="${SCRATCH}/${RUN_ID}"
	resolve_bucket
	collect_artifacts "$RUN_ID" "$OUT_DIR"
	report
}

main() {
	mkdir -p -- "$SCRATCH"
	local verb="${1:-help}"
	shift || true
	case "$verb" in
	verify) cmd_verify "$@" ;;
	collect) cmd_collect "$@" ;;
	help | -h | --help) usage ;;
	*)
		usage
		die "unknown verb: ${verb}"
		;;
	esac
}

main "$@"
