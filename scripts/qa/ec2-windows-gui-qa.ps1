# =============================================================================
# AgentLens Windows GUI QA -- runs INSIDE an EC2 Windows Server instance.
#
# Delivered by SSM Run Command (AWS-RunPowerShellScript). Answers the one
# question a headless CodeBuild container structurally cannot: does the
# undecorated window actually exist, with the right geometry and the right
# icon, once the NSIS installer has really run?
#
# What it does, in order:
#   0. If this process is in session 0 and an interactive logon session exists,
#      hand the ENTIRE run over to that session through a scheduled task and act
#      only as a supervisor. See Invoke-SessionHandoff. Session 0 cannot host a
#      WebView2 window, and a session-0 process cannot even enumerate a window
#      that belongs to session 1, so both halves have to move together.
#   1. Pre-flight: record OS, session id, screen metrics, WebView2 presence.
#   2. Install the WebView2 Evergreen Runtime if absent. Without it the
#      WebView2 window cannot render at all, so this gates everything.
#   3. Download the AgentLens x64 NSIS setup from S3 and VERIFY its SHA-256.
#      The file name is derived from the workspace version, so it changes with
#      every release; see $script:SetupName. A mismatch is a hard failure;
#      nothing is installed.
#   4. Install silently (/S /NS), then resolve the real install directory out
#      of the registry rather than assuming it.
#   5. Launch the app, wait for a visible top-level HWND owned by its pid.
#   6. Run MACHINE-CHECKABLE assertions: window style bits, geometry, the
#      minimum-size clamp, the window title, the maximize work-area fit, and
#      byte-exact SHA-256 of every RT_ICON payload in the installed exe.
#   7. Capture screenshots (best effort -- see the session-0 note below).
#   8. Write results.json + a human log, and upload everything to S3.
#
# ---- Session 0, and why step 0 exists -------------------------------------
# SSM Run Command executes as NT AUTHORITY\SYSTEM. Since Windows Vista, SYSTEM
# services live in session 0, which has no interactive desktop and no DWM. A
# CopyFromScreen/BitBlt attempted from here returns an all-black image or throws
# "The handle is invalid"; a session-0 process cannot see or capture windows
# belonging to session 1+; and a GUI app launched from here with Start-Process
# inherits session 0. For an ordinary Win32 app that is survivable -- the window
# is real, with an HWND, a style and a rect, just never composited. For a Tauri
# app it is not: the window IS a WebView2 window, the WebView2 browser process is
# what session 0 refuses to host, and measurement finds no window at all. Two
# earlier runs proved exactly that, with msedgewebview2.exe count 0.
#
# So this script no longer tries to measure session 0 when it does not have to.
# Step 0 moves the whole run into an interactive session when one exists, and the
# window assertions are recorded NOT EXECUTED -- never FAIL -- when it does not.
# scripts/qa/ec2-windows-gui-qa.sh 'interactive' is what creates that session.
#
# The in-guest PNGs remain secondary either way. GetWindowLong and GetWindowRect
# report the truth regardless of what any desktop is showing, and the
# authoritative VISUAL channel for this QA is `aws ec2 get-console-screenshot`,
# taken from the DRIVER side, which reads the hypervisor framebuffer and needs
# nothing from inside this guest. Once a desktop is reachable, Set-ScreenResolution
# widens it so that capture can contain the whole 1180x780 window frame.
# See .omo/evidence/h7-ec2-gui-qa-plan.md section 3.
#
# ---- Two hard constraints inherited from scripts/install.ps1 ---------------
# (1) A non-zero exit from a NATIVE command does NOT stop a PowerShell script,
#     and $ErrorActionPreference does not apply to native exit codes either.
#     Every native process launched here is therefore checked explicitly
#     (Start-Process -PassThru -Wait, then inspect .ExitCode). Do not
#     "simplify" that away: silently ignoring a failed installer is exactly
#     the wash-failure-into-green defect this project has already fixed once.
# (2) This file is strictly ASCII-only. Windows PowerShell 5.1 reads a .ps1 as
#     ANSI unless it carries a BOM, so a single non-ASCII character can tear a
#     string literal apart and kill the script with no useful error. Keep
#     every character in this file inside 7-bit ASCII.
#
# ---- The installer is UNSIGNED ---------------------------------------------
# SmartScreen behaviour is recorded, not asserted. A fresh Server desktop has
# no SmartScreen reputation history and Server defaults differ from client
# Windows, so the ABSENCE of a prompt here is NOT evidence that an end user on
# Windows 11 will not see one.
#
# Configuration is by environment variable, set through SSM parameters:
#   AGENTLENS_QA_BUCKET     S3 bucket for input artifact and output upload.
#                           REQUIRED, no default -- see below.
#   AGENTLENS_QA_KEY        S3 key of the ZIP holding the setup exe
#   AGENTLENS_QA_OUT_PREFIX S3 prefix to upload results under
#   AGENTLENS_QA_SHA256     expected SHA-256 of the setup exe
#   AGENTLENS_QA_RUN_ID     correlation id, echoed into results.json
# =============================================================================

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# =============================================================================
# THE INTERACTIVE HANDOFF, AND WHY THIS FILE READS A HANDOFF FILE FIRST.
#
# SSM Run Command executes as SYSTEM in session 0, and a WebView2 window cannot
# be composited in session 0 at all. So when this script starts in session 0 and
# an interactive logon session exists on the box, it does NOT measure session 0:
# it relaunches ITSELF as the interactive user through a scheduled task with
# -LogonType Interactive, waits, and propagates the child's exit code. The child
# is the run that installs, launches the app and measures the window, and it is
# the child's session id -- read from inside the child's own process -- that the
# artifacts record. See Invoke-SessionHandoff near the bottom of this file.
#
# A scheduled task does NOT inherit the launching process's environment, and
# every setting here arrives as an AGENTLENS_QA_* environment variable set by the
# SSM parameters document. The settings are therefore handed over in a plain
# ASCII key=value file that the child reads back into its own environment BEFORE
# any configuration is resolved below. A real environment variable always wins
# over the file, so a direct session-0 run behaves exactly as it did before this
# mechanism existed, and a stale handoff file from a previous run cannot override
# the current run's SSM parameters.
#
# The file carries no credential. The autologon password is generated inside the
# guest by scripts/qa/ec2-windows-gui-qa.sh and is never handed to this script.
# =============================================================================
$script:HandoffPath = 'C:\agentlens-qa\qa-handoff.txt'

function Import-Handoff {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    foreach ($line in (Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue)) {
        $t = "$line".Trim()
        if (-not $t) { continue }
        if ($t.StartsWith('#')) { continue }
        $i = $t.IndexOf('=')
        if ($i -lt 1) { continue }
        $k = $t.Substring(0, $i).Trim()
        $v = $t.Substring($i + 1)
        if ($k -notlike 'AGENTLENS_QA_*') { continue }
        # A real environment variable wins: only fill in what is not already set.
        if ([Environment]::GetEnvironmentVariable($k, 'Process')) { continue }
        [Environment]::SetEnvironmentVariable($k, $v, 'Process')
    }
}

Import-Handoff -Path $script:HandoffPath

# --- configuration -----------------------------------------------------------

# REQUIRED, with no fallback. Both entry points always supply it: the driver's
# send_qa_command sets $env:AGENTLENS_QA_BUCKET in the SSM document, and the
# session-0 supervisor writes it into the handoff file Import-Handoff just read.
# So a missing value means the harness is broken, not that a default is wanted --
# and guessing a bucket name here would reach for storage that may belong to
# somebody else's account. Stop-WithError is not defined yet at this point in the
# file, hence the bare throw: this runs before any log file exists, so there is
# nothing to write to and the SSM invocation output is the only channel.
if (-not $env:AGENTLENS_QA_BUCKET) {
    throw 'AGENTLENS_QA_BUCKET is not set. It carries the S3 bucket for both the installer download and the results upload, and there is deliberately no default: the bucket name is derived from the caller AWS account by scripts/qa/ec2-windows-gui-qa.sh (resolve_bucket) and passed in. Run this script through that driver, or set the variable explicitly before invoking it directly.'
}
$script:Bucket = $env:AGENTLENS_QA_BUCKET
$script:Key = if ($env:AGENTLENS_QA_KEY) { $env:AGENTLENS_QA_KEY } else { 'artifacts/39f89617-585d-443c-a7fb-031a1d9f60ee/agentlens-windows' }
$script:OutPrefix = if ($env:AGENTLENS_QA_OUT_PREFIX) { $env:AGENTLENS_QA_OUT_PREFIX } else { 'qa/h7-gui' }
$script:ExpectedSha = if ($env:AGENTLENS_QA_SHA256) { $env:AGENTLENS_QA_SHA256 } else { 'ad6accacfb9b69b9fd05545e89ecad8dd122d460191134d0749cb9e6220d360d' }
$script:RunId = if ($env:AGENTLENS_QA_RUN_ID) { $env:AGENTLENS_QA_RUN_ID } else { 'local' }

# Every S3 cmdlet below is passed -Region explicitly. There is no aws CLI on this
# AMI and therefore no CLI config/profile to inherit a region from, and the
# AWSPowerShell default-region resolution would otherwise depend on IMDS or on
# Set-DefaultAWSRegion having been called. An explicit value removes that guess.
$script:Region = if ($env:AGENTLENS_QA_REGION) { $env:AGENTLENS_QA_REGION } else { 'us-east-2' }
$script:AwsPsModule = ''

$script:WorkDir = 'C:\agentlens-qa'
$script:OutDir = Join-Path $script:WorkDir 'out'
$script:LogPath = Join-Path $script:OutDir 'qa.log'
$script:ProductName = 'AgentLens'
# SINGLE SOURCE for the version; the setup file name is DERIVED from it so a
# release-please version bump does not leave this harness looking for an artifact
# that no longer exists. The default is the version currently in the root
# Cargo.toml [workspace.package]; unlike AGENTLENS_QA_BUCKET that is an honest
# default rather than a guess, so it is optional and not a hard throw.
$script:Version = if ($env:AGENTLENS_QA_VERSION) { $env:AGENTLENS_QA_VERSION } else { '0.1.0' }
$script:SetupName = "AgentLens_$($script:Version)_x64-setup.exe"

# The main binary is NOT productName.exe. tauri.conf.json declares no
# bundle.windows.mainBinaryName, so the NSIS template ships the Cargo bin name:
# the installed file is agentlens-tauri.exe. Hardcoding 'AgentLens.exe' here is
# what previously made Start-App fail on a perfectly good install. The registry's
# MainBinaryName value is authoritative and is preferred at run time; this list is
# only the fallback for a package rebuilt with a different bin name.
$script:MainBinaryName = ''
$script:BinaryNameCandidates = @('agentlens-tauri.exe', 'AgentLens.exe', 'agentlens.exe')

# WebView2 Evergreen Runtime, per-machine and per-user detection keys. The
# GUID is the Runtime's own Edge Update AppID -- Edge Stable is a DIFFERENT
# app with a different GUID, so having Edge installed does NOT imply this key
# exists. Both hives must be checked: HKLM carrying pv = 0.0.0.0 while the
# real version sits in HKCU is Microsoft-confirmed By Design.
$script:Wv2Guid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$script:Wv2HklmWow = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$script:Wv2Guid"
$script:Wv2Hklm = "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$script:Wv2Guid"
$script:Wv2Hkcu = "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$script:Wv2Guid"
# Confirmed permanent link: this exact URL is hardcoded in Tauri's own NSIS
# installer template, which is the strongest confirmation available.
$script:Wv2Bootstrapper = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'

# Byte-exact RT_ICON payload digests taken from src-tauri/icons/icon.ico
# (33,803 bytes, sha256 7d930f82a04982a4a20149046dba6de8c41aaf7e26cabe8862cfaac18972ce94,
# exactly 6 frames, all 32bpp). A 6/6 match against the INSTALLED binary is a
# byte-level icon proof that requires nobody to look at a picture.
$script:ExpectedIconFrames = @(
    @{ Size = '32x32';   Bytes = 2359;  Sha = 'c7889a729b46aaea2dcc7f0128b7793b574ec45f16ffec3323288ae50f4a2f11' }
    @{ Size = '16x16';   Bytes = 861;   Sha = '90a59bb9b5d3fced028f0802fd80e42d5799d79575e24f12590250bd49a336cb' }
    @{ Size = '24x24';   Bytes = 1572;  Sha = '6bae3883591cc2d901d184e669b19c2860abdfe30f429324d4443e3501b0b3d2' }
    @{ Size = '48x48';   Bytes = 3886;  Sha = 'e3ef9792bd6ae434b41f90b5c776f3abcab06a7c672375389d7c7bc1771b44f3' }
    @{ Size = '64x64';   Bytes = 5278;  Sha = '84f497b1791da4ef403ff668a978085201ed3920dacb4806ef8e46684fa70253' }
    @{ Size = '256x256'; Bytes = 19745; Sha = 'eb389b239e475c2177de9d9fad9e8695bde3fda5c77786f8316ad63543762ede' }
)

# Expected window contract, from src-tauri/tauri.conf.json app.windows[0].
$script:ExpectedWidth = 1180
$script:ExpectedHeight = 780
$script:ExpectedMinWidth = 900
$script:ExpectedMinHeight = 600
$script:ExpectedTitle = 'AgentLens'

$script:Results = New-Object System.Collections.ArrayList
$script:Diagnostics = [ordered]@{}
$script:Summary = $null

# Set by the session-0 supervisor when it could NOT hand off to an interactive
# session, so the reason travels into diagnostics.json instead of only into the
# supervisor log. Empty on the interactive child and on a clean handoff.
$script:HandoffNote = ''
$script:HandoffTaskName = 'AgentLensGuiQaInteractive'
$script:SupervisorLog = Join-Path $script:OutDir 'supervisor.log'

# =============================================================================
# THE EXPECTED SET -- why this exists, and what it fixes.
#
# Run h7-20260805T022009Z wrote `verdict: PASS` on a run that died at
# install-directory resolution, BEFORE Start-Process was ever called. The three
# assertions that are the entire reason this QA exists -- the window style bits,
# the window geometry, and the installed-binary icon frame hashes -- produced no
# data at all. They were not recorded as FAIL; they were simply ABSENT, and the
# old verdict logic counted only what was present, so absence read as success.
#
# The defect was structural: a verdict computed purely from the records that
# happen to exist can never notice a record that does not. So the expected set
# is declared here, as DATA, up front. Save-Results diffs executed against
# expected and reports every gap as NOT EXECUTED -- a third verdict distinct
# from both PASS and FAIL -- which forces the overall verdict to INCOMPLETE.
# An aborted run therefore cannot read PASS: PASS now requires every name below
# to have actually run AND passed.
#
# Adding an assertion to the script means adding its name here. That coupling is
# deliberate: an assertion nobody declared is an assertion nobody will notice is
# missing.
# =============================================================================
$script:ExpectedAssertions = @(
    @{ Name = 'webview2.present';           Stage = 'environment'; Why = 'a Tauri window cannot render without the Evergreen Runtime' }
    @{ Name = 'installer.sha256';           Stage = 'artifact';    Why = 'the bytes installed are the bytes CodeBuild produced' }
    @{ Name = 'installer.exit';             Stage = 'install';     Why = 'the NSIS installer reported success' }
    @{ Name = 'install.registry';           Stage = 'install';     Why = 'the uninstall key exists under productName' }
    @{ Name = 'install.directory';          Stage = 'install';     Why = 'the recorded InstallLocation resolves to a real directory' }
    @{ Name = 'install.main_binary';        Stage = 'install';     Why = 'the main binary named by the registry is present on disk' }
    @{ Name = 'icon.frames';                Stage = 'icon';        Why = 'byte-exact icon proof against the installed binary' }
    @{ Name = 'icon.shell_resolves';        Stage = 'icon';        Why = 'the shell icon-resolution path the taskbar and Alt+Tab use yields a real icon from the main binary' }
    @{ Name = 'window.exists';              Stage = 'window';      Why = 'a real HWND -- not a live process -- is what proves a window' }
    @{ Name = 'style.no_native_caption';    Stage = 'window';      Why = 'decorations:false must mean no native title bar is DRAWN: vertical non-client overhead below the runtime SM_CYCAPTION' }
    @{ Name = 'geometry.client';            Stage = 'window';      Why = 'the window is the size tauri.conf.json asks for' }
    @{ Name = 'window.title';               Stage = 'window';      Why = 'the window is the app window and not some other window' }
    @{ Name = 'geometry.min_clamp';         Stage = 'window';      Why = 'the window reports the configured minimum through WM_GETMINMAXINFO, which is what clamps every user sizing operation' }
    @{ Name = 'window.maximize_work_area';  Stage = 'window';      Why = 'maximizing respects the work area and does not cover the taskbar' }

    # ---- REAL INPUT ---------------------------------------------------------
    # Everything above inspects STATE. Not one of those assertions ever sent an
    # input event, so up to this round nothing had ever clicked the three window
    # buttons or dragged the bar: the self-drawn title bar was proven to EXIST
    # and to be the right size, and proven VISIBLE in a console screenshot, and
    # that is all. Visible in a screenshot is not clickable.
    #
    # These five drive the OS input queue with SendInput against the live window
    # and then read the outcome back out of Win32. That is a stronger proof than
    # a synthetic DOM click: the event travels the real
    # SendInput -> USER32 -> WebView2 -> Chromium -> React path, which is exactly
    # where a hand-drawn title bar breaks -- a mispositioned control, a dead
    # pointer-events region, a drag region that never reaches `start_dragging`
    # all look perfect in a screenshot and do nothing when clicked.
    #
    # They need a real desktop AND a real input queue, so like every other
    # window assertion they are NOT EXECUTED (never PASS) without an HWND.
    @{ Name = 'input.drag_move';            Stage = 'input';       Why = 'a real press-move-release inside data-tauri-drag-region moves the window, which is the only proof start_dragging is wired to the bar' }
    @{ Name = 'input.doubleclick_maximize'; Stage = 'input';       Why = 'a real double-click on the drag region toggles maximize through internal_toggle_maximize, and toggles back' }
    @{ Name = 'input.button_minimize';      Stage = 'input';       Why = 'a real click on the computed minimize centre makes the window iconic, so that button is not merely drawn' }
    @{ Name = 'input.button_maximize';      Stage = 'input';       Why = 'a real click on the computed maximize centre zooms the window, and a second click restores it' }
    @{ Name = 'input.button_close';         Stage = 'input';       Why = 'a real click on the computed close centre reaches the CloseRequested handler, which hides the window to the tray and keeps the process alive' }
)
# ---- three assertions were RE-SCOPED, not relaxed ---------------------------
# Run h7-20260805T064522Z reported 17 pass / 3 fail. All three failures were
# defects in the TEST, not in AgentLens, and each was replaced by an assertion
# that actually exercises the mechanism it claims to:
#
#   style.WS_CAPTION      -> style.no_native_caption
#       The premise was false. Tauri v2 undecorated windows KEEP WS_CAPTION and
#       erase the non-client area in WM_NCCALCSIZE, which is what preserves Aero
#       Snap, the resize borders, the drop shadow and the rounded corners. The
#       measurement that distinguishes "no title bar" from "title bar" is the
#       vertical non-client overhead, compared against the runtime SM_CYCAPTION.
#
#   icon.handle           -> icon.shell_resolves
#       A window that never calls WM_SETICON and registers no class icon reports
#       NULL, and Windows then falls back to the EXECUTABLE's icon for the
#       taskbar and Alt+Tab. NULL was therefore not a defect. The thing that
#       matters is that the shell's own extraction path resolves an icon out of
#       the installed binary; the window's NULL slots are kept as INFO evidence.
#
#   geometry.min_clamp    (same name, real mechanism)
#       SetWindowPos does NOT send WM_GETMINMAXINFO, and tao enforces
#       minWidth/minHeight there, so the old probe could never trigger the clamp
#       -- it proved nothing whether it passed or failed. It now queries
#       WM_GETMINMAXINFO directly and reads ptMinTrackSize, which is the value
#       the system uses to clamp every user sizing operation.

# ---- input.button_close asserts HIDE, not EXIT, and that is not a relaxation -
# The obvious assertion for a close button is "the process exits". On AgentLens
# that assertion would be FALSE BY CONSTRUCTION and would manufacture a defect,
# in exactly the way the discarded style.WS_CAPTION premise did.
#
# src-tauri/src/tray.rs::handle_window_event intercepts WindowEvent::CloseRequested
# for the main window, calls api.prevent_close() and then window.hide(): closing
# the main window HIDES it to the resident tray icon and deliberately keeps the
# webview alive. The module header says so, and the only paths that really
# terminate the process are the tray "quit" menu item and the debug-only
# tray::test_quit command, which does not exist in this release build.
#
# So the mechanism a real close click must demonstrate is: the click reaches
# React -> appWindow.close() -> IPC -> CloseRequested -> prevent_close + hide.
# The observable is IsWindowVisible flipping to false WHILE the process stays
# alive. Both halves are asserted: a click that hit nothing leaves the window
# visible (FAIL), and a build that ignored prevent_close would die (also FAIL).

# The per-frame icon assertions are derived from the same data that drives the
# comparison, so the two cannot drift apart.
foreach ($f in $script:ExpectedIconFrames) {
    $script:ExpectedAssertions += @{ Name = "icon.frame.$($f.Size)"; Stage = 'icon'; Why = 'one RT_ICON payload matches its known digest byte for byte' }
}

# Verdicts an assertion record may carry. NOT EXECUTED is the one this round
# added: it means no measurement was taken, which is neither a pass nor a
# failure of the thing under test, and must never be silently folded into either.
$script:VerdictNotExecuted = 'NOT EXECUTED'

# --- logging -----------------------------------------------------------------

function Write-Note {
    param([string]$Message)
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $line = "[$stamp] $Message"
    Write-Host $line
    if (Test-Path -LiteralPath $script:OutDir) {
        Add-Content -LiteralPath $script:LogPath -Value $line -Encoding ASCII
    }
}

function Write-Section {
    param([string]$Title)
    Write-Note ''
    Write-Note "=== $Title ==="
}

function Stop-WithError {
    param([string]$Message, [string]$Detail = '')
    Write-Host "error: $Message" -ForegroundColor Red
    if ($Detail) {
        foreach ($l in ($Detail -split "`n")) { Write-Host "    $l" -ForegroundColor Red }
    }
    if (Test-Path -LiteralPath $script:OutDir) {
        Add-Content -LiteralPath $script:LogPath -Value "error: $Message" -Encoding ASCII
        if ($Detail) { Add-Content -LiteralPath $script:LogPath -Value $Detail -Encoding ASCII }
    }
    Save-Diagnostics
    Save-Results
    Send-OutputToS3
    exit 1
}

# --- assertion recording -----------------------------------------------------

function Add-Assertion {
    param(
        [string]$Name,
        [string]$Class,
        $Expected,
        $Observed,
        [string]$Verdict,
        [string]$Note = ''
    )
    $null = $script:Results.Add([ordered]@{
        name     = $Name
        class    = $Class
        expected = "$Expected"
        observed = "$Observed"
        verdict  = $Verdict
        note     = $Note
    })
    $tag = switch ($Verdict) {
        'PASS' { 'PASS' }
        'FAIL' { 'FAIL' }
        'NOT EXECUTED' { 'SKIP' }
        default { 'INFO' }
    }
    Write-Note ("  [{0}] {1}: expected={2} observed={3}{4}" -f $tag, $Name, $Expected, $Observed, $(if ($Note) { "  ({0})" -f $Note } else { '' }))
}

# Records a machine-checkable assertion that DELIBERATELY did not run, with the
# reason. This is not a courtesy: "we could not measure this" and "we measured
# this and it was wrong" are different claims about the product, and collapsing
# the first into the second manufactures a false accusation against the app,
# exactly as collapsing it into PASS manufactures a false clean bill of health.
# Anything NOT EXECUTED forces the overall verdict to INCOMPLETE.
function Add-NotExecuted {
    param([string]$Name, [string]$Reason, $Expected = 'a measurement', $Observed = 'no measurement was taken')
    Add-Assertion -Name $Name -Class 'machine-checkable' -Expected $Expected -Observed $Observed `
        -Verdict $script:VerdictNotExecuted -Note $Reason
}

function Test-Assertion {
    param([string]$Name, $Expected, $Observed, [string]$Note = '')
    $verdict = if ("$Expected" -eq "$Observed") { 'PASS' } else { 'FAIL' }
    Add-Assertion -Name $Name -Class 'machine-checkable' -Expected $Expected -Observed $Observed -Verdict $verdict -Note $Note
    return ($verdict -eq 'PASS')
}

# --- Win32 interop -----------------------------------------------------------
# GetWindowLongPtr is used on 64-bit: GetWindowLong truncates a pointer-sized
# value to 32 bits, which is harmless for GWL_STYLE but wrong for the GCLP_*
# class longs, so the pointer-safe entry points are used throughout for
# consistency rather than mixing the two.

function Initialize-Interop {
    if ('AgentLensQa.Win32' -as [type]) { return }
    $source = @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

namespace AgentLensQa {
    // MOUSEINPUT / INPUT, the payload SendInput takes. Declared sequentially and
    // never packed: the native INPUT is a DWORD type followed by a union whose
    // largest member is MOUSEINPUT, and because MOUSEINPUT ends in a pointer its
    // alignment is 8 on x64. Sequential layout with default packing therefore
    // reproduces the native offsets exactly -- 40 bytes on x64, 28 on x86 -- and
    // SendInput rejects any other cbSize, so a wrong layout fails loudly rather
    // than silently injecting garbage. The size is logged by the caller.
    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT {
        public uint type;
        public MOUSEINPUT mi;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int x, y; }

    // MINMAXINFO, the structure WM_GETMINMAXINFO fills in. ptMinTrackSize is the
    // only field this QA reads: it is the smallest OUTER window size the system
    // will allow any user sizing operation to produce, and it is where tao (and
    // therefore Tauri) publishes tauri.conf.json's minWidth/minHeight.
    [StructLayout(LayoutKind.Sequential)]
    public struct MINMAXINFO {
        public POINT ptReserved;
        public POINT ptMaxSize;
        public POINT ptMaxPosition;
        public POINT ptMinTrackSize;
        public POINT ptMaxTrackSize;
    }

    // DEVMODEW, needed to ask the display driver to widen the desktop. Only the
    // dm* fields this QA sets are named; the rest is padding kept at the exact
    // documented offsets, because ChangeDisplaySettingsExW validates dmSize
    // against the real structure size and rejects a short one.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct DEVMODE {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public ushort dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
        public uint dmFields;
        public int dmPositionX, dmPositionY;
        public uint dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public ushort dmLogPixels;
        public uint dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public uint dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
        public uint dmPanningWidth, dmPanningHeight;
    }

    public static class Win32 {
        public const int GWL_STYLE = -16;
        public const int GWL_EXSTYLE = -20;
        public const int GCLP_HICON = -14;
        public const int GCLP_HICONSM = -34;
        public const uint WS_CAPTION = 0x00C00000;
        public const uint WS_THICKFRAME = 0x00040000;
        public const uint WS_SYSMENU = 0x00080000;
        public const uint WS_MINIMIZEBOX = 0x00020000;
        public const uint WS_MAXIMIZEBOX = 0x00010000;
        public const uint WS_POPUP = 0x80000000;
        public const uint WS_BORDER = 0x00800000;
        public const uint WS_DLGFRAME = 0x00400000;
        public const int SW_MAXIMIZE = 3;
        public const int SW_RESTORE = 9;
        public const int SW_SHOW = 5;
        public const uint SWP_NOZORDER = 0x0004;
        public const uint SWP_NOACTIVATE = 0x0010;
        public const uint SPI_GETWORKAREA = 0x0030;
        public const uint WM_GETICON = 0x007F;
        public const uint WM_GETMINMAXINFO = 0x0024;
        public const int ICON_SMALL = 0;
        public const int ICON_BIG = 1;
        public const uint SMTO_ABORTIFHUNG = 0x0002;
        public const uint RT_ICON = 3;
        public const uint RT_GROUP_ICON = 14;
        public const uint LOAD_LIBRARY_AS_DATAFILE = 0x00000002;

        // System metrics that describe what a REAL native window frame costs on
        // this host at this DPI. Queried at run time and never hardcoded: the
        // caption height differs between Windows versions, themes and DPI, so a
        // literal (31, or anything else) would be a guess that silently rots.
        public const int SM_CYCAPTION = 4;
        public const int SM_CXBORDER = 5;
        public const int SM_CYBORDER = 6;
        public const int SM_CXSIZEFRAME = 32;
        public const int SM_CYSIZEFRAME = 33;
        public const int SM_CXPADDEDBORDER = 92;

        // Real-input constants. INPUT_MOUSE plus the three mouse flags this QA
        // uses. MOUSEEVENTF_ABSOLUTE coordinates are normalized to 0..65535 across
        // the PRIMARY display, which is why the mapping is done in code below and
        // then verified against GetCursorPos instead of being trusted.
        public const uint INPUT_MOUSE = 0;
        public const uint MOUSEEVENTF_MOVE = 0x0001;
        public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
        public const uint MOUSEEVENTF_LEFTUP = 0x0004;
        public const uint MOUSEEVENTF_ABSOLUTE = 0x8000;
        public const int LOGPIXELSX = 88;
        public const int LOGPIXELSY = 90;

        public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
        public delegate bool EnumResNameProc(IntPtr hModule, IntPtr lpType, IntPtr lpName, IntPtr lParam);

        [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
        [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hWnd);
        [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
        [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr GetClassLongPtr(IntPtr hWnd, int nIndex);
        [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
        [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
        [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
        [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder s, int max);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder s, int max);
        [DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint action, uint param, ref RECT data, uint winIni);
        [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
        [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
        [DllImport("user32.dll", SetLastError = true)] public static extern bool AdjustWindowRectEx(ref RECT r, uint style, bool menu, uint exStyle);
        [DllImport("user32.dll", SetLastError = true)] public static extern bool DestroyIcon(IntPtr icon);

        // --- real input -----------------------------------------------------
        // SendInput is the only supported way to put an event on the OS input
        // queue as if hardware produced it. mouse_event is the deprecated
        // predecessor and keybd_event's sibling; it is not used here because it
        // cannot report how many events were actually accepted, and "the call
        // returned" is precisely the kind of non-proof this QA exists to reject.
        [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
        [DllImport("user32.dll", SetLastError = true)] public static extern bool GetCursorPos(out POINT p);
        [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
        [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] public static extern IntPtr SetActiveWindow(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
        [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
        [DllImport("user32.dll")] public static extern uint GetDoubleClickTime();
        // GetDpiForWindow is Windows 10 1607 and later. It is called through a
        // guarded wrapper so an older host degrades to the desktop DC's
        // LOGPIXELSX instead of taking the whole script down with an
        // EntryPointNotFoundException.
        [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hWnd, IntPtr hdc);
        [DllImport("gdi32.dll")] public static extern int GetDeviceCaps(IntPtr hdc, int index);
        // Two bindings for one export. ExtractIconExW with nIndex -1 returns the
        // icon-group COUNT and documents phicon* as NULL for that call, so the
        // count probe needs an IntPtr signature; the extraction needs out params.
        [DllImport("shell32.dll", EntryPoint = "ExtractIconExW", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern uint ExtractIconCountW(string file, int index, IntPtr large, IntPtr small, uint count);
        [DllImport("shell32.dll", EntryPoint = "ExtractIconExW", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern uint ExtractIconExW(string file, int index, out IntPtr large, out IntPtr small, uint count);

        public const int ENUM_CURRENT_SETTINGS = -1;
        public const uint DM_PELSWIDTH = 0x00080000;
        public const uint DM_PELSHEIGHT = 0x00100000;
        public const uint DM_BITSPERPEL = 0x00040000;
        public const uint CDS_UPDATEREGISTRY = 0x00000001;
        public const uint CDS_TEST = 0x00000002;
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettingsW(string device, int mode, ref DEVMODE dm);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettingsExW(string device, ref DEVMODE dm, IntPtr hwnd, uint flags, IntPtr param);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern IntPtr LoadLibraryExW(string file, IntPtr hFile, uint flags);
        [DllImport("kernel32.dll", SetLastError = true)] public static extern bool FreeLibrary(IntPtr h);
        [DllImport("kernel32.dll", SetLastError = true)] public static extern bool EnumResourceNamesW(IntPtr hModule, IntPtr lpType, EnumResNameProc cb, IntPtr lParam);
        [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr FindResourceW(IntPtr hModule, IntPtr lpName, IntPtr lpType);
        [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr LoadResource(IntPtr hModule, IntPtr hResInfo);
        [DllImport("kernel32.dll")] public static extern IntPtr LockResource(IntPtr hResData);
        [DllImport("kernel32.dll", SetLastError = true)] public static extern uint SizeofResource(IntPtr hModule, IntPtr hResInfo);

        // Finds the visible, top-level windows owned by a pid. Deliberately
        // matches on pid and not on window title: matching a title would
        // happily find some other window that merely says "AgentLens", which
        // is precisely the false positive this QA must not produce.
        public static List<IntPtr> TopLevelWindowsForPid(uint target) {
            List<IntPtr> found = new List<IntPtr>();
            EnumWindows(delegate(IntPtr h, IntPtr l) {
                uint pid;
                GetWindowThreadProcessId(h, out pid);
                if (pid == target && IsWindowVisible(h) && GetParent(h) == IntPtr.Zero) {
                    found.Add(h);
                }
                return true;
            }, IntPtr.Zero);
            return found;
        }

        // EVERY top-level window owned by the pid, visible or not, as tab
        // separated "hwnd, visible, class, title, WxH". The caller needs the
        // class name because a Tauri process owns helper windows that are NOT
        // the app window -- the tray-icon crate's "tray_icon_app" and tao's
        // "Tao Thread Event Target" -- and picking one of those up would be a
        // false positive of exactly the kind this QA exists to prevent. The
        // invisible ones are returned too so the log can show what WAS there
        // when no app window was found, instead of just saying "nothing".
        public static List<string> DescribeTopLevelWindowsForPid(uint target) {
            List<string> found = new List<string>();
            EnumWindows(delegate(IntPtr h, IntPtr l) {
                uint pid;
                GetWindowThreadProcessId(h, out pid);
                if (pid != target || GetParent(h) != IntPtr.Zero) { return true; }
                StringBuilder cls = new StringBuilder(256);
                GetClassNameW(h, cls, cls.Capacity);
                StringBuilder txt = new StringBuilder(512);
                GetWindowTextW(h, txt, txt.Capacity);
                RECT r;
                GetWindowRect(h, out r);
                found.Add(string.Format("0x{0:X}\t{1}\t{2}\t{3}\t{4}x{5}",
                    h.ToInt64(), IsWindowVisible(h) ? "visible" : "hidden",
                    cls.ToString(), txt.ToString(), r.Right - r.Left, r.Bottom - r.Top));
                return true;
            }, IntPtr.Zero);
            return found;
        }

        // Reads every RT_ICON payload out of a PE file, byte for byte, without
        // executing it (LOAD_LIBRARY_AS_DATAFILE). These are the same bytes the
        // .ico frames were compiled from, so they can be hashed and compared
        // against known-good digests.
        public static List<byte[]> ReadIconResources(string path) {
            List<byte[]> blobs = new List<byte[]>();
            IntPtr mod = LoadLibraryExW(path, IntPtr.Zero, LOAD_LIBRARY_AS_DATAFILE);
            if (mod == IntPtr.Zero) {
                throw new Exception("LoadLibraryExW failed for " + path + " (win32 " + Marshal.GetLastWin32Error() + ")");
            }
            try {
                List<IntPtr> names = new List<IntPtr>();
                EnumResourceNamesW(mod, new IntPtr(RT_ICON), delegate(IntPtr m, IntPtr t, IntPtr n, IntPtr l) {
                    names.Add(n);
                    return true;
                }, IntPtr.Zero);
                foreach (IntPtr n in names) {
                    IntPtr info = FindResourceW(mod, n, new IntPtr(RT_ICON));
                    if (info == IntPtr.Zero) { continue; }
                    uint size = SizeofResource(mod, info);
                    IntPtr data = LoadResource(mod, info);
                    if (data == IntPtr.Zero || size == 0) { continue; }
                    IntPtr p = LockResource(data);
                    if (p == IntPtr.Zero) { continue; }
                    byte[] buf = new byte[size];
                    Marshal.Copy(p, buf, 0, (int)size);
                    blobs.Add(buf);
                }
            } finally {
                FreeLibrary(mod);
            }
            return blobs;
        }

        // Every display mode the driver advertises, as "WxHxBPP". Enumerating
        // is strictly better than guessing a resolution: the EC2 emulated
        // display adapter offers a short, fixed mode list, and asking for a
        // mode that is not on it fails with DISP_CHANGE_BADMODE while a
        // perfectly good larger mode sits unused. From session 0 the
        // enumeration returns nothing at all, which is itself the answer.
        public static List<string> ListDisplayModes() {
            List<string> modes = new List<string>();
            DEVMODE dm = new DEVMODE();
            dm.dmDeviceName = "";
            dm.dmFormName = "";
            dm.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODE));
            for (int i = 0; i < 1024; i++) {
                if (!EnumDisplaySettingsW(null, i, ref dm)) { break; }
                modes.Add(dm.dmPelsWidth + "x" + dm.dmPelsHeight + "x" + dm.dmBitsPerPel);
            }
            return modes;
        }

        // Raises the desktop resolution, done entirely in C# on purpose.
        //
        // Run h7-20260805T060925Z proved the PowerShell path could not do it. In
        // the INTERACTIVE session, EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS)
        // still failed, while indexed enumeration of the very same adapter
        // returned 14 modes including 1920x1080x32. Seeding a DEVMODE from a
        // failed ENUM_CURRENT_SETTINGS call leaves it not describing any real
        // mode, so the change was never even attempted and the desktop stayed
        // 1024x768 -- which then clamped the app window to 1028 client px and
        // turned geometry.client into a measurement of the framebuffer instead
        // of a measurement of the app.
        //
        // Seeding from an INDEXED enumeration fixes that: it yields a fully
        // populated, driver-supplied DEVMODE for a mode the driver has just
        // said it supports, which is what ChangeDisplaySettingsExW wants.
        // Largest area first, since the console screenshot has to contain the
        // whole window frame. CDS_TEST gates every candidate.
        public static string RaiseDisplayMode(int minW, int minH) {
            List<string> notes = new List<string>();
            List<DEVMODE> fits = new List<DEVMODE>();
            DEVMODE probe = new DEVMODE();
            probe.dmDeviceName = "";
            probe.dmFormName = "";
            probe.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODE));
            for (int i = 0; i < 1024; i++) {
                if (!EnumDisplaySettingsW(null, i, ref probe)) { break; }
                if (probe.dmPelsWidth < (uint)minW || probe.dmPelsHeight < (uint)minH) { continue; }
                if (probe.dmBitsPerPel < 32) { continue; }
                fits.Add(probe);
            }
            if (fits.Count == 0) {
                return "no enumerated mode is at least " + minW + "x" + minH + " at 32bpp";
            }
            fits.Sort(delegate(DEVMODE a, DEVMODE b) {
                long aa = (long)a.dmPelsWidth * a.dmPelsHeight;
                long bb = (long)b.dmPelsWidth * b.dmPelsHeight;
                return bb.CompareTo(aa);
            });
            foreach (DEVMODE m in fits) {
                DEVMODE dm = m;
                string label = dm.dmPelsWidth + "x" + dm.dmPelsHeight + "x" + dm.dmBitsPerPel;
                dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_BITSPERPEL;
                int test = ChangeDisplaySettingsExW(null, ref dm, IntPtr.Zero, CDS_TEST, IntPtr.Zero);
                if (test != 0) {
                    notes.Add(label + " rejected by CDS_TEST (code " + test + ")");
                    continue;
                }
                int apply = ChangeDisplaySettingsExW(null, ref dm, IntPtr.Zero, CDS_UPDATEREGISTRY, IntPtr.Zero);
                notes.Add(label + " applied -> ChangeDisplaySettingsExW code " + apply);
                if (apply == 0) { break; }
            }
            return string.Join("; ", notes.ToArray());
        }

        // Asks the WINDOW ITSELF what its minimum size is, by sending
        // WM_GETMINMAXINFO and reading ptMinTrackSize back out.
        //
        // This replaces a probe that could not work. The previous test called
        // SetWindowPos(400,300) and expected the window to come back clamped to
        // 900x600. SetWindowPos does NOT send WM_GETMINMAXINFO -- the system
        // sends it while TRACKING a size (a user drag, a maximize), not for a
        // programmatic move -- and tao enforces min_inner_size in exactly that
        // handler. So SetWindowPos honoured 400x300, and the assertion was
        // measuring nothing about the clamp: it would have "failed" identically
        // on a correct build and on a build with no minimum at all.
        //
        // ptMinTrackSize is the authoritative value. It is what the system
        // consults to clamp every user sizing operation, so a correct value here
        // IS the enforcement. Note it is an OUTER window size: tao runs the
        // configured client minimum through AdjustWindowRectEx before publishing
        // it, so ptMinTrackSize is >= the configured client minimum by the frame
        // overhead. The caller reports both.
        //
        // The struct is zeroed before the send so a window that ignores the
        // message is distinguishable from one that answers: 0x0 back means
        // nothing published a minimum, which is a real finding, not a no-op.
        public static string QueryMinTrackSize(IntPtr hWnd) {
            int size = Marshal.SizeOf(typeof(MINMAXINFO));
            IntPtr buf = Marshal.AllocHGlobal(size);
            try {
                for (int i = 0; i < size; i++) { Marshal.WriteByte(buf, i, 0); }
                IntPtr res;
                IntPtr ok = SendMessageTimeout(hWnd, WM_GETMINMAXINFO, IntPtr.Zero, buf,
                    SMTO_ABORTIFHUNG, 5000, out res);
                if (ok == IntPtr.Zero) {
                    return "unanswered:SendMessageTimeout(WM_GETMINMAXINFO) failed or timed out (win32 "
                        + Marshal.GetLastWin32Error() + ")";
                }
                MINMAXINFO got = (MINMAXINFO)Marshal.PtrToStructure(buf, typeof(MINMAXINFO));
                return "ok:" + got.ptMinTrackSize.x + "x" + got.ptMinTrackSize.y
                    + ":maxtrack=" + got.ptMaxTrackSize.x + "x" + got.ptMaxTrackSize.y;
            } finally {
                Marshal.FreeHGlobal(buf);
            }
        }

        // Runs the shell's own icon-resolution path against the installed
        // executable, which is what the taskbar and Alt+Tab fall back to when a
        // window carries no icon of its own. Returns the icon-group count and the
        // pixel size of the large and small icons the shell handed back, so the
        // record says what was resolved rather than just "non-NULL".
        //
        // Handles are destroyed here: ExtractIconExW transfers ownership, and a
        // QA run that leaked two icons per invocation would be its own small bug.
        public static string ResolveExecutableIcon(string path) {
            uint groups = ExtractIconCountW(path, -1, IntPtr.Zero, IntPtr.Zero, 0);
            IntPtr large = IntPtr.Zero;
            IntPtr small = IntPtr.Zero;
            uint got = ExtractIconExW(path, 0, out large, out small, 1);
            string largeDesc = "0x0";
            string smallDesc = "0x0";
            try {
                if (large != IntPtr.Zero) {
                    using (Icon ic = Icon.FromHandle(large)) {
                        largeDesc = "0x" + large.ToInt64().ToString("X") + " " + ic.Width + "x" + ic.Height;
                    }
                }
                if (small != IntPtr.Zero) {
                    using (Icon ic = Icon.FromHandle(small)) {
                        smallDesc = "0x" + small.ToInt64().ToString("X") + " " + ic.Width + "x" + ic.Height;
                    }
                }
            } finally {
                if (large != IntPtr.Zero) { DestroyIcon(large); }
                if (small != IntPtr.Zero) { DestroyIcon(small); }
            }
            return "groups=" + groups + " extracted=" + got + " large=" + largeDesc + " small=" + smallDesc;
        }

        // --- real input, driven through the OS input queue -------------------
        //
        // Everything in this block exists so a PowerShell caller never has to
        // marshal an INPUT array by hand, and -- more importantly -- so every
        // synthetic event is FOLLOWED BY A READBACK. "SendInput returned 1" is
        // not evidence that the pointer is where it was asked to be, and a click
        // at the wrong place is indistinguishable from a dead button unless the
        // landing position is recorded. So each helper returns a string naming
        // what was requested and what actually happened, and the caller writes
        // both into the assertion record.

        // Maps a device pixel to the 0..65535 normalized space MOUSEEVENTF_ABSOLUTE
        // uses. The system inverts this with a truncating divide by the screen
        // span, so the rounding here can leave the cursor one pixel off; that is
        // reported rather than silently corrected, because the targets are 44 px
        // wide and a 1 px residual is irrelevant while a large one is a finding.
        private static int Normalize(int v, int span) {
            if (span <= 1) { return 0; }
            return (int)((((double)v * 65535.0) / (double)(span - 1)) + 0.5);
        }

        private static uint SendMouse(uint flags, int nx, int ny) {
            INPUT[] buf = new INPUT[1];
            buf[0].type = INPUT_MOUSE;
            buf[0].mi.dx = nx;
            buf[0].mi.dy = ny;
            buf[0].mi.mouseData = 0;
            buf[0].mi.dwFlags = flags;
            buf[0].mi.time = 0;
            buf[0].mi.dwExtraInfo = IntPtr.Zero;
            return SendInput(1, buf, Marshal.SizeOf(typeof(INPUT)));
        }

        public static int InputStructSize() {
            return Marshal.SizeOf(typeof(INPUT));
        }

        // Moves the pointer to an absolute screen pixel and reports where it
        // really landed. The drift field is the whole point of the readback.
        public static string MoveTo(int x, int y) {
            int sw = GetSystemMetrics(0);
            int sh = GetSystemMetrics(1);
            uint sent = SendMouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, Normalize(x, sw), Normalize(y, sh));
            POINT got;
            GetCursorPos(out got);
            int drift = Math.Abs(got.x - x) + Math.Abs(got.y - y);
            return "sent=" + sent + " requested=" + x + "," + y + " effective=" + got.x + "," + got.y + " drift=" + drift;
        }

        public static string CursorPos() {
            POINT got;
            if (!GetCursorPos(out got)) { return "unavailable (win32 " + Marshal.GetLastWin32Error() + ")"; }
            return got.x + "," + got.y;
        }

        // One or two clicks at an absolute screen pixel, with the pointer parked
        // there first so the click cannot be delivered somewhere else. `clicks`
        // is 2 for the double-click case; the gap is kept well under
        // GetDoubleClickTime and the pointer is NOT moved between the two presses,
        // because the system also requires the second press inside the
        // double-click rectangle before it will raise the click count to 2 --
        // which is what Tauri's injected drag script tests with `e.detail === 2`.
        public static string Click(int x, int y, int clicks, int holdMs, int gapMs) {
            string move = MoveTo(x, y);
            System.Threading.Thread.Sleep(60);
            uint downs = 0;
            uint ups = 0;
            for (int i = 0; i < clicks; i++) {
                if (i > 0) { System.Threading.Thread.Sleep(gapMs); }
                downs += SendMouse(MOUSEEVENTF_LEFTDOWN, 0, 0);
                System.Threading.Thread.Sleep(holdMs);
                ups += SendMouse(MOUSEEVENTF_LEFTUP, 0, 0);
            }
            return move + " clicks=" + clicks + " down_accepted=" + downs + " up_accepted=" + ups
                + " dblclk_time=" + GetDoubleClickTime() + "ms";
        }

        // Makes the app window the foreground window, using the documented
        // AttachThreadInput handshake rather than a bare SetForegroundWindow.
        //
        // Run h7-20260805T083710Z needed this. A bare SetForegroundWindow
        // returned FALSE there and the app window was never activated
        // (foreground stayed 0x301C6 while the target was 0xA0038), because
        // Windows only grants the foreground to a process that already owns it,
        // owns the active window, or is responding to user input -- and this QA
        // runs from a scheduled task that owns none of those. The consequence
        // was not cosmetic: simple button clicks still worked, because a click
        // is delivered to whatever window is under the cursor, but the drag
        // gesture went to an INACTIVE window, which is a different code path and
        // confounds the measurement.
        //
        // Attaching this thread's input queue to the current foreground thread
        // borrows that thread's foreground right for the duration, which is the
        // long-standing supported way for a test harness to activate a window.
        // It changes only the ENVIRONMENT the gesture runs in, never the
        // gesture's outcome: the assertion still passes or fails on whether the
        // window moved.
        public static string ForceForeground(IntPtr hWnd) {
            IntPtr fg = GetForegroundWindow();
            uint fgPid = 0;
            uint fgThread = 0;
            if (fg != IntPtr.Zero) { fgThread = GetWindowThreadProcessId(fg, out fgPid); }
            uint myThread = GetCurrentThreadId();
            bool attached = false;
            if (fgThread != 0 && fgThread != myThread) {
                attached = AttachThreadInput(myThread, fgThread, true);
            }
            ShowWindow(hWnd, SW_SHOW);
            BringWindowToTop(hWnd);
            bool set = SetForegroundWindow(hWnd);
            SetActiveWindow(hWnd);
            if (attached) { AttachThreadInput(myThread, fgThread, false); }
            System.Threading.Thread.Sleep(250);
            IntPtr now = GetForegroundWindow();
            return "attached=" + attached + " SetForegroundWindow=" + set
                + " was=0x" + fg.ToInt64().ToString("X")
                + " now=0x" + now.ToInt64().ToString("X")
                + " target=0x" + hWnd.ToInt64().ToString("X")
                + " match=" + (now == hWnd);
        }

        private static string RectText(IntPtr hWnd) {
            RECT r;
            if (!GetWindowRect(hWnd, out r)) { return "?"; }
            return r.Left + "," + r.Top;
        }

        // Press, travel, release -- the real gesture, not a synthesized
        // WM_NCLBUTTONDOWN.
        //
        // The hold after the press is not padding. A mousedown on a Tauri drag
        // region runs JS -> `window.__TAURI_INTERNALS__.invoke('plugin:window|start_dragging')`
        // -> IPC -> Rust, and only then does the window call ReleaseCapture and
        // hand itself to the system move loop, which anchors on wherever the
        // cursor is AT THAT MOMENT. Moving before the loop exists would either
        // do nothing or shift the anchor, so the assertion would measure the
        // race and not the feature.
        //
        // The travel is stepped for the same reason: the move loop follows mouse
        // MOVEMENT, and one teleport can be coalesced into a single event that
        // arrives before the loop is listening.
        //
        // The window origin is sampled at every stage of the gesture, not only
        // before and after. "The window did not move" and "the window moved and
        // then snapped back on release" are different defects, and a
        // before/after pair alone cannot tell them apart.
        public static string Drag(IntPtr hWnd, int x, int y, int dx, int dy, int settleMs, int steps, int stepMs) {
            if (steps < 1) { steps = 1; }
            string move = MoveTo(x, y);
            System.Threading.Thread.Sleep(80);
            string atPress = RectText(hWnd);
            uint down = SendMouse(MOUSEEVENTF_LEFTDOWN, 0, 0);
            System.Threading.Thread.Sleep(settleMs);
            POINT afterPress;
            GetCursorPos(out afterPress);
            string afterSettle = RectText(hWnd);
            IntPtr fgAtPress = GetForegroundWindow();
            uint moves = 0;
            int sw = GetSystemMetrics(0);
            int sh = GetSystemMetrics(1);
            List<string> trace = new List<string>();
            for (int i = 1; i <= steps; i++) {
                int tx = x + (int)Math.Round(((double)dx * i) / steps);
                int ty = y + (int)Math.Round(((double)dy * i) / steps);
                moves += SendMouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, Normalize(tx, sw), Normalize(ty, sh));
                System.Threading.Thread.Sleep(stepMs);
                trace.Add(RectText(hWnd));
            }
            System.Threading.Thread.Sleep(120);
            POINT beforeRelease;
            GetCursorPos(out beforeRelease);
            string beforeUp = RectText(hWnd);
            uint up = SendMouse(MOUSEEVENTF_LEFTUP, 0, 0);
            System.Threading.Thread.Sleep(200);
            string afterUp = RectText(hWnd);
            return move + " down=" + down + " moves=" + moves + "/" + steps + " up=" + up
                + " anchor=" + afterPress.x + "," + afterPress.y
                + " released_at=" + beforeRelease.x + "," + beforeRelease.y
                + " cursor_delta=" + (beforeRelease.x - afterPress.x) + "," + (beforeRelease.y - afterPress.y)
                + " fg_at_press=0x" + fgAtPress.ToInt64().ToString("X")
                + " origin_at_press=" + atPress
                + " origin_after_settle=" + afterSettle
                + " origin_trace=[" + string.Join(" ", trace.ToArray()) + "]"
                + " origin_before_up=" + beforeUp
                + " origin_after_up=" + afterUp;
        }

        // Resolves the DPI the click geometry must be scaled by, or 0 when the
        // host answers neither way. Returning 0 rather than a silent 96 keeps the
        // caller honest: "assumed 96" and "measured 96" are different claims.
        public static int EffectiveDpi(IntPtr hWnd) {
            try {
                uint d = GetDpiForWindow(hWnd);
                if (d > 0) { return (int)d; }
            } catch (EntryPointNotFoundException) {
                // fall through to the desktop DC
            } catch (Exception) {
                // fall through to the desktop DC
            }
            IntPtr dc = GetDC(IntPtr.Zero);
            if (dc != IntPtr.Zero) {
                int caps = GetDeviceCaps(dc, LOGPIXELSX);
                ReleaseDC(IntPtr.Zero, dc);
                if (caps > 0) { return caps; }
            }
            return 0;
        }

        // The DPI the window is actually being rendered at, plus the desktop DC's
        // logical-pixel density as a cross-check and as the fallback on hosts
        // without GetDpiForWindow. This matters because the title-bar geometry
        // this QA derives its click targets from is expressed in CSS pixels
        // (36 px bar, 44 px buttons); those equal device pixels only at 96 DPI,
        // so the scale factor has to be measured, not assumed.
        public static string DescribeDpi(IntPtr hWnd) {
            uint winDpi = 0;
            string note = "";
            try {
                winDpi = GetDpiForWindow(hWnd);
            } catch (EntryPointNotFoundException) {
                note = " (GetDpiForWindow unavailable on this host)";
            } catch (Exception e) {
                note = " (GetDpiForWindow threw: " + e.GetType().Name + ")";
            }
            int dcDpiX = 0;
            int dcDpiY = 0;
            IntPtr dc = GetDC(IntPtr.Zero);
            if (dc != IntPtr.Zero) {
                dcDpiX = GetDeviceCaps(dc, LOGPIXELSX);
                dcDpiY = GetDeviceCaps(dc, LOGPIXELSY);
                ReleaseDC(IntPtr.Zero, dc);
            }
            return "window_dpi=" + winDpi + " desktop_dpi=" + dcDpiX + "x" + dcDpiY + note;
        }

        // The client area's origin in SCREEN coordinates, together with its size.
        // Every click target in this QA is derived from this pair at the moment of
        // the click and never from a remembered screen number: a drag moves the
        // origin and a maximize changes both, so a cached coordinate would aim at
        // where the button used to be and then blame the app for not responding.
        public static string ClientBoxOnScreen(IntPtr hWnd) {
            RECT c;
            if (!GetClientRect(hWnd, out c)) {
                return "error:GetClientRect failed (win32 " + Marshal.GetLastWin32Error() + ")";
            }
            POINT origin;
            origin.x = 0;
            origin.y = 0;
            if (!ClientToScreen(hWnd, ref origin)) {
                return "error:ClientToScreen failed (win32 " + Marshal.GetLastWin32Error() + ")";
            }
            return "ok:" + origin.x + ":" + origin.y + ":" + (c.Right - c.Left) + ":" + (c.Bottom - c.Top);
        }

        // Best effort only. From session 0 this returns a black frame or
        // throws; both outcomes are recorded rather than treated as fatal.
        public static string CaptureScreen(string outPath) {
            int w = GetSystemMetrics(0);
            int h = GetSystemMetrics(1);
            if (w <= 0 || h <= 0) { return "no screen metrics (w=" + w + " h=" + h + ")"; }
            using (Bitmap bmp = new Bitmap(w, h)) {
                using (Graphics g = Graphics.FromImage(bmp)) {
                    g.CopyFromScreen(0, 0, 0, 0, new Size(w, h));
                }
                bmp.Save(outPath, ImageFormat.Png);
            }
            return "ok " + w + "x" + h;
        }
    }
}
'@
    Add-Type -TypeDefinition $source -ReferencedAssemblies 'System.Drawing' | Out-Null
}

# --- helpers -----------------------------------------------------------------

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sha256OfBytes {
    param([byte[]]$Bytes)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash($Bytes) | ForEach-Object { $_.ToString('x2') }) -join '')
    } finally {
        $sha.Dispose()
    }
}

function Invoke-Native {
    param(
        [string]$FilePath,
        [string[]]$Arguments = @(),
        [int]$TimeoutSec = 900,
        [string]$What = ''
    )
    $label = if ($What) { $What } else { $FilePath }
    Write-Note "run: $FilePath $($Arguments -join ' ')"
    $p = $null
    if ($Arguments.Count -gt 0) {
        $p = Start-Process -FilePath $FilePath -ArgumentList $Arguments -PassThru -Wait -NoNewWindow
    } else {
        $p = Start-Process -FilePath $FilePath -PassThru -Wait -NoNewWindow
    }
    # Start-Process -Wait already blocked, but .ExitCode can be unpopulated on
    # some hosts until the object refreshes; WaitForExit is cheap insurance.
    $p.WaitForExit($TimeoutSec * 1000) | Out-Null
    $code = $p.ExitCode
    Write-Note "exit: $label -> $code"
    return $code
}

function Assert-AwsPowerShell {
    if ($script:AwsPsModule) { return }
    foreach ($m in @('AWSPowerShell', 'AWSPowerShell.NetCore', 'AWS.Tools.S3')) {
        if (Get-Module -ListAvailable -Name $m) {
            Import-Module $m -ErrorAction SilentlyContinue
            break
        }
    }
    $missing = @()
    foreach ($c in @('Read-S3Object', 'Write-S3Object')) {
        if (-not (Get-Command $c -ErrorAction SilentlyContinue)) { $missing += $c }
    }
    if ($missing.Count -gt 0) {
        $have = (Get-Module -ListAvailable |
            Where-Object { $_.Name -like 'AWS*' } |
            ForEach-Object { "$($_.Name) $($_.Version)" }) -join ', '
        if (-not $have) { $have = '(no AWS* modules found at all)' }
        $script:AwsPsModule = 'unavailable'
        Stop-WithError 'the AWSPowerShell S3 cmdlets are unavailable' @"
missing: $($missing -join ', ')
AWS modules present: $have
This AMI has no aws CLI either, so there is no S3 transport left. This is an
ENVIRONMENT defect, not an AgentLens defect: nothing about the application has
been tested at this point.
"@
    }
    $cmd = Get-Command Read-S3Object
    $script:AwsPsModule = "$($cmd.Module.Name) $($cmd.Module.Version)"
    $script:Diagnostics['aws_powershell'] = $script:AwsPsModule
    $script:Diagnostics['aws_region'] = $script:Region
    Write-Note "aws sdk   : $script:AwsPsModule (region $script:Region)"
}

function Get-Wv2Version {
    foreach ($k in @($script:Wv2HklmWow, $script:Wv2Hklm, $script:Wv2Hkcu)) {
        if (-not (Test-Path -LiteralPath $k)) { continue }
        $item = Get-ItemProperty -LiteralPath $k -ErrorAction SilentlyContinue
        if ($null -eq $item) { continue }
        if (-not ($item.PSObject.Properties.Name -contains 'pv')) { continue }
        $pv = "$($item.pv)".Trim()
        if ($pv -and $pv -ne '0.0.0.0') { return @{ Hive = $k; Version = $pv } }
    }
    return $null
}

# --- steps -------------------------------------------------------------------

function Initialize-Workspace {
    New-Item -ItemType Directory -Force -Path $script:WorkDir | Out-Null
    New-Item -ItemType Directory -Force -Path $script:OutDir | Out-Null
    Set-Content -LiteralPath $script:LogPath -Value '' -Encoding ASCII
    Write-Note "AgentLens Windows GUI QA -- run id $script:RunId"
    Write-Note "workdir $script:WorkDir"
}

function Write-PreFlight {
    Write-Section 'pre-flight'
    $os = Get-CimInstance Win32_OperatingSystem
    $who = "$env:USERDOMAIN\$env:USERNAME"
    $sessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    $script:Diagnostics['os_caption'] = $os.Caption
    $script:Diagnostics['os_version'] = $os.Version
    $script:Diagnostics['identity'] = $who
    $script:Diagnostics['session_id'] = $sessionId
    $script:Diagnostics['powershell'] = $PSVersionTable.PSVersion.ToString()
    Write-Note "os        : $($os.Caption) ($($os.Version))"
    Write-Note "identity  : $who"
    Write-Note "session   : $sessionId"
    Write-Note "powershell: $($PSVersionTable.PSVersion)"

    # Recording the session id makes the black-screenshot outcome
    # self-explaining in the artifacts instead of a mystery.
    if ($sessionId -eq 0) {
        Write-Note 'note: running in session 0 -- no interactive desktop, no DWM.'
        Write-Note '      In-guest screenshots are expected to be black or to fail.'
        Write-Note '      The authoritative visual channel is ec2 get-console-screenshot.'
    } else {
        Write-Note "note: running in INTERACTIVE session $sessionId -- there is a real desktop here."
        Write-Note '      The window assertions below are therefore measurable, and a missing'
        Write-Note '      window in this session is an APP defect, not an environment limit.'
    }
    if (-not $script:HandoffNote) {
        $script:HandoffNote = if ($sessionId -eq 0) {
            'ran directly in session 0'
        } else {
            "measured from interactive session $sessionId"
        }
    }
    $script:Diagnostics['interactive_handoff'] = $script:HandoffNote

    # Informational, not part of the expected set: it changes no verdict, it just
    # makes the single most important fact about a run readable without having to
    # reason about which branch of Wait-ForWindow was taken.
    Add-Assertion -Name 'session.interactive' -Class 'informational' `
        -Expected 'session id != 0, so a WebView2 window can be composited' `
        -Observed "session $sessionId" -Verdict 'INFO' `
        -Note $(if ($sessionId -eq 0) {
            'Session 0 is the non-interactive service window station Service-0x0-3e7$. The window assertions cannot be measured here and are recorded NOT EXECUTED.'
        } else {
            'Interactive session: the window assertions are measurable, and no window here would be a real FAIL against AgentLens.'
        })

    Initialize-Interop
    $w = [AgentLensQa.Win32]::GetSystemMetrics(0)
    $h = [AgentLensQa.Win32]::GetSystemMetrics(1)
    $script:Diagnostics['screen_before'] = "${w}x${h}"
    Write-Note "screen before: ${w}x${h}"

    $modes = @()
    try { $modes = @([AgentLensQa.Win32]::ListDisplayModes()) } catch { $modes = @() }
    $script:Diagnostics['display_modes'] = if ($modes.Count -gt 0) { $modes -join ',' } else { 'none advertised (no display driver reachable from this session)' }
    Write-Note "display modes advertised: $(if ($modes.Count -gt 0) { $modes.Count } else { 0 })"

    if ($w -lt $script:ExpectedWidth -or $h -lt $script:ExpectedHeight) {
        Write-Note "desktop is smaller than ${script:ExpectedWidth}x${script:ExpectedHeight}; attempting to raise it."
        $r = Set-ScreenResolution -MinWidth $script:ExpectedWidth -MinHeight $script:ExpectedHeight
        Write-Note "resolution change: $r"
        $script:Diagnostics['screen_change'] = $r
        # A mode change is applied asynchronously; re-read after it has settled
        # rather than assuming the requested size took effect.
        Start-Sleep -Seconds 3
        $w = [AgentLensQa.Win32]::GetSystemMetrics(0)
        $h = [AgentLensQa.Win32]::GetSystemMetrics(1)
    } else {
        $script:Diagnostics['screen_change'] = "not attempted: ${w}x${h} already contains the ${script:ExpectedWidth}x${script:ExpectedHeight} window"
    }

    $script:Diagnostics['screen_after'] = "${w}x${h}"
    $script:Diagnostics['screen'] = "${w}x${h}"
    Write-Note "screen after : ${w}x${h}"

    # The visual channel for this QA is ec2 get-console-screenshot, which returns
    # the hypervisor framebuffer at whatever the guest desktop is actually set to.
    # A 1024-wide framebuffer physically cannot contain an 1180-wide window, so
    # the three window buttons at its top-right fall outside the captured frame.
    # That MUST be said out loud: a reader who sees a screenshot and no caveat
    # will assume the buttons were checked, and they were not. The style-bit
    # assertions (Test-WindowStyle) remain valid either way -- they read
    # GetWindowLongPtr and do not care how big the desktop is.
    $fits = ($w -ge $script:ExpectedWidth -and $h -ge $script:ExpectedHeight)
    if ($fits) {
        $note = "Desktop is at least ${script:ExpectedWidth}x${script:ExpectedHeight}, so a console screenshot can contain the whole window frame."
        $observed = "${w}x${h} (window fits)"
    } else {
        $note = "CROPPED: the ${script:ExpectedWidth}x${script:ExpectedHeight} window does NOT fit in a ${w}x${h} framebuffer. Window-button visibility is NOT VERIFIABLE from the console screenshot for this run -- the top-right buttons are outside the captured frame. Do not read the screenshot as confirming them. Style bits are still asserted separately below and are unaffected."
        $observed = "${w}x${h} (window does NOT fit -- screenshot will be cropped)"
    }
    Write-Note $note
    $script:Diagnostics['console_screenshot_shows_whole_window'] = $fits
    $script:Diagnostics['window_buttons_verifiable_from_screenshot'] = $fits
    Add-Assertion -Name 'desktop.metrics' -Class 'informational' `
        -Expected ">= ${script:ExpectedWidth}x${script:ExpectedHeight} to display the window whole" `
        -Observed $observed -Verdict 'INFO' -Note $note
}

# Best effort by design, and the outcome is RETURNED and logged rather than
# asserted: the caller re-reads GetSystemMetrics instead of assuming the requested
# size took effect. All of the real work is in Win32::RaiseDisplayMode -- see the
# comment there for why the DEVMODE has to be seeded from an INDEXED enumeration
# rather than from ENUM_CURRENT_SETTINGS, which fails on this adapter even in the
# interactive session.
function Set-ScreenResolution {
    param([int]$MinWidth, [int]$MinHeight)
    try {
        return [AgentLensQa.Win32]::RaiseDisplayMode($MinWidth, $MinHeight)
    } catch {
        return "RaiseDisplayMode threw: $($_.Exception.Message)"
    }
}

function Install-WebView2 {
    Write-Section 'WebView2 Evergreen Runtime'
    $before = Get-Wv2Version
    if ($null -ne $before) {
        $script:Diagnostics['webview2_before'] = "$($before.Version) @ $($before.Hive)"
        Write-Note "before: pv=$($before.Version) in $($before.Hive)"
        Add-Assertion -Name 'webview2.present' -Class 'machine-checkable' -Expected 'pv > 0.0.0.0' `
            -Observed $before.Version -Verdict 'PASS' -Note "hive $($before.Hive)"
        return
    }

    $script:Diagnostics['webview2_before'] = 'absent'
    Write-Note 'before: ABSENT in all three hives (HKLM, HKLM\WOW6432Node, HKCU).'
    Write-Note 'A Tauri window cannot render at all without this runtime, so this is a GATE.'
    Write-Note 'installing via the Evergreen bootstrapper.'
    Write-Note 'The bootstrapper DOWNLOADS the runtime, so this needs outbound internet.'
    $boot = Join-Path $script:WorkDir 'MicrosoftEdgeWebview2Setup.exe'
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $script:Wv2Bootstrapper -OutFile $boot -UseBasicParsing
    } catch {
        Stop-WithError 'ENVIRONMENT UNFIT: failed to download the WebView2 bootstrapper' @"
url: $script:Wv2Bootstrapper
$($_.Exception.Message)

This says NOTHING about AgentLens. The WebView2 Evergreen Runtime is absent from
this instance and could not be fetched, most likely because the instance has no
outbound internet route. The GUI assertions were NOT run and the installer was
NOT launched. Fix the environment and re-run; do not read this as an app defect.
"@
    }
    # SSM runs as SYSTEM, therefore elevated, therefore this lands per-machine
    # under HKLM\...\WOW6432Node rather than per-user.
    $code = Invoke-Native -FilePath $boot -Arguments @('/silent', '/install') -What 'WebView2 bootstrapper'
    # Re-detect from the registry rather than trusting the exit code: the
    # bootstrapper can report success while the runtime lands somewhere the app
    # will not find, and the registry is what actually decides whether a window
    # can be composited.
    $after = Get-Wv2Version
    if ($code -ne 0 -or $null -eq $after) {
        $script:Diagnostics['webview2_after'] = 'absent'
        $script:Diagnostics['webview2_bootstrapper_exit'] = $code
        Write-Note "after : ABSENT (bootstrapper exit $code)"
        Add-Assertion -Name 'webview2.present' -Class 'machine-checkable' -Expected 'pv > 0.0.0.0' `
            -Observed 'absent' -Verdict 'FAIL' -Note 'environment gate, not an app defect'
        Stop-WithError 'ENVIRONMENT UNFIT: WebView2 Runtime still absent after install' @"
bootstrapper exit code: $code
re-detected after install: still absent in all three hives

This is an ENVIRONMENT failure, NOT an AgentLens failure. Distinguish the two
carefully: without the WebView2 runtime a Tauri app produces NO window and NO
HWND, which is indistinguishable at the surface from a packaging defect. Running
the GUI assertions now would manufacture a FALSE NEGATIVE against the app, so
the run is stopped here instead. The installer was NOT launched and no claim
whatsoever is being made about whether AgentLens works.
"@
    }
    $script:Diagnostics['webview2_after'] = "$($after.Version) @ $($after.Hive)"
    Write-Note "after : pv=$($after.Version) in $($after.Hive)"
    Add-Assertion -Name 'webview2.present' -Class 'machine-checkable' -Expected 'pv > 0.0.0.0' `
        -Observed $after.Version -Verdict 'PASS' -Note "installed during this run, hive $($after.Hive)"
}

function Get-Installer {
    Write-Section 'fetch and verify the installer'
    $zip = Join-Path $script:WorkDir 'agentlens-windows.zip'
    Write-Note "downloading s3://$script:Bucket/$script:Key -> $zip"
    try {
        Read-S3Object -BucketName $script:Bucket -Key $script:Key -File $zip -Region $script:Region | Out-Null
    } catch {
        Stop-WithError 'failed to download the installer artifact from S3' @"
s3://$script:Bucket/$script:Key
region: $script:Region
$($_.Exception.Message)
The instance profile needs s3:GetObject on this key.
"@
    }
    if (-not (Test-Path -LiteralPath $zip) -or ((Get-Item -LiteralPath $zip).Length -eq 0)) {
        Stop-WithError 'the installer artifact downloaded as missing or empty' "s3://$script:Bucket/$script:Key -> $zip"
    }
    Write-Note "downloaded $((Get-Item -LiteralPath $zip).Length) bytes"

    $extract = Join-Path $script:WorkDir 'extract'
    if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force

    $setup = Get-ChildItem -LiteralPath $extract -Recurse -Filter $script:SetupName | Select-Object -First 1
    if ($null -eq $setup) {
        $listing = (Get-ChildItem -LiteralPath $extract -Recurse | ForEach-Object { $_.FullName }) -join "`n"
        Stop-WithError "did not find $script:SetupName in the artifact" $listing
    }

    $actual = Get-Sha256 -Path $setup.FullName
    Write-Note "setup : $($setup.FullName)"
    Write-Note "bytes : $($setup.Length)"
    Write-Note "sha256: $actual"
    $script:Diagnostics['setup_bytes'] = $setup.Length
    $script:Diagnostics['setup_sha256'] = $actual
    if ($actual -ne $script:ExpectedSha.ToLowerInvariant()) {
        Remove-Item -LiteralPath $setup.FullName -Force -ErrorAction SilentlyContinue
        Stop-WithError 'SHA-256 mismatch on the setup exe -- refusing to install' "expected: $script:ExpectedSha`nactual  : $actual"
    }
    Add-Assertion -Name 'installer.sha256' -Class 'machine-checkable' `
        -Expected $script:ExpectedSha -Observed $actual -Verdict 'PASS'
    return $setup.FullName
}

function Install-App {
    param([string]$SetupPath)
    Write-Section 'silent install'
    # /S silent, /NS no shortcuts. NOT /SILENT -- that is Inno Setup syntax and
    # a Tauri NSIS installer treats it as an unrecognised argument, silently.
    # /NS matters because with /S the Finish page is skipped and the template
    # then explicitly calls CreateOrUpdateDesktopShortcut, so a silent install
    # DOES leave a desktop shortcut unless it is suppressed.
    $code = Invoke-Native -FilePath $SetupPath -Arguments @('/S', '/NS') -What 'NSIS setup'
    $script:Diagnostics['installer_exit'] = $code
    if ($code -ne 0) {
        Stop-WithError 'the NSIS installer returned a non-zero exit code' "exit code: $code"
    }
    Add-Assertion -Name 'installer.exit' -Class 'machine-checkable' -Expected 0 -Observed $code -Verdict 'PASS'
}

# =============================================================================
# WOW64 FILESYSTEM REDIRECTION -- read this before touching Resolve-InstallDir.
#
# The Tauri NSIS stub is a 32-BIT process. Running as SYSTEM it expands
# $LOCALAPPDATA through the redirected view, so it installs into, and writes into
# the registry, the literal string
#     C:\Windows\system32\config\systemprofile\AppData\Local\AgentLens
# while the bytes physically land in
#     C:\Windows\SysWOW64\config\systemprofile\AppData\Local\AgentLens
# because for a 32-bit process every reference to %WinDir%\System32 is silently
# redirected to %WinDir%\SysWOW64 by the WOW64 layer.
#
# This QA script is 64-BIT PowerShell. It gets the REAL System32, where that
# directory does not exist. So the registry's InstallLocation is a correct string
# for its 32-bit author and a dangling path for a 64-bit reader. That -- not any
# registry hive problem -- is what made a successful install look like a missing
# one, and it is why the old error message ("the currentUser/SYSTEM hive trap")
# sent the investigation down a dead end.
#
# Sysnative is NOT a usable escape here: %WinDir%\Sysnative exists only for
# 32-bit processes, so from this 64-bit script it does not resolve at all. The
# fix is plain string substitution on the two directory names, tried in a defined
# order, with the form that actually resolved recorded in the artifacts.
# =============================================================================
function Get-Wow64PathVariants {
    param([string]$Path)
    $variants = New-Object System.Collections.ArrayList
    $null = $variants.Add([ordered]@{ Form = 'registry-literal'; Path = $Path })
    # -replace is case-insensitive in PowerShell, which matters: the registry
    # value says "system32" in lower case while the API name is "System32".
    $toWow = $Path -replace '\\Windows\\System32(\\|$)', '\Windows\SysWOW64$1'
    if ($toWow -ne $Path) {
        $null = $variants.Add([ordered]@{ Form = 'wow64-redirected (System32 -> SysWOW64)'; Path = $toWow })
    }
    $toSys = $Path -replace '\\Windows\\SysWOW64(\\|$)', '\Windows\System32$1'
    if ($toSys -ne $Path) {
        $null = $variants.Add([ordered]@{ Form = 'wow64-dereferenced (SysWOW64 -> System32)'; Path = $toSys })
    }
    return $variants
}

function Resolve-InstallDir {
    Write-Section 'resolve the install directory'
    # Two independent traps stack here, and they are NOT the same trap.
    #
    # (1) WOW64 filesystem redirection -- see the block comment above. This is
    #     what actually broke run h7-20260805T022009Z: the install had SUCCEEDED
    #     and the registry key was found, but InstallLocation named a System32
    #     path that only exists in the 32-bit view.
    #
    # (2) The currentUser/SYSTEM hive question. tauri.conf.json has no
    #     bundle.windows block, so the Tauri v2 bundler falls back to
    #     NSISInstallerMode::CurrentUser, which writes uninstall metadata to
    #     HKCU. This script runs as SYSTEM, whose HKCU is not a normal user's
    #     HKCU, so every loaded hive under HKEY_USERS is searched too. That
    #     branch is kept because it CAN still legitimately happen -- an install
    #     performed by an interactive user would land in a hive that is not
    #     loaded while this runs -- but it is NOT the explanation for the
    #     observed failure and must not be presented as one.
    $rel = "Software\Microsoft\Windows\CurrentVersion\Uninstall\$script:ProductName"
    $candidates = New-Object System.Collections.ArrayList
    $null = $candidates.Add(@{ Label = 'HKLM'; Path = "HKLM:\$rel" })
    $null = $candidates.Add(@{ Label = 'HKCU (SYSTEM own)'; Path = "HKCU:\$rel" })

    if (-not (Get-PSDrive -Name HKU -ErrorAction SilentlyContinue)) {
        New-PSDrive -Name HKU -PSProvider Registry -Root HKEY_USERS -Scope Script | Out-Null
    }
    foreach ($hive in (Get-ChildItem -LiteralPath 'HKU:\' -ErrorAction SilentlyContinue)) {
        $sid = Split-Path -Leaf $hive.Name
        if ($sid -like '*_Classes') { continue }
        $null = $candidates.Add(@{ Label = "HKU\$sid"; Path = "HKU:\$sid\$rel" })
    }

    $searched = New-Object System.Collections.ArrayList
    $probed = New-Object System.Collections.ArrayList
    $keyFound = $false
    foreach ($c in $candidates) {
        $null = $searched.Add($c.Path)
        if (-not (Test-Path -LiteralPath $c.Path)) { continue }
        $p = Get-ItemProperty -LiteralPath $c.Path -ErrorAction SilentlyContinue
        if ($null -eq $p) { continue }
        # InstallLocation is written WITH surrounding quotes by the Tauri NSIS
        # template, so it has to be trimmed before use.
        $loc = ''
        if ($p.PSObject.Properties.Name -contains 'InstallLocation') { $loc = "$($p.InstallLocation)".Trim().Trim('"') }
        $ver = ''
        if ($p.PSObject.Properties.Name -contains 'DisplayVersion') { $ver = "$($p.DisplayVersion)" }
        $main = ''
        if ($p.PSObject.Properties.Name -contains 'MainBinaryName') { $main = "$($p.MainBinaryName)".Trim().Trim('"') }
        $uninst = ''
        if ($p.PSObject.Properties.Name -contains 'UninstallString') { $uninst = "$($p.UninstallString)".Trim().Trim('"') }

        Write-Note "found uninstall key in $($c.Label)"
        Write-Note "  DisplayVersion : $ver"
        Write-Note "  InstallLocation: $loc"
        Write-Note "  MainBinaryName : $main"
        Write-Note "  UninstallString: $uninst"
        $script:Diagnostics['uninstall_hive'] = $c.Path
        $script:Diagnostics['display_version'] = $ver
        $script:Diagnostics['install_location_registry'] = $loc
        $script:Diagnostics['uninstall_string'] = $uninst
        if ($main) { $script:MainBinaryName = $main }
        $script:Diagnostics['main_binary_name_registry'] = $script:MainBinaryName

        if (-not $keyFound) {
            $keyFound = $true
            Add-Assertion -Name 'install.registry' -Class 'machine-checkable' `
                -Expected "uninstall key named '$script:ProductName'" -Observed $c.Path -Verdict 'PASS' `
                -Note 'keyed on productName, not on the identifier and not on a GUID'
        }

        if (-not $loc) {
            Write-Note 'warn: this key carries no InstallLocation value; continuing to search.'
            continue
        }

        # Probe the literal registry string first, then the WOW64-substituted
        # forms. No form is silently preferred: whichever one resolves is logged
        # by name and recorded in diagnostics, so a future reader can see that
        # the path came out of redirection rather than out of the registry.
        foreach ($v in (Get-Wow64PathVariants -Path $loc)) {
            $exists = Test-Path -LiteralPath $v.Path
            $null = $probed.Add("[$(if ($exists) { 'EXISTS' } else { 'missing' })] $($v.Form): $($v.Path)")
            Write-Note "  probe $($v.Form): $($v.Path) -> $(if ($exists) { 'EXISTS' } else { 'missing' })"
            if (-not $exists) { continue }
            if ($v.Form -ne 'registry-literal') {
                Write-Note "  note: resolved through WOW64 substitution, NOT from the registry string."
                Write-Note '        The 32-bit NSIS stub wrote a System32 path; the real bytes are under SysWOW64.'
            }
            $script:Diagnostics['install_dir_resolved'] = $v.Path
            $script:Diagnostics['install_dir_resolved_form'] = $v.Form
            $script:Diagnostics['install_dir_probes'] = ($probed -join "`n")
            Add-Assertion -Name 'install.directory' -Class 'machine-checkable' `
                -Expected 'InstallLocation resolves to a directory that exists' -Observed $v.Path -Verdict 'PASS' `
                -Note "resolved via $($v.Form); registry said '$loc'"
            return $v.Path
        }
        Write-Note "warn: no form of InstallLocation '$loc' exists on disk; continuing to search."
    }

    $script:Diagnostics['uninstall_searched'] = ($searched -join "`n")
    $script:Diagnostics['install_dir_probes'] = ($probed -join "`n")
    if ($keyFound) {
        Add-Assertion -Name 'install.directory' -Class 'machine-checkable' `
            -Expected 'InstallLocation resolves to a directory that exists' `
            -Observed 'no form of the recorded InstallLocation exists on disk' -Verdict 'FAIL'
    }
    Stop-WithError 'could not locate the AgentLens install directory' (
        "Registry keys searched:`n" + (($searched | ForEach-Object { "  $_" }) -join "`n") +
        "`n`nFilesystem paths probed:`n" + (($probed | ForEach-Object { "  $_" }) -join "`n") +
        "`n`nBoth the literal InstallLocation string and its WOW64-substituted forms " +
        "(System32 <-> SysWOW64) were probed, so simple 32-bit installer redirection is " +
        "already ruled out. Remaining possibilities, in order: the installer wrote no " +
        "InstallLocation at all; it installed somewhere neither form names; or the install " +
        "was performed by an interactive user whose registry hive is not loaded while this " +
        "runs as SYSTEM, in which case the key itself would also be absent above. " +
        "Note this is NOT simply 'the SYSTEM hive trap': every loaded HKEY_USERS hive was " +
        "searched, and that explanation was previously asserted here and was WRONG."
    )
}

# Picks the executable to launch. The registry's MainBinaryName is authoritative
# because tauri.conf.json declares no bundle.windows.mainBinaryName, so the
# shipped name is the Cargo bin name (agentlens-tauri.exe) and NOT
# productName.exe. Assuming productName.exe is what previously made this step
# fail against a completely healthy install.
function Resolve-MainBinary {
    param([string]$InstallDir)
    Write-Section 'resolve the main binary'
    $names = New-Object System.Collections.ArrayList
    if ($script:MainBinaryName) { $null = $names.Add($script:MainBinaryName) }
    foreach ($n in $script:BinaryNameCandidates) {
        if (-not $names.Contains($n)) { $null = $names.Add($n) }
    }
    $tried = New-Object System.Collections.ArrayList
    foreach ($n in $names) {
        $p = Join-Path $InstallDir $n
        $exists = Test-Path -LiteralPath $p
        $null = $tried.Add("[$(if ($exists) { 'EXISTS' } else { 'missing' })] $p")
        Write-Note "  candidate $n -> $(if ($exists) { 'EXISTS' } else { 'missing' })"
        if (-not $exists) { continue }
        $script:Diagnostics['main_binary'] = $p
        $script:Diagnostics['main_binary_bytes'] = (Get-Item -LiteralPath $p).Length
        $script:Diagnostics['main_binary_candidates'] = ($tried -join "`n")
        Add-Assertion -Name 'install.main_binary' -Class 'machine-checkable' `
            -Expected 'the binary named by the registry exists in the install directory' `
            -Observed $p -Verdict 'PASS' `
            -Note $(if ($script:MainBinaryName -and $n -eq $script:MainBinaryName) { "from the registry's MainBinaryName" } else { 'from the fallback candidate list' })
        return $p
    }
    $listing = ''
    if (Test-Path -LiteralPath $InstallDir) {
        $listing = (Get-ChildItem -LiteralPath $InstallDir -Recurse -ErrorAction SilentlyContinue |
            ForEach-Object { "$($_.Length)`t$($_.FullName)" }) -join "`n"
    }
    $script:Diagnostics['main_binary_candidates'] = ($tried -join "`n")
    Add-Assertion -Name 'install.main_binary' -Class 'machine-checkable' `
        -Expected 'the binary named by the registry exists in the install directory' `
        -Observed 'none of the candidate names exist' -Verdict 'FAIL'
    Stop-WithError 'the main binary is missing from the install directory' (
        "Tried:`n" + (($tried | ForEach-Object { "  $_" }) -join "`n") +
        "`n`nInstall directory contents:`n" + $listing
    )
}


function Start-App {
    param([string]$ExePath)
    Write-Section 'launch'
    Write-Note "starting $ExePath"
    $proc = Start-Process -FilePath $ExePath -PassThru
    Start-Sleep -Seconds 2
    if ($proc.HasExited) {
        $script:Diagnostics['app_exit_code'] = $proc.ExitCode
        Stop-WithError 'the app exited immediately after launch' "exit code: $($proc.ExitCode)`nThis is a real and important outcome: the installer produced a binary that cannot start. See the event-log section of diagnostics.json."
    }
    Write-Note "pid $($proc.Id) is alive"
    $script:Diagnostics['app_pid'] = $proc.Id

    # The session the APP is in, read from the app's own process object rather
    # than inferred from this launcher. They are the same session here by
    # construction, but "the launcher was interactive" and "the launched GUI
    # process is interactive" are different claims and only the second one
    # decides whether a WebView2 window can be composited.
    $appSession = -1
    try {
        $appSession = [int](Get-Process -Id $proc.Id -ErrorAction Stop).SessionId
    } catch {
        $appSession = -1
    }
    $script:Diagnostics['app_session_id'] = $appSession
    Write-Note "app session id: $appSession (0 = the non-interactive service session)"
    Add-Assertion -Name 'session.app_process' -Class 'informational' `
        -Expected 'the launched GUI process is in a session other than 0' `
        -Observed "session $appSession" -Verdict 'INFO' `
        -Note 'Read from the launched process itself via Get-Process, not inferred from the launcher.'
    return $proc
}

# Window classes owned by a Tauri process that are NOT the app window. A live
# process is not a window, and neither is one of these: the tray-icon crate
# registers a hidden "tray_icon_app" helper window and tao registers a hidden
# "Tao Thread Event Target" message sink. Both are real top-level HWNDs owned by
# the app pid, so anything that merely counts HWNDs per pid would report a window
# that no user could ever see. They are excluded by class name, and every window
# found is logged either way so the artifacts show what was really there.
$script:NonAppWindowClasses = @('tray_icon_app', 'Tao Thread Event Target')

# Returns the app window HWND, or IntPtr::Zero when there is none. Zero is NOT a
# fatal error here on purpose: the caller decides whether "no window" means the
# app failed or the environment cannot host one, and that decision is the single
# most important judgement this script makes.
function Wait-ForWindow {
    param([System.Diagnostics.Process]$Proc, [int]$TimeoutSec = 90)
    Write-Section 'wait for a top-level window'
    Write-Note "A created process is NOT a window. 'Still running after Ns' is NOT a window."
    Write-Note "Only a top-level HWND owned by pid $($Proc.Id), visible, and not one of the known"
    Write-Note "helper classes ($($script:NonAppWindowClasses -join ', ')) counts as a window."
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $inventory = @()
    while ((Get-Date) -lt $deadline) {
        if ($Proc.HasExited) {
            $script:Diagnostics['app_exit_code'] = $Proc.ExitCode
            $script:Diagnostics['hwnd_appeared'] = $false
            Add-Assertion -Name 'window.exists' -Class 'machine-checkable' `
                -Expected 'a visible top-level HWND owned by the app pid' `
                -Observed "the process exited with code $($Proc.ExitCode) before any window appeared" -Verdict 'FAIL' `
                -Note 'The app died on its own. This is an APP outcome, not an environment one: the process was launched successfully and terminated itself.'
            Stop-WithError 'the app exited while waiting for its window' "exit code: $($Proc.ExitCode)"
        }
        $inventory = @([AgentLensQa.Win32]::DescribeTopLevelWindowsForPid([uint32]$Proc.Id))
        $candidates = @($inventory | Where-Object {
            $parts = $_ -split "`t"
            $parts[1] -eq 'visible' -and ($script:NonAppWindowClasses -notcontains $parts[2])
        })
        if ($candidates.Count -gt 0) {
            $parts = $candidates[0] -split "`t"
            $h = [IntPtr][int64][Convert]::ToInt64($parts[0], 16)
            Write-Note "app window found: hwnd=$($parts[0]) class=[$($parts[2])] title=[$($parts[3])] rect=$($parts[4])"
            $script:Diagnostics['hwnd'] = $parts[0]
            $script:Diagnostics['hwnd_class'] = $parts[2]
            $script:Diagnostics['hwnd_appeared'] = $true
            $script:Diagnostics['window_inventory'] = ($inventory -join "`n")
            Add-Assertion -Name 'window.exists' -Class 'machine-checkable' `
                -Expected 'a visible top-level HWND owned by the app pid, excluding known helper classes' `
                -Observed "$($parts[0]) class=[$($parts[2])]" -Verdict 'PASS'
            return $h
        }
        Start-Sleep -Milliseconds 500
    }

    # No app window inside the timeout, and the process is still alive.
    $script:Diagnostics['hwnd_appeared'] = $false
    $script:Diagnostics['window_inventory'] = if ($inventory.Count -gt 0) { $inventory -join "`n" } else { 'no top-level windows at all' }
    $webviewProcs = @(Get-Process -Name 'msedgewebview2' -ErrorAction SilentlyContinue).Count
    $script:Diagnostics['msedgewebview2_processes'] = $webviewProcs
    Write-Note "no app window after ${TimeoutSec}s. Top-level windows owned by the pid:"
    if ($inventory.Count -eq 0) { Write-Note '  (none)' }
    foreach ($i in $inventory) { Write-Note "  $i" }
    Write-Note "msedgewebview2.exe processes running: $webviewProcs"

    # ---- environment cannot show a window, vs app failed to make one --------
    # These are DIFFERENT findings and must never be conflated. Session 0 is
    # reserved for services and "does not support processes that interact with
    # the user" (Microsoft, Service Changes for Windows Vista), and the window
    # station here is the non-interactive Service-0x0-3e7$. A Tauri window IS a
    # WebView2 window: it only exists once the WebView2 browser process is up, and
    # that browser process is exactly what session 0 will not host. So from
    # session 0, "no window" carries NO information about the application, and
    # reporting FAIL would be a fabricated accusation against AgentLens.
    #
    # Observationally this is not a guess: the process stays alive, it creates its
    # ordinary USER32 helper windows within seconds -- so window creation as such
    # works on this desktop -- and yet no msedgewebview2.exe ever appears.
    #
    # In an interactive session (session id != 0) none of that applies, and a
    # missing window IS an app defect. That branch reports FAIL, loudly.
    $sessionId = $script:Diagnostics['session_id']
    if ($sessionId -eq 0) {
        $reason = "NOT EXECUTED, not FAIL: this run is in session 0 on window station " +
                  "Service-0x0-3e7$, which is reserved for services and does not support " +
                  "processes that interact with the user. A Tauri window is a WebView2 window " +
                  "and cannot be composited here; msedgewebview2.exe processes seen: $webviewProcs. " +
                  "The process stayed alive for ${TimeoutSec}s and did create its ordinary helper " +
                  "windows, so window creation as such is not blocked -- the webview host is. " +
                  "This says NOTHING about whether AgentLens shows a window on a real desktop; " +
                  "to settle that, run this QA from an interactive session (session id != 0)."
        Write-Note 'ENVIRONMENT LIMIT, NOT AN APP DEFECT:'
        foreach ($l in ($reason -split '(?<=\.) ')) { Write-Note "  $l" }
        Add-NotExecuted -Name 'window.exists' -Reason $reason `
            -Expected 'a visible top-level HWND owned by the app pid' `
            -Observed 'no app window, and session 0 structurally cannot host one'
        return [IntPtr]::Zero
    }

    Add-Assertion -Name 'window.exists' -Class 'machine-checkable' `
        -Expected 'a visible top-level HWND owned by the app pid' `
        -Observed "none within ${TimeoutSec}s while the process stayed alive" -Verdict 'FAIL' `
        -Note ("This IS an app defect: session $sessionId is interactive, so the environment can host a window and the app did not produce one. " +
               "Likely causes, in order: the WebView2 runtime failed to initialise; the webview crashed on startup (see crashpad_dirs in diagnostics.json); " +
               "or the frontend bundle failed to load. msedgewebview2.exe processes seen: $webviewProcs.")
    Write-Note 'APP DEFECT: an interactive session could have shown a window and none appeared.'
    return [IntPtr]::Zero
}

# Every window assertion needs a live HWND. When there is none, each one is
# recorded as NOT EXECUTED with the reason carried over, so the reader sees the
# specific list of things that were not checked rather than a silent gap.
#
# The 'input' stage is covered by the same sweep on purpose: a real SendInput
# assertion needs a live HWND *and* an interactive input queue, so from session 0
# it is exactly as unmeasurable as the state assertions and must land on the same
# NOT EXECUTED verdict rather than on PASS or FAIL.
function Add-WindowAssertionSkips {
    param([string]$Reason)
    Write-Section 'window assertions -- NOT EXECUTED'
    foreach ($e in $script:ExpectedAssertions) {
        if ($e.Stage -ne 'window' -and $e.Stage -ne 'input') { continue }
        if ($e.Name -eq 'window.exists') { continue }
        Add-NotExecuted -Name $e.Name -Reason $Reason `
            -Expected $e.Why -Observed 'no HWND was available to measure'
    }
}


function Test-WindowStyle {
    param([IntPtr]$Hwnd)
    Write-Section 'window style bits and whether a native caption is DRAWN'
    $style = [uint32]([AgentLensQa.Win32]::GetWindowLongPtr($Hwnd, [AgentLensQa.Win32]::GWL_STYLE)).ToInt64()
    $ex = [uint32]([AgentLensQa.Win32]::GetWindowLongPtr($Hwnd, [AgentLensQa.Win32]::GWL_EXSTYLE)).ToInt64()
    Write-Note ('GWL_STYLE   = 0x{0:X8}' -f $style)
    Write-Note ('GWL_EXSTYLE = 0x{0:X8}' -f $ex)
    $script:Diagnostics['gwl_style'] = ('0x{0:X8}' -f $style)
    $script:Diagnostics['gwl_exstyle'] = ('0x{0:X8}' -f $ex)

    $hasCaption = ($style -band [AgentLensQa.Win32]::WS_CAPTION) -ne 0
    $hasThick = ($style -band [AgentLensQa.Win32]::WS_THICKFRAME) -ne 0

    # ---- is there a native TITLE BAR? -------------------------------------
    # "WS_CAPTION must be clear" was the wrong question, and run
    # h7-20260805T064522Z is the proof: WS_CAPTION was SET while the measured
    # vertical non-client overhead was 8px, which cannot contain a title bar.
    #
    # Tauri v2 undecorated windows KEEP WS_CAPTION|WS_THICKFRAME and erase the
    # non-client area by returning the full proposed rect from WM_NCCALCSIZE.
    # That is deliberate: the style bits are what Windows uses to grant Aero
    # Snap, the resize borders, the drop shadow and the rounded corners, all of
    # which README.md documents as still working. So the bits stay set and
    # nothing is drawn -- asserting on the bit alone measures the wrong layer.
    #
    # The observable question is whether a caption is DRAWN, and the observable
    # answer is the vertical non-client overhead: an undecorated window pays only
    # for its sizing border, while a captioned window must additionally pay for
    # the caption strip. So the threshold is SM_CYCAPTION, read from the running
    # system rather than hardcoded, since it varies by Windows version, theme and
    # DPI. Overhead below one caption height means no caption was drawn; overhead
    # at or above it means one was.
    $smCyCaption = [AgentLensQa.Win32]::GetSystemMetrics([AgentLensQa.Win32]::SM_CYCAPTION)
    $smCySizeFrame = [AgentLensQa.Win32]::GetSystemMetrics([AgentLensQa.Win32]::SM_CYSIZEFRAME)
    $smCxSizeFrame = [AgentLensQa.Win32]::GetSystemMetrics([AgentLensQa.Win32]::SM_CXSIZEFRAME)
    $smCyBorder = [AgentLensQa.Win32]::GetSystemMetrics([AgentLensQa.Win32]::SM_CYBORDER)
    $smCxPadded = [AgentLensQa.Win32]::GetSystemMetrics([AgentLensQa.Win32]::SM_CXPADDEDBORDER)
    $script:Diagnostics['sm_cycaption'] = $smCyCaption
    $script:Diagnostics['sm_cysizeframe'] = $smCySizeFrame
    $script:Diagnostics['sm_cxsizeframe'] = $smCxSizeFrame
    $script:Diagnostics['sm_cyborder'] = $smCyBorder
    $script:Diagnostics['sm_cxpaddedborder'] = $smCxPadded

    # What a REAL captioned window of this style would cost vertically, computed
    # by the same API the window manager uses. Recorded so the reader can see the
    # two numbers side by side instead of taking the threshold on faith.
    $probe = New-Object AgentLensQa.RECT
    $probe.Left = 0; $probe.Top = 0; $probe.Right = 500; $probe.Bottom = 500
    [void][AgentLensQa.Win32]::AdjustWindowRectEx([ref]$probe, $style, $false, $ex)
    $decoratedDy = ($probe.Bottom - $probe.Top) - 500
    $decoratedDx = ($probe.Right - $probe.Left) - 500
    $script:Diagnostics['decorated_frame_overhead'] = "dx=$decoratedDx dy=$decoratedDy"

    $wr = New-Object AgentLensQa.RECT
    $cr = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$wr)
    [void][AgentLensQa.Win32]::GetClientRect($Hwnd, [ref]$cr)
    $dx = ($wr.Right - $wr.Left) - ($cr.Right - $cr.Left)
    $dy = ($wr.Bottom - $wr.Top) - ($cr.Bottom - $cr.Top)
    $script:Diagnostics['frame_overhead'] = "dx=$dx dy=$dy"
    Write-Note "non-client overhead: dx=$dx dy=$dy"
    Write-Note "SM_CYCAPTION=$smCyCaption SM_CYSIZEFRAME=$smCySizeFrame SM_CXSIZEFRAME=$smCxSizeFrame SM_CXPADDEDBORDER=$smCxPadded"
    Write-Note "AdjustWindowRectEx for this style would cost dx=$decoratedDx dy=$decoratedDy if the frame were drawn"

    $noCaptionDrawn = ($dy -lt $smCyCaption)
    Add-Assertion -Name 'style.no_native_caption' -Class 'machine-checkable' `
        -Expected "vertical non-client overhead < SM_CYCAPTION ($smCyCaption px)" `
        -Observed "dy=$dy px" `
        -Verdict $(if ($noCaptionDrawn) { 'PASS' } else { 'FAIL' }) `
        -Note ("dx=$dx dy=$dy measured; SM_CYCAPTION=$smCyCaption SM_CYSIZEFRAME=$smCySizeFrame " +
               "SM_CXSIZEFRAME=$smCxSizeFrame SM_CXPADDEDBORDER=$smCxPadded; AdjustWindowRectEx says a DRAWN " +
               "frame of this style costs dx=$decoratedDx dy=$decoratedDy. Overhead below one caption height " +
               "means the caption strip is not being drawn. Raw style bits are recorded separately as INFO: " +
               "Tauri keeps WS_CAPTION set and erases the non-client area in WM_NCCALCSIZE, so the bit is not the evidence.")

    # The raw bit, kept as evidence and no longer as a verdict. It is genuinely
    # useful -- it explains WHY Aero Snap and the resize borders still work --
    # but on its own it says nothing about whether a title bar is visible.
    Add-Assertion -Name 'style.WS_CAPTION' -Class 'informational' `
        -Expected 'set on an undecorated Tauri window (WM_NCCALCSIZE erases the area, the bit stays)' `
        -Observed $(if ($hasCaption) { 'set' } else { 'clear' }) -Verdict 'INFO' `
        -Note ('raw 0x{0:X8}; WS_CAPTION = WS_BORDER|WS_DLGFRAME = 0x00C00000. Superseded as a verdict by style.no_native_caption.' -f $style)

    # WS_THICKFRAME is deliberately NOT asserted clear. README.md states the
    # resize borders, Aero Snap, drop shadow and rounded corners all still work
    # on Windows, which requires a sizing frame -- so THICKFRAME being SET is
    # the expected, correct state for an undecorated-but-resizable window.
    # Asserting it clear would manufacture a false failure.
    Add-Assertion -Name 'style.WS_THICKFRAME' -Class 'informational' `
        -Expected 'set (undecorated but still resizable)' `
        -Observed $(if ($hasThick) { 'set' } else { 'clear' }) -Verdict 'INFO' `
        -Note 'Not a pass/fail: an undecorated Tauri window keeps a sizing frame so resize borders and Aero Snap keep working.'

    foreach ($bit in @(
        @{ N = 'WS_SYSMENU'; V = [AgentLensQa.Win32]::WS_SYSMENU },
        @{ N = 'WS_MINIMIZEBOX'; V = [AgentLensQa.Win32]::WS_MINIMIZEBOX },
        @{ N = 'WS_MAXIMIZEBOX'; V = [AgentLensQa.Win32]::WS_MAXIMIZEBOX },
        @{ N = 'WS_POPUP'; V = [AgentLensQa.Win32]::WS_POPUP },
        @{ N = 'WS_BORDER'; V = [AgentLensQa.Win32]::WS_BORDER },
        @{ N = 'WS_DLGFRAME'; V = [AgentLensQa.Win32]::WS_DLGFRAME }
    )) {
        $set = ($style -band [uint32]$bit.V) -ne 0
        Add-Assertion -Name "style.$($bit.N)" -Class 'informational' -Expected 'recorded, not asserted' `
            -Observed $(if ($set) { 'set' } else { 'clear' }) -Verdict 'INFO'
    }
}

function Test-WindowGeometry {
    param([IntPtr]$Hwnd)
    Write-Section 'geometry'
    $wr = New-Object AgentLensQa.RECT
    $cr = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$wr)
    [void][AgentLensQa.Win32]::GetClientRect($Hwnd, [ref]$cr)
    $ww = $wr.Right - $wr.Left
    $wh = $wr.Bottom - $wr.Top
    $cw = $cr.Right - $cr.Left
    $ch = $cr.Bottom - $cr.Top
    Write-Note "window rect: ${ww}x${wh} at ($($wr.Left),$($wr.Top))"
    Write-Note "client rect: ${cw}x${ch}"
    $script:Diagnostics['window_rect'] = "${ww}x${wh}+$($wr.Left)+$($wr.Top)"
    $script:Diagnostics['client_rect'] = "${cw}x${ch}"

    Test-Assertion -Name 'geometry.client' -Expected "$($script:ExpectedWidth)x$($script:ExpectedHeight)" `
        -Observed "${cw}x${ch}" -Note 'from tauri.conf.json app.windows[0] width/height' | Out-Null

    # A second, independent undecorated signal, now cross-checked against the
    # runtime SM_CYCAPTION by style.no_native_caption. Kept informational here so
    # the raw pair stays visible next to the geometry it was measured from.
    Add-Assertion -Name 'geometry.frame_overhead' -Class 'informational' `
        -Expected 'near zero for an undecorated window' `
        -Observed ("dx={0} dy={1}" -f ($ww - $cw), ($wh - $ch)) -Verdict 'INFO' `
        -Note 'The pass/fail form of this measurement is style.no_native_caption, which compares dy against the runtime SM_CYCAPTION.'
}

function Test-MinimumSize {
    param([IntPtr]$Hwnd)
    Write-Section 'minimum size enforcement (WM_GETMINMAXINFO)'

    # ---- why this does not use SetWindowPos --------------------------------
    # DO NOT "simplify" this back to SetWindowPos(400,300) + GetWindowRect.
    #
    # SetWindowPos does not send WM_GETMINMAXINFO. The system sends that message
    # while TRACKING a size -- a user drag of a border, a maximize, a snap -- and
    # tao (the windowing layer under Tauri) enforces min_inner_size there and
    # only there. A programmatic SetWindowPos therefore honours whatever it is
    # given, and run h7-20260805T064522Z duly recorded "requested 400x300, got
    # 400x300, expected >= 900x600" as a FAIL against AgentLens. That FAIL was
    # meaningless: the same probe would report the same thing on a build with a
    # correct minimum and on a build with no minimum at all, so it could not
    # distinguish them and proved nothing either way.
    #
    # WM_GETMINMAXINFO is the mechanism itself. ptMinTrackSize is the value the
    # system consults for every user sizing operation, so reading it back is
    # reading the enforcement, not a side effect of it.
    $probe = [AgentLensQa.Win32]::QueryMinTrackSize($Hwnd)
    Write-Note "WM_GETMINMAXINFO -> $probe"
    $script:Diagnostics['min_track_size_probe'] = $probe

    # tao publishes an OUTER size: it runs the configured client minimum through
    # AdjustWindowRectEx before writing ptMinTrackSize. Computing the same
    # adjustment here gives the reader both numbers, so "916x639" is legible as
    # "900x600 of client plus this window's frame" instead of looking like drift.
    $style = [uint32]([AgentLensQa.Win32]::GetWindowLongPtr($Hwnd, [AgentLensQa.Win32]::GWL_STYLE)).ToInt64()
    $ex = [uint32]([AgentLensQa.Win32]::GetWindowLongPtr($Hwnd, [AgentLensQa.Win32]::GWL_EXSTYLE)).ToInt64()
    $adj = New-Object AgentLensQa.RECT
    $adj.Left = 0; $adj.Top = 0
    $adj.Right = $script:ExpectedMinWidth; $adj.Bottom = $script:ExpectedMinHeight
    [void][AgentLensQa.Win32]::AdjustWindowRectEx([ref]$adj, $style, $false, $ex)
    $outerMinW = $adj.Right - $adj.Left
    $outerMinH = $adj.Bottom - $adj.Top
    $script:Diagnostics['min_track_size_expected_outer'] = "${outerMinW}x${outerMinH}"
    Write-Note "AdjustWindowRectEx($($script:ExpectedMinWidth)x$($script:ExpectedMinHeight)) for this style = ${outerMinW}x${outerMinH} outer"

    if ($probe -notlike 'ok:*') {
        # The window never answered. That is a failure of the MEASUREMENT, not a
        # statement about the app, so it must not be recorded as a FAIL.
        $script:Diagnostics['min_track_size'] = 'not measured'
        Add-NotExecuted -Name 'geometry.min_clamp' `
            -Reason "WM_GETMINMAXINFO could not be delivered to the window, so no minimum was read back: $probe" `
            -Expected ">= $($script:ExpectedMinWidth)x$($script:ExpectedMinHeight) outer from ptMinTrackSize" `
            -Observed 'the window did not answer WM_GETMINMAXINFO'
    } else {
        $parts = $probe.Split(':')
        $track = $parts[1]
        $wh = $track.Split('x')
        $minW = [int]$wh[0]
        $minH = [int]$wh[1]
        $script:Diagnostics['min_track_size'] = "${minW}x${minH}"
        $ok = ($minW -ge $script:ExpectedMinWidth) -and ($minH -ge $script:ExpectedMinHeight)
        $note = "ptMinTrackSize is the OUTER minimum: tao adjusts the configured client minimum " +
                "($($script:ExpectedMinWidth)x$($script:ExpectedMinHeight) from tauri.conf.json) through AdjustWindowRectEx, " +
                "which for this window's style gives ${outerMinW}x${outerMinH}. An outer minimum at or above the configured " +
                "client minimum is the enforcement: no user sizing operation can go below it. " +
                "Full probe: $probe"
        if ($minW -eq 0 -and $minH -eq 0) {
            $note = "ptMinTrackSize came back 0x0, meaning the window answered WM_GETMINMAXINFO but published NO minimum. " +
                    "That is a real defect: minWidth/minHeight would not be enforced against a user drag. " + $note
        }
        Add-Assertion -Name 'geometry.min_clamp' -Class 'machine-checkable' `
            -Expected ">= $($script:ExpectedMinWidth)x$($script:ExpectedMinHeight) (outer, from ptMinTrackSize)" `
            -Observed "${minW}x${minH}" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) -Note $note

        # ---- the one number this does NOT settle -------------------------
        # ptMinTrackSize is an OUTER size, so the smallest CLIENT area a user can
        # drag the window down to is ptMinTrackSize minus the live non-client
        # overhead. Recorded, not asserted, because it exposes a convention
        # question this QA cannot answer on its own: tauri.conf.json's
        # minWidth/minHeight are inner sizes, tao is documented to run them
        # through AdjustWindowRectEx before publishing ptMinTrackSize (which for
        # this window would give the value in min_track_size_expected_outer), and
        # what actually came back is the unadjusted pair. Whether the small
        # resulting shortfall is intended by Tauri or a rounding of the erased
        # non-client area is a product question, not a measurement, so it is put
        # on the record rather than turned into a verdict either way.
        $wrNow = New-Object AgentLensQa.RECT
        $crNow = New-Object AgentLensQa.RECT
        [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$wrNow)
        [void][AgentLensQa.Win32]::GetClientRect($Hwnd, [ref]$crNow)
        $ovDx = ($wrNow.Right - $wrNow.Left) - ($crNow.Right - $crNow.Left)
        $ovDy = ($wrNow.Bottom - $wrNow.Top) - ($crNow.Bottom - $crNow.Top)
        $impliedW = $minW - $ovDx
        $impliedH = $minH - $ovDy
        $script:Diagnostics['min_client_implied'] = "${impliedW}x${impliedH}"
        Add-Assertion -Name 'geometry.min_client_implied' -Class 'informational' `
            -Expected "$($script:ExpectedMinWidth)x$($script:ExpectedMinHeight) if ptMinTrackSize were the adjusted outer size" `
            -Observed "${impliedW}x${impliedH}" -Verdict 'INFO' `
            -Note ("ptMinTrackSize ${minW}x${minH} minus the live non-client overhead dx=$ovDx dy=$ovDy. " +
                   "AdjustWindowRectEx would have published ${outerMinW}x${outerMinH} for a client minimum of " +
                   "$($script:ExpectedMinWidth)x$($script:ExpectedMinHeight); the window published the unadjusted pair, " +
                   "so the smallest reachable CLIENT area is this. Recorded as an open observation, not a verdict.")
    }

    # The old probe, kept as INFO and explicitly labelled as NOT a clamp test, so
    # the evidence for the paragraph above is in the artifact and not only in this
    # comment. 400x300 being honoured is the expected, correct behaviour of
    # SetWindowPos and says nothing about minWidth/minHeight.
    $before = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$before)
    $flags = [AgentLensQa.Win32]::SWP_NOZORDER -bor [AgentLensQa.Win32]::SWP_NOACTIVATE
    [void][AgentLensQa.Win32]::SetWindowPos($Hwnd, [IntPtr]::Zero, $before.Left, $before.Top, 400, 300, $flags)
    Start-Sleep -Milliseconds 600
    $after = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$after)
    $w = $after.Right - $after.Left
    $h = $after.Bottom - $after.Top
    Write-Note "SetWindowPos(400x300) -> ${w}x${h} (expected to be honoured: this path does not consult the minimum)"
    $script:Diagnostics['setwindowpos_400x300_result'] = "${w}x${h}"
    Add-Assertion -Name 'geometry.setwindowpos_bypasses_min' -Class 'informational' `
        -Expected '400x300 honoured, because SetWindowPos does not send WM_GETMINMAXINFO' `
        -Observed "${w}x${h}" -Verdict 'INFO' `
        -Note 'NOT a clamp test. Recorded so nobody re-introduces it as one: the clamp lives in WM_GETMINMAXINFO, which SetWindowPos never sends. geometry.min_clamp is the real assertion.'

    # Put it back so the maximize check and any screenshot start from default.
    [void][AgentLensQa.Win32]::SetWindowPos($Hwnd, [IntPtr]::Zero, $before.Left, $before.Top,
        ($before.Right - $before.Left), ($before.Bottom - $before.Top), $flags)
    Start-Sleep -Milliseconds 400
}

function Test-WindowTitle {
    param([IntPtr]$Hwnd)
    Write-Section 'window title'
    $sb = New-Object System.Text.StringBuilder 512
    [void][AgentLensQa.Win32]::GetWindowTextW($Hwnd, $sb, $sb.Capacity)
    $title = $sb.ToString()
    $script:Diagnostics['window_title'] = $title
    Test-Assertion -Name 'window.title' -Expected $script:ExpectedTitle -Observed $title | Out-Null
}

function Test-Maximize {
    param([IntPtr]$Hwnd)
    Write-Section 'maximize fits the work area'
    $work = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::SystemParametersInfo([AgentLensQa.Win32]::SPI_GETWORKAREA, 0, [ref]$work, 0)
    $sw = [AgentLensQa.Win32]::GetSystemMetrics(0)
    $sh = [AgentLensQa.Win32]::GetSystemMetrics(1)
    [void][AgentLensQa.Win32]::ShowWindow($Hwnd, [AgentLensQa.Win32]::SW_MAXIMIZE)
    Start-Sleep -Milliseconds 800
    $r = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$r)
    $mw = $r.Right - $r.Left
    $mh = $r.Bottom - $r.Top
    $workW = $work.Right - $work.Left
    $workH = $work.Bottom - $work.Top
    Write-Note "work area : ${workW}x${workH}"
    Write-Note "screen    : ${sw}x${sh}"
    Write-Note "maximized : ${mw}x${mh}"
    $script:Diagnostics['work_area'] = "${workW}x${workH}"
    $script:Diagnostics['maximized_rect'] = "${mw}x${mh}"

    # The interesting property is that maximizing respects the work area and
    # therefore does not cover the taskbar. A tolerance is allowed because an
    # undecorated window with a sizing frame can overhang by the frame width.
    $tol = 16
    $ok = ([Math]::Abs($mw - $workW) -le $tol) -and ([Math]::Abs($mh - $workH) -le $tol)
    Add-Assertion -Name 'window.maximize_work_area' -Class 'machine-checkable' `
        -Expected "${workW}x${workH} (+/- ${tol}px)" -Observed "${mw}x${mh}" `
        -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
        -Note 'Matching the work area rather than the full screen is what proves the taskbar is not covered.'

    [void][AgentLensQa.Win32]::ShowWindow($Hwnd, [AgentLensQa.Win32]::SW_RESTORE)
    Start-Sleep -Milliseconds 500
}

# =============================================================================
# REAL INPUT
#
# Why this section exists at all.
#
# Run h7-20260805T080906Z reported 20/20 machine-checkable assertions passing,
# and the window really was on screen: two console screenshots show the
# self-drawn bar, the three buttons at the top right and the nav. And yet every
# one of those 20 assertions read STATE. GetWindowLongPtr, GetClientRect,
# WM_GETMINMAXINFO, ExtractIconExW, ShowWindow -- not one of them put a single
# event on the OS input queue. The three window buttons had never been clicked.
# The title bar had never been dragged. Nobody had ever double-clicked it.
#
# A screenshot proves pixels were painted. It cannot distinguish a working
# button from a picture of a button: an element at the wrong screen offset, a
# `pointer-events: none` ancestor, a drag region whose mousedown never reaches
# `plugin:window|start_dragging`, an onClick wired to a handle that came back
# null -- all of them render identically and all of them do nothing when a real
# user clicks. That gap is the entire subject of this section.
#
# What makes this stronger than the WebdriverIO suite: a synthetic DOM click
# dispatches an Event object inside the page and skips USER32, the WebView2 host
# window, Chromium's input pipeline and hit testing entirely. SendInput starts
# one hop earlier than the hardware driver and traverses all of it. For a
# hand-drawn title bar that is precisely the interesting path.
#
# Three rules govern the order, and they are not stylistic:
#   - close DESTROYS the interaction surface, so it runs last, after everything;
#   - minimize HIDES the window, so it is undone before anything else is asked;
#   - maximize MOVES every button, so the targets are re-derived from
#     GetClientRect + ClientToScreen immediately before each gesture and never
#     remembered.
# =============================================================================

# The title bar's own geometry, in CSS pixels, taken from the source rather than
# from a screenshot:
#   frontend/src/index.css      --titlebar-height: 2.25rem            -> 36 px
#   TitleBar.tsx CONTROL_BASE   h-full w-11                           -> 44 px
#   TitleBar.tsx                flex items-stretch, minimize/maximize/close
#   frontend/src/index.css      --titlebar-inset-start: 0rem on Windows
# The controls are flush to the trailing edge of the client area, so measured
# leftwards from the client right edge their centres sit at -0.5, -1.5 and -2.5
# button widths. On Windows `platform !== 'macos'`, so they ARE rendered.
$script:TitlebarCssHeight = 36
$script:TitlebarCssButton = 44

# Polls a predicate instead of sleeping a guessed interval, and returns how long
# the state took to change (-1 on timeout). The input is sent ONCE; only the
# observation is repeated. That distinction matters: re-sending clicks until
# something moves would prove nothing about where the button is.
function Wait-ForCondition {
    param([scriptblock]$Predicate, [int]$TimeoutMs = 4000, [int]$PollMs = 100)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        if (& $Predicate) { return [int]$sw.ElapsedMilliseconds }
        Start-Sleep -Milliseconds $PollMs
    } while ($sw.ElapsedMilliseconds -lt $TimeoutMs)
    return -1
}

# Derives the click targets from the window as it is RIGHT NOW. Returns $null if
# the client box cannot be read, which the caller turns into NOT EXECUTED rather
# than guessing coordinates.
function Get-InputTargets {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx)
    $box = [AgentLensQa.Win32]::ClientBoxOnScreen($Hwnd)
    if ($box -notlike 'ok:*') {
        Write-Note "cannot derive click targets: $box"
        return $null
    }
    $p = $box -split ':'
    $ox = [int]$p[1]
    $oy = [int]$p[2]
    $cw = [int]$p[3]
    $ch = [int]$p[4]
    $right = $ox + $cw
    # A point on the bar that is inside the drag region and clear of the three
    # buttons. Capped at a third of the width so a narrow window cannot push it
    # underneath the controls.
    $dragInset = [Math]::Min(300, [int]($cw / 3))
    return [ordered]@{
        origin_x   = $ox
        origin_y   = $oy
        client_w   = $cw
        client_h   = $ch
        bar_y      = $oy + [int][Math]::Round($BarPx / 2.0)
        close_x    = $right - [int][Math]::Round($BtnPx * 0.5)
        maximize_x = $right - [int][Math]::Round($BtnPx * 1.5)
        minimize_x = $right - [int][Math]::Round($BtnPx * 2.5)
        drag_x     = $ox + $dragInset
    }
}

function Format-InputTargets {
    param($T)
    return ("client={0}x{1} at ({2},{3}) bar_y={4} minimize_x={5} maximize_x={6} close_x={7} drag_x={8}" -f `
        $T.client_w, $T.client_h, $T.origin_x, $T.origin_y, $T.bar_y, `
        $T.minimize_x, $T.maximize_x, $T.close_x, $T.drag_x)
}

# Brings the window to the foreground before a gesture, through the
# AttachThreadInput handshake in Win32::ForceForeground. Recorded, not asserted:
# activation is the ENVIRONMENT a gesture runs in, not the thing under test.
#
# It stopped being optional after run h7-20260805T083710Z, where a bare
# SetForegroundWindow returned FALSE and every gesture therefore ran against an
# INACTIVE window. Button clicks survived that -- a click goes to whatever window
# is under the cursor -- but a caption drag on an inactive window is a different
# path, so leaving it unactivated meant input.drag_move was measuring activation
# as much as the drag region.
function Set-InputFocus {
    param([IntPtr]$Hwnd)
    return [AgentLensQa.Win32]::ForceForeground($Hwnd)
}

function Get-WindowRectText {
    param([IntPtr]$Hwnd)
    $r = New-Object AgentLensQa.RECT
    [void][AgentLensQa.Win32]::GetWindowRect($Hwnd, [ref]$r)
    return $r
}

# Returns the window to a normal, visible, non-iconic, non-zoomed state between
# gestures, and says out loud what it had to undo. This is cleanup AFTER an
# assertion has already recorded its own verdict -- it never changes one.
function Reset-WindowState {
    param([IntPtr]$Hwnd, [string]$Because)
    $iconic = [AgentLensQa.Win32]::IsIconic($Hwnd)
    $zoomed = [AgentLensQa.Win32]::IsZoomed($Hwnd)
    if ($iconic -or $zoomed) {
        Write-Note "normalising window state before $Because (iconic=$iconic zoomed=$zoomed) via SW_RESTORE"
        [void][AgentLensQa.Win32]::ShowWindow($Hwnd, [AgentLensQa.Win32]::SW_RESTORE)
        Start-Sleep -Milliseconds 600
        if ([AgentLensQa.Win32]::IsIconic($Hwnd)) {
            [void][AgentLensQa.Win32]::ShowWindow($Hwnd, [AgentLensQa.Win32]::SW_RESTORE)
            Start-Sleep -Milliseconds 600
        }
    }
    Write-Note ("state before {0}: iconic={1} zoomed={2} visible={3}" -f $Because, `
        [AgentLensQa.Win32]::IsIconic($Hwnd), [AgentLensQa.Win32]::IsZoomed($Hwnd), `
        [AgentLensQa.Win32]::IsWindowVisible($Hwnd))
}

function Test-RealInput {
    param([IntPtr]$Hwnd, [System.Diagnostics.Process]$Proc)
    Write-Section 'real input: SendInput against the live window'

    $inputNames = @('input.drag_move', 'input.doubleclick_maximize', 'input.button_minimize',
                    'input.button_maximize', 'input.button_close')

    # ---- DPI, because CSS pixels are only device pixels at 96 -----------------
    $dpiDesc = [AgentLensQa.Win32]::DescribeDpi($Hwnd)
    $dpi = [AgentLensQa.Win32]::EffectiveDpi($Hwnd)
    $dpiMeasured = ($dpi -gt 0)
    if (-not $dpiMeasured) { $dpi = 96 }
    $scale = [double]$dpi / 96.0
    $barPx = [int][Math]::Round($script:TitlebarCssHeight * $scale)
    $btnPx = [int][Math]::Round($script:TitlebarCssButton * $scale)
    $structSize = [AgentLensQa.Win32]::InputStructSize()
    Write-Note "DPI: $dpiDesc"
    Write-Note ("scale={0} -> titlebar {1} CSS px = {2} device px, button {3} CSS px = {4} device px" -f `
        $scale, $script:TitlebarCssHeight, $barPx, $script:TitlebarCssButton, $btnPx)
    Write-Note "sizeof(INPUT) = $structSize bytes (40 on x64, 28 on x86; SendInput rejects anything else)"
    $script:Diagnostics['input_dpi'] = $dpiDesc
    $script:Diagnostics['input_scale'] = "$scale"
    $script:Diagnostics['input_struct_size'] = "$structSize"
    Add-Assertion -Name 'input.dpi' -Class 'informational' `
        -Expected 'the scale factor the CSS-pixel titlebar geometry must be multiplied by' `
        -Observed "$dpiDesc scale=$scale bar=${barPx}px button=${btnPx}px" -Verdict 'INFO' `
        -Note $(if ($dpiMeasured) {
            'Measured, not assumed. At 96 DPI the CSS pixels in index.css and TitleBar.tsx equal device pixels, so the derived offsets are exact.'
        } else {
            'NOT measured: neither GetDpiForWindow nor GetDeviceCaps answered, so 96 DPI was ASSUMED. Read the click coordinates below with that caveat.'
        })

    $focus = Set-InputFocus -Hwnd $Hwnd
    Write-Note "focus: $focus"
    Add-Assertion -Name 'input.foreground' -Class 'informational' `
        -Expected 'the app window is the foreground window before any gesture' `
        -Observed $focus -Verdict 'INFO' `
        -Note 'Context only. Windows delivers a click to the window under the cursor regardless of activation, so this is not a verdict.'

    $targets = Get-InputTargets -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx
    if ($null -eq $targets) {
        $why = 'the client box could not be read (GetClientRect / ClientToScreen failed), so no click coordinate could be derived. Guessing one would prove nothing about where the buttons are.'
        foreach ($n in $inputNames) {
            Add-NotExecuted -Name $n -Reason $why -Expected 'a real input event and the resulting state change' `
                -Observed 'no click coordinate could be derived'
        }
        return
    }
    Write-Note ("computed targets: {0}" -f (Format-InputTargets -T $targets))
    $script:Diagnostics['input_targets'] = (Format-InputTargets -T $targets)
    Add-Assertion -Name 'input.click_targets' -Class 'informational' `
        -Expected 'button centres derived from the live client box, never hardcoded' `
        -Observed (Format-InputTargets -T $targets) -Verdict 'INFO' `
        -Note ("Offsets from the client right edge: close -{0}, maximize -{1}, minimize -{2}, all at client top + {3}. Re-derived before every gesture because a drag or a maximize moves them." -f `
            [int][Math]::Round($btnPx * 0.5), [int][Math]::Round($btnPx * 1.5), [int][Math]::Round($btnPx * 2.5), [int][Math]::Round($barPx / 2.0))

    Test-DragMove -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx
    Test-DoubleClickMaximize -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx
    Test-MinimizeButton -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx
    Test-MaximizeButton -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx
    # LAST. Nothing may depend on the window after this.
    Test-CloseButton -Hwnd $Hwnd -BarPx $barPx -BtnPx $btnPx -Proc $Proc
}

# 1. Press inside data-tauri-drag-region, travel, release. The window must follow.
#
# This is the only assertion that can prove the drag region is wired up. Tauri's
# injected drag.js matches `data-tauri-drag-region="deep"` anywhere in the
# composed path -- unless a clickable element intervenes, which is exactly how
# the three buttons stay clickable -- and on a plain mousedown invokes
# `plugin:window|start_dragging`. tao answers with ReleaseCapture plus
# WM_NCLBUTTONDOWN/HTCAPTION, which hands the window to the system move loop.
# If any link in that chain is missing the press does nothing and the window
# does not move, which no screenshot could ever reveal.
function Test-DragMove {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx)
    Write-Section 'input 1/5 -- drag the title bar with a real press-move-release'
    $name = 'input.drag_move'
    $wantDx = 120
    $wantDy = 90
    $tol = 6
    try {
        Reset-WindowState -Hwnd $Hwnd -Because 'the drag'
        $focus = Set-InputFocus -Hwnd $Hwnd
        Write-Note "focus before the drag: $focus"
        $t = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        if ($null -eq $t) {
            Add-NotExecuted -Name $name -Reason 'the client box could not be read at gesture time' `
                -Expected "the window rect moves by +$wantDx,+$wantDy" -Observed 'no coordinate'
            return
        }
        $before = Get-WindowRectText -Hwnd $Hwnd
        Write-Note ("before: rect ({0},{1})-({2},{3})" -f $before.Left, $before.Top, $before.Right, $before.Bottom)
        Write-Note ("pressing at ({0},{1}) -- inside the drag region, {2}px clear of the minimize button" -f `
            $t.drag_x, $t.bar_y, ($t.minimize_x - $t.drag_x))
        $probe = [AgentLensQa.Win32]::Drag($Hwnd, $t.drag_x, $t.bar_y, $wantDx, $wantDy, 450, 8, 40)
        Write-Note "gesture: $probe"
        $expectLeft = $before.Left + $wantDx
        $expectTop = $before.Top + $wantDy
        $settled = Wait-ForCondition -Predicate {
            $r = Get-WindowRectText -Hwnd $Hwnd
            ([Math]::Abs($r.Left - $expectLeft) -le $tol) -and ([Math]::Abs($r.Top - $expectTop) -le $tol)
        } -TimeoutMs 3000
        $after = Get-WindowRectText -Hwnd $Hwnd
        $gotDx = $after.Left - $before.Left
        $gotDy = $after.Top - $before.Top
        Write-Note ("after : rect ({0},{1})-({2},{3})" -f $after.Left, $after.Top, $after.Right, $after.Bottom)
        Write-Note ("delta : requested +$wantDx,+$wantDy  achieved +$gotDx,+$gotDy  settled_after=${settled}ms")
        $script:Diagnostics['input_drag_before'] = "$($before.Left),$($before.Top)"
        $script:Diagnostics['input_drag_after'] = "$($after.Left),$($after.Top)"
        $script:Diagnostics['input_drag_delta'] = "$gotDx,$gotDy"
        $script:Diagnostics['input_drag_probe'] = $probe
        $ok = ([Math]::Abs($gotDx - $wantDx) -le $tol) -and ([Math]::Abs($gotDy - $wantDy) -le $tol)
        Add-Assertion -Name $name -Class 'machine-checkable' `
            -Expected "window origin moves by +$wantDx,+$wantDy (+/- ${tol}px)" `
            -Observed "moved by +$gotDx,+$gotDy (from $($before.Left),$($before.Top) to $($after.Left),$($after.Top))" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
            -Note ("Real SendInput press/move/release. Focus: $focus. $probe. Settled after ${settled}ms. " +
                   "A miss here means the mousedown never reached plugin:window|start_dragging, or start_dragging " +
                   "never got the window into the system move loop. The origin_* fields say which: an origin that " +
                   "never changes at any stage means the move loop never ran at all.")
    } catch {
        Add-NotExecuted -Name $name -Reason "the measurement threw: $($_.Exception.Message)" `
            -Expected "the window rect moves by +$wantDx,+$wantDy" -Observed 'the gesture could not be completed'
    }
}

# 2. Double-click the drag region. drag.js maps `e.detail === 2` on a drag region
#    to `plugin:window|internal_toggle_maximize`, so the window must zoom, and a
#    second double-click must restore it.
function Test-DoubleClickMaximize {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx)
    Write-Section 'input 2/5 -- double-click the title bar to maximize, then restore'
    $name = 'input.doubleclick_maximize'
    try {
        Reset-WindowState -Hwnd $Hwnd -Because 'the double-click'
        [void](Set-InputFocus -Hwnd $Hwnd)
        $z0 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        $t = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        if ($null -eq $t) {
            Add-NotExecuted -Name $name -Reason 'the client box could not be read at gesture time' `
                -Expected 'IsZoomed toggles true then false' -Observed 'no coordinate'
            return
        }
        Write-Note ("double-clicking at ({0},{1}); IsZoomed before = {2}" -f $t.drag_x, $t.bar_y, $z0)
        $probe1 = [AgentLensQa.Win32]::Click($t.drag_x, $t.bar_y, 2, 40, 90)
        Write-Note "gesture 1: $probe1"
        $ms1 = Wait-ForCondition -Predicate { [AgentLensQa.Win32]::IsZoomed($Hwnd) } -TimeoutMs 4000
        $z1 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        Write-Note "IsZoomed after the first double-click = $z1 (after ${ms1}ms)"

        # Re-derive: a maximized window has a different client box, so the bar
        # point for the restoring double-click is NOT the one used above.
        $t2 = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        $probe2 = 'not attempted'
        $ms2 = -1
        if ($null -ne $t2) {
            Write-Note ("double-clicking again at ({0},{1}) to restore" -f $t2.drag_x, $t2.bar_y)
            $probe2 = [AgentLensQa.Win32]::Click($t2.drag_x, $t2.bar_y, 2, 40, 90)
            Write-Note "gesture 2: $probe2"
            $ms2 = Wait-ForCondition -Predicate { -not [AgentLensQa.Win32]::IsZoomed($Hwnd) } -TimeoutMs 4000
        }
        $z2 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        Write-Note "IsZoomed after the second double-click = $z2 (after ${ms2}ms)"
        $script:Diagnostics['input_dblclick_states'] = "$z0 -> $z1 -> $z2"
        $ok = ((-not $z0) -and $z1 -and (-not $z2))
        Add-Assertion -Name $name -Class 'machine-checkable' `
            -Expected 'IsZoomed False -> True -> False across two real double-clicks' `
            -Observed "IsZoomed $z0 -> $z1 -> $z2" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
            -Note ("first: $probe1 (zoomed after ${ms1}ms); second: $probe2 (restored after ${ms2}ms). " +
                   "Exercises drag.js e.detail===2 -> plugin:window|internal_toggle_maximize.")
    } catch {
        Add-NotExecuted -Name $name -Reason "the measurement threw: $($_.Exception.Message)" `
            -Expected 'IsZoomed toggles true then false' -Observed 'the gesture could not be completed'
    }
}

# 3. Click the computed minimize centre. IsIconic must become true; the window is
#    then restored with SW_RESTORE so the rest of the run has something to drive.
function Test-MinimizeButton {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx)
    Write-Section 'input 3/5 -- click the minimize button'
    $name = 'input.button_minimize'
    try {
        Reset-WindowState -Hwnd $Hwnd -Because 'the minimize click'
        [void](Set-InputFocus -Hwnd $Hwnd)
        $t = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        if ($null -eq $t) {
            Add-NotExecuted -Name $name -Reason 'the client box could not be read at gesture time' `
                -Expected 'IsIconic becomes true' -Observed 'no coordinate'
            return
        }
        $i0 = [AgentLensQa.Win32]::IsIconic($Hwnd)
        Write-Note ("clicking minimize at ({0},{1}); IsIconic before = {2}" -f $t.minimize_x, $t.bar_y, $i0)
        $probe = [AgentLensQa.Win32]::Click($t.minimize_x, $t.bar_y, 1, 40, 0)
        Write-Note "gesture: $probe"
        $ms = Wait-ForCondition -Predicate { [AgentLensQa.Win32]::IsIconic($Hwnd) } -TimeoutMs 4000
        $i1 = [AgentLensQa.Win32]::IsIconic($Hwnd)
        Write-Note "IsIconic after the click = $i1 (after ${ms}ms)"

        # Undo it, or nothing after this point has a window to click.
        [void][AgentLensQa.Win32]::ShowWindow($Hwnd, [AgentLensQa.Win32]::SW_RESTORE)
        $msBack = Wait-ForCondition -Predicate { -not [AgentLensQa.Win32]::IsIconic($Hwnd) } -TimeoutMs 4000
        $i2 = [AgentLensQa.Win32]::IsIconic($Hwnd)
        Write-Note "IsIconic after SW_RESTORE = $i2 (after ${msBack}ms)"
        $script:Diagnostics['input_minimize_states'] = "$i0 -> $i1 -> $i2"
        $ok = ((-not $i0) -and $i1 -and (-not $i2))
        Add-Assertion -Name $name -Class 'machine-checkable' `
            -Expected 'IsIconic False -> True from a real click, then False again after SW_RESTORE' `
            -Observed "IsIconic $i0 -> $i1 -> $i2" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
            -Note ("Clicked ($($t.minimize_x),$($t.bar_y)), the computed centre of the w-11 minimize button. " +
                   "$probe. Iconic after ${ms}ms, restored after ${msBack}ms.")
    } catch {
        Add-NotExecuted -Name $name -Reason "the measurement threw: $($_.Exception.Message)" `
            -Expected 'IsIconic becomes true' -Observed 'the gesture could not be completed'
    }
}

# 4. Click the computed maximize centre, then click it again. IsZoomed must go
#    true and then back to false, which also proves the button re-renders as the
#    restore control (TitleBar.tsx swaps Square for Copy on isMaximized).
function Test-MaximizeButton {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx)
    Write-Section 'input 4/5 -- click the maximize button, then restore with it'
    $name = 'input.button_maximize'
    try {
        Reset-WindowState -Hwnd $Hwnd -Because 'the maximize click'
        [void](Set-InputFocus -Hwnd $Hwnd)
        $t = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        if ($null -eq $t) {
            Add-NotExecuted -Name $name -Reason 'the client box could not be read at gesture time' `
                -Expected 'IsZoomed toggles true then false' -Observed 'no coordinate'
            return
        }
        $z0 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        Write-Note ("clicking maximize at ({0},{1}); IsZoomed before = {2}" -f $t.maximize_x, $t.bar_y, $z0)
        $probe1 = [AgentLensQa.Win32]::Click($t.maximize_x, $t.bar_y, 1, 40, 0)
        Write-Note "gesture 1: $probe1"
        $ms1 = Wait-ForCondition -Predicate { [AgentLensQa.Win32]::IsZoomed($Hwnd) } -TimeoutMs 4000
        $z1 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        Write-Note "IsZoomed after the click = $z1 (after ${ms1}ms)"

        $t2 = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        $probe2 = 'not attempted'
        $ms2 = -1
        if ($null -ne $t2) {
            Write-Note ("clicking restore at ({0},{1}) -- re-derived, the maximized client box moved it" -f $t2.maximize_x, $t2.bar_y)
            $probe2 = [AgentLensQa.Win32]::Click($t2.maximize_x, $t2.bar_y, 1, 40, 0)
            Write-Note "gesture 2: $probe2"
            $ms2 = Wait-ForCondition -Predicate { -not [AgentLensQa.Win32]::IsZoomed($Hwnd) } -TimeoutMs 4000
        }
        $z2 = [AgentLensQa.Win32]::IsZoomed($Hwnd)
        Write-Note "IsZoomed after the second click = $z2 (after ${ms2}ms)"
        $script:Diagnostics['input_maximize_states'] = "$z0 -> $z1 -> $z2"
        $ok = ((-not $z0) -and $z1 -and (-not $z2))
        Add-Assertion -Name $name -Class 'machine-checkable' `
            -Expected 'IsZoomed False -> True -> False from two real clicks on the maximize button' `
            -Observed "IsZoomed $z0 -> $z1 -> $z2" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
            -Note ("first click ($($t.maximize_x),$($t.bar_y)): $probe1, zoomed after ${ms1}ms; " +
                   "second click: $probe2, restored after ${ms2}ms.")
    } catch {
        Add-NotExecuted -Name $name -Reason "the measurement threw: $($_.Exception.Message)" `
            -Expected 'IsZoomed toggles true then false' -Observed 'the gesture could not be completed'
    }
}

# 5. LAST. Click the computed close centre.
#
# The observable is HIDE, not EXIT, and that is the product's real contract, not
# a softened assertion. src-tauri/src/tray.rs::handle_window_event catches
# WindowEvent::CloseRequested for the main window, calls api.prevent_close() and
# then window.hide(): AgentLens closes to its resident tray icon and keeps the
# webview alive on purpose. Asserting "the process exits" would fail a correct
# build, which is the same mistake the discarded style.WS_CAPTION premise made.
#
# Both halves are checked, so neither failure mode can hide:
#   - the window must actually become invisible: a click that landed on nothing
#     leaves it visible, and that is a FAIL;
#   - the process must still be alive: a build that ignored prevent_close would
#     die here, and that is also a FAIL.
function Test-CloseButton {
    param([IntPtr]$Hwnd, [int]$BarPx, [int]$BtnPx, [System.Diagnostics.Process]$Proc)
    Write-Section 'input 5/5 -- click the close button (LAST: it removes the window)'
    $name = 'input.button_close'
    Add-Assertion -Name 'input.close_semantics' -Class 'informational' `
        -Expected 'closing the main window hides it to the tray instead of exiting' `
        -Observed 'src-tauri/src/tray.rs handle_window_event: prevent_close + window.hide()' -Verdict 'INFO' `
        -Note 'This is why input.button_close asserts hidden-and-alive rather than process exit. The only real exit paths are the tray quit item and the debug-only tray::test_quit, which is absent from a release build.'
    try {
        Reset-WindowState -Hwnd $Hwnd -Because 'the close click'
        [void](Set-InputFocus -Hwnd $Hwnd)

        # A last visual record while there is still a window to photograph.
        $shot = Join-Path $script:OutDir 'desktop-before-close.png'
        $capture = ''
        try { $capture = [AgentLensQa.Win32]::CaptureScreen($shot) } catch { $capture = "threw: $($_.Exception.Message)" }
        Write-Note "pre-close capture: $capture"
        Add-Assertion -Name 'screenshot.before_close' -Class 'visual-only' `
            -Expected 'a PNG of the desktop with the dragged window still on it' `
            -Observed "$capture" -Verdict 'INFO' `
            -Note 'Not evidence either way; the authoritative visual channel is ec2 get-console-screenshot from the driver.'

        $t = Get-InputTargets -Hwnd $Hwnd -BarPx $BarPx -BtnPx $BtnPx
        if ($null -eq $t) {
            Add-NotExecuted -Name $name -Reason 'the client box could not be read at gesture time' `
                -Expected 'the window becomes hidden and the process survives' -Observed 'no coordinate'
            return
        }
        $v0 = [AgentLensQa.Win32]::IsWindowVisible($Hwnd)
        Write-Note ("clicking close at ({0},{1}); IsWindowVisible before = {2}" -f $t.close_x, $t.bar_y, $v0)
        $probe = [AgentLensQa.Win32]::Click($t.close_x, $t.bar_y, 1, 40, 0)
        Write-Note "gesture: $probe"
        $ms = Wait-ForCondition -Predicate {
            (-not [AgentLensQa.Win32]::IsWindowVisible($Hwnd)) -or $Proc.HasExited
        } -TimeoutMs 10000
        $v1 = [AgentLensQa.Win32]::IsWindowVisible($Hwnd)
        $exited = $Proc.HasExited
        $exitCode = if ($exited) { "$($Proc.ExitCode)" } else { 'n/a, still running' }
        Write-Note "IsWindowVisible after the click = $v1 (after ${ms}ms); process exited = $exited (exit code $exitCode)"
        $script:Diagnostics['input_close_visible'] = "$v0 -> $v1"
        $script:Diagnostics['input_close_process_exited'] = "$exited"
        $script:Diagnostics['input_close_elapsed_ms'] = "$ms"
        $ok = ($v0 -and (-not $v1) -and (-not $exited))
        Add-Assertion -Name $name -Class 'machine-checkable' `
            -Expected 'IsWindowVisible True -> False from a real click, with the process still alive (close-to-tray)' `
            -Observed "IsWindowVisible $v0 -> $v1, process_exited=$exited (exit code $exitCode)" `
            -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
            -Note ("Clicked ($($t.close_x),$($t.bar_y)), the computed centre of the w-11 close button. $probe. " +
                   "State changed after ${ms}ms. Hidden-and-alive is the documented contract in src-tauri/src/tray.rs; " +
                   "a still-visible window means the click hit nothing, and an exited process means prevent_close did not run.")
    } catch {
        Add-NotExecuted -Name $name -Reason "the measurement threw: $($_.Exception.Message)" `
            -Expected 'the window becomes hidden and the process survives' -Observed 'the gesture could not be completed'
    }
}

function Test-WindowIconHandle {
    param([IntPtr]$Hwnd)
    Write-Section 'window icon slots (informational)'

    # ---- why this is INFO and not a verdict --------------------------------
    # A NULL icon handle here is NOT a defect, and asserting non-NULL on it was
    # wrong. WM_GETICON returns what the window published with WM_SETICON, and
    # GCLP_HICON/GCLP_HICONSM return what the window CLASS was registered with.
    # A window that does neither reports NULL -- and Windows then falls back to
    # the EXECUTABLE's icon resource for the taskbar button and Alt+Tab, which is
    # the documented behaviour and produces a perfectly correct icon on screen.
    #
    # So the three NULL slots are recorded as evidence about HOW the icon is
    # supplied, and the assertions that actually prove the icon are elsewhere:
    #   icon.frames          -- the 6 RT_ICON payloads inside the installed
    #                           binary match their known digests byte for byte
    #   icon.shell_resolves  -- the shell's own extraction path resolves a real,
    #                           sized icon out of that binary, which is exactly
    #                           what the taskbar and Alt+Tab do
    # What is NOT proven by any of this: that a human saw the right picture in
    # the taskbar. Nothing in this QA looks at the taskbar.
    $slots = New-Object System.Collections.ArrayList
    $res = [IntPtr]::Zero
    [void][AgentLensQa.Win32]::SendMessageTimeout($Hwnd, [AgentLensQa.Win32]::WM_GETICON,
        [IntPtr][AgentLensQa.Win32]::ICON_BIG, [IntPtr]::Zero,
        [AgentLensQa.Win32]::SMTO_ABORTIFHUNG, 2000, [ref]$res)
    $null = $slots.Add(('WM_GETICON(ICON_BIG)=0x{0:X}' -f [int64]$res))
    $got = $res
    $via = 'WM_GETICON(ICON_BIG)'

    $cls = [AgentLensQa.Win32]::GetClassLongPtr($Hwnd, [AgentLensQa.Win32]::GCLP_HICON)
    $null = $slots.Add(('GCLP_HICON=0x{0:X}' -f [int64]$cls))
    if ($got -eq [IntPtr]::Zero) { $got = $cls; $via = 'GetClassLongPtr(GCLP_HICON)' }

    $clsSm = [AgentLensQa.Win32]::GetClassLongPtr($Hwnd, [AgentLensQa.Win32]::GCLP_HICONSM)
    $null = $slots.Add(('GCLP_HICONSM=0x{0:X}' -f [int64]$clsSm))
    if ($got -eq [IntPtr]::Zero) { $got = $clsSm; $via = 'GetClassLongPtr(GCLP_HICONSM)' }

    $script:Diagnostics['icon_handle'] = ('0x{0:X}' -f [int64]$got)
    $script:Diagnostics['icon_handle_via'] = $via
    $script:Diagnostics['icon_window_slots'] = ($slots -join ' ')
    Add-Assertion -Name 'icon.handle' -Class 'informational' `
        -Expected 'either a handle or NULL: NULL is legitimate and means the exe icon is used' `
        -Observed ($slots -join ' ') -Verdict 'INFO' `
        -Note ('First non-NULL slot: ' + $via + '. Superseded as a verdict by icon.shell_resolves and icon.frames. ' +
               'A window with no icon of its own inherits the executable icon for the taskbar and Alt+Tab.')
}

# The icon assertion that means something for what a user sees: run the shell's
# own extraction against the installed binary. This is the fallback path Windows
# takes for the taskbar button and Alt+Tab when the window publishes no icon, so
# a real, sized icon coming back is what makes that fallback correct.
#
# Stage is 'icon', not 'window': it needs the binary and not the HWND, so it is
# measurable even on a run where no window ever appeared.
function Test-ShellIconResolution {
    param([string]$ExePath)
    Write-Section 'shell icon resolution against the installed binary'
    $desc = $null
    try {
        $desc = [AgentLensQa.Win32]::ResolveExecutableIcon($ExePath)
    } catch {
        $script:Diagnostics['shell_icon'] = "threw: $($_.Exception.Message)"
        Add-NotExecuted -Name 'icon.shell_resolves' `
            -Reason "ExtractIconExW could not be invoked: $($_.Exception.Message). This is a harness/interop failure, not a finding about the binary." `
            -Expected 'at least one icon group, and a non-NULL large icon handle' `
            -Observed 'the extraction call did not complete'
        return
    }
    Write-Note $desc
    $script:Diagnostics['shell_icon'] = $desc
    # PASS needs BOTH: at least one icon group in the PE, and a large icon handle
    # the shell actually managed to realise from it. groups>0 with a NULL handle
    # would mean the resource is there but unusable, which is worth a FAIL.
    $hasGroup = ($desc -notmatch 'groups=0\b')
    $hasLarge = ($desc -notmatch 'large=0x0\b')
    $ok = ($hasGroup -and $hasLarge)
    Add-Assertion -Name 'icon.shell_resolves' -Class 'machine-checkable' `
        -Expected 'at least one icon group, and a non-NULL large icon handle' `
        -Observed $desc -Verdict $(if ($ok) { 'PASS' } else { 'FAIL' }) `
        -Note ('ExtractIconExW is the shell path the taskbar and Alt+Tab use. This proves the installed binary carries a ' +
               'realisable icon; icon.frames proves the payloads are byte-identical to src-tauri/icons/icon.ico. ' +
               'Neither proves a human saw the right picture -- nothing here inspects the taskbar.')
}

function Test-IconResources {
    param([string]$ExePath)
    Write-Section 'RT_ICON payloads in the installed binary (byte-exact)'
    $blobs = $null
    try {
        $blobs = [AgentLensQa.Win32]::ReadIconResources($ExePath)
    } catch {
        Add-Assertion -Name 'icon.frames' -Class 'machine-checkable' -Expected '6/6 known digests' `
            -Observed "resource read failed: $($_.Exception.Message)" -Verdict 'FAIL'
        return
    }
    Write-Note "RT_ICON resources found: $($blobs.Count)"
    $observed = @{}
    foreach ($b in $blobs) {
        $h = Get-Sha256OfBytes -Bytes $b
        $observed[$h] = $b.Length
        Write-Note ("  {0,6} bytes  {1}" -f $b.Length, $h)
    }
    $script:Diagnostics['rt_icon_count'] = $blobs.Count
    $script:Diagnostics['rt_icon_digests'] = ($observed.Keys -join ',')

    $hits = 0
    foreach ($f in $script:ExpectedIconFrames) {
        $found = $observed.ContainsKey($f.Sha)
        if ($found) { $hits++ }
        Add-Assertion -Name "icon.frame.$($f.Size)" -Class 'machine-checkable' `
            -Expected "$($f.Sha) ($($f.Bytes) bytes)" `
            -Observed $(if ($found) { "present, $($observed[$f.Sha]) bytes" } else { 'absent' }) `
            -Verdict $(if ($found) { 'PASS' } else { 'FAIL' })
    }
    Add-Assertion -Name 'icon.frames' -Class 'machine-checkable' -Expected '6/6' `
        -Observed "$hits/6" -Verdict $(if ($hits -eq 6) { 'PASS' } else { 'FAIL' }) `
        -Note 'Digests are of the RT_ICON payloads from src-tauri/icons/icon.ico. 6/6 is a byte-level icon proof.'
}

function Save-Screenshots {
    param([IntPtr]$Hwnd)
    Write-Section 'in-guest screenshots (best effort)'
    $shot = Join-Path $script:OutDir 'desktop-session.png'
    $outcome = ''
    try {
        $outcome = [AgentLensQa.Win32]::CaptureScreen($shot)
    } catch {
        $outcome = "threw: $($_.Exception.Message)"
    }
    Write-Note "capture: $outcome"
    $script:Diagnostics['in_guest_capture'] = $outcome
    $exists = Test-Path -LiteralPath $shot
    $bytes = if ($exists) { (Get-Item -LiteralPath $shot).Length } else { 0 }
    Add-Assertion -Name 'screenshot.in_guest' -Class 'visual-only' `
        -Expected 'a PNG, but expected to be black or to fail from session 0' `
        -Observed "$outcome ($bytes bytes)" -Verdict 'INFO' `
        -Note 'Not evidence either way. The authoritative visual channel is ec2 get-console-screenshot from the driver.'
}

function Save-Diagnostics {
    if (-not (Test-Path -LiteralPath $script:OutDir)) { return }
    Write-Note 'collecting diagnostics'
    # Listing %LOCALAPPDATA%\AgentLens directly would report "absent" on this
    # instance even though the install succeeded, for exactly the WOW64 reason
    # documented above Resolve-InstallDir: the 32-bit installer wrote under
    # SysWOW64 while this 64-bit process sees the real System32. So the resolved
    # directory is listed when it is known, and BOTH forms of the profile path
    # are probed when it is not -- a "absent" line here previously sent an
    # investigation in the wrong direction and must not do so again.
    try {
        $targets = New-Object System.Collections.ArrayList
        if ($script:Diagnostics.Contains('install_dir_resolved')) {
            $null = $targets.Add($script:Diagnostics['install_dir_resolved'])
        } else {
            foreach ($v in (Get-Wow64PathVariants -Path (Join-Path $env:LOCALAPPDATA $script:ProductName))) {
                $null = $targets.Add($v.Path)
            }
        }
        $lines = New-Object System.Collections.ArrayList
        foreach ($t in $targets) {
            if (Test-Path -LiteralPath $t) {
                $null = $lines.Add("== $t")
                foreach ($f in (Get-ChildItem -LiteralPath $t -Recurse -ErrorAction SilentlyContinue)) {
                    $null = $lines.Add("$($f.Length)`t$($f.FullName)")
                }
            } else {
                $null = $lines.Add("== absent: $t")
            }
        }
        $script:Diagnostics['install_dir_listing'] = ($lines -join "`n")
    } catch { $script:Diagnostics['install_dir_listing'] = "error: $($_.Exception.Message)" }

    foreach ($pair in @(@{ L = 'Application'; N = 'eventlog_application' }, @{ L = 'System'; N = 'eventlog_system' })) {
        try {
            $ev = Get-WinEvent -LogName $pair.L -MaxEvents 200 -ErrorAction Stop |
                Where-Object {
                    $_.LevelDisplayName -in @('Error', 'Critical', 'Warning') -and
                    ($_.Message -match 'AgentLens' -or $_.ProviderName -match 'Application Error|Windows Error Reporting|\.NET Runtime|WebView2')
                } |
                Select-Object -First 25
            $script:Diagnostics[$pair.N] =
                (($ev | ForEach-Object { "$($_.TimeCreated.ToString('s'))  $($_.LevelDisplayName)  $($_.ProviderName)  $($_.Id)`n$($_.Message)" }) -join "`n---`n")
        } catch { $script:Diagnostics[$pair.N] = "error: $($_.Exception.Message)" }
    }

    try {
        $crash = Get-ChildItem -LiteralPath $env:LOCALAPPDATA -Recurse -Directory -Filter '*Crashpad*' -ErrorAction SilentlyContinue |
            Select-Object -First 5 -ExpandProperty FullName
        $script:Diagnostics['crashpad_dirs'] = if ($crash) { $crash -join "`n" } else { 'none' }
    } catch { $script:Diagnostics['crashpad_dirs'] = "error: $($_.Exception.Message)" }

    try {
        $sm = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' -ErrorAction SilentlyContinue
        $script:Diagnostics['smartscreen_policy'] =
            if ($null -ne $sm -and ($sm.PSObject.Properties.Name -contains 'EnableSmartScreen')) { "EnableSmartScreen=$($sm.EnableSmartScreen)" } else { 'not set' }
        $script:Diagnostics['smartscreen_note'] =
            'The installer is UNSIGNED. Server has no SmartScreen reputation history and different defaults from client Windows, so no prompt here is NOT evidence of no prompt on an end user Windows 11 machine.'
    } catch { $script:Diagnostics['smartscreen_policy'] = "error: $($_.Exception.Message)" }

    $path = Join-Path $script:OutDir 'diagnostics.json'
    ($script:Diagnostics | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath $path -Encoding ASCII
    Write-Note "wrote $path"
}

function Save-Results {
    if (-not (Test-Path -LiteralPath $script:OutDir)) { return }

    # ---- absence must not read as success -----------------------------------
    # The old body computed the verdict from $script:Results alone: pass = count
    # of PASS, fail = count of FAIL, verdict = PASS when fail was 0. On a run
    # that aborted early that arithmetic is 6 pass / 0 fail / verdict PASS, while
    # the window was never even launched. The fix is to diff what ran against the
    # declared expected set, so a missing measurement is visible AS missing.
    $executed = @{}
    foreach ($r in $script:Results) {
        if ($r.class -eq 'machine-checkable') { $executed[$r.name] = $r.verdict }
    }

    # Synthesised records for expected assertions that left no trace at all --
    # the run died before reaching them. Deliberate, reasoned skips already
    # recorded themselves through Add-NotExecuted and are not duplicated here.
    $synthetic = New-Object System.Collections.ArrayList
    foreach ($e in $script:ExpectedAssertions) {
        if ($executed.ContainsKey($e.Name)) { continue }
        $null = $synthetic.Add([ordered]@{
            name     = $e.Name
            class    = 'machine-checkable'
            expected = $e.Why
            observed = 'no measurement was taken'
            verdict  = $script:VerdictNotExecuted
            note     = "The run ended before the '$($e.Stage)' stage could execute this assertion. Absence of a result is NOT evidence of success."
        })
    }

    $all = @($script:Results) + @($synthetic)
    $machine = @($all | Where-Object { $_.class -eq 'machine-checkable' })
    $pass = @($machine | Where-Object { $_.verdict -eq 'PASS' }).Count
    $fail = @($machine | Where-Object { $_.verdict -eq 'FAIL' }).Count
    $notRun = @($machine | Where-Object { $_.verdict -eq $script:VerdictNotExecuted })
    $missingNames = @($notRun | ForEach-Object { $_.name })

    # PASS requires every declared assertion to have RUN and PASSED. Anything
    # short of that is FAIL or INCOMPLETE -- never PASS.
    #
    # FAIL is checked BEFORE the gap check on purpose. A run that both failed an
    # assertion and left gaps must not report the milder INCOMPLETE: hiding a
    # real failure behind "we did not finish" would be the same class of defect
    # as the one this rewrite exists to remove, just pointing the other way. The
    # `complete` flag below carries the gap information independently, so nothing
    # is lost either way.
    $expectedNames = @($script:ExpectedAssertions | ForEach-Object { $_.Name })
    $ranAndPassed = @($expectedNames | Where-Object { $executed.ContainsKey($_) -and $executed[$_] -eq 'PASS' }).Count
    $verdict = if ($fail -gt 0) {
        'FAIL'
    } elseif ($missingNames.Count -gt 0) {
        'INCOMPLETE'
    } elseif ($ranAndPassed -eq $expectedNames.Count) {
        'PASS'
    } else {
        'INCOMPLETE'
    }

    $doc = [ordered]@{
        run_id            = $script:RunId
        generated_utc     = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        verdict           = $verdict
        complete          = ($missingNames.Count -eq 0)
        verdict_rule      = "PASS requires all $($expectedNames.Count) declared machine-checkable assertions to have RUN and PASSED. " +
                            'An assertion that produced no measurement is recorded as NOT EXECUTED, sets complete=false, and ' +
                            'yields INCOMPLETE (or FAIL, if something also genuinely failed). ' +
                            'AN ABORTED RUN CANNOT BE A PASS: absence of a result is not evidence of success, and a verdict ' +
                            'computed only from the records that happen to exist can never notice the ones that do not.'
        machine_checkable = [ordered]@{
            expected      = $expectedNames.Count
            pass          = $pass
            fail          = $fail
            not_executed  = $missingNames.Count
            recorded      = $machine.Count
        }
        not_executed      = @($notRun | ForEach-Object { [ordered]@{ name = $_.name; reason = $_.note } })
        assertions        = @($all)
    }
    $script:Summary = $doc
    $path = Join-Path $script:OutDir 'results.json'
    ($doc | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath $path -Encoding ASCII
    Write-Note "wrote $path"
    Write-Note ("verdict $verdict -- machine-checkable: $pass pass / $fail fail / $($missingNames.Count) NOT EXECUTED of $($expectedNames.Count) expected")
    if ($missingNames.Count -gt 0) {
        Write-Note 'NOT EXECUTED (no measurement was taken -- this is NOT a pass):'
        foreach ($n in $notRun) { Write-Note "  - $($n.name): $($n.note)" }
    }
}

function Send-OutputToS3 {
    if (-not (Test-Path -LiteralPath $script:OutDir)) { return }
    # This runs on the failure path too, so it must never throw and must never
    # re-enter Stop-WithError: losing the artifacts is bad, but masking the
    # original error with an upload error is worse. Hence the local try/catch and
    # the direct Get-Command probe instead of calling Assert-AwsPowerShell.
    if (-not (Get-Command Write-S3Object -ErrorAction SilentlyContinue)) {
        Write-Host 'error: Write-S3Object is unavailable; results stay on the instance only.' -ForegroundColor Red
        Write-Host "    local copy: $script:OutDir" -ForegroundColor Red
        return
    }
    $prefix = "$script:OutPrefix/$script:RunId"
    Write-Note "uploading $script:OutDir -> s3://$script:Bucket/$prefix/ (region $script:Region)"
    # There is no Sync-S3Object, so each file is uploaded individually. The key is
    # built from the path relative to OutDir so nested files keep their layout.
    $base = (Resolve-Path -LiteralPath $script:OutDir).Path.TrimEnd('\')
    $sent = 0
    $failed = 0
    foreach ($f in (Get-ChildItem -LiteralPath $script:OutDir -Recurse -File -ErrorAction SilentlyContinue)) {
        $rel = $f.FullName.Substring($base.Length).TrimStart('\').Replace('\', '/')
        try {
            Write-S3Object -BucketName $script:Bucket -Key "$prefix/$rel" `
                -File $f.FullName -Region $script:Region | Out-Null
            $sent++
        } catch {
            $failed++
            Write-Host "error: failed to upload $rel -- $($_.Exception.Message)" -ForegroundColor Red
        }
    }
    if ($failed -gt 0) {
        Write-Host "error: $failed of $($sent + $failed) file(s) failed to upload to s3://$script:Bucket/$prefix/" -ForegroundColor Red
        Write-Host '    The instance profile likely lacks s3:PutObject on this prefix.' -ForegroundColor Red
    } else {
        Write-Note "upload ok ($sent file(s))"
    }
}

# =============================================================================
# THE SESSION-0 SUPERVISOR.
#
# Two earlier runs of this QA launched AgentLens successfully and measured
# nothing, for the same structural reason both times: SSM Run Command executes as
# SYSTEM in session 0, on window station Service-0x0-3e7$, which is reserved for
# services and "does not support processes that interact with the user". A Tauri
# window IS a WebView2 window, and the WebView2 browser process is exactly what
# session 0 will not host -- msedgewebview2.exe count stayed 0 while the app
# process itself lived, logged its tray icon and created its ordinary USER32
# helper HWNDs. So USER32 window creation was never the blocker; the webview host
# was, and no amount of retrying inside session 0 changes that.
#
# The fix is not to try harder in session 0. It is to run the measurement in a
# session that has a desktop. scripts/qa/ec2-windows-gui-qa.sh 'interactive'
# establishes one via Windows autologon, and this supervisor hands the whole run
# over to it.
#
# WHY THE MEASUREMENT ITSELF HAS TO MOVE, not just the app launch. EnumWindows
# enumerates the windows of the CALLING thread's desktop only. A session-0 process
# cannot see, let alone measure, an HWND that belongs to session 1. Launching the
# app into session 1 while measuring from session 0 would therefore report "no
# window" for a window that is on screen -- a false accusation against the app,
# which is precisely the class of error this QA exists to avoid. So the entire
# script re-executes in the interactive session and the session-0 process becomes
# a supervisor that starts it, waits, and propagates its exit code.
#
# HOW the handoff crosses the session boundary: a scheduled task registered with
# -LogonType Interactive. That logon type needs no password at registration and
# runs the task inside the named user's existing interactive session, which is
# what autologon has already created. The alternative -- WTSQueryUserToken plus
# CreateProcessAsUser -- would work too and was rejected only because it needs a
# P/Invoke surface and a duplicated token for no additional guarantee.
#
# The child proves its own session id from inside its own process
# (Write-PreFlight reads Process.GetCurrentProcess().SessionId) and the app's
# session id from the app's own process object (Start-App). Neither is inferred
# from this supervisor.
# =============================================================================

function Write-Supervisor {
    param([string]$Message)
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $line = "[$stamp] supervisor: $Message"
    Write-Host $line
    try {
        Add-Content -LiteralPath $script:SupervisorLog -Value $line -Encoding ASCII
    } catch {
        # The supervisor log is a convenience, never a gate.
    }
}

# The interactive logon session, or $null. explorer.exe is the probe rather than
# qwinsta because it answers both halves of the question in one shot: a shell
# process outside session 0 means a real desktop exists, and its owner is the
# account a scheduled task must name. qwinsta reports a session id but not the
# account, and it lists console session 1 as existing even when nobody is logged
# on -- which is exactly the state this box was in before autologon.
function Get-InteractiveSession {
    $procs = @()
    try {
        $procs = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop |
            Where-Object { "$($_.Name)" -eq 'explorer.exe' })
    } catch {
        return $null
    }
    foreach ($p in $procs) {
        if ([int]$p.SessionId -eq 0) { continue }
        $owner = $null
        try {
            $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction Stop
        } catch {
            continue
        }
        if ($null -eq $owner) { continue }
        if ([int]$owner.ReturnValue -ne 0) { continue }
        $user = "$($owner.User)".Trim()
        if (-not $user) { continue }
        $domain = "$($owner.Domain)".Trim()
        if (-not $domain) { $domain = $env:COMPUTERNAME }
        return @{ SessionId = [int]$p.SessionId; User = "$domain\$user"; ShellPid = [int]$p.ProcessId }
    }
    return $null
}

# The settings a scheduled task cannot inherit, written where the child's
# Import-Handoff will read them. No credential is written here.
function Write-Handoff {
    $lines = @(
        '# Written by the session-0 supervisor of ec2-windows-gui-qa.ps1.',
        '# Read by Import-Handoff in the interactive child, which a scheduled',
        '# task starts with an environment that carries none of this.',
        "AGENTLENS_QA_BUCKET=$script:Bucket",
        "AGENTLENS_QA_KEY=$script:Key",
        "AGENTLENS_QA_OUT_PREFIX=$script:OutPrefix",
        "AGENTLENS_QA_SHA256=$script:ExpectedSha",
        "AGENTLENS_QA_RUN_ID=$script:RunId",
        "AGENTLENS_QA_REGION=$script:Region"
    )
    Set-Content -LiteralPath $script:HandoffPath -Value $lines -Encoding ASCII
}

# Uploads only the supervisor log. The child has already uploaded results.json,
# diagnostics.json and qa.log through Send-OutputToS3, and re-uploading them from
# here would race the child's own writes for no benefit. Deliberately does not
# call Assert-AwsPowerShell: that one writes into Diagnostics and through
# Write-Note into the CHILD's qa.log, which this process must not touch.
function Send-SupervisorLogToS3 {
    if (-not (Test-Path -LiteralPath $script:SupervisorLog)) { return }
    foreach ($m in @('AWSPowerShell', 'AWSPowerShell.NetCore', 'AWS.Tools.S3')) {
        if (Get-Module -ListAvailable -Name $m) {
            Import-Module $m -ErrorAction SilentlyContinue
            break
        }
    }
    if (-not (Get-Command Write-S3Object -ErrorAction SilentlyContinue)) {
        Write-Host 'supervisor: Write-S3Object is unavailable; supervisor.log stays on the instance only.'
        return
    }
    try {
        Write-S3Object -BucketName $script:Bucket -Key "$script:OutPrefix/$script:RunId/supervisor.log" `
            -File $script:SupervisorLog -Region $script:Region | Out-Null
        Write-Host 'supervisor: uploaded supervisor.log'
    } catch {
        Write-Host "supervisor: failed to upload supervisor.log -- $($_.Exception.Message)"
    }
}

# Returns $null when this process should run the QA itself, or an exit code when
# the interactive child ran and this process is only a supervisor. Exits directly
# on the one outcome that must not fall back: a child that started and then did
# not finish, where running inline would reinstall over a half-finished run and
# clobber its artifacts.
function Invoke-SessionHandoff {
    $sessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    if ($sessionId -ne 0) { return $null }

    New-Item -ItemType Directory -Force -Path $script:WorkDir | Out-Null
    New-Item -ItemType Directory -Force -Path $script:OutDir | Out-Null

    Write-Supervisor "this process is in session 0; a WebView2 window cannot be composited here."
    $sess = Get-InteractiveSession
    if ($null -eq $sess) {
        $script:HandoffNote = 'no interactive logon session existed (no explorer.exe outside session 0), so this run measured session 0 and the window assertions could not be executed. Establish one with: scripts/qa/ec2-windows-gui-qa.sh interactive'
        Write-Supervisor 'no interactive logon session found: no explorer.exe outside session 0.'
        Write-Supervisor 'the window assertions will be NOT EXECUTED for this run.'
        Write-Supervisor 'establish one first: scripts/qa/ec2-windows-gui-qa.sh interactive'
        Write-Supervisor 'running inline in session 0.'
        return $null
    }
    Write-Supervisor "interactive session found: id=$($sess.SessionId) user=$($sess.User) (explorer.exe pid $($sess.ShellPid))"

    $me = $PSCommandPath
    if (-not $me) { $me = "$($MyInvocation.MyCommand.Path)" }
    if (-not $me -or -not (Test-Path -LiteralPath $me)) {
        $script:HandoffNote = "an interactive session (id $($sess.SessionId), user $($sess.User)) was present but this script's own path could not be resolved, so it could not relaunch itself there; this run measured session 0."
        Write-Supervisor 'cannot resolve this script path; running inline in session 0.'
        return $null
    }
    Write-Supervisor "relaunching this script in session $($sess.SessionId): $me"

    $handoffWrittenAt = Get-Date
    try {
        Write-Handoff
        Write-Supervisor "wrote the settings handoff: $script:HandoffPath"
    } catch {
        $script:HandoffNote = "an interactive session (id $($sess.SessionId), user $($sess.User)) was present but the settings handoff file could not be written ($($_.Exception.Message)), so this run measured session 0."
        Write-Supervisor "could not write the handoff file: $($_.Exception.Message)"
        Write-Supervisor 'running inline in session 0.'
        return $null
    }

    $registered = $false
    try {
        $arg = '-NoProfile -ExecutionPolicy Bypass -File "' + $me + '"'
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg -WorkingDirectory $script:WorkDir
        $principal = New-ScheduledTaskPrincipal -UserId $sess.User -LogonType Interactive -RunLevel Highest
        $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) `
            -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
        Register-ScheduledTask -TaskName $script:HandoffTaskName -Action $action `
            -Principal $principal -Settings $settings -Force | Out-Null
        Start-ScheduledTask -TaskName $script:HandoffTaskName
        $registered = $true
    } catch {
        Write-Supervisor "the scheduled-task handoff FAILED: $($_.Exception.Message)"
    }
    if (-not $registered) {
        $script:HandoffNote = "an interactive session (id $($sess.SessionId), user $($sess.User)) WAS present, but registering or starting the -LogonType Interactive scheduled task '$script:HandoffTaskName' failed, so this run measured session 0 instead. This is an environment/tooling failure of the handoff, NOT a finding about AgentLens."
        Write-Supervisor 'running inline in session 0.'
        return $null
    }
    Write-Supervisor "started scheduled task '$script:HandoffTaskName' as $($sess.User); waiting for it."

    # Completion is detected from the STATE TRANSITION Running -> not Running, and
    # deliberately NOT from LastRunTime.
    #
    # Run h7-20260805T060925Z is why. This task carries no trigger -- it is
    # registered and then started by hand -- and for a triggerless task the
    # scheduler reports a sentinel LastRunTime (an epoch far in the past, not the
    # actual start), so a "did it run since I started it" test on LastRunTime can
    # never become true. That run's child finished at 06:14:28 and this loop kept
    # waiting until 06:37:23 with the state already sitting at 'Ready', then
    # reported "did NOT finish" and exited 1 -- turning a completed run with real
    # measurements into a spurious harness failure. The state transition is the
    # fact that matters and it is unambiguous. LastRunTime is now recorded for
    # information only and gates nothing.
    $deadline = (Get-Date).AddMinutes(25)
    $startDeadline = (Get-Date).AddSeconds(180)
    $state = 'unknown'
    $lastRun = $null
    $lastResult = $null
    $sawRunning = $false
    $finished = $false
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 5
        try {
            $task = Get-ScheduledTask -TaskName $script:HandoffTaskName -ErrorAction Stop
            $state = "$($task.State)"
            $info = Get-ScheduledTaskInfo -TaskName $script:HandoffTaskName -ErrorAction Stop
            $lastRun = $info.LastRunTime
            $lastResult = $info.LastTaskResult
        } catch {
            continue
        }
        if ($state -eq 'Running') {
            if (-not $sawRunning) { Write-Supervisor 'the interactive task is Running.' }
            $sawRunning = $true
            continue
        }
        if ($sawRunning) {
            $finished = $true
            break
        }
        # Secondary signal, for a child that starts and exits entirely inside one
        # 5s sleep so 'Running' is never observed. A qa.log touched after the
        # handoff file was written can only have been written by this child.
        if ((Test-Path -LiteralPath $script:LogPath) -and
            (Get-Item -LiteralPath $script:LogPath).LastWriteTime -gt $handoffWrittenAt) {
            Write-Supervisor 'the interactive task already touched qa.log; treating it as started.'
            $finished = $true
            break
        }
        if ((Get-Date) -gt $startDeadline) { break }
    }
    Write-Supervisor "task bookkeeping: state='$state' LastTaskResult=$lastResult LastRunTime=$lastRun (LastRunTime is informational; a triggerless task reports a sentinel value for it)"

    if (-not $finished -and -not $sawRunning) {
        $script:HandoffNote = "an interactive session (id $($sess.SessionId), user $($sess.User)) WAS present and the scheduled task '$script:HandoffTaskName' registered, but it never entered the Running state within 180s (last state '$state'), so this run measured session 0 instead. This is an environment/tooling failure of the handoff, NOT a finding about AgentLens."
        Write-Supervisor "the interactive task never started (last state '$state'); running inline in session 0."
        return $null
    }

    if (-not $finished) {
        Write-Supervisor "the interactive task started but did NOT finish within 25 minutes (last state '$state')."
        Write-Supervisor 'NOT falling back to session 0: a second inline run would reinstall over the'
        Write-Supervisor 'half-finished interactive run and overwrite its artifacts. Whatever the child'
        Write-Supervisor 'did write is left intact on the instance and in S3.'
        try {
            Stop-ScheduledTask -TaskName $script:HandoffTaskName -ErrorAction SilentlyContinue
        } catch {
            # Best effort.
        }
        Send-SupervisorLogToS3
        exit 1
    }

    Write-Supervisor "the interactive task finished: LastTaskResult=$lastResult (last state '$state')."

    # The child owns qa.log. Echoing its tail is the only way the SSM inline
    # output says anything at all about what happened, since the child's stdout
    # went to a scheduled task and not to this invocation.
    if (Test-Path -LiteralPath $script:LogPath) {
        Write-Host '--- tail of the interactive run qa.log ---'
        foreach ($l in (Get-Content -LiteralPath $script:LogPath -Tail 60 -ErrorAction SilentlyContinue)) {
            Write-Host "$l"
        }
        Write-Host '--- end of tail; the full log is in the S3 artifacts ---'
    } else {
        Write-Supervisor "the child wrote no qa.log at $script:LogPath -- it may have failed before Initialize-Workspace."
    }

    $verdict = 'UNKNOWN (no results.json)'
    $resultsPath = Join-Path $script:OutDir 'results.json'
    if (Test-Path -LiteralPath $resultsPath) {
        try {
            $verdict = "$((Get-Content -LiteralPath $resultsPath -Raw | ConvertFrom-Json).verdict)"
        } catch {
            $verdict = "UNPARSEABLE: $($_.Exception.Message)"
        }
    }
    Write-Supervisor "child verdict from results.json: $verdict"
    Send-SupervisorLogToS3

    $code = 1
    if ($null -ne $lastResult) { $code = [int]$lastResult }
    return $code
}

# --- main --------------------------------------------------------------------

function Invoke-Main {
    # Before anything else: if this is session 0 and a real desktop exists, the
    # measurement belongs over there, not here.
    $handoff = Invoke-SessionHandoff
    if ($null -ne $handoff) { exit ([int]$handoff) }

    Initialize-Workspace
    Write-PreFlight
    Assert-AwsPowerShell
    Install-WebView2
    $setup = Get-Installer
    Install-App -SetupPath $setup
    $dir = Resolve-InstallDir
    $exe = Resolve-MainBinary -InstallDir $dir
    Test-IconResources -ExePath $exe
    Test-ShellIconResolution -ExePath $exe
    $proc = Start-App -ExePath $exe
    try {
        $hwnd = Wait-ForWindow -Proc $proc
        if ($hwnd -ne [IntPtr]::Zero) {
            Test-WindowStyle -Hwnd $hwnd
            Test-WindowGeometry -Hwnd $hwnd
            Test-WindowTitle -Hwnd $hwnd
            Test-WindowIconHandle -Hwnd $hwnd
            Test-MinimumSize -Hwnd $hwnd
            Save-Screenshots -Hwnd $hwnd
            Test-Maximize -Hwnd $hwnd
            # Real input goes last of the window work, and inside it the close
            # click goes last of all: it is the only assertion that removes the
            # window, so anything scheduled after it would be measuring nothing.
            Test-RealInput -Hwnd $hwnd -Proc $proc
        } else {
            # No HWND. Wait-ForWindow has already recorded WHY -- either NOT
            # EXECUTED because session 0 cannot host a webview window, or FAIL
            # because an interactive session could have and the app did not.
            # Either way the dependent assertions get an explicit NOT EXECUTED
            # record naming themselves, so the gap is visible instead of silent.
            $w = @($script:Results | Where-Object { $_.name -eq 'window.exists' })
            $why = if ($w.Count -gt 0 -and $w[0].verdict -eq $script:VerdictNotExecuted) {
                'No HWND was available: the environment cannot host a webview window (see window.exists).'
            } else {
                'No HWND was available: window.exists FAILED, so there was nothing to measure (see window.exists).'
            }
            Add-WindowAssertionSkips -Reason $why
        }
    } finally {
        if (-not $proc.HasExited) {
            Write-Note "stopping pid $($proc.Id)"
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        }
    }

    Save-Diagnostics
    Save-Results
    Send-OutputToS3

    Write-Section 'verdict'
    $verdict = if ($null -ne $script:Summary) { $script:Summary.verdict } else { 'FAIL' }
    $machine = @($script:Results | Where-Object { $_.class -eq 'machine-checkable' })
    foreach ($f in ($machine | Where-Object { $_.verdict -eq 'FAIL' })) {
        Write-Host "FAIL $($f.name): expected=$($f.expected) observed=$($f.observed)" -ForegroundColor Red
    }
    if ($verdict -eq 'PASS') {
        Write-Note "PASS -- all $($script:ExpectedAssertions.Count) expected machine-checkable assertions ran and passed"
        Write-Note 'Visual claims remain unproven by this script by design; see results.json class=visual-only.'
        return
    }
    # Send-OutputToS3 already ran, so the artifacts are safe before this exits.
    Stop-WithError "verdict $verdict -- see results.json" (
        "This run did not produce a clean pass. results.json carries the authoritative breakdown: " +
        "verdict, the pass/fail/not_executed counts, and the named list of assertions that produced " +
        "no measurement. A NOT EXECUTED assertion is neither a pass nor a failure of AgentLens -- it " +
        "means this run did not measure it, and an aborted or environment-limited run cannot be a pass."
    )
}

Invoke-Main
