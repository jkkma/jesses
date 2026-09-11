#Requires -Version 7.0
<#
.SYNOPSIS
Install pinned official Windows SVT forks into Jesses' per-user tools directory.
.DESCRIPTION
Verifies published archive digests, extracted executable digests, and version
identities. Keeps licenses and download provenance. Existing matching installs
are reused; different existing files are never overwritten. PATH is unchanged.
The pinned upstream assets require x86-64-v3; no v2 build is published.
#>
[CmdletBinding()]
param([ValidateSet('all', '5fish', 'hdr')][string]$Fork = 'all')

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'This installer requires x64 PowerShell 7 on Windows. Other platform assets are recorded in svt-forks.json.'
}
if (-not [System.Runtime.Intrinsics.X86.Avx2]::IsSupported -or
    -not [System.Runtime.Intrinsics.X86.Bmi2]::IsSupported -or
    -not [System.Runtime.Intrinsics.X86.Fma]::IsSupported) {
    throw 'The pinned upstream binaries require an x86-64-v3 CPU (including AVX2, BMI2, and FMA).'
}
if (-not $env:LOCALAPPDATA -or -not [System.IO.Path]::IsPathFullyQualified($env:LOCALAPPDATA)) {
    throw 'LOCALAPPDATA must name an absolute user data directory.'
}
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'svt-forks.json') -Raw | ConvertFrom-Json
$toolsDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'jesses\tools'))
New-Item -ItemType Directory -Path $toolsDirectory -Force | Out-Null
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $tar -PathType Leaf)) { throw 'Windows tar.exe is required.' }

foreach ($encoder in $manifest.encoders | Where-Object { $Fork -eq 'all' -or $_.selector -eq $Fork }) {
    $asset = $encoder.assets.windowsX86_64
    $destination = [System.IO.Path]::GetFullPath((Join-Path $toolsDirectory $encoder.id))
    $toolsPrefix = $toolsDirectory.TrimEnd('\') + '\'
    if (-not $destination.StartsWith($toolsPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The manifest install destination leaves the managed tools directory.'
    }
    $binary = Join-Path $destination $asset.entry
    if (Test-Path -LiteralPath $destination) {
        if ((Get-Item -LiteralPath $destination).Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            throw "Refusing a redirected install directory: $destination"
        }
        if ((Test-Path -LiteralPath $binary -PathType Leaf) -and
            (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -eq $asset.executableSha256) {
            Write-Output "Already verified: $binary"
            continue
        }
        throw "Existing installation differs from the pinned release. Inspect it before replacing it: $destination"
    }
    $staging = [System.IO.Path]::GetFullPath((Join-Path $toolsDirectory ('.install-' + $encoder.id + '-' + [guid]::NewGuid().ToString('N'))))
    if (-not $staging.StartsWith($toolsPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The staging destination leaves the managed tools directory.'
    }
    New-Item -ItemType Directory -Path $staging | Out-Null
    $archive = Join-Path $staging 'release.tar.xz'
    Invoke-WebRequest -Uri $asset.url -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $asset.sha256) {
        throw "Archive checksum mismatch; files retained for inspection at $staging"
    }
    $entries = @(& $tar -tf $archive)
    if ($LASTEXITCODE -ne 0 -or $entries.Count -ne 1 -or $entries[0] -cne $asset.entry) {
        throw "Unexpected archive entries; files retained for inspection at $staging"
    }
    & $tar -xf $archive -C $staging $asset.entry
    if ($LASTEXITCODE -ne 0) { throw "Archive extraction failed: $staging" }
    $stagedBinary = Join-Path $staging $asset.entry
    if ((Get-FileHash -LiteralPath $stagedBinary -Algorithm SHA256).Hash -ne $asset.executableSha256) {
        throw "Executable checksum mismatch: $staging"
    }
    $version = (& $stagedBinary --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $version.Contains($encoder.versionMarker)) {
        throw "Encoder identity check failed: $version"
    }
    foreach ($license in $encoder.licenses) {
        Invoke-WebRequest -Uri ($encoder.licenseBaseUrl + $license) -OutFile (Join-Path $staging $license)
    }
    [ordered]@{
        repository = $encoder.repository
        release = $encoder.release
        commit = $encoder.commit
        assetUrl = $asset.url
        architecture = $asset.architecture
        archiveSha256 = $asset.sha256
        executableSha256 = $asset.executableSha256
        version = $version
        licenseBaseUrl = $encoder.licenseBaseUrl
        installedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $staging 'provenance.json') -Encoding utf8
    # Publish the complete directory with a no-overwrite rename. Keep the verified
    # archive as provenance; the installer performs no recursive cleanup.
    [System.IO.Directory]::Move($staging, $destination)
    Write-Output "Installed $version at $binary"
}
