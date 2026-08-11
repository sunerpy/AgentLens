# =============================================================================
# Collect the Windows release assets out of a Tauri bundle tree and write the
# SHA-256 manifest that scripts/install.ps1 verifies against.
#
# WHY THIS IS A SCRIPT AND NOT INLINE IN THE WORKFLOWS
#   Two workflows need the identical logic with different destinations
#   (.github/workflows/release.yml -> upload\sha256sums-windows.txt,
#   .github/workflows/ci.yml -> artifacts\dist\sha256sums.txt). When the logic
#   lived inline in both, adding a second installer format meant editing the
#   same 12 lines twice and hoping they stayed in step. Worse, it made the
#   behaviour untestable off Windows: the only way to see what the manifest
#   would contain was to run a release.
#
# ---- THE DEFECT THIS FILE EXISTS TO PREVENT ---------------------------------
# The inline version collected the manifest with a suffix glob:
#
#     Get-ChildItem "*.exe" | ForEach-Object { ... } | Set-Content sha256sums...
#
# That is correct exactly as long as every published Windows asset happens to
# end in .exe. The day a second format is added (MSI), the copy step gets a new
# entry and the manifest step does not -- so the release ships an asset that is
# NOT covered by any digest, and NOTHING fails. Not the build, not the upload,
# not any gate. scripts/install.ps1 verifies downloads against this manifest, so
# an asset missing from it is an asset nobody can verify.
#
# The fix is structural, not a wider glob: the manifest is derived from the SAME
# validated list the copy step used, so an asset can never be copied without
# being hashed. $Formats below is the single hardcoded expectation, and it is
# asserted in both directions -- every format must produce exactly one artifact,
# and the finished manifest must carry exactly one line per format.
#
# ---- CONSTRAINTS ------------------------------------------------------------
# (1) ASCII-only. Windows PowerShell 5.1 reads a .ps1 as ANSI unless it carries
#     a BOM, so one non-ASCII character can tear a string literal apart and kill
#     the script with no useful error. Same rule as scripts/install.ps1.
# (2) The manifest is written with Set-Content -Encoding ascii, which yields
#     CRLF on Windows. That exact byte shape is what scripts/install.ps1 has
#     been verified against; do not "normalise" it.
# (3) A missing or ambiguous artifact is a hard failure. There is deliberately
#     no "skip the format that was not built" path: a Windows release with only
#     one of its two installers is precisely the silent-degradation this file
#     exists to stop.
#
# Local, hermetic test: scripts/qa/windows-dual-format-manifest.sh
# =============================================================================

[CmdletBinding()]
param(
    # Tauri's bundle root. Native Windows build: target\release\bundle
    [Parameter(Mandatory = $true)][string]$BundleRoot,
    # Where the assets and the manifest are collected.
    [Parameter(Mandatory = $true)][string]$Destination,
    # Manifest file name. release.yml and ci.yml deliberately differ: a release
    # carries all three platforms' manifests side by side and they must not
    # collide, while the CI artifact is per-platform already.
    [Parameter(Mandatory = $true)][string]$ManifestName
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# THE single expectation. Hardcoded on purpose: if this list were derived from
# whatever the bundler happened to emit, a build that produced only the NSIS
# installer would lower the expectation to match and the gate would become
# vacuously true -- exactly the decorative gate it is here to replace.
$Formats = @(
    [pscustomobject]@{ Kind = 'NSIS'; Dir = 'nsis'; Pattern = '*-setup.exe'; Suffix = '-setup.exe' }
    [pscustomobject]@{ Kind = 'MSI'; Dir = 'msi'; Pattern = '*.msi'; Suffix = '.msi' }
)

if (-not (Test-Path -LiteralPath $BundleRoot)) {
    throw "bundle root does not exist: $BundleRoot"
}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

# --- locate and copy, one format at a time ----------------------------------
$collected = @()
foreach ($format in $Formats) {
    $dir = Join-Path $BundleRoot $format.Dir
    $glob = Join-Path $dir $format.Pattern
    $found = @(Get-ChildItem -File $glob -ErrorAction SilentlyContinue | Sort-Object Name)

    if ($found.Count -eq 0) {
        $listing = 'the directory does not exist'
        if (Test-Path -LiteralPath $dir) {
            $names = @(Get-ChildItem -File $dir -ErrorAction SilentlyContinue |
                ForEach-Object { $_.Name })
            if ($names.Count -eq 0) { $listing = 'the directory is empty' }
            else { $listing = 'it holds: ' + ($names -join ', ') }
        }
        throw ("$($format.Kind) installer not produced under $glob -- $listing. " +
            'A Windows release must carry both installers; refusing to publish one of them.')
    }
    if ($found.Count -gt 1) {
        throw ("$($format.Kind): $glob matched $($found.Count) files (" +
            (($found | ForEach-Object { $_.Name }) -join ', ') +
            '). Refusing to guess which one is the release asset.')
    }

    $asset = $found[0]
    Copy-Item -LiteralPath $asset.FullName -Destination $Destination -Force
    Write-Host ("collected $($format.Kind): $($asset.Name) ($($asset.Length) bytes)")
    $collected += [pscustomobject]@{ Kind = $format.Kind; Suffix = $format.Suffix; Name = $asset.Name }
}

# --- manifest ---------------------------------------------------------------
# Derived from $collected, NOT from a fresh glob of $Destination. That is the
# whole point: the list that passed the guards above is the list that gets
# hashed, so no asset can slip into the destination unhashed.
Push-Location -LiteralPath $Destination
try {
    $lines = foreach ($item in ($collected | Sort-Object Name)) {
        if (-not (Test-Path -LiteralPath $item.Name)) {
            throw "copied asset vanished from ${Destination}: $($item.Name)"
        }
        $digest = (Get-FileHash -LiteralPath $item.Name -Algorithm SHA256).Hash.ToLowerInvariant()
        "$digest  $($item.Name)"
    }
    # Two spaces between digest and name, no comment header: that is the shape
    # scripts/install.ps1 (Read-Manifest) and `sha256sum -c` both accept.
    $lines | Set-Content -Encoding ascii $ManifestName

    # Read the file back rather than trusting $lines. This is what catches an
    # encoding or line-ending surprise, and it is cheap.
    $written = @(Get-Content -LiteralPath $ManifestName)
    if ($written.Count -ne $Formats.Count) {
        throw ("$ManifestName has $($written.Count) lines but $($Formats.Count) formats " +
            'are published. Every published asset must carry a digest.')
    }
    foreach ($format in $Formats) {
        $hits = @($written | Where-Object { $_.TrimEnd().EndsWith($format.Suffix) })
        if ($hits.Count -ne 1) {
            throw ("$ManifestName covers $($hits.Count) assets ending in '$($format.Suffix)', " +
                "expected exactly 1 for $($format.Kind). Manifest:`n" + ($written -join "`n"))
        }
    }
    Write-Host "--- $ManifestName ---"
    Get-Content -LiteralPath $ManifestName | Write-Host
    Write-Host '---'
}
finally {
    Pop-Location
}

if (-not (Get-ChildItem -LiteralPath $Destination)) {
    throw "$Destination is empty, refusing to continue"
}
Get-ChildItem -LiteralPath $Destination | Format-List Name, Length | Out-String | Write-Host
Write-Host "gate ok: $($Formats.Count)/$($Formats.Count) Windows installer formats collected and hashed"
