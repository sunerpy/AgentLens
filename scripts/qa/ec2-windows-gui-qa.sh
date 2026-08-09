#!/usr/bin/env bash
# =============================================================================
# AgentLens EC2 Windows GUI QA driver (runs on a Linux workstation).
#
# Nobody has ever launched the Windows installer on a real interactive desktop.
# This script does that on an EC2 Windows Server 2022 instance and brings back
# machine-checkable evidence.
#
# LIFECYCLE MODEL: LONG-LIVED INSTANCE, EXPLICIT DESTROY.
# This script is a verb dispatcher, not a one-shot. The instance outlives every
# individual command and is destroyed only by an explicit, confirmed request:
#
#   provision   find-or-create the QA instance, wait for running + SSM Online
#   interactive give the guest an interactive logon session (autologon + reboot)
#   qa          run the GUI QA against the existing instance (re-runnable)
#   status      state, IP, SSM ping, install state, uptime, accumulated cost
#   stop        stop-instances  (stops compute billing, keeps the disk + state)
#   start       start-instances, then re-wait for SSM Online
#   destroy     terminate-instances -- REQUIRES `destroy --yes`
#   help        usage
#
# WHY THE AUTOMATIC TEARDOWN WAS REMOVED. An earlier revision of this file
# installed `trap on_exit EXIT INT TERM` before run-instances, so the instance
# died on every exit path including failure. That was correct for a one-shot
# smoke test and is wrong for this work, for two reasons the operator stated
# directly: the GUI QA takes several look-adjust-look rounds against the same
# desktop, and a FAILED round is precisely when the box must stay alive so it
# can be inspected. Self-destructing on failure destroyed the evidence needed to
# fix the failure. There is now NO trap that terminates anything, under any
# condition, including a "safety timeout". terminate-instances is reachable from
# exactly one place: the `destroy` verb, and only when it is passed --yes.
#
# WHAT THAT COSTS, AND WHO IS NOW HOLDING THE RISK. The trap was a cost guard,
# and deleting a guard means accepting what it guarded against: a t3.large
# Windows on-demand instance left running bills $0.1108/hr, about $2.66/day,
# indefinitely, plus its EBS volume even while stopped. Nothing in this script
# will ever reclaim it. So `provision`, `start` and `status` all print the rate,
# the per-day figure and the literal `destroy --yes` command; `status` also
# prints uptime and the accumulated estimate. The operator is expected to run
# `destroy --yes` once the project passes final acceptance.
#
# IDEMPOTENCE. `provision` resolves the instance by tag (Name=agentlens-gui-qa,
# states pending/running/stopping/stopped) and reuses whatever it finds, so
# running it twice yields ONE instance rather than a second $2.66/day box. If
# the tag ever matches more than one instance the script dies listing all of
# them instead of guessing -- silently picking one is how an orphan gets paid
# for forever. The id is also cached in a scratch file so a later session can
# find the box even if the tag lookup is unavailable.
#
# THE QA ITSELF, unchanged and re-runnable any number of times against the same
# instance (`qa` creates nothing and destroys nothing):
#   1. Grab a hypervisor console screenshot: the authoritative visual channel.
#   2. Stage the companion ec2-windows-gui-qa.ps1 into the run's S3 prefix.
#   3. Send a short bootstrap through AWS-RunPowerShellScript that fetches the
#      staged script and runs it.
#   4. Poll the invocation to a terminal status, printing BOTH stdout and stderr.
#   5. Second console screenshot, after the GUI QA ran.
#   6. Pull results.json / diagnostics.json / qa.log / screenshots from S3.
#   7. Print absolute paths plus the verdict parsed out of results.json.
#
# TWO AWS INVARIANTS, both learned the hard way:
#   * TMPDIR is redirected into the workspace scratch dir. The root filesystem
#     hit 100% earlier in this project and the AWS CLI writes temp files.
#   * The ambient shell exports AWS_REGION=cn-northwest-1, which is the WRONG
#     PARTITION for this account. Every single aws invocation therefore carries
#     "${AWS_ARGS[@]}" (--region us-east-2 --profile us). Region and profile
#     live in one readonly array precisely so no call site can forget them.
#
# TERMINATION IS ALWAYS BY CAPTURED INSTANCE ID, never by --filters, so a
# destroy can only ever reach the one instance that was resolved and printed
# first. Unrelated boxes in this region (comfyui-gpu, win-tmp) do not carry the
# Name tag above and are therefore unreachable from here.
# =============================================================================
set -euo pipefail

# The AWS CLI writes temp files; keep them off the root filesystem.
export TMPDIR=/config/workspace/.scratch/agentlens

readonly PROGRAM='AgentLens EC2 Windows GUI QA'
# Region AND profile in one place. The ambient AWS_REGION is the wrong partition.
readonly AWS_ARGS=(--region us-east-2 --profile us)
readonly AWS_REGION_LITERAL='us-east-2'
readonly AWS_PROFILE_LITERAL='us'
readonly SSM_WINDOWS_AMI_PARAM='/aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base'
# The single identity of the QA box: both the tag written by provision and the
# filter every other verb resolves through. They MUST stay the same string or
# provision stops being idempotent and destroy stops being able to find its
# target.
readonly INSTANCE_NAME_TAG='agentlens-gui-qa'
# Where the instance id survives between shell invocations. Deliberately in the
# workspace scratch dir, which is outside the repository tree, so it can never
# be committed.
readonly STATE_FILE='/config/workspace/.scratch/agentlens/h7-instance.json'
# t3.large Windows on-demand, us-east-2. Held as integer hundredths of a cent
# per hour so the accumulated estimate needs no bc, awk or python.
readonly COST_RATE_LITERAL='0.1108'
readonly COST_MICROCENTS_PER_HOUR=1108

# Mirrors app.windows[0].width in src-tauri/tauri.conf.json, and must stay in
# step with $script:ExpectedWidth in the companion .ps1. Used only to decide
# whether a console screenshot can contain the whole window frame.
readonly GUI_WINDOW_WIDTH=1180

# --- configuration (all overridable from the environment) --------------------
# This repository is public, so no AWS account id, bucket name or subnet id is
# written down here.
#
# The bucket is DERIVED from the caller's own account id at run time (see
# resolve_bucket), and the account id comes from sts get-caller-identity. Both
# resolutions are deliberately LAZY -- doing them at load time would make even
# 'help' require credentials, and would make 'status' / 'stop' / 'start' /
# 'destroy' fail on a box that can reach EC2 but not STS. Nothing here calls AWS
# until a verb that actually needs the value asks for it.
BUCKET="${AGENTLENS_QA_BUCKET:-}"
AWS_ACCOUNT_ID="${AGENTLENS_QA_ACCOUNT:-}"
INSTALLER_KEY="${AGENTLENS_QA_KEY:-artifacts/39f89617-585d-443c-a7fb-031a1d9f60ee/agentlens-windows}"
EXPECTED_SHA256="${AGENTLENS_QA_SHA256:-ad6accacfb9b69b9fd05545e89ecad8dd122d460191134d0749cb9e6220d360d}"
INSTANCE_TYPE="${AGENTLENS_QA_INSTANCE_TYPE:-t3.large}"
# NO DEFAULT, and deliberately not auto-picked from the default VPC. A subnet is
# a network: silently choosing a different one would move the instance onto
# different routing and security-group defaults, and the failure would surface
# much later as "SSM never came Online" with no hint that the network changed.
# Required only by instance creation -- see require_subnet.
SUBNET_ID="${AGENTLENS_QA_SUBNET:-}"
AMI_ID_OVERRIDE="${AGENTLENS_QA_AMI:-}"
INSTANCE_PROFILE="${AGENTLENS_QA_INSTANCE_PROFILE:-}"
RUN_ID="${AGENTLENS_QA_RUN_ID:-h7-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_PREFIX="${AGENTLENS_QA_OUT_PREFIX:-qa/h7-gui}"
OUT_DIR="${AGENTLENS_QA_OUT_DIR:-artifacts/h7-ec2-gui-qa/${RUN_ID}}"

# Bounded polling. Windows takes several minutes to register with SSM.
readonly SSM_ONLINE_ATTEMPTS=60
readonly SSM_ONLINE_SLEEP=10
readonly CMD_ATTEMPTS=180
readonly CMD_SLEEP=10
readonly TERMINATE_ATTEMPTS=30
readonly TERMINATE_SLEEP=5
# running/stopped transitions; a Windows guest shutdown is not instant.
readonly STATE_ATTEMPTS=60
readonly STATE_SLEEP=5
# Post-reboot autologon confirmation: the guest has to shut down, boot, let
# Winlogon perform the automatic logon, start explorer.exe, and re-register the
# SSM agent. 40 * 15s = 10 minutes, which is generous for a t3.large.
readonly INTERACTIVE_ATTEMPTS=40
readonly INTERACTIVE_SLEEP=15
# The disposable local account autologon signs in as. Local, not domain.
readonly INTERACTIVE_USER='agentlens-qa'

# --- mutable globals --------------------------------------------------------
AMI_ID=''
INSTANCE_ID=''
COMMAND_ID=''
COMMAND_STATUS=''
SCRIPT_DIR=''
PS1_PATH=''
STAGED_PS1_URI=''

log() { printf '%s\n' "$*" >&2; }
die() {
	printf 'error: %s\n' "$1" >&2
	shift || true
	for line in "$@"; do printf '       %s\n' "$line" >&2; done
	exit 1
}

usage() {
	cat <<EOF
AgentLens EC2 Windows GUI QA driver (run from Linux)

  scripts/qa/ec2-windows-gui-qa.sh <verb>

The instance is LONG-LIVED. Nothing here terminates it except 'destroy --yes'.

Verbs:
  provision   Find-or-create the QA instance (Name=${INSTANCE_NAME_TAG}), wait
              for running and for the SSM agent to report Online, cache the id.
              Idempotent: reuses an existing tagged instance, so running it
              twice does NOT give you two instances. Runs no QA.
              Needs AGENTLENS_QA_INSTANCE_PROFILE.
  interactive Give the guest an INTERACTIVE logon session (session id != 0) with
              Windows autologon, then REBOOT it and confirm from inside the guest
              that a session other than 0 owns a logged-on explorer.exe. Run this
              once before 'qa': SSM runs as SYSTEM in session 0, which cannot host
              a WebView2 window, so without it every window assertion comes back
              NOT EXECUTED. Creates a disposable local account whose password is
              generated in the guest and left CLEARTEXT in the Winlogon registry
              key, because that is the only form autologon accepts; see the notice
              the verb prints. Reboots but never terminates. Idempotent.
  qa          Run the GUI QA against the existing instance: stage the .ps1 to
              S3, send the bootstrap over SSM, take console screenshots, pull
              the artifacts back. Re-runnable any number of times. Creates
              nothing, destroys nothing. Fails if no instance exists.
              When an interactive session exists the in-guest script re-executes
              itself inside it, so the window is both hosted AND measured there.
  status      Instance id, state, public IP/DNS, SSM ping, whether AgentLens is
              installed, uptime and the accumulated cost estimate, plus the last
              local QA run. Works on a stopped instance. Run this first in a new
              session to find out where things stand.
  stop        stop-instances, then wait for 'stopped'. Ends compute billing;
              the EBS volume and everything installed on it survive.
  start       start-instances, wait for 'running', then re-wait for SSM Online
              (the agent needs time to re-register after a start).
  destroy     THE ONLY path that calls terminate-instances, and it requires
              an explicit --yes:
                destroy        print the target and the exact command, exit 2
                destroy --yes  terminate, wait for 'terminated', drop the cache
  help        This text.

Cost: ${INSTANCE_TYPE} Windows on-demand is \$${COST_RATE_LITERAL}/hr, about
\$2.66/day, and it keeps billing until you run 'destroy --yes'. There is no
timeout and no trap that will do it for you.

Environment:
  AGENTLENS_QA_INSTANCE_PROFILE  REQUIRED by 'provision', no default. IAM
                                 instance profile name. NOT required by status,
                                 stop, start or destroy, so an instance can
                                 always be inspected and killed without knowing
                                 which profile created it.
  AGENTLENS_QA_BUCKET            S3 bucket. Default: derived at run time as
                                 agentlens-build-use2-<account>, where <account>
                                 comes from sts get-caller-identity. Resolved
                                 only by 'provision' (when it creates) and 'qa'.
  AGENTLENS_QA_ACCOUNT           skip the sts lookup and use this account id
  AGENTLENS_QA_KEY               installer object key inside that bucket
  AGENTLENS_QA_SHA256            expected sha256 of the downloaded setup .exe
  AGENTLENS_QA_INSTANCE_TYPE     default: t3.large
  AGENTLENS_QA_SUBNET            REQUIRED to create an instance, no default. A
                                 subnet is not derivable and will not be guessed
                                 from the default VPC. Not needed by qa, status,
                                 stop, start or destroy.
  AGENTLENS_QA_AMI               skip SSM AMI resolution and use this AMI id
  AGENTLENS_QA_RUN_ID            default: h7-<utc timestamp>; scopes S3 + local out
  AGENTLENS_QA_OUT_PREFIX        S3 output prefix (default: qa/h7-gui)
  AGENTLENS_QA_OUT_DIR           local output dir
                                 (default: artifacts/h7-ec2-gui-qa/<run id>)

Instance id cache: ${STATE_FILE}

Region and profile are NOT configurable: they are pinned to ${AWS_REGION_LITERAL} /
${AWS_PROFILE_LITERAL} because the ambient AWS_REGION names a different partition.
EOF
}

# --- instance identity: cache file + tag lookup ------------------------------
state_write() {
	mkdir -p -- "$(dirname -- "$STATE_FILE")"
	printf '{"instance_id":"%s","region":"%s","profile":"%s","name_tag":"%s","written_utc":"%s"}\n' \
		"$INSTANCE_ID" "$AWS_REGION_LITERAL" "$AWS_PROFILE_LITERAL" \
		"$INSTANCE_NAME_TAG" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$STATE_FILE"
	log "instance id cached: ${STATE_FILE}"
}

state_read() {
	[[ -f $STATE_FILE ]] || return 1
	local id
	# Deliberately a sed extraction rather than a JSON parser: the file is
	# written by state_write above and by nothing else, and requiring python3
	# just to read back one instance id would make 'destroy' fragile.
	id="$(sed -n 's/.*"instance_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
		"$STATE_FILE" 2>/dev/null | head -n 1)"
	[[ -n $id ]] || return 1
	printf '%s' "$id"
}

state_clear() {
	if [[ -f $STATE_FILE ]]; then
		rm -f -- "$STATE_FILE"
		log "dropped the instance id cache: ${STATE_FILE}"
	fi
}

# Echoes the tagged instance id, or nothing. --output text yields a TAB-separated
# list when the tag matches several instances, and 'None' instead of an empty
# string on some CLI versions; both are handled by the caller.
find_instance() {
	aws ec2 describe-instances "${AWS_ARGS[@]}" \
		--filters "Name=tag:Name,Values=${INSTANCE_NAME_TAG}" \
		"Name=instance-state-name,Values=pending,running,stopping,stopped" \
		--query 'Reservations[].Instances[].InstanceId' \
		--output text 2>/dev/null || true
}

instance_state() {
	aws ec2 describe-instances "${AWS_ARGS[@]}" \
		--instance-ids "$1" \
		--query 'Reservations[0].Instances[0].State.Name' \
		--output text 2>/dev/null || printf 'unknown'
}

# Sets INSTANCE_ID from the tag, falling back to the cache file. Returns 1 when
# no instance can be found anywhere; dies outright on an ambiguous tag, because
# guessing which of several instances to act on is how one gets left running and
# billed forever.
resolve_instance() {
	local found
	found="$(find_instance)"
	if [[ $found == 'None' ]]; then
		found=''
	fi
	local ids=()
	read -r -a ids <<<"$found" || true
	case "${#ids[@]}" in
	1)
		INSTANCE_ID="${ids[0]}"
		return 0
		;;
	0) ;;
	*)
		die "tag Name=${INSTANCE_NAME_TAG} matches ${#ids[@]} instances: ${ids[*]}" \
			'Refusing to guess which one to act on. Inspect them and terminate the' \
			'extras by hand, so that exactly one is left:' \
			"  aws ec2 describe-instances --instance-ids ${ids[*]} --region ${AWS_REGION_LITERAL} --profile ${AWS_PROFILE_LITERAL}"
		;;
	esac

	local cached
	if cached="$(state_read)"; then
		local state
		state="$(instance_state "$cached")"
		if [[ $state == 'terminated' || $state == 'unknown' ]]; then
			log "cached id ${cached} is ${state}; ignoring the cache"
			return 1
		fi
		INSTANCE_ID="$cached"
		log "resolved from the id cache (tag lookup found nothing): ${INSTANCE_ID}"
		return 0
	fi
	return 1
}

require_instance() {
	if resolve_instance; then
		return 0
	fi
	die "no QA instance exists (no instance tagged Name=${INSTANCE_NAME_TAG}, no usable id cache)" \
		'This verb operates on an existing instance and will not create one.' \
		'Create it first:' \
		'  AGENTLENS_QA_INSTANCE_PROFILE=<name> scripts/qa/ec2-windows-gui-qa.sh provision'
}

wait_for_state() {
	local want="$1" attempt state
	log "waiting for ${INSTANCE_ID} to reach ${want}"
	for ((attempt = 1; attempt <= STATE_ATTEMPTS; attempt++)); do
		state="$(instance_state "$INSTANCE_ID")"
		if [[ $state == "$want" ]]; then
			log "instance ${INSTANCE_ID} is ${state}"
			return 0
		fi
		if ((attempt % 6 == 0)); then
			log "  still ${state} (${attempt}/${STATE_ATTEMPTS})"
		fi
		sleep "$STATE_SLEEP"
	done
	log "WARNING: ${INSTANCE_ID} did not reach ${want} within" \
		"$((STATE_ATTEMPTS * STATE_SLEEP))s (last state: ${state})"
	return 1
}

# --- cost -------------------------------------------------------------------
# Integer arithmetic only, so no verb depends on bc/awk/python being present:
# COST_MICROCENTS_PER_HOUR is cents*100 per hour, so cents = s * rate / 360000.
format_cost_for_seconds() {
	local cents=$((${1:-0} * COST_MICROCENTS_PER_HOUR / 360000))
	printf '$%d.%02d' "$((cents / 100))" "$((cents % 100))"
}

print_cost_notice() {
	cat >&2 <<EOF

COST: this instance is now billing and NOTHING will stop it automatically.
  rate         \$${COST_RATE_LITERAL}/hr (${INSTANCE_TYPE} Windows on-demand, ${AWS_REGION_LITERAL})
  per day      ~\$2.66/day, indefinitely
  stop paying  scripts/qa/ec2-windows-gui-qa.sh destroy --yes
  pause paying scripts/qa/ec2-windows-gui-qa.sh stop   (keeps the disk, and its
               EBS charge, but ends the hourly compute charge)
EOF
}

# --- teardown: reachable ONLY from the destroy verb --------------------------
manual_terminate_hint() {
	printf 'MANUAL CLEANUP REQUIRED -- run this yourself:\n' >&2
	printf '  aws ec2 terminate-instances --instance-ids %s --region %s --profile %s\n' \
		"$INSTANCE_ID" "$AWS_REGION_LITERAL" "$AWS_PROFILE_LITERAL" >&2
}

terminate_instance() {
	log "terminating instance ${INSTANCE_ID} (by id only, never by --filters)"
	if ! aws ec2 terminate-instances "${AWS_ARGS[@]}" \
		--instance-ids "$INSTANCE_ID" \
		--query 'TerminatingInstances[0].CurrentState.Name' \
		--output text >/dev/null 2>&1; then
		log "WARNING: terminate-instances call FAILED for ${INSTANCE_ID}"
		manual_terminate_hint
		return 1
	fi

	local attempt state
	for ((attempt = 1; attempt <= TERMINATE_ATTEMPTS; attempt++)); do
		state="$(aws ec2 describe-instances "${AWS_ARGS[@]}" \
			--instance-ids "$INSTANCE_ID" \
			--query 'Reservations[0].Instances[0].State.Name' \
			--output text 2>/dev/null || printf 'unknown')"
		case "$state" in
		shutting-down | terminated)
			log "instance ${INSTANCE_ID} is ${state} -- teardown confirmed"
			return 0
			;;
		esac
		sleep "$TERMINATE_SLEEP"
	done

	log "WARNING: ${INSTANCE_ID} did not reach shutting-down/terminated after" \
		"$((TERMINATE_ATTEMPTS * TERMINATE_SLEEP)) seconds (last state: ${state})"
	manual_terminate_hint
	return 1
}

# There is deliberately NO trap in this file. The predecessor's
# `trap on_exit EXIT INT TERM` -> terminate_instance chain has been deleted
# outright rather than narrowed, because any surviving exit-path teardown would
# destroy the instance on exactly the failed run that needs inspecting. The only
# caller of terminate_instance is cmd_destroy, and only under --yes.

# --- preflight ---------------------------------------------------------------
require_tools() {
	command -v aws >/dev/null 2>&1 ||
		die 'the aws CLI is not on PATH' \
			'every step of this script goes through the AWS API'
	command -v base64 >/dev/null 2>&1 ||
		die 'base64 is not on PATH' \
			'console screenshots come back as base64-encoded JPEG'
}

require_instance_profile() {
	if [[ -n $INSTANCE_PROFILE ]]; then
		return 0
	fi
	die 'AGENTLENS_QA_INSTANCE_PROFILE is not set, and there is deliberately no default' \
		'The instance needs BOTH AmazonSSMManagedInstanceCore (so the SSM agent can' \
		'register, which is the only control channel here) AND s3:PutObject on the' \
		'output prefix (so the QA results can be uploaded).' \
		'' \
		'Two failure modes to watch for when you pick one:' \
		'  TOO NARROW    a profile with AmazonSSMManagedInstanceCore but NO' \
		'                s3:PutObject lets the whole run proceed and then fails at' \
		'                the very end, when it tries to upload the results.' \
		'  TOO BROAD     an existing admin-grade profile will work, and is grossly' \
		'                over-privileged for a throwaway QA box that only needs to' \
		'                talk to SSM and write one S3 prefix. Do not reach for one' \
		'                because it is convenient.' \
		'  RECOMMENDED   a purpose-scoped role: AmazonSSMManagedInstanceCore plus' \
		'                s3:PutObject limited to the one output prefix. Creating it' \
		'                is a separate, explicitly approved step.' \
		'' \
		'This script will not silently escalate privileges on your behalf. Choose:' \
		'  AGENTLENS_QA_INSTANCE_PROFILE=<name> scripts/qa/ec2-windows-gui-qa.sh provision' \
		'' \
		'Only provision needs this. status, stop, start and destroy deliberately do' \
		'not, so an existing instance can always be inspected and killed without' \
		'knowing which profile created it.'
}

# Lazy, memoised: the first caller pays one sts call, later callers pay nothing.
# Called only from resolve_bucket, i.e. only by verbs that need the bucket.
resolve_account_id() {
	if [[ -n $AWS_ACCOUNT_ID ]]; then
		return 0
	fi
	AWS_ACCOUNT_ID="$(aws sts get-caller-identity "${AWS_ARGS[@]}" \
		--query Account --output text 2>/dev/null || printf '')"
	if [[ -z $AWS_ACCOUNT_ID || $AWS_ACCOUNT_ID == 'None' ]]; then
		die 'could not determine the AWS account id' \
			"tried: aws sts get-caller-identity ${AWS_ARGS[*]} --query Account" \
			'The S3 bucket name is derived from it, so there is nothing to fall back' \
			'to. Either make the credentials work, or supply both explicitly:' \
			'  AGENTLENS_QA_ACCOUNT=<12 digits> ...' \
			'  AGENTLENS_QA_BUCKET=<bucket> ...'
	fi
	log "account id (from sts get-caller-identity): ${AWS_ACCOUNT_ID}"
}

# The mapping mirrors the Makefile's S3_BUCKET_us-east-2, which is the authority
# on where CodeBuild puts its artifacts. This script is pinned to us-east-2, so
# there is only one form to derive.
resolve_bucket() {
	if [[ -n $BUCKET ]]; then
		log "bucket (from AGENTLENS_QA_BUCKET): ${BUCKET}"
		return 0
	fi
	resolve_account_id
	BUCKET="agentlens-build-use2-${AWS_ACCOUNT_ID}"
	log "bucket (derived for ${AWS_REGION_LITERAL}): ${BUCKET}"
}

# Enforced at the point of CREATION, not at load time, so every verb that only
# touches an existing instance keeps working with zero configuration.
require_subnet() {
	if [[ -n $SUBNET_ID ]]; then
		return 0
	fi
	die 'AGENTLENS_QA_SUBNET is not set, and there is deliberately no default' \
		'A subnet id cannot be derived, and this script will not pick one out of the' \
		'default VPC for you: a different subnet is a different network, so a quietly' \
		'substituted one would change routing and security-group defaults and surface' \
		'much later as an instance that never registers with SSM.' \
		'' \
		'Pick a subnet with a route to the internet (the guest must reach SSM, S3 and' \
		"the WebView2 bootstrapper) in ${AWS_REGION_LITERAL}:" \
		"  aws ec2 describe-subnets --region ${AWS_REGION_LITERAL} --profile ${AWS_PROFILE_LITERAL} \\" \
		"    --query 'Subnets[].{Id:SubnetId,Vpc:VpcId,Az:AvailabilityZone,Public:MapPublicIpOnLaunch}' \\" \
		'    --output table' \
		'' \
		'Then:' \
		'  AGENTLENS_QA_SUBNET=subnet-... AGENTLENS_QA_INSTANCE_PROFILE=<name> \' \
		'    scripts/qa/ec2-windows-gui-qa.sh provision' \
		'' \
		'Only instance creation needs this. qa, status, stop, start and destroy do' \
		'not, so the existing instance is unaffected.'
}

locate_ps1() {
	SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
	PS1_PATH="${SCRIPT_DIR}/ec2-windows-gui-qa.ps1"
	[[ -f $PS1_PATH ]] ||
		die "companion PowerShell script not found: ${PS1_PATH}" \
			'It must sit next to this driver; its contents are sent verbatim to' \
			'AWS-RunPowerShellScript.'
}

resolve_ami() {
	if [[ -n $AMI_ID_OVERRIDE ]]; then
		AMI_ID="$AMI_ID_OVERRIDE"
		log "AMI (override): ${AMI_ID}"
		return 0
	fi
	AMI_ID="$(aws ssm get-parameter "${AWS_ARGS[@]}" \
		--name "$SSM_WINDOWS_AMI_PARAM" \
		--query Parameter.Value --output text 2>/dev/null || true)"
	if [[ -z $AMI_ID || $AMI_ID == 'None' ]]; then
		die "could not resolve the Windows AMI from ${SSM_WINDOWS_AMI_PARAM}" \
			'That is a public SSM parameter; a failure here usually means the' \
			'credentials or the region are wrong.' \
			'Override it explicitly with AGENTLENS_QA_AMI=ami-...'
	fi
	# Was ami-0a309571b4f421554 when last checked; it moves with each Windows
	# patch release, which is exactly why it is resolved at run time.
	log "AMI (resolved from SSM public parameter): ${AMI_ID}"
}

print_plan() {
	cat >&2 <<EOF
plan:
  region          ${AWS_REGION_LITERAL} (profile ${AWS_PROFILE_LITERAL})
  ami             ${AMI_ID}
  instance type   ${INSTANCE_TYPE}
  subnet          ${SUBNET_ID}
  instance profile ${INSTANCE_PROFILE}
  bucket          ${BUCKET}
  installer key   ${INSTALLER_KEY}
  expected sha256 ${EXPECTED_SHA256}
  run id          ${RUN_ID}
  s3 output       s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/
  local output    ${OUT_DIR}
  name tag        ${INSTANCE_NAME_TAG} (find-or-create key; reused if it exists)
  cost            \$${COST_RATE_LITERAL}/hr, ~\$2.66/day, until 'destroy --yes'
  teardown        NONE automatic. Explicit only: 'destroy --yes'
EOF
}

# --- launch ------------------------------------------------------------------
# shutdown-behavior is 'stop', NOT 'terminate'. The predecessor used 'terminate'
# as a second safety net behind the (now deleted) teardown trap. Under the
# long-lived model that setting is a liability instead: a Windows Update reboot,
# or anything else that shuts the guest down from the inside, would silently
# destroy the instance the operator asked to keep until final acceptance.
launch_instance() {
	log "launching ${INSTANCE_TYPE} from ${AMI_ID}"
	local id
	id="$(aws ec2 run-instances "${AWS_ARGS[@]}" \
		--image-id "$AMI_ID" \
		--instance-type "$INSTANCE_TYPE" \
		--subnet-id "$SUBNET_ID" \
		--iam-instance-profile "Name=${INSTANCE_PROFILE}" \
		--associate-public-ip-address \
		--instance-initiated-shutdown-behavior stop \
		--tag-specifications \
		"ResourceType=instance,Tags=[{Key=Name,Value=${INSTANCE_NAME_TAG}},{Key=RunId,Value=${RUN_ID}}]" \
		--query 'Instances[0].InstanceId' --output text)"

	INSTANCE_ID="$id"

	if [[ -z $INSTANCE_ID || $INSTANCE_ID == 'None' ]]; then
		die 'run-instances returned no InstanceId' \
			'If an instance WAS created despite this, find it by the Name tag' \
			"Name=${INSTANCE_NAME_TAG} (RunId=${RUN_ID}) and decide by hand:" \
			'  scripts/qa/ec2-windows-gui-qa.sh status'
	fi
	log "instance: ${INSTANCE_ID} (RunId=${RUN_ID})"
	state_write
}

# --- wait for the SSM agent --------------------------------------------------
ssm_ping() {
	aws ssm describe-instance-information "${AWS_ARGS[@]}" \
		--filters "Key=InstanceIds,Values=${1}" \
		--query 'InstanceInformationList[0].PingStatus' \
		--output text 2>/dev/null || printf 'None'
}

# Returns 1 on timeout instead of dying, so the caller can still print the cost
# notice and the destroy command before it exits: an instance that booted but
# never registered is billing exactly like one that did.
wait_for_ssm() {
	log "waiting for the SSM agent on ${INSTANCE_ID} (Windows boot takes minutes)"
	local attempt ping
	for ((attempt = 1; attempt <= SSM_ONLINE_ATTEMPTS; attempt++)); do
		ping="$(ssm_ping "$INSTANCE_ID")"
		if [[ $ping == 'Online' ]]; then
			log "SSM agent Online after ~$((attempt * SSM_ONLINE_SLEEP))s"
			return 0
		fi
		if ((attempt % 6 == 0)); then
			log "  still waiting (${attempt}/${SSM_ONLINE_ATTEMPTS}, PingStatus=${ping})"
		fi
		sleep "$SSM_ONLINE_SLEEP"
	done
	log "WARNING: the SSM agent never reported Online within $((SSM_ONLINE_ATTEMPTS * SSM_ONLINE_SLEEP))s"
	return 1
}

# Runs one AWS-RunPowerShellScript document, waits for a terminal status, and
# echoes StandardOutputContent on STDOUT (every log line in this file goes to
# stderr, so a caller can capture the guest's output cleanly with $(...)).
# Returns non-zero on any status other than Success, after dumping stderr.
#
# The document is passed as file://... rather than through the --parameters
# shorthand for the same reason report_install_state does it: the shorthand parser
# eats backslashes, and every one of these documents carries Windows registry
# paths. Inside the JSON, \\ decodes to one backslash, which PowerShell then
# treats literally inside a single-quoted string, so no PowerShell line in these
# documents may contain a double quote.
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
		log '--- StandardErrorContent ---'
		aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
			--query StandardErrorContent --output text >&2 2>/dev/null || true
		log '--- end StandardErrorContent ---'
		return 1
	fi
	return 0
}

ssm_offline_hint() {
	log 'The usual cause is a missing or wrong IAM instance profile: without the'
	log 'AmazonSSMManagedInstanceCore permissions the agent cannot register with'
	log 'Systems Manager at all, so the instance boots fine and stays invisible.'
	log "profile requested: ${INSTANCE_PROFILE:-<not set for this verb>}"
	log 'A console screenshot can confirm the OS actually booted:'
	log "  aws ec2 get-console-screenshot --instance-id ${INSTANCE_ID} --region ${AWS_REGION_LITERAL} --profile ${AWS_PROFILE_LITERAL}"
	log 'The instance has NOT been terminated. It is still there to be inspected.'
}

# --- console screenshots -----------------------------------------------------
# The authoritative visual channel for this QA run: get-console-screenshot reads
# the hypervisor framebuffer, so it needs no agent inside the guest and no RDP or
# inbound security-group rule. It is free, returns a JPEG capped at 100 KB, and
# works on t3 -- but NOT on *.metal, NVIDIA-GRID instances, Graviton, Outposts or
# Local Zones.
take_screenshot() {
	local label="$1" dest="${OUT_DIR}/$2"
	local b64
	b64="$(aws ec2 get-console-screenshot "${AWS_ARGS[@]}" \
		--instance-id "$INSTANCE_ID" \
		--query ImageData --output text 2>/dev/null || true)"
	if [[ -z $b64 || $b64 == 'None' ]]; then
		log "WARNING: no console screenshot available (${label})"
		return 0
	fi
	if printf '%s' "$b64" | base64 -d >"$dest" 2>/dev/null; then
		log "console screenshot (${label}): ${dest}"
		note_screenshot_crop "$dest"
	else
		log "WARNING: could not decode the console screenshot (${label})"
		rm -f "$dest"
	fi
}

# The window is 1180x780. A console framebuffer narrower than that physically
# cannot contain the whole window, so the three window buttons at its top-right
# are outside the captured frame. The caveat is emitted from the JPEG's OWN
# dimensions rather than from an assumption about the guest resolution, and it is
# stated every time so nobody can look at a cropped capture and believe the
# buttons were checked. Raising the resolution from inside the guest was measured
# to be impossible: SSM runs in session 0, where EnumDisplaySettingsW fails and
# ChangeDisplaySettingsEx returns DISP_CHANGE_FAILED.
note_screenshot_crop() {
	local img="$1" dims w
	command -v python3 >/dev/null 2>&1 || return 0
	dims="$(python3 -c '
import struct, sys

# Minimal JPEG SOF scanner: walk the marker segments and read the frame header.
try:
    d = open(sys.argv[1], "rb").read()
    i = 2
    while i < len(d) - 9:
        if d[i] != 0xFF:
            i += 1
            continue
        m = d[i + 1]
        if m in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            h, w = struct.unpack(">HH", d[i + 5:i + 9])
            print("%d %d" % (w, h))
            break
        if m in (0xD8, 0x01) or 0xD0 <= m <= 0xD7:
            i += 2
            continue
        i += 2 + struct.unpack(">H", d[i + 2:i + 4])[0]
except Exception:
    pass
' "$img" 2>/dev/null || true)"
	[[ -n $dims ]] || return 0
	w="${dims%% *}"
	log "  capture is ${dims// /x} px"
	if ((w < GUI_WINDOW_WIDTH)); then
		log "  CAVEAT: ${w}px wide < ${GUI_WINDOW_WIDTH}px window. The window does NOT fit."
		log "  Window-button visibility is NOT VERIFIABLE from this capture: the"
		log "  top-right buttons fall outside the frame. Do NOT read this image as"
		log "  confirming them. Style bits are asserted separately in results.json"
		log "  (window.style.*) and are unaffected by framebuffer size."
	else
		log "  capture is wide enough to contain the whole ${GUI_WINDOW_WIDTH}px window frame"
	fi
}

# --- push and run the PowerShell QA -----------------------------------------
# The companion .ps1 is ~50 KB / ~1000 lines. Inlining it into the SSM commands
# array does not work: once JSON-escaped it breaches the size ceiling SSM applies
# to an inline command document, so send-command rejects the request outright and
# the whole run dies here for a purely mechanical reason. So the script is staged
# into this run's S3 prefix and the command document carries only a short
# bootstrap that fetches it with the preinstalled AWSPowerShell module and runs
# it. NOTE: the bootstrap deliberately does NOT use the aws CLI -- the bare
# Windows_Server-2022-English-Full-Base AMI does not have one.
#
# Staging is also better provenance: the exact script that ran ends up parked in
# the same S3 prefix as the results it produced, instead of existing only inside
# the body of one API call that nobody can retrieve afterwards.
#
# --parameters is still built as a JSON file by python3 rather than by hand: jq
# may not be present, and the bootstrap contains quotes and backslashed Windows
# paths that would not survive shell-level string building.
stage_ps1() {
	STAGED_PS1_URI="s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/ec2-windows-gui-qa.ps1"
	log "staging the in-guest script: ${STAGED_PS1_URI}"
	if ! aws s3 cp "${AWS_ARGS[@]}" "$PS1_PATH" "$STAGED_PS1_URI" >&2; then
		die "could not upload ${PS1_PATH} to ${STAGED_PS1_URI}" \
			"this is THIS workstation's own permission, not the instance profile's:" \
			"the caller's credentials need s3:PutObject on ${OUT_PREFIX}/ in bucket" \
			"${BUCKET}." \
			"The instance profile needs the matching s3:GetObject to fetch it back," \
			"plus s3:PutObject for the results -- those are separate grants."
	fi
	log 'staged ok'
}

send_qa_command() {
	local params_file="${TMPDIR}/ssm-params-${RUN_ID}.json"
	log 'building the SSM parameters document (bootstrap only, not the full script)'
	STAGED_PS1_URI="$STAGED_PS1_URI" \
		QA_BUCKET="$BUCKET" QA_KEY="$INSTALLER_KEY" \
		QA_OUT_PREFIX="$OUT_PREFIX" QA_SHA256="$EXPECTED_SHA256" \
		QA_RUN_ID="$RUN_ID" QA_PARAMS_FILE="$params_file" \
		QA_AWS_REGION="$AWS_REGION_LITERAL" \
		python3 - <<'PYEOF'
import json, os

env = [
    ("AGENTLENS_QA_BUCKET", os.environ["QA_BUCKET"]),
    ("AGENTLENS_QA_KEY", os.environ["QA_KEY"]),
    ("AGENTLENS_QA_OUT_PREFIX", os.environ["QA_OUT_PREFIX"]),
    ("AGENTLENS_QA_SHA256", os.environ["QA_SHA256"]),
    ("AGENTLENS_QA_RUN_ID", os.environ["QA_RUN_ID"]),
    ("AGENTLENS_QA_REGION", os.environ["QA_AWS_REGION"]),
]

staged = os.environ["STAGED_PS1_URI"]
region = os.environ["QA_AWS_REGION"]
local = r"C:\agentlens-qa\ec2-windows-gui-qa.ps1"

# The bootstrap must fetch the staged script with the AWSPowerShell module, NOT
# the aws CLI. Windows_Server-2022-English-Full-Base has NO aws CLI: neither
# C:\Program Files\Amazon\AWSCLIV2\aws.exe nor ...\AWSCLI\bin\aws.exe exists and
# Get-Command aws resolves to nothing, so `aws s3 cp` died here with
# CommandNotFoundException before a single line of the QA script ever ran. The
# CodeBuild Windows image does ship the CLI, which is what made this look safe;
# a build image and a bare AMI are different toolchains on the same OS version.
# AWSPowerShell 5.0.246 IS preinstalled and Read-S3Object/Write-S3Object both
# resolve under a fresh SYSTEM session, so that is the transport used throughout.
#
# Read-S3Object needs the bucket and key as separate parameters, so the s3:// URI
# is split here rather than in PowerShell.
if not staged.startswith("s3://"):
    raise SystemExit("staged URI is not an s3:// URI: %s" % staged)
_staged_bucket, _, _staged_key = staged[len("s3://"):].partition("/")
if not _staged_bucket or not _staged_key:
    raise SystemExit("could not split bucket/key out of %s" % staged)


def pslit(value):
    # Single-quoted PowerShell literal; ' is escaped by doubling it.
    return "'%s'" % value.replace("'", "''")


commands = []
for name, value in env:
    commands.append("$env:%s=%s" % (name, pslit(value)))
commands.append("$ErrorActionPreference='Stop'")
commands.append(r"New-Item -ItemType Directory -Force -Path C:\agentlens-qa | Out-Null")

# Resolve the S3 cmdlets explicitly instead of trusting module autoload. Autoload
# is a PSModulePath lookup and it can be off for SYSTEM; a bare Read-S3Object
# call would then fail with the same CommandNotFoundException class of error this
# change exists to remove. Both module layouts are tried (monolithic
# AWSPowerShell, which is what is installed, and the modular AWS.Tools.S3) with
# errors suppressed, then Get-Command is the single hard gate.
commands.append(
    "foreach ($m in @('AWSPowerShell','AWSPowerShell.NetCore','AWS.Tools.S3')) "
    "{ if (Get-Module -ListAvailable -Name $m) "
    "{ Import-Module $m -ErrorAction SilentlyContinue; break } }"
)
commands.append(
    "if (-not (Get-Command Read-S3Object -ErrorAction SilentlyContinue)) { "
    "Write-Error 'Read-S3Object is unavailable: the AWSPowerShell module is not "
    "installed or failed to import. This AMI has no aws CLI either, so there is "
    "no way to fetch the staged QA script.'; exit 91 }"
)
commands.append(
    "Write-Host ('AWSPowerShell: ' + "
    "((Get-Command Read-S3Object).Module.Name) + ' ' + "
    "((Get-Command Read-S3Object).Module.Version))"
)
commands.append(
    "Read-S3Object -BucketName %s -Key %s -File %s -Region %s | Out-Null"
    % (pslit(_staged_bucket), pslit(_staged_key), pslit(local), pslit(region))
)
commands.append(
    "if (-not (Test-Path %s) -or ((Get-Item %s).Length -eq 0)) { "
    "Write-Error ('staged QA script did not arrive: ' + %s); exit 90 }"
    % (pslit(local), pslit(local), pslit(local))
)
commands.append(
    "powershell -NoProfile -ExecutionPolicy Bypass -File %s" % pslit(local)
)
# Propagate the child exit code so the SSM invocation status reflects the QA
# verdict instead of always reporting Success.
commands.append("if ($null -eq $LASTEXITCODE) { exit 0 } else { exit $LASTEXITCODE }")

doc = {"commands": commands, "executionTimeout": ["3600"]}
with open(os.environ["QA_PARAMS_FILE"], "w", encoding="utf-8") as fh:
    json.dump(doc, fh)
PYEOF

	[[ -s $params_file ]] ||
		die "failed to build the SSM parameters file: ${params_file}"

	log 'sending AWS-RunPowerShellScript'
	COMMAND_ID="$(aws ssm send-command "${AWS_ARGS[@]}" \
		--document-name AWS-RunPowerShellScript \
		--instance-ids "$INSTANCE_ID" \
		--comment "AgentLens GUI QA ${RUN_ID}" \
		--parameters "file://${params_file}" \
		--query 'Command.CommandId' --output text)"
	[[ -n $COMMAND_ID && $COMMAND_ID != 'None' ]] ||
		die 'send-command returned no CommandId'
	log "command id: ${COMMAND_ID}"
}

# Inline invocation output is capped at roughly 2500 characters by SSM, which is
# exactly why the real artifacts travel through S3. Both streams are printed on
# any non-Success status rather than swallowed.
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

	if [[ $COMMAND_STATUS != 'Success' ]]; then
		log '--- StandardOutputContent (SSM truncates this at ~2500 chars) ---'
		aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$COMMAND_ID" --instance-id "$INSTANCE_ID" \
			--query StandardOutputContent --output text >&2 2>/dev/null || true
		log '--- StandardErrorContent ---'
		aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$COMMAND_ID" --instance-id "$INSTANCE_ID" \
			--query StandardErrorContent --output text >&2 2>/dev/null || true
		log '--- end inline output (full logs are in the S3 artifacts below) ---'
	fi
}

# --- collect -----------------------------------------------------------------
SYNCED_ANY=0
collect_artifacts() {
	local src="s3://${BUCKET}/${OUT_PREFIX}/${RUN_ID}/"
	log "syncing ${src} -> ${OUT_DIR}/"
	aws s3 sync "${AWS_ARGS[@]}" "$src" "${OUT_DIR}/" >&2 || true

	local count
	count="$(find "$OUT_DIR" -maxdepth 1 -type f \
		\( -name 'results.json' -o -name 'diagnostics.json' -o -name 'qa.log' -o -name '*.png' \) |
		wc -l | tr -d ' ')"
	if [[ $count == '0' ]]; then
		SYNCED_ANY=0
		log "NOTHING WAS DOWNLOADED from ${src}"
		log 'This is NOT a pass: the in-guest QA either never uploaded, or the'
		log 'instance profile lacks s3:PutObject on that prefix.'
	else
		SYNCED_ANY=1
		log "downloaded ${count} artifact file(s)"
	fi
}

report() {
	printf '\n%s -- run %s\n' "$PROGRAM" "$RUN_ID" >&2
	printf 'output directory (absolute paths follow, readable directly):\n' >&2
	local abs
	abs="$(cd -- "$OUT_DIR" && pwd)"
	printf '  %s\n' "$abs" >&2
	local f
	while IFS= read -r f; do
		printf '  %s\n' "$f" >&2
	done < <(find "$abs" -type f | sort)
	printf '\n' >&2
}

VERDICT=''
print_verdict() {
	local results="${OUT_DIR}/results.json"
	if [[ ! -f $results ]]; then
		log 'no results.json -- there is no machine-checkable verdict for this run'
		return 0
	fi
	if ! command -v python3 >/dev/null 2>&1; then
		log "python3 not available; read the verdict yourself from ${results}"
		return 0
	fi
	VERDICT="$(python3 -c '
import json, sys
try:
	with open(sys.argv[1], "r", encoding="utf-8-sig") as fh:
		data = json.load(fh)
except Exception as exc:
	print("UNPARSEABLE: %s" % exc)
	sys.exit(0)
print(data.get("verdict", "MISSING"))
' "$results" 2>/dev/null || printf 'UNPARSEABLE')"
	log "verdict from results.json: ${VERDICT}"
}

# --- verbs -------------------------------------------------------------------
print_instance_summary() {
	local state ip dns
	state="$(instance_state "$INSTANCE_ID")"
	read -r ip dns <<<"$(aws ec2 describe-instances "${AWS_ARGS[@]}" \
		--instance-ids "$INSTANCE_ID" \
		--query 'Reservations[0].Instances[0].[PublicIpAddress,PublicDnsName]' \
		--output text 2>/dev/null || printf 'None None')"
	cat >&2 <<EOF

instance:
  id            ${INSTANCE_ID}
  name tag      ${INSTANCE_NAME_TAG}
  state         ${state}
  public ip     ${ip}
  public dns    ${dns}
  ssm ping      $(ssm_ping "$INSTANCE_ID")
EOF
}

cmd_provision() {
	log 'provision: preflight'
	require_tools
	require_instance_profile
	mkdir -p -- "$TMPDIR"

	if resolve_instance; then
		local state
		state="$(instance_state "$INSTANCE_ID")"
		log "provision: reusing the existing instance ${INSTANCE_ID} (state ${state})"
		log 'provision: nothing was created; this verb is idempotent by name tag'
		state_write
		if [[ $state == 'stopped' || $state == 'stopping' ]]; then
			die "the existing instance is ${state}, so there is nothing to wait for" \
				'Start it before running the QA:' \
				'  scripts/qa/ec2-windows-gui-qa.sh start'
		fi
	else
		log "provision: no instance tagged Name=${INSTANCE_NAME_TAG}; creating one"
		require_subnet
		resolve_bucket
		resolve_ami
		print_plan
		launch_instance
	fi

	wait_for_state running || true
	if ! wait_for_ssm; then
		ssm_offline_hint
		print_instance_summary
		print_cost_notice
		return 1
	fi
	print_instance_summary
	print_cost_notice
	log 'provision: ready. Run the QA with:'
	log '  scripts/qa/ec2-windows-gui-qa.sh qa'
	return 0
}

cmd_qa() {
	log 'qa: preflight'
	require_tools
	locate_ps1
	require_instance
	resolve_bucket

	local state
	state="$(instance_state "$INSTANCE_ID")"
	if [[ $state != 'running' ]]; then
		die "instance ${INSTANCE_ID} is ${state}, not running" \
			'The QA needs a running instance. Start it first:' \
			'  scripts/qa/ec2-windows-gui-qa.sh start'
	fi
	if [[ "$(ssm_ping "$INSTANCE_ID")" != 'Online' ]]; then
		die "instance ${INSTANCE_ID} is running but its SSM agent is not Online" \
			'SSM is the only control channel here, so the QA cannot be sent.' \
			'  scripts/qa/ec2-windows-gui-qa.sh status'
	fi

	mkdir -p -- "$TMPDIR" "$OUT_DIR"
	log "qa: run ${RUN_ID} against ${INSTANCE_ID} (creates nothing, destroys nothing)"

	log 'qa 1/7: console screenshot (pre-QA baseline)'
	take_screenshot 'before QA' 'console-01-before-qa.jpg'

	log 'qa 2/7: staging the in-guest QA script in S3'
	stage_ps1

	log 'qa 3/7: sending the bootstrap command'
	send_qa_command

	log 'qa 4/7: waiting for the QA script to finish'
	wait_for_command

	log 'qa 5/7: console screenshot (post-QA)'
	take_screenshot 'after QA' 'console-02-after-qa.jpg'

	log 'qa 6/7: collecting S3 artifacts'
	collect_artifacts

	log 'qa 7/7: reporting'
	report
	print_verdict
	log 'qa: the instance is still running and untouched. Re-run this verb freely.'
	print_cost_notice

	if [[ $VERDICT == 'INCOMPLETE' ]]; then
		log ''
		log 'INCOMPLETE usually means the window assertions were NOT EXECUTED because'
		log 'the run measured session 0, which structurally cannot host a WebView2'
		log 'window. Establish an interactive session and re-run:'
		log '  scripts/qa/ec2-windows-gui-qa.sh interactive'
		log '  scripts/qa/ec2-windows-gui-qa.sh qa'
		log 'results.json / diagnostics.json name the exact reason either way; check'
		log 'diagnostics.session_id and diagnostics.interactive_handoff first.'
	fi

	if [[ $COMMAND_STATUS != 'Success' ]]; then
		log "FAIL: the SSM command status was ${COMMAND_STATUS}, not Success"
		return 1
	fi
	if [[ $SYNCED_ANY != '1' ]]; then
		log 'FAIL: the command succeeded but no artifacts arrived in S3'
		return 1
	fi
	if [[ $VERDICT != 'PASS' ]]; then
		log "FAIL: results.json verdict is '${VERDICT}', not PASS"
		return 1
	fi
	log 'PASS: SSM command succeeded and results.json reports verdict=PASS'
	return 0
}

# =============================================================================
# THE INTERACTIVE SESSION -- why this verb exists and what it deliberately costs.
#
# Two earlier QA rounds installed AgentLens, launched it and measured NOTHING,
# both times for the same structural reason: SSM Run Command executes as SYSTEM in
# session 0 on window station Service-0x0-3e7$, which is reserved for services.
# A Tauri window is a WebView2 window, and the WebView2 browser process is exactly
# what session 0 will not host: msedgewebview2.exe count stayed 0 while the app
# process itself stayed alive, logged its tray icon and created its ordinary
# USER32 helper HWNDs. So USER32 window creation was never blocked -- the webview
# host was -- and no retry inside session 0 can change that. qwinsta additionally
# showed console session 1 existing with NO logged-on user, which is why there was
# no interactive token to launch into either.
#
# This verb creates that token the only way a headless instance can: Windows
# autologon. It writes AutoAdminLogon / DefaultUserName / DefaultPassword /
# DefaultDomainName under Winlogon, reboots, and then CONFIRMS from the guest that
# a session other than 0 now has a logged-on user, rather than assuming the
# reboot worked.
#
# THE PLAINTEXT PASSWORD IS THE PRICE, AND IT IS PAID KNOWINGLY. Winlogon reads
# DefaultPassword as cleartext; there is no hashed variant it accepts. The
# mitigations are all of: the password is generated INSIDE the guest by
# RandomNumberGenerator so it never crosses the wire; this workstation never
# learns it; it is never written to artifacts/, to S3 or to qa.log; it belongs to
# a purpose-made local account that exists nowhere else; and it dies with the
# instance on 'destroy --yes'. print_interactive_credential_notice states all of
# that out loud, twice, so a teardown can account for it.
#
# THE REBOOT IS SAFE HERE. --instance-initiated-shutdown-behavior is 'stop' (see
# launch_instance), so a guest restart cannot destroy the box, and this verb uses
# reboot-instances on the one captured instance id. terminate-instances remains
# reachable from cmd_destroy alone.
# =============================================================================
print_interactive_credential_notice() {
	cat >&2 <<EOF

PLAINTEXT CREDENTIAL -- deliberate, and here is why it is acceptable.
Windows autologon has exactly one supported mechanism: at boot Winlogon reads
  HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon\\DefaultPassword
as CLEARTEXT. No hashed or DPAPI-protected form of that value works, so giving a
headless instance an interactive session necessarily leaves a readable password in
its registry.

  account        <computer>\\${INTERACTIVE_USER}, a local account added to Administrators
  password       generated INSIDE the guest by RandomNumberGenerator. It is never
                 transmitted, never printed, never written to artifacts/, to S3 or
                 to qa.log, and THIS WORKSTATION DOES NOT KNOW IT -- nothing
                 outside the instance can reuse it.
  blast radius   this one disposable QA instance, ${INSTANCE_ID}. The account is
                 purpose-made, reused nowhere, and has no access to anything else.
  removal        'destroy --yes' terminates the instance and takes the account and
                 the registry value with it. There is nothing else to revoke.
EOF
}

cmd_interactive() {
	require_tools
	require_instance
	local state ping
	state="$(instance_state "$INSTANCE_ID")"
	if [[ $state != 'running' ]]; then
		die "instance ${INSTANCE_ID} is ${state}, not running" \
			'Autologon has to be written and then rebooted into, so the instance has' \
			'to be running first:' \
			'  scripts/qa/ec2-windows-gui-qa.sh start'
	fi
	ping="$(ssm_ping "$INSTANCE_ID")"
	if [[ $ping != 'Online' ]]; then
		die "instance ${INSTANCE_ID} is running but its SSM PingStatus is ${ping}" \
			'SSM is the only control channel here, so the registry cannot be written.' \
			'  scripts/qa/ec2-windows-gui-qa.sh status'
	fi

	mkdir -p -- "$TMPDIR"
	print_interactive_credential_notice

	# Every PowerShell line below is SINGLE-quoted so the document carries no "
	# character, and every backslash is doubled because JSON \\ decodes to one
	# backslash which PowerShell then treats literally inside single quotes.
	local setup_doc="${TMPDIR}/ssm-autologon-setup.json"
	cat >"$setup_doc" <<'SETUPEOF'
{"commands":[
"$ErrorActionPreference='Stop'",
"$u='agentlens-qa'",
"$alphabet='abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'",
"$bytes=New-Object byte[] 28",
"$rng=[System.Security.Cryptography.RandomNumberGenerator]::Create()",
"$rng.GetBytes($bytes)",
"$pw=(-join ($bytes | ForEach-Object { $alphabet[$_ % $alphabet.Length] })) + '#7aZ'",
"$sec=ConvertTo-SecureString -String $pw -AsPlainText -Force",
"if (Get-LocalUser -Name $u -ErrorAction SilentlyContinue) { Set-LocalUser -Name $u -Password $sec -PasswordNeverExpires $true; Enable-LocalUser -Name $u } else { New-LocalUser -Name $u -Password $sec -FullName 'AgentLens GUI QA' -Description 'disposable QA autologon account' -PasswordNeverExpires -AccountNeverExpires | Out-Null }",
"Add-LocalGroupMember -Group 'Administrators' -Member $u -ErrorAction SilentlyContinue",
"$wl='HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon'",
"Set-ItemProperty -LiteralPath $wl -Name 'AutoAdminLogon' -Value '1' -Type String",
"Set-ItemProperty -LiteralPath $wl -Name 'DefaultUserName' -Value $u -Type String",
"Set-ItemProperty -LiteralPath $wl -Name 'DefaultPassword' -Value $pw -Type String",
"Set-ItemProperty -LiteralPath $wl -Name 'DefaultDomainName' -Value $env:COMPUTERNAME -Type String",
"Remove-ItemProperty -LiteralPath $wl -Name 'AutoLogonCount' -ErrorAction SilentlyContinue",
"Remove-ItemProperty -LiteralPath $wl -Name 'AutoLogonChecked' -ErrorAction SilentlyContinue",
"$sys='HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System'",
"if (-not (Test-Path -LiteralPath $sys)) { New-Item -Path $sys -Force | Out-Null }",
"Set-ItemProperty -LiteralPath $sys -Name 'InactivityTimeoutSecs' -Value 0 -Type DWord",
"$pwLen=$pw.Length",
"Remove-Variable -Name pw -ErrorAction SilentlyContinue",
"Remove-Variable -Name sec -ErrorAction SilentlyContinue",
"$rng.Dispose()",
"$now=Get-ItemProperty -LiteralPath $wl",
"Write-Output ('AUTOLOGON_USER=' + $env:COMPUTERNAME + '\\' + $u)",
"Write-Output ('AUTOLOGON_FLAG=' + $now.AutoAdminLogon)",
"Write-Output ('AUTOLOGON_DEFAULTUSER=' + $now.DefaultUserName)",
"Write-Output ('AUTOLOGON_DEFAULTDOMAIN=' + $now.DefaultDomainName)",
"Write-Output ('PASSWORD_PLAINTEXT_IN_REGISTRY=yes length=' + $pwLen + ' the-value-is-printed-nowhere')",
"Write-Output ('LEGAL_NOTICE=' + (('' + $now.LegalNoticeCaption).Length + ('' + $now.LegalNoticeText).Length))",
"Write-Output ('BOOTTIME=' + (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))"
],"executionTimeout":["300"]}
SETUPEOF

	local verify_doc="${TMPDIR}/ssm-autologon-verify.json"
	cat >"$verify_doc" <<'VERIFYEOF'
{"commands":[
"$ErrorActionPreference='Continue'",
"Write-Output ('BOOTTIME=' + (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))",
"Write-Output ('PROBE_SESSION=' + [System.Diagnostics.Process]::GetCurrentProcess().SessionId)",
"$wl='HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon'",
"Write-Output ('AUTOLOGON_FLAG=' + (Get-ItemProperty -LiteralPath $wl -ErrorAction SilentlyContinue).AutoAdminLogon)",
"$found=0",
"foreach ($p in (Get-CimInstance -ClassName Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -eq 'explorer.exe' })) { $o=Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction SilentlyContinue; $who=''; if ($o -and $o.ReturnValue -eq 0) { $who=('' + $o.Domain + '\\' + $o.User) }; if ($p.SessionId -ne 0 -and $who) { $found=$found+1 }; Write-Output ('EXPLORER session=' + $p.SessionId + ' user=' + $who + ' pid=' + $p.ProcessId) }",
"Write-Output ('INTERACTIVE_SHELLS=' + $found)",
"foreach ($l in (& qwinsta 2>&1)) { Write-Output ('QWINSTA ' + $l) }"
],"executionTimeout":["300"]}
VERIFYEOF

	log ''
	log 'interactive 1/4: creating the autologon account and writing the Winlogon values'
	local setup_out boot_before
	if ! setup_out="$(ssm_run_document "$setup_doc" 'AgentLens QA autologon setup' 40)"; then
		if [[ -n $setup_out ]]; then printf '%s\n' "$setup_out" >&2; fi
		die 'could not configure autologon in the guest' \
			'Nothing was rebooted, and nothing was terminated. Re-running this verb is' \
			'safe and idempotent: an existing account is reused and the registry values' \
			'are simply overwritten.'
	fi
	printf '%s\n' "$setup_out" >&2
	boot_before="$(printf '%s\n' "$setup_out" | sed -n 's/^BOOTTIME=//p' | head -n 1)"
	log "  guest boot time before the reboot: ${boot_before:-unknown}"

	log 'interactive 2/4: rebooting so Winlogon performs the automatic logon'
	log '  safe here: shutdown-behavior is stop, and this is reboot-instances on one'
	log '  captured id. terminate-instances is reachable only from destroy --yes.'
	if ! aws ec2 reboot-instances "${AWS_ARGS[@]}" --instance-ids "$INSTANCE_ID" >/dev/null 2>&1; then
		die "reboot-instances failed for ${INSTANCE_ID}" \
			'Autologon is configured but does not take effect until the guest restarts.' \
			'Reboot it by hand, then re-run this verb to verify:' \
			"  aws ec2 reboot-instances --instance-ids ${INSTANCE_ID} --region ${AWS_REGION_LITERAL} --profile ${AWS_PROFILE_LITERAL}"
	fi
	log '  reboot requested'

	log 'interactive 3/4: waiting for the guest to come back WITH a logged-on session'
	log '  two conditions, both required: the boot time must change (so we are not'
	log '  reading the pre-reboot state) and a session other than 0 must own an'
	log '  explorer.exe with a real user.'
	local attempt verify_out boot_now shells probe_out
	verify_out=''
	boot_now=''
	shells='0'
	for ((attempt = 1; attempt <= INTERACTIVE_ATTEMPTS; attempt++)); do
		sleep "$INTERACTIVE_SLEEP"
		if [[ "$(ssm_ping "$INSTANCE_ID")" != 'Online' ]]; then
			if ((attempt % 4 == 0)); then
				log "  SSM agent not back yet (${attempt}/${INTERACTIVE_ATTEMPTS})"
			fi
			continue
		fi
		if ! probe_out="$(ssm_run_document "$verify_doc" 'AgentLens QA interactive session probe' 24)"; then
			if ((attempt % 4 == 0)); then
				log "  probe not answering yet (${attempt}/${INTERACTIVE_ATTEMPTS})"
			fi
			continue
		fi
		verify_out="$probe_out"
		boot_now="$(printf '%s\n' "$probe_out" | sed -n 's/^BOOTTIME=//p' | head -n 1)"
		shells="$(printf '%s\n' "$probe_out" | sed -n 's/^INTERACTIVE_SHELLS=//p' | head -n 1)"
		if [[ -n $boot_before && $boot_now == "$boot_before" ]]; then
			log "  guest has not restarted yet (boot time still ${boot_now})"
			continue
		fi
		if [[ ${shells:-0} == '0' ]]; then
			log "  restarted at ${boot_now}, but no interactive shell yet (${attempt}/${INTERACTIVE_ATTEMPTS})"
			continue
		fi
		break
	done

	log 'interactive 4/4: what the guest reports'
	if [[ -n $verify_out ]]; then
		printf '%s\n' "$verify_out" >&2
	else
		log '  the probe never answered at all after the reboot'
	fi

	local session_line session_id
	session_line="$(printf '%s\n' "$verify_out" |
		grep -E '^EXPLORER session=[1-9][0-9]* user=[^ ]' | head -n 1 || true)"
	if [[ -z $session_line ]]; then
		log ''
		log 'BLOCKED: the guest still reports no interactive logon session after the reboot.'
		log "  boot time before   : ${boot_before:-unknown}"
		log "  boot time after    : ${boot_now:-unknown}"
		log "  interactive shells : ${shells:-0}"
		log 'Autologon was written but did not take effect. Check, in this order:'
		log '  * AUTOLOGON_FLAG above must be 1 and AUTOLOGON_DEFAULTUSER must be set'
		log '  * LEGAL_NOTICE above must be 0: a logon banner blocks autologon outright'
		log '  * the local password policy may have rejected the generated password,'
		log '    in which case the account exists but cannot sign in'
		log '  * QWINSTA above shows whether a console session exists at all'
		log 'The instance is untouched and still running. Nothing was terminated.'
		print_cost_notice
		return 1
	fi
	session_id="${session_line#*session=}"
	session_id="${session_id%% *}"

	log ''
	log "INTERACTIVE SESSION ESTABLISHED: session id ${session_id}"
	log "  ${session_line}"
	log 'The QA can now measure a real window. The in-guest script detects this'
	log 'session and re-executes ITSELF inside it, because a session-0 process can'
	log 'neither host a WebView2 window nor enumerate one that belongs to session'
	log "${session_id}. Run it now:"
	log '  scripts/qa/ec2-windows-gui-qa.sh qa'
	print_interactive_credential_notice
	print_cost_notice
	return 0
}

# Mirrors the .ps1's own detection, and there are TWO separate traps here.
#
# (1) WOW64 FILESYSTEM REDIRECTION. The Tauri NSIS stub is a 32-bit process, so
#     running as SYSTEM it resolves %LOCALAPPDATA% through the redirected view:
#     it records InstallLocation as ...\Windows\system32\config\systemprofile\...
#     while the bytes physically land under ...\Windows\SysWOW64\config\... A
#     64-bit reader gets the real System32 and sees nothing there, so a perfectly
#     good install reads as missing. The probe below therefore tests the recorded
#     path AND its System32<->SysWOW64 substitutions, and reports which one
#     resolved. Sysnative is no help: it exists only for 32-bit processes.
#
# (2) The per-user hive question, which is a DIFFERENT thing and was previously
#     mis-stated here as the explanation for (1). The package installs per-user,
#     so its uninstall key can land in a real user's hive rather than the SYSTEM
#     account's HKCU that an SSM command starts out in. HKLM plus every loaded
#     hive under HKEY_USERS is searched for that reason -- but on this instance
#     the key IS found and it was (1) that broke the run.
report_install_state() {
	local probe="${TMPDIR}/ssm-status-probe.json"
	mkdir -p -- "$TMPDIR"
	# Passed as file:// rather than through --parameters shorthand, and every
	# PowerShell string below is SINGLE-quoted, so the document carries no "
	# characters that would need JSON escaping and no backslash the shorthand
	# parser could eat. JSON \\ decodes to one backslash, which PowerShell then
	# treats literally inside single quotes. The System32/SysWOW64 substitution
	# uses [IO.Path]::Combine and Substring rather than a regex, precisely so it
	# needs no extra backslash escaping in this JSON.
	cat >"$probe" <<'PROBEEOF'
{"commands":[
"$ErrorActionPreference='Continue'",
"$rel='Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\AgentLens'",
"if (-not (Get-PSDrive -Name HKU -ErrorAction SilentlyContinue)) { New-PSDrive -Name HKU -PSProvider Registry -Root HKEY_USERS | Out-Null }",
"$paths=@(('HKLM:\\'+$rel),('HKCU:\\'+$rel))",
"foreach ($h in (Get-ChildItem -LiteralPath 'HKU:\\' -ErrorAction SilentlyContinue)) { $paths += ('HKU:\\'+(Split-Path -Leaf $h.Name)+'\\'+$rel) }",
"$hit=$null",
"foreach ($p in $paths) { if (Test-Path -LiteralPath $p) { $hit=Get-ItemProperty -LiteralPath $p -ErrorAction SilentlyContinue; if ($hit) { break } } }",
"if (-not $hit) { Write-Output 'not-installed'; exit 0 }",
"$loc=(''+$hit.InstallLocation).Trim().Trim([char]34)",
"$s32=[IO.Path]::Combine($env:WinDir,'System32')",
"$w64=[IO.Path]::Combine($env:WinDir,'SysWOW64')",
"$cands=@($loc)",
"if ($loc.ToLower().StartsWith($s32.ToLower())) { $cands += ($w64 + $loc.Substring($s32.Length)) }",
"if ($loc.ToLower().StartsWith($w64.ToLower())) { $cands += ($s32 + $loc.Substring($w64.Length)) }",
"$found=''",
"foreach ($c in $cands) { if ($c -and (Test-Path -LiteralPath $c)) { $found=$c; break } }",
"$bin=(''+$hit.MainBinaryName).Trim().Trim([char]34)",
"if ($found) { Write-Output ('installed, DisplayVersion=' + $hit.DisplayVersion + ', mainBinary=' + $bin + ', dir=' + $found) } else { Write-Output ('REGISTRY ONLY, DisplayVersion=' + $hit.DisplayVersion + ' -- no directory exists for ' + $loc + ' or its WOW64 substitutions') }"
],"executionTimeout":["60"]}
PROBEEOF

	local cmd_id status out
	cmd_id="$(aws ssm send-command "${AWS_ARGS[@]}" \
		--document-name AWS-RunPowerShellScript \
		--instance-ids "$INSTANCE_ID" \
		--comment 'AgentLens QA status probe' \
		--parameters "file://${probe}" \
		--query 'Command.CommandId' --output text 2>/dev/null || printf '')"
	if [[ -z $cmd_id || $cmd_id == 'None' ]]; then
		printf '  agentlens     unknown (could not send the SSM probe)\n' >&2
		return 0
	fi

	local attempt
	for ((attempt = 1; attempt <= 20; attempt++)); do
		status="$(aws ssm get-command-invocation "${AWS_ARGS[@]}" \
			--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
			--query Status --output text 2>/dev/null || printf 'Pending')"
		case "$status" in
		Success | Failed | Cancelled | TimedOut) break ;;
		esac
		sleep 3
	done
	if [[ $status != 'Success' ]]; then
		printf '  agentlens     unknown (probe status %s)\n' "$status" >&2
		return 0
	fi
	out="$(aws ssm get-command-invocation "${AWS_ARGS[@]}" \
		--command-id "$cmd_id" --instance-id "$INSTANCE_ID" \
		--query StandardOutputContent --output text 2>/dev/null | tr -d '\r' | head -n 1)"
	printf '  agentlens     %s\n' "${out:-unknown (empty probe output)}" >&2
}

report_last_qa_run() {
	local base
	base="$(dirname -- "$OUT_DIR")"
	if [[ ! -d $base ]]; then
		printf '  last local qa no local QA output under %s\n' "$base" >&2
		return 0
	fi
	local newest
	newest="$(find "$base" -mindepth 2 -maxdepth 2 -name 'results.json' \
		-printf '%T@ %p\n' 2>/dev/null | sort -rn | head -n 1 | cut -d' ' -f2-)"
	if [[ -z $newest ]]; then
		printf '  last local qa none found under %s\n' "$base" >&2
		return 0
	fi
	printf '  last local qa %s (%s)\n' \
		"$(date -u -r "$newest" +%Y-%m-%dT%H:%M:%SZ)" "$newest" >&2
}

cmd_status() {
	require_tools
	if ! resolve_instance; then
		log "no instance tagged Name=${INSTANCE_NAME_TAG} and no usable id cache"
		log 'nothing is billing. Create one with:'
		log '  AGENTLENS_QA_INSTANCE_PROFILE=<name> scripts/qa/ec2-windows-gui-qa.sh provision'
		report_last_qa_run
		return 0
	fi

	local state launched ping
	state="$(instance_state "$INSTANCE_ID")"
	launched="$(aws ec2 describe-instances "${AWS_ARGS[@]}" \
		--instance-ids "$INSTANCE_ID" \
		--query 'Reservations[0].Instances[0].LaunchTime' \
		--output text 2>/dev/null || printf 'None')"
	print_instance_summary

	if [[ $launched != 'None' && -n $launched ]]; then
		local launched_epoch now_epoch secs
		launched_epoch="$(date -u -d "$launched" +%s 2>/dev/null || printf '0')"
		now_epoch="$(date -u +%s)"
		if ((launched_epoch > 0 && now_epoch > launched_epoch)); then
			secs=$((now_epoch - launched_epoch))
			printf '  launched      %s\n' "$launched" >&2
			printf '  uptime        %dd %dh %dm (wall clock since launch)\n' \
				"$((secs / 86400))" "$((secs % 86400 / 3600))" "$((secs % 3600 / 60))" >&2
			printf '  cost estimate %s at $%s/hr, if it never stopped\n' \
				"$(format_cost_for_seconds "$secs")" "$COST_RATE_LITERAL" >&2
		fi
	fi

	ping="$(ssm_ping "$INSTANCE_ID")"
	if [[ $ping == 'Online' ]]; then
		report_install_state
	else
		printf '  agentlens     not probed (SSM PingStatus=%s; a stopped or\n' "$ping" >&2
		printf '                unregistered instance cannot be asked)\n' >&2
	fi
	report_last_qa_run

	if [[ $state == 'running' ]]; then
		print_cost_notice
	else
		log ''
		log "state is ${state}: no hourly compute charge, but the EBS volume still bills."
		log "  resume   scripts/qa/ec2-windows-gui-qa.sh start"
		log "  finish   scripts/qa/ec2-windows-gui-qa.sh destroy --yes"
	fi
	return 0
}

cmd_stop() {
	require_tools
	require_instance
	local state
	state="$(instance_state "$INSTANCE_ID")"
	if [[ $state == 'stopped' ]]; then
		log "instance ${INSTANCE_ID} is already stopped"
		return 0
	fi
	log "stopping ${INSTANCE_ID} (by id; the instance is NOT terminated)"
	aws ec2 stop-instances "${AWS_ARGS[@]}" \
		--instance-ids "$INSTANCE_ID" --output text >/dev/null ||
		die "stop-instances failed for ${INSTANCE_ID}"
	wait_for_state stopped || true
	print_instance_summary
	log 'stopped: no hourly compute charge now; the EBS volume still bills.'
	log '  resume   scripts/qa/ec2-windows-gui-qa.sh start'
	return 0
}

cmd_start() {
	require_tools
	require_instance
	local state
	state="$(instance_state "$INSTANCE_ID")"
	if [[ $state == 'running' ]]; then
		log "instance ${INSTANCE_ID} is already running"
	else
		log "starting ${INSTANCE_ID}"
		aws ec2 start-instances "${AWS_ARGS[@]}" \
			--instance-ids "$INSTANCE_ID" --output text >/dev/null ||
			die "start-instances failed for ${INSTANCE_ID}"
		wait_for_state running || true
	fi
	if ! wait_for_ssm; then
		ssm_offline_hint
		print_instance_summary
		print_cost_notice
		return 1
	fi
	print_instance_summary
	print_cost_notice
	return 0
}

# The ONLY caller of terminate_instance in this file.
cmd_destroy() {
	require_tools
	require_instance
	local state
	state="$(instance_state "$INSTANCE_ID")"

	if [[ ${1:-} != '--yes' ]]; then
		cat >&2 <<EOF
destroy refused: this is irreversible and needs an explicit --yes.

  target        ${INSTANCE_ID}
  name tag      ${INSTANCE_NAME_TAG}
  state         ${state}
  region        ${AWS_REGION_LITERAL} (profile ${AWS_PROFILE_LITERAL})

It would run exactly this, by instance id and never by --filters:
  aws ec2 terminate-instances --instance-ids ${INSTANCE_ID} --region ${AWS_REGION_LITERAL} --profile ${AWS_PROFILE_LITERAL}

Everything on the instance -- the installed build, the logs, the desktop state --
is destroyed with it and cannot be recovered. If you only want to stop paying the
hourly compute charge while keeping all of that, run 'stop' instead.

Confirm with:
  scripts/qa/ec2-windows-gui-qa.sh destroy --yes
EOF
		return 2
	fi

	log "destroy --yes: terminating ${INSTANCE_ID} (state ${state})"
	if ! terminate_instance; then
		log 'the id cache was KEPT so the instance can still be found and finished'
		return 1
	fi
	wait_for_state terminated || true
	state_clear
	log "destroyed ${INSTANCE_ID}; billing for it has ended"
	return 0
}

main() {
	local verb="${1:-}"
	shift || true
	case "$verb" in
	provision) cmd_provision "$@" ;;
	interactive) cmd_interactive "$@" ;;
	qa) cmd_qa "$@" ;;
	status) cmd_status "$@" ;;
	stop) cmd_stop "$@" ;;
	start) cmd_start "$@" ;;
	destroy) cmd_destroy "$@" ;;
	help | -h | --help)
		usage
		return 0
		;;
	'')
		usage
		die 'no verb given' 'pick one of: provision interactive qa status stop start destroy help'
		;;
	*)
		usage
		die "unknown verb: ${verb}" 'pick one of: provision interactive qa status stop start destroy help'
		;;
	esac
}

main "$@"
