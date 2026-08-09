# =============================================================================
# AgentLens installer for Windows.
#
# Mirrors scripts/install.sh: detect architecture, resolve the version, read the
# exact asset name and digest out of sha256sums-windows.txt, download, VERIFY
# the SHA-256, then hand the NSIS installer over (without running it unless
# asked to).
#
# ---- Two hard constraints this project already learned the hard way ---------
# (1) A non-zero exit from a NATIVE command does NOT stop a PowerShell script,
#     and $ErrorActionPreference does not apply to native exit codes either.
#     Every native process launched here is therefore checked explicitly
#     (Start-Process -PassThru -Wait, then inspect .ExitCode). Do not "simplify"
#     that away: silently ignoring a failed installer is exactly the
#     wash-failure-into-green defect this project has already had to fix once.
# (2) This file is strictly ASCII-only. Windows PowerShell 5.1 reads a .ps1 as
#     ANSI unless it carries a BOM, so a single non-ASCII character can tear a
#     string literal apart and kill the script with no useful error. Keep every
#     character in this file inside 7-bit ASCII.
#
# DEFAULT REPO: the default below names the real repository, sunerpy/AgentLens.
# That repository has not been created yet, so every release URL derived from it
# 404s until it exists. Set $env:AGENTLENS_REPO to use a fork or a mirror.
# The download path has never
# fetched a real GitHub release, but it HAS verified the real 4,104,407-byte
# CodeBuild NSIS installer against the real sha256sums-windows.txt (CRLF, as
# Set-Content -Encoding ascii writes it), including a one-byte-tamper rejection,
# under pwsh 7 on Linux: .omo/evidence/install-scripts-real-artifacts.md.
#
# Usage:
#   irm https://raw.githubusercontent.com/sunerpy/AgentLens/main/scripts/install.ps1 | iex
# =============================================================================

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
# Safe here (unlike inside a GitHub Actions pwsh block, where the runner already
# prepends Stop): this makes cmdlet errors terminate. It does NOT cover native
# exit codes -- see constraint (1).
$ErrorActionPreference = 'Stop'

$Program = 'AgentLens'
$DefaultRepo = 'sunerpy/AgentLens'

function Get-EnvOrDefault {
    param([string]$Name, [string]$Default = '')
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) { return $Default }
    return $value
}

$Repo = Get-EnvOrDefault 'AGENTLENS_REPO' $DefaultRepo
$Version = Get-EnvOrDefault 'AGENTLENS_VERSION' ''
# Test / mirror seam: when set, the GitHub release URL is not used at all.
$BaseUrl = Get-EnvOrDefault 'AGENTLENS_BASE_URL' ''
$AllowInsecureUrl = (Get-EnvOrDefault 'AGENTLENS_ALLOW_INSECURE_URL' '0') -eq '1'
$ApiUrl = Get-EnvOrDefault 'AGENTLENS_API_URL' ''
$DownloadDir = Get-EnvOrDefault 'AGENTLENS_DOWNLOAD_DIR' ''
$DoInstall = (Get-EnvOrDefault 'AGENTLENS_INSTALL' '0') -eq '1'
$DryRun = (Get-EnvOrDefault 'AGENTLENS_DRY_RUN' '0') -eq '1'
# Diagnostic override for the architecture probe: lets the mapping be inspected
# without the matching hardware.
$ArchOverride = Get-EnvOrDefault 'AGENTLENS_ARCH' ''

if ([string]::IsNullOrWhiteSpace($DownloadDir)) {
    $localAppData = [Environment]::GetFolderPath('LocalApplicationData')
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        $localAppData = [System.IO.Path]::GetTempPath()
    }
    $DownloadDir = Join-Path (Join-Path $localAppData 'AgentLens') 'downloads'
}

function Write-Note {
    param([string]$Message)
    Write-Host $Message
}

function Stop-WithError {
    param([string]$Message, [string[]]$Detail = @())
    Write-Host "error: $Message" -ForegroundColor Red
    foreach ($line in $Detail) { Write-Host "       $line" }
    exit 1
}

# --- host / architecture ----------------------------------------------------
function Test-WindowsHost {
    # $IsWindows exists in PowerShell 6+. Windows PowerShell 5.1 does not define
    # it, and 5.1 only ever runs on Windows, so absence means Windows.
    if (Test-Path 'variable:IsWindows') { return [bool]$IsWindows }
    return $true
}

function Resolve-Architecture {
    if (-not [string]::IsNullOrWhiteSpace($ArchOverride)) { return $ArchOverride }
    # PROCESSOR_ARCHITEW6432 is set when a 32-bit process runs under WOW64; it
    # reports the real machine architecture, so it must win over the emulated
    # PROCESSOR_ARCHITECTURE value.
    $wow = [Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITEW6432')
    if (-not [string]::IsNullOrWhiteSpace($wow)) { return $wow }
    $arch = [Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE')
    if ([string]::IsNullOrWhiteSpace($arch)) { return 'unknown' }
    return $arch
}

# Mapping is derived from .github/workflows/release.yml, the only authority on
# which assets a release carries. Windows publishes exactly one installer:
#   AgentLens_<version>_x64-setup.exe  +  sha256sums-windows.txt
# There is deliberately no arm64 and no 32-bit build, so those must fail with an
# explanation instead of downloading the wrong installer.
function Select-Artifact {
    param([string]$Arch)
    switch ($Arch.ToUpperInvariant()) {
        'AMD64' { return @{ Sums = 'sha256sums-windows.txt'; Suffix = '-setup.exe' } }
        'X64' { return @{ Sums = 'sha256sums-windows.txt'; Suffix = '-setup.exe' } }
        'ARM64' {
            Stop-WithError 'no arm64 Windows installer is published' @(
                'The release builds the NSIS installer on an x64 runner only.',
                'Windows on ARM can run the x64 installer under emulation, but this',
                'script will not silently do that for you. Either install the x64',
                'package by hand from the release page, or build from source:',
                'docs/installation.md ("From source").')
        }
        'X86' {
            Stop-WithError 'no 32-bit Windows installer is published' @(
                'AgentLens ships a 64-bit build only. Build from source on a 64-bit',
                'host: docs/installation.md ("From source").')
        }
        default {
            Stop-WithError "unsupported architecture: '$Arch'" @(
                'AgentLens publishes a Windows installer for x64 only.',
                'Build from source instead: docs/installation.md ("From source").')
        }
    }
}

# --- version ----------------------------------------------------------------
# The version is interpolated into a URL and a local filename, so it is
# validated against a strict semver shape first. This is what stops
# path-traversal input such as ..\..\Windows\System32 from producing a
# traversing URL or a write outside the download directory.
function Assert-VersionShape {
    param([string]$Value)
    if ($Value -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$') {
        Stop-WithError "refusing version '$Value': not a plain semver version" @(
            'Expected something like 0.1.0 (optionally 0.1.0-rc.1).',
            'A version becomes part of a URL and a filename, so anything containing',
            'a path separator or traversal is rejected outright.')
    }
}

function Assert-RepoShape {
    if ($Repo -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') {
        Stop-WithError "refusing AGENTLENS_REPO='$Repo': expected owner/repo"
    }
    # '.' is inside the class above, so the shape check alone accepted '../..'
    # and interpolated it into the release URL. GitHub has no such owner or repo.
    $parts = $Repo -split '/', 2
    if ($parts[0] -eq '.' -or $parts[0] -eq '..' -or $parts[1] -eq '.' -or $parts[1] -eq '..') {
        Stop-WithError "refusing AGENTLENS_REPO='$Repo': '.' and '..' are not owner or repo names"
    }
}

function Resolve-Version {
    if (-not [string]::IsNullOrWhiteSpace($Version)) {
        Assert-VersionShape $Version
        return $Version
    }
    if (-not [string]::IsNullOrWhiteSpace($BaseUrl)) {
        Stop-WithError 'AGENTLENS_BASE_URL is set but AGENTLENS_VERSION is not' @(
            'A custom base URL has no releases API to query, so the version must',
            'be given explicitly.')
    }
    $api = $ApiUrl
    if ([string]::IsNullOrWhiteSpace($api)) {
        $api = "https://api.github.com/repos/$Repo/releases/latest"
    }
    Assert-UrlScheme -Uri $api
    try {
        $response = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'agentlens-install' }
    }
    catch {
        Stop-WithError "could not query the latest release for $Repo" @(
            "tried: $api",
            'If the repository has no releases yet (or no remote exists yet), pin',
            'one explicitly: $env:AGENTLENS_VERSION = "0.1.0"')
    }
    $tag = [string]$response.tag_name
    if ([string]::IsNullOrWhiteSpace($tag)) {
        Stop-WithError "no tag_name in the release API response from $api"
    }
    # release-please tags are v-prefixed (tag_pattern ^v[0-9]+...).
    $resolved = $tag.TrimStart('v')
    Assert-VersionShape $resolved
    Write-Note "resolved latest release: $resolved"
    return $resolved
}

# --- download helpers -------------------------------------------------------
# Invoke-WebRequest happily fetches plaintext http, and there is no -Proto style
# flag to stop it, so the allowlist is enforced on the URL. Over http whoever
# answers supplies BOTH the installer and the sha256sums manifest it is compared
# against, which makes the verification below worthless. install.sh applies the
# same rule for the same reason -- keep the two in step.
function Assert-UrlScheme {
    param([string]$Uri)
    if ($Uri -like 'https://*' -or $Uri -like 'file://*') { return }
    if ($AllowInsecureUrl) {
        Write-Warning "unauthenticated transport, digests are not trustworthy: $Uri"
        Write-Warning 'AGENTLENS_ALLOW_INSECURE_URL=1 -- whoever serves this URL can swap the installer AND the manifest digest together.'
        return
    }
    Stop-WithError "refusing to fetch over a non-https URL: $Uri" @(
        'Only https:// is allowed by default.',
        'Over plaintext http an attacker can substitute the installer AND the',
        'sha256sums manifest it is verified against, so the checksum below would',
        'prove nothing.',
        'For a source you control, set $env:AGENTLENS_ALLOW_INSECURE_URL = "1".')
}

function Get-RemoteFile {
    param([string]$Uri, [string]$OutFile)
    Assert-UrlScheme -Uri $Uri
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing `
            -Headers @{ 'User-Agent' = 'agentlens-install' }
    }
    catch {
        Stop-WithError "download failed: $Uri" @($_.Exception.Message)
    }
}

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# --- manifest ---------------------------------------------------------------
# Manifest shape: optional '#' comment lines, then "<64 hex>  <bare filename>".
# The Windows job writes it with Get-FileHash from inside the upload directory,
# so the filename field is a bare basename by construction.
function Read-Manifest {
    param([string]$SumsUrl, [string]$SumsPath, [string]$Suffix)

    Write-Note "manifest: $SumsUrl"
    Get-RemoteFile -Uri $SumsUrl -OutFile $SumsPath

    $matched = @()
    foreach ($line in (Get-Content -LiteralPath $SumsPath)) {
        $trimmed = $line.Trim()
        if ($trimmed.Length -eq 0) { continue }
        if ($trimmed.StartsWith('#')) { continue }
        $fields = $trimmed -split '\s+', 2
        if ($fields.Count -lt 2) { continue }
        $name = $fields[1].Trim()
        if (-not $name.EndsWith($Suffix)) { continue }
        $matched += , @{ Hash = $fields[0].ToLowerInvariant(); Name = $name }
    }

    if ($matched.Count -eq 0) {
        $detail = @('manifest contents:')
        $detail += (Get-Content -LiteralPath $SumsPath | ForEach-Object { "  $_" })
        Stop-WithError "no asset ending in '$Suffix' is listed in $(Split-Path -Leaf $SumsPath)" $detail
    }
    if ($matched.Count -gt 1) {
        Stop-WithError "$(Split-Path -Leaf $SumsPath) lists $($matched.Count) assets ending in '$Suffix'" @(
            'Refusing to guess which one is meant.')
    }

    $entry = $matched[0]
    # The manifest is remote content, so its filename field is untrusted: it is
    # joined into both a URL and a local path below. Reject anything that is not
    # a bare filename.
    if ($entry.Name -match '[\\/]' -or $entry.Name -eq '.' -or $entry.Name -eq '..') {
        Stop-WithError "manifest entry '$($entry.Name)' is not a bare filename" @(
            'Refusing to use a path taken from a downloaded manifest.')
    }
    if ($entry.Hash -notmatch '^[0-9a-f]{64}$') {
        Stop-WithError "manifest digest for '$($entry.Name)' is not a SHA-256 hex string: '$($entry.Hash)'"
    }
    return $entry
}

# --- main -------------------------------------------------------------------
$onWindows = Test-WindowsHost
if (-not $onWindows) {
    if ($DoInstall) {
        Stop-WithError 'this is the Windows installer and cannot install on this host' @(
            'Use scripts/install.sh instead (Linux and macOS).')
    }
    Write-Warning 'Not running on Windows: doing download and checksum verification only, no install.'
}

Assert-RepoShape
$arch = Resolve-Architecture
$artifact = Select-Artifact -Arch $arch
$Version = Resolve-Version

if ([string]::IsNullOrWhiteSpace($BaseUrl)) {
    $BaseUrl = "https://github.com/$Repo/releases/download/v$Version"
}

New-Item -ItemType Directory -Force -Path $DownloadDir | Out-Null

Write-Note 'plan:'
Write-Note "  repo          $Repo"
Write-Note "  version       $Version"
Write-Note "  detected arch $arch"
Write-Note "  manifest      $BaseUrl/$($artifact.Sums)"
Write-Note "  asset match   *$($artifact.Suffix)"
Write-Note "  download dir  $DownloadDir"

if ($DryRun) {
    Write-Note 'AGENTLENS_DRY_RUN=1: stopping before download'
    exit 0
}

$sumsPath = Join-Path $DownloadDir $artifact.Sums
$entry = Read-Manifest -SumsUrl "$BaseUrl/$($artifact.Sums)" -SumsPath $sumsPath -Suffix $artifact.Suffix

$assetPath = Join-Path $DownloadDir $entry.Name
Write-Note "download: $BaseUrl/$($entry.Name)"
Get-RemoteFile -Uri "$BaseUrl/$($entry.Name)" -OutFile $assetPath

$actual = Get-Sha256 -Path $assetPath
if ($actual -ne $entry.Hash) {
    Remove-Item -LiteralPath $assetPath -Force -ErrorAction SilentlyContinue
    Stop-WithError "SHA-256 MISMATCH for $($entry.Name) -- refusing to install" @(
        "expected $($entry.Hash)",
        "actual   $actual",
        'The downloaded file has been deleted. Either the download was corrupted',
        'or the artifact does not match the published manifest. Do not install it.')
}
Write-Note "sha256 ok: $($entry.Name)"

Write-Note ''
Write-Note "$Program $Version verified at:"
Write-Note "  $assetPath"
Write-Note ''

if (-not $onWindows) {
    Write-Note 'Verified only. Run scripts/install.ps1 on Windows to install.'
    exit 0
}

if (-not $DoInstall) {
    Write-Note 'Not installed yet -- this installer does not launch the setup program on'
    Write-Note 'its own. Run:'
    Write-Note ''
    Write-Note "  & '$assetPath'"
    Write-Note ''
    Write-Note 'The NSIS installer may prompt for elevation. Re-run this script with'
    Write-Note '$env:AGENTLENS_INSTALL = "1" to have it launched for you.'
    Write-Note 'Installed file layout: docs/installation.md'
    exit 0
}

Write-Note "AGENTLENS_INSTALL=1: launching $($entry.Name) (it may prompt for elevation)"
# TWO invariants live in the six lines below. Both have been violated once. Do
# not "simplify" either of them away.
#
# (1) THE EXIT CODE MUST BE READ EXPLICITLY. A non-zero exit from a native
#     command does not stop a PowerShell script, and $ErrorActionPreference does
#     not cover native exit codes either. So the process object is kept and
#     .ExitCode is compared below. Silently ignoring a failed installer is the
#     wash-failure-into-green defect this project has already had to fix once.
#     Measured on Windows Server 2022 / PowerShell 5.1.20348.5386: cancelling
#     the real NSIS wizard exits 1, and a stand-in setup exe returning 3 is
#     reported as "installer exited with code 3" -- both reach Stop-WithError.
#
# (2) THIS MUST NOT BE Start-Process -Wait. PowerShell's -Wait waits for the
#     launched process AND ITS DESCENDANTS. The NSIS finish page's "Run
#     AgentLens" checkbox is CHECKED by default, so pressing Finish leaves
#     AgentLens running as a descendant; and AgentLens is a tray app whose
#     window close is prevent_close + hide (src-tauri/src/tray.rs), so that
#     descendant may never exit. With -Wait this script therefore hung forever
#     AFTER a successful install and never printed the line below.
#     Measured, same host, run installps1-20260805T105106Z: with the checkbox at
#     its default the script was still blocked at the 150s cap (setup process
#     gone, app process alive); with only that one checkbox cleared it exited 0
#     in 13s. An AgentLens-independent probe pinned the mechanism: for a launcher
#     that spawns a 60s child and exits at once, Start-Process -Wait returned
#     after 62s while [Process]::Start + WaitForExit() returned after 0s.
#     WaitForExit() waits for the installer alone, which is what is wanted here:
#     launch the app, do not wait for the user to quit it.
#
# UseShellExecute stays $true -- that is what Start-Process did by default, and
# it is what lets the installer's UAC elevation prompt appear (see the note
# above). The .NET default is $false, which would bypass ShellExecute and change
# elevation behaviour. Redirecting stdout/stderr is incompatible with it and is
# deliberately not done: the installer's own console output goes straight to this
# console, exactly as before.
$proc = $null
try {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $assetPath
    $psi.UseShellExecute = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
}
catch {
    # Without this the user would get a raw .NET exception instead of the clean
    # error every other failure path in this script produces.
    Stop-WithError "could not start $($entry.Name)" @($_.Exception.Message)
}
if ($null -eq $proc) {
    Stop-WithError "could not start $($entry.Name)"
}
$proc.WaitForExit()
if ($proc.ExitCode -ne 0) {
    Stop-WithError "installer exited with code $($proc.ExitCode)" @(
        'AgentLens was NOT installed. The setup program reported a failure.')
}
Write-Note "installed: $Program $Version"
