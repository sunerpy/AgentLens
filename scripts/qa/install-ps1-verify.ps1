# =============================================================================
# In-guest verifier for scripts/install.ps1 -- the SHIPPED Windows installer
# bootstrap, run on real Windows for the first time.
#
# WHAT THIS FILE IS AND IS NOT
#   This is a TEST DRIVER. scripts/install.ps1 is the ARTIFACT UNDER TEST and is
#   copied to this machine byte-for-byte and executed unmodified, as a separate
#   powershell.exe process, so its `exit 1` is observable as a real process exit
#   code rather than as a swallowed function return. This driver never sources,
#   patches or reimplements any part of it. Both sides record install.ps1's
#   SHA-256 so the report can prove which bytes ran.
#
# WHY A LOCAL HTTP MIRROR
#   install.ps1 defaults to github.com/sunerpy/AgentLens, which does not exist
#   yet, so every default URL 404s. AGENTLENS_BASE_URL is the script's own
#   documented mirror seam (install.ps1:56, :299), so the real CodeBuild NSIS
#   installer is served from a loopback System.Net.HttpListener together with a
#   sha256sums-windows.txt written in the exact shape Read-Manifest parses
#   (install.ps1:239-241): optional '#' comments, then "<64 hex>  <basename>",
#   CRLF, ASCII. Loopback http is deliberate: it is also what makes the
#   non-https refusal at install.ps1:213 testable.
#
# WHY SESSION 1
#   SSM runs as SYSTEM in session 0. The NSIS installer is installMode
#   currentUser, so from session 0 it installs into the SERVICE profile, which is
#   not where a user installs and would make the whole measurement
#   unrepresentative. So this script re-executes itself inside the existing
#   interactive session with a -LogonType Interactive scheduled task -- the same
#   crossing scripts/qa/ec2-windows-gui-qa.ps1 uses and for the same reason. The
#   child proves its own session id from inside its own process
#   (Process.GetCurrentProcess().SessionId); nothing is inferred by the
#   supervisor. There is NO fallback to session 0: a session-0 result would be a
#   measurement of the wrong thing, so this refuses instead.
#
# WHY A WIZARD DRIVER
#   install.ps1:362 is `Start-Process -FilePath $assetPath -PassThru -Wait` with
#   NO arguments. The real Tauri NSIS installer therefore shows its wizard
#   ("AgentLens Setup", dialog class #32770, Next = control id 1, Cancel = 2 --
#   measured, not assumed) and waits for a human. This driver supplies the human
#   by posting WM_COMMAND, which is what a user's clicks do. It does NOT pass
#   /S to the installer, because install.ps1 does not, and adding it would be
#   testing a different program.
#
# ASCII-ONLY. Windows PowerShell 5.1 reads a .ps1 as ANSI unless it carries a
# BOM, so one non-ASCII character can tear a string literal apart. Keep every
# character in this file inside 7-bit ASCII.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:WorkDir = 'C:\agentlens-installps1'
$script:OutDir = Join-Path $script:WorkDir 'out'
$script:MirrorDir = Join-Path $script:WorkDir 'mirror'
$script:LogPath = Join-Path $script:OutDir 'verify.log'
$script:SupervisorLog = Join-Path $script:OutDir 'supervisor.log'
$script:ResultsPath = Join-Path $script:OutDir 'results.json'
$script:HandoffPath = Join-Path $script:WorkDir 'verify-handoff.txt'
# Deliberately NOT 'AgentLensGuiQaInteractive': that task belongs to
# scripts/qa/ec2-windows-gui-qa.ps1 and must not be clobbered by this run.
$script:TaskName = 'AgentLensInstallPs1Verify'
$script:InstallPs1 = Join-Path $script:WorkDir 'install.ps1'
$script:ServerPs1 = Join-Path $script:WorkDir 'mirror-server.ps1'
$script:Port = 18080
# SINGLE SOURCE for the version. Everything version-bearing below is DERIVED
# from it, so a release-please version bump does not leave this harness pointing
# at an artifact that no longer exists. The default is the version currently in
# the root Cargo.toml [workspace.package]; unlike AGENTLENS_QA_BUCKET this is an
# honest default rather than a guess, so it is optional and not a hard throw.
$script:Version = if ($env:AGENTLENS_QA_VERSION) { $env:AGENTLENS_QA_VERSION } else { '0.1.0' }
$script:SetupName = "AgentLens_$($script:Version)_x64-setup.exe"
# The NSIS setup process name is the file name without its extension, which is
# how Get-Process reports it. Derived, never spelled out again.
$script:SetupProcessName = [IO.Path]::GetFileNameWithoutExtension($script:SetupName)
$script:SumsName = 'sha256sums-windows.txt'
$script:InstallDir = ''

$script:Bucket = ''
$script:Key = ''
$script:OutPrefix = ''
$script:ExpectedSha = ''
$script:RunId = ''
$script:Region = ''
$script:InstallPs1Sha = ''

$script:Checks = @()
$script:Scenarios = @()
$script:Diag = @{}

New-Item -ItemType Directory -Force -Path $script:WorkDir | Out-Null
New-Item -ItemType Directory -Force -Path $script:OutDir | Out-Null

function Write-Note {
    param([string]$Message)
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $line = "[$stamp] $Message"
    Write-Host $line
    try { Add-Content -LiteralPath $script:LogPath -Value $line -Encoding ASCII } catch { }
}

function Write-Supervisor {
    param([string]$Message)
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $line = "[$stamp] supervisor: $Message"
    Write-Host $line
    try { Add-Content -LiteralPath $script:SupervisorLog -Value $line -Encoding ASCII } catch { }
}

function Stop-WithError {
    param([string]$Message, [string[]]$Detail = @())
    Write-Note "BLOCKED: $Message"
    foreach ($d in $Detail) { Write-Note "         $d" }
    Save-Results -Verdict 'BLOCKED' -Note $Message
    # The supervisor propagates whatever is in child.exit, so a BLOCKED child
    # must write it too or a hard stop would be reported as a bookkeeping fault
    # instead of as the blocked run it is.
    try { Set-Content -LiteralPath (Join-Path $script:OutDir 'child.exit') -Value '20' -Encoding ASCII } catch { }
    Send-OutputToS3
    exit 20
}

# A check is a single machine-checkable assertion. Nothing here computes a
# verdict from an expectation: the verdict is the comparison of the OBSERVED
# value against the expectation, and a mismatch is recorded as FAIL, never
# softened or dropped.
function Add-Check {
    param(
        [string]$Scenario,
        [string]$Name,
        [string]$Expected,
        [string]$Observed,
        [bool]$Ok,
        [string]$Why
    )
    $verdict = 'FAIL'
    if ($Ok) { $verdict = 'PASS' }
    $script:Checks += , [pscustomobject]@{
        scenario = $Scenario
        name     = $Name
        expected = $Expected
        observed = $Observed
        verdict  = $verdict
        why      = $Why
    }
    Write-Note "  [$verdict] $Scenario/$Name expected=$Expected observed=$Observed"
}

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
        try { $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction Stop } catch { continue }
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

function Import-S3Module {
    foreach ($m in @('AWSPowerShell', 'AWSPowerShell.NetCore', 'AWS.Tools.S3')) {
        if (Get-Module -ListAvailable -Name $m) {
            Import-Module $m -ErrorAction SilentlyContinue
            break
        }
    }
}

# Defined ahead of the supervisor block on purpose: the supervisor calls it and
# then exits, so a definition placed after that block would never have executed
# by the time the call is reached.
function Send-SupervisorLog {
    Import-S3Module
    if (-not (Get-Command Write-S3Object -ErrorAction SilentlyContinue)) { return }
    if (-not (Test-Path -LiteralPath $script:SupervisorLog)) { return }
    try {
        Write-S3Object -BucketName $script:Bucket -Key "$script:OutPrefix/$script:RunId/supervisor.log" `
            -File $script:SupervisorLog -Region $script:Region | Out-Null
    } catch { }
}

function Send-OutputToS3 {
    Import-S3Module
    if (-not (Get-Command Write-S3Object -ErrorAction SilentlyContinue)) {
        Write-Host 'Write-S3Object unavailable; artifacts stay on the instance only.'
        return
    }
    if (-not $script:Bucket -or -not $script:OutPrefix -or -not $script:RunId) { return }
    foreach ($f in (Get-ChildItem -LiteralPath $script:OutDir -File -ErrorAction SilentlyContinue)) {
        try {
            Write-S3Object -BucketName $script:Bucket `
                -Key "$script:OutPrefix/$script:RunId/$($f.Name)" `
                -File $f.FullName -Region $script:Region | Out-Null
        } catch {
            Write-Host "failed to upload $($f.Name): $($_.Exception.Message)"
        }
    }
}

function Save-Results {
    param([string]$Verdict, [string]$Note = '')
    $pass = @($script:Checks | Where-Object { $_.verdict -eq 'PASS' }).Count
    $fail = @($script:Checks | Where-Object { $_.verdict -eq 'FAIL' }).Count
    $doc = [pscustomobject]@{
        run_id             = $script:RunId
        generated_utc      = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        artifact_under_test = 'scripts/install.ps1'
        install_ps1_sha256 = $script:InstallPs1Sha
        installer_sha256   = $script:ExpectedSha
        verdict            = $Verdict
        note               = $Note
        checks_pass        = $pass
        checks_fail        = $fail
        scenarios          = $script:Scenarios
        checks             = $script:Checks
        diagnostics        = $script:Diag
    }
    $doc | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $script:ResultsPath -Encoding ASCII
}

# ============================================================== SUPERVISOR ====
$mySession = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
if ($mySession -eq 0) {
    Write-Supervisor "session 0, whoami=$([Security.Principal.WindowsIdentity]::GetCurrent().Name)"
    $script:Bucket = "$($env:ALV_BUCKET)"
    $script:Key = "$($env:ALV_KEY)"
    $script:OutPrefix = "$($env:ALV_OUT_PREFIX)"
    $script:ExpectedSha = "$($env:ALV_SHA256)"
    $script:RunId = "$($env:ALV_RUN_ID)"
    $script:Region = "$($env:ALV_REGION)"

    $sess = Get-InteractiveSession
    if ($null -eq $sess) {
        Write-Supervisor 'no interactive logon session (no explorer.exe outside session 0).'
        Write-Supervisor 'REFUSING to fall back to session 0: the NSIS installer is installMode'
        Write-Supervisor 'currentUser, so a session-0 run installs into the SERVICE profile and'
        Write-Supervisor 'measures something no user will ever experience.'
        Write-Supervisor 'establish one first: scripts/qa/ec2-windows-gui-qa.sh interactive'
        Send-SupervisorLog
        exit 30
    }
    Write-Supervisor "interactive session id=$($sess.SessionId) user=$($sess.User) (explorer.exe pid $($sess.ShellPid))"

    Set-Content -LiteralPath $script:HandoffPath -Encoding ASCII -Value @(
        '# Written by the session-0 supervisor of install-ps1-verify.ps1.',
        '# A scheduled task inherits none of the caller environment, so the',
        '# settings cross the session boundary in this file. No credential here.',
        "ALV_BUCKET=$script:Bucket",
        "ALV_KEY=$script:Key",
        "ALV_OUT_PREFIX=$script:OutPrefix",
        "ALV_SHA256=$script:ExpectedSha",
        "ALV_RUN_ID=$script:RunId",
        "ALV_REGION=$script:Region"
    )
    Write-Supervisor "wrote the settings handoff: $script:HandoffPath"

    $me = $PSCommandPath
    if (-not $me) { $me = "$($MyInvocation.MyCommand.Path)" }
    if (-not $me -or -not (Test-Path -LiteralPath $me)) {
        Write-Supervisor 'cannot resolve this script path; refusing to run in session 0.'
        Send-SupervisorLog
        exit 31
    }

    $handoffWrittenAt = Get-Date
    try {
        $arg = '-NoProfile -ExecutionPolicy Bypass -File "' + $me + '"'
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg -WorkingDirectory $script:WorkDir
        $principal = New-ScheduledTaskPrincipal -UserId $sess.User -LogonType Interactive -RunLevel Highest
        $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 40) `
            -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
        Register-ScheduledTask -TaskName $script:TaskName -Action $action `
            -Principal $principal -Settings $settings -Force | Out-Null
        Start-ScheduledTask -TaskName $script:TaskName
    } catch {
        Write-Supervisor "the scheduled-task handoff FAILED: $($_.Exception.Message)"
        Write-Supervisor 'refusing to fall back to session 0.'
        Send-SupervisorLog
        exit 32
    }
    Write-Supervisor "started '$script:TaskName' as $($sess.User); waiting for it."

    # Completion is the state transition Running -> not Running. LastRunTime is
    # a sentinel for a triggerless task and is recorded for information only --
    # gating on it turned a finished run into a spurious failure once already.
    $deadline = (Get-Date).AddMinutes(35)
    $startDeadline = (Get-Date).AddSeconds(180)
    $state = 'unknown'
    $lastResult = $null
    $sawRunning = $false
    $finished = $false
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 5
        try {
            $state = "$((Get-ScheduledTask -TaskName $script:TaskName -ErrorAction Stop).State)"
            $lastResult = (Get-ScheduledTaskInfo -TaskName $script:TaskName -ErrorAction Stop).LastTaskResult
        } catch { continue }
        if ($state -eq 'Running') {
            if (-not $sawRunning) { Write-Supervisor 'the interactive task is Running.' }
            $sawRunning = $true
            continue
        }
        if ($sawRunning) { $finished = $true; break }
        if ((Test-Path -LiteralPath $script:LogPath) -and
            (Get-Item -LiteralPath $script:LogPath).LastWriteTime -gt $handoffWrittenAt) {
            Write-Supervisor 'the interactive task already touched verify.log; treating it as started.'
            $finished = $true
            break
        }
        if ((Get-Date) -gt $startDeadline) { break }
    }
    Write-Supervisor "task bookkeeping: state='$state' LastTaskResult=$lastResult (informational)"

    if (-not $sawRunning -and -not $finished) {
        Write-Supervisor "the interactive task never entered Running within 180s (last state '$state')."
        Write-Supervisor 'refusing to fall back to session 0.'
        Send-SupervisorLog
        exit 33
    }
    if (-not $finished) {
        Write-Supervisor "the interactive task started but did NOT finish within 35 minutes (last state '$state')."
        try { Stop-ScheduledTask -TaskName $script:TaskName -ErrorAction SilentlyContinue } catch { }
        Send-SupervisorLog
        exit 34
    }
    Write-Supervisor 'the interactive task finished.'

    if (Test-Path -LiteralPath $script:LogPath) {
        Write-Host '--- tail of the interactive verify.log ---'
        foreach ($l in (Get-Content -LiteralPath $script:LogPath -Tail 80 -ErrorAction SilentlyContinue)) {
            Write-Host "$l"
        }
        Write-Host '--- end of tail; the full log is in the S3 artifacts ---'
    }
    try { Unregister-ScheduledTask -TaskName $script:TaskName -Confirm:$false } catch { }
    Send-SupervisorLog

    $childExit = 0
    $exitFile = Join-Path $script:OutDir 'child.exit'
    if (Test-Path -LiteralPath $exitFile) {
        $childExit = [int]((Get-Content -LiteralPath $exitFile -Raw).Trim())
    } else {
        Write-Host 'supervisor: the child wrote no exit file; treating that as a failure.'
        $childExit = 35
    }
    Write-Host "supervisor: propagating the child exit code $childExit"
    exit $childExit
}

# =================================================================== CHILD ====
$script:ChildSession = $mySession
Write-Note "child: session=$mySession user=$([Security.Principal.WindowsIdentity]::GetCurrent().Name)"
Write-Note "child: PowerShell $($PSVersionTable.PSVersion.ToString())"
$script:Diag['session_id'] = $mySession
$script:Diag['whoami'] = "$([Security.Principal.WindowsIdentity]::GetCurrent().Name)"
$script:Diag['powershell'] = "$($PSVersionTable.PSVersion.ToString())"
$script:Diag['session_id_source'] = 'System.Diagnostics.Process.GetCurrentProcess().SessionId, read inside the measuring process'

foreach ($line in (Get-Content -LiteralPath $script:HandoffPath -ErrorAction Stop)) {
    if ($line.StartsWith('#')) { continue }
    $kv = $line -split '=', 2
    if ($kv.Count -ne 2) { continue }
    switch ($kv[0]) {
        'ALV_BUCKET' { $script:Bucket = $kv[1] }
        'ALV_KEY' { $script:Key = $kv[1] }
        'ALV_OUT_PREFIX' { $script:OutPrefix = $kv[1] }
        'ALV_SHA256' { $script:ExpectedSha = $kv[1] }
        'ALV_RUN_ID' { $script:RunId = $kv[1] }
        'ALV_REGION' { $script:Region = $kv[1] }
    }
}
Write-Note "child: run=$script:RunId bucket=$script:Bucket key=$script:Key region=$script:Region"

if (-not (Test-Path -LiteralPath $script:InstallPs1)) {
    Stop-WithError "the artifact under test is missing: $script:InstallPs1"
}
$script:InstallPs1Sha = (Get-FileHash -LiteralPath $script:InstallPs1 -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Note "child: install.ps1 under test sha256=$script:InstallPs1Sha"

# --- fetch and verify the real installer ------------------------------------
Import-S3Module
if (-not (Get-Command Read-S3Object -ErrorAction SilentlyContinue)) {
    Stop-WithError 'Read-S3Object is unavailable; cannot fetch the real installer.'
}
$zip = Join-Path $script:WorkDir 'agentlens-windows.zip'
$extract = Join-Path $script:WorkDir 'extract'
if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
New-Item -ItemType Directory -Force -Path $extract | Out-Null
Write-Note "child: fetching s3://$script:Bucket/$script:Key"
Read-S3Object -BucketName $script:Bucket -Key $script:Key -File $zip -Region $script:Region | Out-Null
Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
$setupSrc = Join-Path $extract $script:SetupName
if (-not (Test-Path -LiteralPath $setupSrc)) {
    Stop-WithError "the archive does not contain $script:SetupName" @(
        (Get-ChildItem -LiteralPath $extract -Recurse | ForEach-Object { "  $($_.FullName)" }))
}
$setupSha = (Get-FileHash -LiteralPath $setupSrc -Algorithm SHA256).Hash.ToLowerInvariant()
$setupLen = (Get-Item -LiteralPath $setupSrc).Length
Write-Note "child: real installer sha256=$setupSha size=$setupLen"
if ($setupSha -ne $script:ExpectedSha.ToLowerInvariant()) {
    Stop-WithError 'the fetched installer does not match the expected SHA-256' @(
        "expected $script:ExpectedSha", "actual   $setupSha")
}
$script:Diag['installer_size'] = $setupLen
$script:Diag['installer_sha256_measured'] = $setupSha

# --- build the mirror -------------------------------------------------------
# Manifest shape is dictated by install.ps1:239-241 -- "<64 hex>  <basename>",
# ASCII, CRLF (which is what Set-Content -Encoding ascii writes).
function New-Manifest {
    param([string]$Dir, [string]$Name, [string]$Hash)
    $path = Join-Path $Dir $script:SumsName
    Set-Content -LiteralPath $path -Encoding ascii -Value @(
        '# AgentLens Windows release digests (local mirror for install.ps1 verification)',
        "$Hash  $Name"
    )
    return $path
}

if (Test-Path -LiteralPath $script:MirrorDir) { Remove-Item -LiteralPath $script:MirrorDir -Recurse -Force }
$goodDir = Join-Path $script:MirrorDir 'good'
$badDir = Join-Path $script:MirrorDir 'badsum'
$failDir = Join-Path $script:MirrorDir 'failexe'
foreach ($d in @($goodDir, $badDir, $failDir)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }

Copy-Item -LiteralPath $setupSrc -Destination (Join-Path $goodDir $script:SetupName) -Force
[void](New-Manifest -Dir $goodDir -Name $script:SetupName -Hash $setupSha)

# Tamper the MANIFEST, not the asset: that is the direction a real supply-chain
# mismatch takes and it keeps the served bytes byte-identical to the real
# installer, so a PASS here cannot be explained by a corrupt download.
$firstChar = $setupSha.Substring(0, 1)
$replacement = 'b'
if ($firstChar -eq 'b') { $replacement = 'c' }
$badSha = $replacement + $setupSha.Substring(1)
if ($badSha -eq $setupSha) { Stop-WithError 'failed to construct a differing digest' }
Copy-Item -LiteralPath $setupSrc -Destination (Join-Path $badDir $script:SetupName) -Force
[void](New-Manifest -Dir $badDir -Name $script:SetupName -Hash $badSha)
Write-Note "child: tampered manifest digest $badSha (real $setupSha)"

# A stand-in setup exe that exits 3. It is served under the canonical asset
# name, with its OWN true digest in the manifest, so install.ps1's verification
# passes and execution reaches the exit-code branch at install.ps1:366 with a
# deterministic non-zero code. Built with the .NET Framework C# compiler that
# ships with Windows, so it is a genuine native process exit, not a simulation.
$failExe = Join-Path $failDir $script:SetupName
$failSrc = @'
public class StandInSetup {
    public static int Main(string[] args) {
        System.Console.Out.WriteLine("stand-in setup: pretending the install failed, exiting 3");
        return 3;
    }
}
'@
Add-Type -TypeDefinition $failSrc -OutputAssembly $failExe -OutputType ConsoleApplication
if (-not (Test-Path -LiteralPath $failExe)) { Stop-WithError 'could not compile the stand-in setup exe' }
$failSha = (Get-FileHash -LiteralPath $failExe -Algorithm SHA256).Hash.ToLowerInvariant()
[void](New-Manifest -Dir $failDir -Name $script:SetupName -Hash $failSha)
Write-Note "child: stand-in setup exe built, sha256=$failSha size=$((Get-Item -LiteralPath $failExe).Length)"
$script:Diag['standin_setup_sha256'] = $failSha

# --- the loopback mirror server ---------------------------------------------
$serverSrc = @'
param([string]$Root, [int]$Port, [string]$LogPath)
$ErrorActionPreference = 'Stop'
function L { param([string]$m) try { Add-Content -LiteralPath $LogPath -Value ((Get-Date).ToUniversalTime().ToString('s') + ' ' + $m) -Encoding ASCII } catch { } }
$listener = New-Object System.Net.HttpListener
$listener.Prefixes.Add("http://127.0.0.1:$Port/")
$listener.Start()
L "listening on http://127.0.0.1:$Port/ root=$Root"
while ($listener.IsListening) {
    $ctx = $null
    try { $ctx = $listener.GetContext() } catch { break }
    $rel = $ctx.Request.Url.AbsolutePath.TrimStart('/')
    if ($rel -eq '__ping') {
        $b = [System.Text.Encoding]::ASCII.GetBytes('pong')
        $ctx.Response.StatusCode = 200
        $ctx.Response.ContentLength64 = $b.Length
        $ctx.Response.OutputStream.Write($b, 0, $b.Length)
        $ctx.Response.OutputStream.Close()
        L "200 $rel"
        continue
    }
    if ($rel -eq '__quit') {
        $ctx.Response.StatusCode = 200
        $ctx.Response.ContentLength64 = 0
        $ctx.Response.OutputStream.Close()
        L 'quit requested'
        break
    }
    $safe = $rel -replace '/', '\'
    $path = Join-Path $Root $safe
    $full = $null
    try { $full = [System.IO.Path]::GetFullPath($path) } catch { $full = $null }
    $rootFull = [System.IO.Path]::GetFullPath($Root)
    if ((-not $full) -or (-not $full.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) -or (-not (Test-Path -LiteralPath $full -PathType Leaf))) {
        $ctx.Response.StatusCode = 404
        $ctx.Response.ContentLength64 = 0
        $ctx.Response.OutputStream.Close()
        L "404 $rel"
        continue
    }
    $bytes = [System.IO.File]::ReadAllBytes($full)
    $ctx.Response.StatusCode = 200
    $ctx.Response.ContentType = 'application/octet-stream'
    $ctx.Response.ContentLength64 = $bytes.Length
    $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
    $ctx.Response.OutputStream.Close()
    L "200 $rel ($($bytes.Length) bytes)"
}
try { $listener.Stop(); $listener.Close() } catch { }
L 'stopped'
'@
Set-Content -LiteralPath $script:ServerPs1 -Value $serverSrc -Encoding ASCII

$serverLog = Join-Path $script:OutDir 'mirror-server.log'
$serverArgs = @(
    '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script:ServerPs1,
    '-Root', $script:MirrorDir, '-Port', "$script:Port", '-LogPath', $serverLog
)
$server = Start-Process -FilePath 'powershell.exe' -ArgumentList $serverArgs -PassThru -WindowStyle Hidden
Write-Note "child: mirror server pid=$($server.Id) root=$script:MirrorDir port=$script:Port"

$ping = $false
for ($i = 1; $i -le 30; $i++) {
    Start-Sleep -Milliseconds 500
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:$script:Port/__ping" -UseBasicParsing -TimeoutSec 5
        if ($r.StatusCode -eq 200) { $ping = $true; break }
    } catch { }
}
if (-not $ping) {
    if (Test-Path -LiteralPath $serverLog) {
        foreach ($l in (Get-Content -LiteralPath $serverLog)) { Write-Note "  server: $l" }
    }
    Stop-WithError "the loopback mirror never answered on port $script:Port"
}
Write-Note 'child: mirror server is answering'

# --- the wizard driver -------------------------------------------------------
if (-not ('AlvWin' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class AlvWin {
    [DllImport("user32.dll")]
    static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
    [DllImport("user32.dll")]
    static extern bool EnumChildWindows(IntPtr h, EnumWindowsProc cb, IntPtr p);
    delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")]
    static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")]
    static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")]
    static extern int GetDlgCtrlID(IntPtr h);
    [DllImport("user32.dll")]
    public static extern IntPtr PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")]
    static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);

    const uint BM_GETCHECK = 0x00F0;
    const uint BM_SETCHECK = 0x00F1;

    static string Cls(IntPtr h) { StringBuilder b = new StringBuilder(256); GetClassName(h, b, 256); return b.ToString(); }
    static string Txt(IntPtr h) { StringBuilder b = new StringBuilder(1024); GetWindowText(h, b, 1024); return b.ToString(); }

    // Every visible top-level dialog whose title contains the needle. Not scoped
    // to a pid: install.ps1 launches the setup exe itself, so this driver never
    // learns that pid.
    public static List<IntPtr> Dialogs(string needle) {
        List<IntPtr> found = new List<IntPtr>();
        string n = needle.ToLowerInvariant();
        EnumWindows(delegate(IntPtr h, IntPtr p) {
            if (!IsWindowVisible(h)) return true;
            if (Cls(h) != "#32770") return true;
            if (Txt(h).ToLowerInvariant().IndexOf(n) < 0) return true;
            found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string Describe(IntPtr dlg) {
        StringBuilder sb = new StringBuilder();
        sb.Append("title=[" + Txt(dlg) + "]");
        EnumChildWindows(dlg, delegate(IntPtr h, IntPtr p) {
            int id = GetDlgCtrlID(h);
            if (!IsWindowVisible(h)) return true;
            string t = Txt(h);
            if (t.Length == 0) return true;
            if (t.Length > 90) t = t.Substring(0, 90) + "...";
            t = t.Replace("\r", " ").Replace("\n", " ");
            sb.Append(" | " + id + ":" + Cls(h) + "=" + t);
            return true;
        }, IntPtr.Zero);
        return sb.ToString();
    }

    // EnumChildWindows walks the whole descendant tree, which is required here:
    // the MUI finish page is an inner #32770 child of the outer wizard dialog, so
    // its checkbox is a grandchild and GetDlgItem on the outer dialog misses it.
    public static IntPtr FindDescendantById(IntPtr parent, int id) {
        IntPtr found = IntPtr.Zero;
        EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) {
            if (GetDlgCtrlID(h) != id) return true;
            found = h;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    public static string ChildText(IntPtr h) { return Txt(h); }

    // BM_GETCHECK / BM_SETCHECK carry no pointers, so they are safe to send
    // across a process boundary; the MUI finish page reads the state with
    // BM_GETCHECK when Finish is pressed, so setting it here is what a user
    // clearing the checkbox does.
    public static int GetCheck(IntPtr h) { return (int)SendMessage(h, BM_GETCHECK, IntPtr.Zero, IntPtr.Zero); }
    public static void SetCheck(IntPtr h, int value) { SendMessage(h, BM_SETCHECK, new IntPtr(value), IntPtr.Zero); }
}
'@
}

# ---------------------------------------------------------------------------
# WHY THE RUNNER USES System.Diagnostics.Process DIRECTLY
#
# The first version of this driver used `Start-Process -PassThru` without -Wait
# and read $proc.ExitCode. That reported 0 for EVERY scenario, including runs
# whose own stdout carried "error: installer exited with code 1" immediately
# before install.ps1's `exit 1`. The exit code was real; the READING of it was
# not -- a Process object handed back by Start-Process -PassThru does not carry a
# handle this process owns, so ExitCode is not reliable. Starting the process
# here means this process owns the handle and ExitCode is authoritative.
#
# Also note -Wait is deliberately NOT used anywhere in this driver: PowerShell's
# -Wait waits for the process AND ITS DESCENDANTS, which is exactly the behaviour
# under investigation in install.ps1 -- using it here would hide it.
#
# The output streams are drained with ReadToEndAsync so a scenario that writes
# more than the pipe buffer cannot deadlock against a driver that is only
# polling HasExited.
# ---------------------------------------------------------------------------

# One pass of the wizard driver. 'Advance' posts IDOK (control id 1: Next /
# Install / Finish); 'Cancel' posts IDCANCEL (control id 2). Both ids were
# MEASURED on this exact installer. When -UncheckRunApp is set, the finish
# page's "Run AgentLens" checkbox (control id 1203, also measured) is cleared
# before Finish is posted.
function Step-Wizard {
    param(
        [ValidateSet('Advance', 'Cancel')][string]$Mode,
        [hashtable]$Seen,
        [bool]$UncheckRunApp
    )
    $WM_COMMAND = 0x0111
    $wparam = [IntPtr]1
    if ($Mode -eq 'Cancel') { $wparam = [IntPtr]2 }
    $posted = 0
    foreach ($d in [AlvWin]::Dialogs('AgentLens Setup')) {
        $desc = [AlvWin]::Describe($d)
        if (-not $Seen.ContainsKey($desc)) {
            $Seen[$desc] = 1
            Write-Note "  wizard page: $desc"
        }
        if ($UncheckRunApp) {
            $cb = [AlvWin]::FindDescendantById($d, 1203)
            if ($cb -ne [IntPtr]::Zero) {
                $txt = [AlvWin]::ChildText($cb)
                $before = [AlvWin]::GetCheck($cb)
                if ($before -ne 0) {
                    [AlvWin]::SetCheck($cb, 0)
                    $after = [AlvWin]::GetCheck($cb)
                    Write-Note "  cleared checkbox 1203 [$txt]: BM_GETCHECK $before -> $after"
                }
            }
        }
        [void][AlvWin]::PostMessage($d, $WM_COMMAND, $wparam, [IntPtr]::Zero)
        $posted++
    }
    return $posted
}

# Runs the REAL scripts/install.ps1 as its own process so its `exit 1` is a real
# process exit code. Every input arrives through the environment because
# install.ps1's param() block is empty; nothing is passed on its command line.
function Invoke-InstallPs1 {
    param(
        [string]$Name,
        [hashtable]$EnvMap,
        [string]$WizardMode = 'None',
        [int]$TimeoutSeconds = 240,
        [bool]$UncheckRunApp = $false
    )
    $names = @(
        'AGENTLENS_REPO', 'AGENTLENS_VERSION', 'AGENTLENS_BASE_URL', 'AGENTLENS_API_URL',
        'AGENTLENS_ALLOW_INSECURE_URL', 'AGENTLENS_DOWNLOAD_DIR', 'AGENTLENS_INSTALL',
        'AGENTLENS_DRY_RUN', 'AGENTLENS_ARCH'
    )
    foreach ($n in $names) { Set-Item -Path "env:$n" -Value '' }
    $shown = @()
    foreach ($k in ($EnvMap.Keys | Sort-Object)) {
        Set-Item -Path "env:$k" -Value "$($EnvMap[$k])"
        $shown += "$k=$($EnvMap[$k])"
    }

    Write-Note ''
    Write-Note "=== scenario $Name ==="
    foreach ($s in $shown) { Write-Note "  env $s" }
    Write-Note "  running: powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script:InstallPs1"
    if ($WizardMode -ne 'None') {
        Write-Note "  wizard driver: $WizardMode (uncheckRunApp=$UncheckRunApp), budget ${TimeoutSeconds}s"
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = 'powershell.exe'
    $psi.Arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + $script:InstallPs1 + '"'
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $started = Get-Date
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $errTask = $p.StandardError.ReadToEndAsync()

    # ONE deadline for the whole scenario. The first version had a separate
    # wizard budget and wait budget, so a hang burned 2 x TimeoutSeconds and the
    # reported elapsed time did not match the configured timeout.
    $deadline = $started.AddSeconds($TimeoutSeconds)
    $seen = @{}
    $wizardPosts = 0
    $timeline = @()
    $lastState = ''
    while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
        if ($WizardMode -ne 'None') {
            $wizardPosts += (Step-Wizard -Mode $WizardMode -Seen $seen -UncheckRunApp $UncheckRunApp)
        }
        # The timeline is what makes a hang diagnosable instead of just slow: it
        # records, second by second, whether the setup process and the installed
        # app are alive while install.ps1 is still running.
        $setupAlive = @(Get-Process -Name $script:SetupProcessName -ErrorAction SilentlyContinue).Count
        $appAlive = @(Get-Process -Name 'agentlens-tauri' -ErrorAction SilentlyContinue).Count
        $state = "setup=$setupAlive app=$appAlive installps1=running"
        if ($state -ne $lastState) {
            $at = [int]((Get-Date) - $started).TotalSeconds
            $timeline += "t=${at}s $state"
            Write-Note "  timeline t=${at}s $state"
            $lastState = $state
        }
        Start-Sleep -Milliseconds 1000
    }

    $timedOut = $false
    if (-not $p.HasExited) {
        $timedOut = $true
        $setupAlive = @(Get-Process -Name $script:SetupProcessName -ErrorAction SilentlyContinue).Count
        $appAlive = @(Get-Process -Name 'agentlens-tauri' -ErrorAction SilentlyContinue).Count
        $timeline += "t=${TimeoutSeconds}s TIMEOUT setup=$setupAlive app=$appAlive installps1=STILL RUNNING"
        Write-Note "  TIMEOUT after ${TimeoutSeconds}s: install.ps1 is STILL RUNNING (setup processes=$setupAlive, app processes=$appAlive); killing install.ps1"
        try { $p.Kill() } catch { }
    }
    [void]$p.WaitForExit(15000)
    # Sampled IMMEDIATELY after install.ps1 exits and before anything is cleaned
    # up. This is the positive measurement of the corrected behaviour: the fix is
    # supposed to launch the app and NOT wait for it, so a successful install
    # should show install.ps1 exited WHILE the app is still running. Under the
    # old -Wait code this number could never be observed at all, because the exit
    # only happened after the app was gone.
    $appAliveAtExit = @(Get-Process -Name 'agentlens-tauri' -ErrorAction SilentlyContinue).Count
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $code = -999
    try { $code = [int]$p.ExitCode } catch { }
    if (-not $timedOut) {
        $timeline += "t=${elapsed}s installps1=exited code=$code app=$appAliveAtExit"
        Write-Note "  timeline t=${elapsed}s installps1=exited code=$code app=$appAliveAtExit"
    }

    # [string] cast, not Get-Content -Raw: a string that came from Get-Content
    # carries PSPath / PSProvider note properties, and ConvertTo-Json then walked
    # the provider object graph and produced a 626 MB results.json.
    $outText = ''
    $errText = ''
    try { if ($outTask.Wait(15000)) { $outText = [string]$outTask.Result } } catch { }
    try { if ($errTask.Wait(15000)) { $errText = [string]$errTask.Result } } catch { }
    if ($null -eq $outText) { $outText = '' }
    if ($null -eq $errText) { $errText = '' }
    Set-Content -LiteralPath (Join-Path $script:OutDir "$Name.stdout.txt") -Value $outText -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $script:OutDir "$Name.stderr.txt") -Value $errText -Encoding ASCII
    $combined = $outText + "`n" + $errText

    Write-Note "  exit code: $code (elapsed ${elapsed}s, timedOut=$timedOut)"
    Write-Note '  --- install.ps1 stdout ---'
    foreach ($l in ($outText -split "`r?`n")) { if ($l.Trim().Length -gt 0) { Write-Note "  | $l" } }
    if ($errText.Trim().Length -gt 0) {
        Write-Note '  --- install.ps1 stderr ---'
        foreach ($l in ($errText -split "`r?`n")) { if ($l.Trim().Length -gt 0) { Write-Note "  ! $l" } }
    }

    $script:Scenarios += , [pscustomobject]@{
        name              = $Name
        env               = $shown
        exit_code         = $code
        timed_out         = $timedOut
        elapsed_s         = $elapsed
        timeout_budget    = $TimeoutSeconds
        wizard_mode       = $WizardMode
        uncheck_run       = $UncheckRunApp
        wizard_posts      = $wizardPosts
        wizard_pages      = @($seen.Keys)
        app_alive_at_exit = $appAliveAtExit
        timeline          = $timeline
        stdout            = $outText
        stderr            = $errText
    }
    return [pscustomobject]@{
        Code           = $code
        Text           = $combined
        TimedOut       = $timedOut
        Timeline       = $timeline
        AppAliveAtExit = $appAliveAtExit
    }
}

function Test-Contains {
    param([string]$Haystack, [string]$Needle)
    return ($Haystack.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0)
}

function Stop-AgentLensApp {
    $killed = 0
    foreach ($p in (Get-Process -Name 'agentlens-tauri' -ErrorAction SilentlyContinue)) {
        try { Stop-Process -Id $p.Id -Force; $killed++ } catch { }
    }
    if ($killed -gt 0) { Write-Note "child: stopped $killed agentlens-tauri process(es) launched by the wizard Finish page" }
    return $killed
}

# LOCALAPPDATA of the INTERACTIVE user, resolved inside the child. In session 0
# this same expression resolves to the service profile, which is exactly the
# difference this run exists to avoid claiming.
$script:InstallDir = Join-Path $env:LOCALAPPDATA 'AgentLens'
$uninstKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\AgentLens'

function Get-InstallState {
    param([string]$Label)
    $state = @{ label = $Label; dir = $script:InstallDir; present = $false; files = @(); registry = $null }
    if (Test-Path -LiteralPath $script:InstallDir) {
        $state.present = $true
        $files = @()
        foreach ($f in (Get-ChildItem -LiteralPath $script:InstallDir -Recurse -File -ErrorAction SilentlyContinue)) {
            $files += , [pscustomobject]@{
                name  = $f.FullName.Substring($script:InstallDir.Length)
                size  = $f.Length
                mtime = $f.LastWriteTimeUtc.ToString('yyyy-MM-ddTHH:mm:ssZ')
                sha256 = (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
        $state.files = $files
    }
    if (Test-Path $uninstKey) {
        $p = Get-ItemProperty $uninstKey
        $state.registry = [pscustomobject]@{
            DisplayName    = "$($p.DisplayName)"
            DisplayVersion = "$($p.DisplayVersion)"
            InstallLocation = "$($p.InstallLocation)"
            Publisher      = "$($p.Publisher)"
        }
    }
    Write-Note "child: install state ($Label): present=$($state.present) files=$($state.files.Count) registryVersion=$(if ($state.registry) { $state.registry.DisplayVersion } else { '<absent>' })"
    return $state
}

$stateBefore = Get-InstallState -Label 'before'
$script:Diag['install_state_before'] = $stateBefore

$baseGood = "http://127.0.0.1:$script:Port/good"
$baseBad = "http://127.0.0.1:$script:Port/badsum"
$baseFail = "http://127.0.0.1:$script:Port/failexe"

# ============================================================== SCENARIO 3 ====
# install.ps1:205-219 -- a non-https base URL must be refused unless
# AGENTLENS_ALLOW_INSECURE_URL=1. Run FIRST because it downloads nothing.
$r = Invoke-InstallPs1 -Name '3-nonhttps-rejected' -TimeoutSeconds 120 -EnvMap @{
    AGENTLENS_VERSION      = $script:Version
    AGENTLENS_BASE_URL     = $baseGood
    AGENTLENS_DOWNLOAD_DIR = (Join-Path $script:WorkDir 'dl-nonhttps')
}
Add-Check -Scenario '3-nonhttps-rejected' -Name 'exit_code' -Expected '1' -Observed "$($r.Code)" `
    -Ok ($r.Code -eq 1) -Why 'install.ps1:213 Stop-WithError exits 1'
Add-Check -Scenario '3-nonhttps-rejected' -Name 'refusal_message' `
    -Expected 'refusing to fetch over a non-https URL' `
    -Observed $(if (Test-Contains $r.Text 'refusing to fetch over a non-https URL') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'refusing to fetch over a non-https URL') `
    -Why 'the user must be told why, not just fail'
Add-Check -Scenario '3-nonhttps-rejected' -Name 'no_download' -Expected 'no setup exe on disk' `
    -Observed $(if (Test-Path -LiteralPath (Join-Path (Join-Path $script:WorkDir 'dl-nonhttps') $script:SetupName)) { 'PRESENT' } else { 'absent' }) `
    -Ok (-not (Test-Path -LiteralPath (Join-Path (Join-Path $script:WorkDir 'dl-nonhttps') $script:SetupName))) `
    -Why 'the refusal must happen before any bytes are fetched'

# ============================================================== SCENARIO 5 ====
# install.ps1:164-187 -- no AGENTLENS_VERSION, no AGENTLENS_BASE_URL, and an
# unreachable https releases API. The https scheme passes Assert-UrlScheme, so
# the failure lands in the catch at :182 and the user sees the :183-186 hint.
$r = Invoke-InstallPs1 -Name '5-missing-version' -TimeoutSeconds 180 -EnvMap @{
    AGENTLENS_API_URL      = 'https://127.0.0.1:1/repos/sunerpy/AgentLens/releases/latest'
    AGENTLENS_DOWNLOAD_DIR = (Join-Path $script:WorkDir 'dl-noversion')
}
Add-Check -Scenario '5-missing-version' -Name 'exit_code' -Expected '1' -Observed "$($r.Code)" `
    -Ok ($r.Code -eq 1) -Why 'install.ps1:183 Stop-WithError exits 1'
foreach ($needle in @(
        'could not query the latest release for sunerpy/AgentLens',
        'tried: https://127.0.0.1:1/repos/sunerpy/AgentLens/releases/latest',
        'AGENTLENS_VERSION')) {
    Add-Check -Scenario '5-missing-version' -Name "message_contains:$needle" -Expected 'present' `
        -Observed $(if (Test-Contains $r.Text $needle) { 'present' } else { 'ABSENT' }) `
        -Ok (Test-Contains $r.Text $needle) -Why 'this is the text a real user sees'
}

# ============================================================== SCENARIO 2 ====
# install.ps1:325-333 -- the manifest digest is wrong, the bytes are the real
# installer. The verifier must refuse, delete the download, and NOT install,
# even with AGENTLENS_INSTALL=1.
$badDl = Join-Path $script:WorkDir 'dl-badsum'
$r = Invoke-InstallPs1 -Name '2-checksum-rejected' -TimeoutSeconds 180 -EnvMap @{
    AGENTLENS_VERSION            = $script:Version
    AGENTLENS_BASE_URL           = $baseBad
    AGENTLENS_ALLOW_INSECURE_URL = '1'
    AGENTLENS_INSTALL            = '1'
    AGENTLENS_DOWNLOAD_DIR       = $badDl
}
Add-Check -Scenario '2-checksum-rejected' -Name 'exit_code' -Expected '1' -Observed "$($r.Code)" `
    -Ok ($r.Code -eq 1) -Why 'install.ps1:328 Stop-WithError exits 1'
Add-Check -Scenario '2-checksum-rejected' -Name 'mismatch_message' -Expected 'SHA-256 MISMATCH' `
    -Observed $(if (Test-Contains $r.Text 'SHA-256 MISMATCH') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'SHA-256 MISMATCH') -Why 'a verifier that cannot reject is not a verifier'
Add-Check -Scenario '2-checksum-rejected' -Name 'expected_digest_shown' -Expected $badSha `
    -Observed $(if (Test-Contains $r.Text $badSha) { $badSha } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text $badSha) -Why 'install.ps1:329 prints the manifest digest'
Add-Check -Scenario '2-checksum-rejected' -Name 'actual_digest_shown' -Expected $setupSha `
    -Observed $(if (Test-Contains $r.Text $setupSha) { $setupSha } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text $setupSha) -Why 'install.ps1:330 prints the measured digest'
Add-Check -Scenario '2-checksum-rejected' -Name 'download_deleted' -Expected 'absent' `
    -Observed $(if (Test-Path -LiteralPath (Join-Path $badDl $script:SetupName)) { 'PRESENT' } else { 'absent' }) `
    -Ok (-not (Test-Path -LiteralPath (Join-Path $badDl $script:SetupName))) `
    -Why 'install.ps1:327 removes the rejected file'
Add-Check -Scenario '2-checksum-rejected' -Name 'installer_not_launched' -Expected 'no launch line' `
    -Observed $(if (Test-Contains $r.Text 'AGENTLENS_INSTALL=1: launching') { 'LAUNCHED' } else { 'not launched' }) `
    -Ok (-not (Test-Contains $r.Text 'AGENTLENS_INSTALL=1: launching')) `
    -Why 'rejection must precede the install, install.ps1:326 before :358'

# ============================================================== SCENARIO 4b ===
# install.ps1:362-369 -- the exit-code branch, made deterministic. The served
# asset is a stand-in console exe that returns 3; it carries its own true digest
# in the manifest, so the verification at :326 passes and execution reaches :362
# for real. Everything except the identity of the binary is the shipped path.
$r = Invoke-InstallPs1 -Name '4b-exitcode-standin' -TimeoutSeconds 180 -EnvMap @{
    AGENTLENS_VERSION            = $script:Version
    AGENTLENS_BASE_URL           = $baseFail
    AGENTLENS_ALLOW_INSECURE_URL = '1'
    AGENTLENS_INSTALL            = '1'
    AGENTLENS_DOWNLOAD_DIR       = (Join-Path $script:WorkDir 'dl-failexe')
}
Add-Check -Scenario '4b-exitcode-standin' -Name 'reached_launch' -Expected 'launch line present' `
    -Observed $(if (Test-Contains $r.Text 'AGENTLENS_INSTALL=1: launching') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'AGENTLENS_INSTALL=1: launching') -Why 'install.ps1:358 ran, so :362 was reached'
Add-Check -Scenario '4b-exitcode-standin' -Name 'exit_code' -Expected '1' -Observed "$($r.Code)" `
    -Ok ($r.Code -eq 1) -Why 'install.ps1:367 Stop-WithError exits 1'
Add-Check -Scenario '4b-exitcode-standin' -Name 'surfaced_installer_code' -Expected 'installer exited with code 3' `
    -Observed $(if (Test-Contains $r.Text 'installer exited with code 3') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'installer exited with code 3') `
    -Why 'the non-zero native exit must be surfaced, not swallowed'
Add-Check -Scenario '4b-exitcode-standin' -Name 'no_false_success' -Expected 'no "installed:" line' `
    -Observed $(if (Test-Contains $r.Text 'installed: AgentLens') { 'FALSE SUCCESS' } else { 'absent' }) `
    -Ok (-not (Test-Contains $r.Text 'installed: AgentLens')) `
    -Why 'install.ps1:370 must not be reached after a failing installer'

# ============================================================== SCENARIO 4a ===
# The same branch, with the REAL NSIS installer. Cancelling the wizard is a
# genuine clean failure path of the real artifact: measured exit code 1.
$r = Invoke-InstallPs1 -Name '4a-exitcode-real-nsis-cancel' -TimeoutSeconds 180 -WizardMode 'Cancel' -EnvMap @{
    AGENTLENS_VERSION            = $script:Version
    AGENTLENS_BASE_URL           = $baseGood
    AGENTLENS_ALLOW_INSECURE_URL = '1'
    AGENTLENS_INSTALL            = '1'
    AGENTLENS_DOWNLOAD_DIR       = (Join-Path $script:WorkDir 'dl-cancel')
}
Add-Check -Scenario '4a-exitcode-real-nsis-cancel' -Name 'sha256_ok' -Expected 'sha256 ok' `
    -Observed $(if (Test-Contains $r.Text 'sha256 ok') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'sha256 ok') -Why 'the real asset verified against the real manifest'
Add-Check -Scenario '4a-exitcode-real-nsis-cancel' -Name 'exit_code' -Expected '1' -Observed "$($r.Code)" `
    -Ok ($r.Code -eq 1) -Why 'install.ps1:367 Stop-WithError exits 1'
Add-Check -Scenario '4a-exitcode-real-nsis-cancel' -Name 'surfaced_installer_code' `
    -Expected 'installer exited with code 1' `
    -Observed $(if (Test-Contains $r.Text 'installer exited with code 1') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r.Text 'installer exited with code 1') `
    -Why 'the real NSIS installer abort code reaches the user'
Add-Check -Scenario '4a-exitcode-real-nsis-cancel' -Name 'not_timed_out' -Expected 'false' `
    -Observed "$($r.TimedOut)" -Ok (-not $r.TimedOut) -Why 'a hang would make the exit code meaningless'
[void](Stop-AgentLensApp)

# ============================================================== SCENARIO 1a ===
# The happy path exactly as a user gets it: the wizard is advanced with every
# control left at its default, which means the finish page's "Run AgentLens"
# checkbox stays CHECKED (measured: control 1203 is checked by default).
#
# The expectations below are what install.ps1 is supposed to do after a
# successful install -- print "installed:" and exit 0. They are deliberately NOT
# relaxed to match whatever this run happens to observe. The scenario timeline
# records, second by second, whether the setup process and the app are alive
# while install.ps1 is still running, so a hang can be attributed rather than
# just noted.
$happyDl = Join-Path $script:WorkDir 'dl-happy-runchecked'
$r1a = Invoke-InstallPs1 -Name '1a-happy-install-run-checked' -TimeoutSeconds 150 `
    -WizardMode 'Advance' -UncheckRunApp $false -EnvMap @{
    AGENTLENS_VERSION            = $script:Version
    AGENTLENS_BASE_URL           = $baseGood
    AGENTLENS_ALLOW_INSECURE_URL = '1'
    AGENTLENS_INSTALL            = '1'
    AGENTLENS_DOWNLOAD_DIR       = $happyDl
}
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'manifest_fetched' -Expected "manifest: $baseGood/$script:SumsName" `
    -Observed $(if (Test-Contains $r1a.Text "manifest: $baseGood/$script:SumsName") { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1a.Text "manifest: $baseGood/$script:SumsName") -Why 'install.ps1:245'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'arch_detected' -Expected 'detected arch AMD64' `
    -Observed $(if (Test-Contains $r1a.Text 'detected arch AMD64') { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1a.Text 'detected arch AMD64') -Why 'install.ps1:94-104 on real hardware'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'sha256_ok' -Expected "sha256 ok: $script:SetupName" `
    -Observed $(if (Test-Contains $r1a.Text "sha256 ok: $script:SetupName") { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1a.Text "sha256 ok: $script:SetupName") -Why 'install.ps1:334'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'downloaded_bytes_match_real_installer' -Expected $setupSha `
    -Observed $(if (Test-Path -LiteralPath (Join-Path $happyDl $script:SetupName)) { (Get-FileHash -LiteralPath (Join-Path $happyDl $script:SetupName) -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'ABSENT' }) `
    -Ok ((Test-Path -LiteralPath (Join-Path $happyDl $script:SetupName)) -and ((Get-FileHash -LiteralPath (Join-Path $happyDl $script:SetupName) -Algorithm SHA256).Hash.ToLowerInvariant() -eq $setupSha)) `
    -Why 'the bytes install.ps1 ran are the bytes CodeBuild produced'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'not_timed_out' -Expected 'false' -Observed "$($r1a.TimedOut)" `
    -Ok (-not $r1a.TimedOut) -Why 'install.ps1 must return control after the setup program finishes'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'exit_code' -Expected '0' -Observed "$($r1a.Code)" `
    -Ok ($r1a.Code -eq 0) -Why 'install.ps1 reaches :370 only on a clean install'
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'success_message' -Expected "installed: AgentLens $script:Version" `
    -Observed $(if (Test-Contains $r1a.Text "installed: AgentLens $script:Version") { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1a.Text "installed: AgentLens $script:Version") -Why 'install.ps1:370'
# The positive form of the fix, not just the absence of the hang: install.ps1 is
# supposed to LAUNCH the app and return, so at the instant it exited the app the
# finish page started must still be running. Under the old Start-Process -Wait
# this could not be observed, because the wait only ended once the app was gone.
Add-Check -Scenario '1a-happy-install-run-checked' -Name 'exited_while_app_running' `
    -Expected 'at least 1 agentlens-tauri process alive when install.ps1 exited' `
    -Observed "$($r1a.AppAliveAtExit)" -Ok ($r1a.AppAliveAtExit -ge 1) `
    -Why 'the corrected wait covers the installer only, not its descendants'

# The attribution, recorded from the timeline rather than asserted: if the setup
# process is gone while install.ps1 is still running and the app it launched is
# alive, the wait is on the DESCENDANT, not on the installer.
$script:Diag['scenario_1a_timeline'] = $r1a.Timeline
$hangWithSetupGone = @($r1a.Timeline | Where-Object { $_ -match 'setup=0 app=[1-9]' -and $_ -match 'installps1=(running|STILL RUNNING)' })
$script:Diag['scenario_1a_samples_setup_gone_app_alive_installps1_running'] = $hangWithSetupGone
Write-Note "child: 1a timeline samples with setup gone, app alive, install.ps1 still running: $($hangWithSetupGone.Count)"
foreach ($t in $r1a.Timeline) { Write-Note "  1a timeline: $t" }

$killedAfter1a = Stop-AgentLensApp
$script:Diag['app_processes_stopped_after_1a'] = $killedAfter1a
# Give install.ps1 a chance to notice, if it was only waiting on the app. This
# is measurement, not remediation: the scenario has already been recorded.
Start-Sleep -Seconds 3

# ============================================================== SCENARIO 1b ===
# The identical install with ONE control changed: the finish page's "Run
# AgentLens" checkbox is cleared before Finish is pressed, so the setup program
# spawns no descendant. This is the control that turns the 1a observation into a
# cause instead of a correlation.
$happyDl2 = Join-Path $script:WorkDir 'dl-happy-rununchecked'
$r1b = Invoke-InstallPs1 -Name '1b-happy-install-run-unchecked' -TimeoutSeconds 240 `
    -WizardMode 'Advance' -UncheckRunApp $true -EnvMap @{
    AGENTLENS_VERSION            = $script:Version
    AGENTLENS_BASE_URL           = $baseGood
    AGENTLENS_ALLOW_INSECURE_URL = '1'
    AGENTLENS_INSTALL            = '1'
    AGENTLENS_DOWNLOAD_DIR       = $happyDl2
}
$script:Diag['scenario_1b_timeline'] = $r1b.Timeline
foreach ($t in $r1b.Timeline) { Write-Note "  1b timeline: $t" }
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'sha256_ok' -Expected "sha256 ok: $script:SetupName" `
    -Observed $(if (Test-Contains $r1b.Text "sha256 ok: $script:SetupName") { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1b.Text "sha256 ok: $script:SetupName") -Why 'install.ps1:334'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'not_timed_out' -Expected 'false' -Observed "$($r1b.TimedOut)" `
    -Ok (-not $r1b.TimedOut) -Why 'with no descendant to wait on, install.ps1 must return'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'exit_code' -Expected '0' -Observed "$($r1b.Code)" `
    -Ok ($r1b.Code -eq 0) -Why 'install.ps1:370 is the only clean exit'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'success_message' -Expected "installed: AgentLens $script:Version" `
    -Observed $(if (Test-Contains $r1b.Text "installed: AgentLens $script:Version") { 'present' } else { 'ABSENT' }) `
    -Ok (Test-Contains $r1b.Text "installed: AgentLens $script:Version") -Why 'install.ps1:370'

$appKilled = Stop-AgentLensApp
$script:Diag['app_processes_stopped_after_install'] = $appKilled

$stateAfter = Get-InstallState -Label 'after'
$script:Diag['install_state_after'] = $stateAfter

Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'install_dir_present' -Expected $script:InstallDir `
    -Observed $(if ($stateAfter.present) { $script:InstallDir } else { 'ABSENT' }) `
    -Ok ([bool]$stateAfter.present) -Why 'the install landed in the interactive user profile, not a service profile'
$mainExe = Join-Path $script:InstallDir 'agentlens-tauri.exe'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'main_binary_present' -Expected 'agentlens-tauri.exe' `
    -Observed $(if (Test-Path -LiteralPath $mainExe) { "$((Get-Item -LiteralPath $mainExe).Length) bytes" } else { 'ABSENT' }) `
    -Ok (Test-Path -LiteralPath $mainExe) -Why 'an installed tree without the app binary is not an install'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'registry_version' -Expected $script:Version `
    -Observed $(if ($stateAfter.registry) { "$($stateAfter.registry.DisplayVersion)" } else { 'ABSENT' }) `
    -Ok (($null -ne $stateAfter.registry) -and ("$($stateAfter.registry.DisplayVersion)" -eq $script:Version)) `
    -Why 'HKCU uninstall entry proves the NSIS install completed its bookkeeping'
Add-Check -Scenario '1b-happy-install-run-unchecked' -Name 'not_a_service_profile_install' -Expected 'no systemprofile path' `
    -Observed $(if ($stateAfter.registry -and (Test-Contains "$($stateAfter.registry.InstallLocation)" 'systemprofile')) { "$($stateAfter.registry.InstallLocation)" } else { 'clean' }) `
    -Ok (-not ($stateAfter.registry -and (Test-Contains "$($stateAfter.registry.InstallLocation)" 'systemprofile'))) `
    -Why 'session 0 would have installed under config\systemprofile; this run must not'

# --- double install: same file set, or drift? --------------------------------
$beforeNames = @($stateBefore.files | ForEach-Object { $_.name })
$afterNames = @($stateAfter.files | ForEach-Object { $_.name })
$onlyBefore = @($beforeNames | Where-Object { $afterNames -notcontains $_ })
$onlyAfter = @($afterNames | Where-Object { $beforeNames -notcontains $_ })
$changed = @()
foreach ($a in $stateAfter.files) {
    $b = @($stateBefore.files | Where-Object { $_.name -eq $a.name })
    if ($b.Count -eq 1 -and $b[0].sha256 -ne $a.sha256) { $changed += "$($a.name) $($b[0].sha256)->$($a.sha256)" }
}
$rewritten = @()
foreach ($a in $stateAfter.files) {
    $b = @($stateBefore.files | Where-Object { $_.name -eq $a.name })
    if ($b.Count -eq 1 -and $b[0].mtime -ne $a.mtime) { $rewritten += "$($a.name) $($b[0].mtime) -> $($a.mtime)" }
}
$script:Diag['double_install_only_before'] = $onlyBefore
$script:Diag['double_install_only_after'] = $onlyAfter
$script:Diag['double_install_content_changed'] = $changed
# Same bytes with a newer mtime means the file really was rewritten by this run.
# Same bytes with the SAME mtime means NSIS skipped it. Recorded rather than
# asserted: which of the two is correct is the installer template's call, not
# something install.ps1 controls.
$script:Diag['double_install_files_rewritten'] = $rewritten
Write-Note "child: double install -- files only before: $($onlyBefore.Count), only after: $($onlyAfter.Count), content changed: $($changed.Count), rewritten (same bytes, new mtime): $($rewritten.Count)"
foreach ($c in $changed) { Write-Note "  changed: $c" }
foreach ($c in $rewritten) { Write-Note "  rewritten: $c" }
Add-Check -Scenario 'double-install' -Name 'no_files_lost' -Expected '0 files present before but gone after' `
    -Observed "$($onlyBefore.Count)" -Ok ($onlyBefore.Count -eq 0) `
    -Why 'installing over an existing install must not strip the tree'

# --- tear down the mirror ----------------------------------------------------
try { Invoke-WebRequest -Uri "http://127.0.0.1:$script:Port/__quit" -UseBasicParsing -TimeoutSec 5 | Out-Null } catch { }
Start-Sleep -Seconds 2
if (-not $server.HasExited) {
    try { Stop-Process -Id $server.Id -Force } catch { }
}
Write-Note "child: mirror server stopped (HasExited=$($server.HasExited))"
if (Test-Path -LiteralPath $serverLog) {
    foreach ($l in (Get-Content -LiteralPath $serverLog)) { Write-Note "  server: $l" }
}

# --- verdict -----------------------------------------------------------------
$pass = @($script:Checks | Where-Object { $_.verdict -eq 'PASS' }).Count
$fail = @($script:Checks | Where-Object { $_.verdict -eq 'FAIL' }).Count
$verdict = 'PASS'
if ($fail -gt 0) { $verdict = 'FAIL' }
Write-Note ''
Write-Note "child: VERDICT $verdict -- $pass passed, $fail failed"
foreach ($c in ($script:Checks | Where-Object { $_.verdict -eq 'FAIL' })) {
    Write-Note "  FAILED: $($c.scenario)/$($c.name) expected=$($c.expected) observed=$($c.observed)"
}
Save-Results -Verdict $verdict
$childExit = 0
if ($fail -gt 0) { $childExit = 1 }
Set-Content -LiteralPath (Join-Path $script:OutDir 'child.exit') -Value "$childExit" -Encoding ASCII
Send-OutputToS3
Write-Note 'child: done'
exit $childExit
