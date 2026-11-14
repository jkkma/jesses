#Requires -Version 7.0
<#
.SYNOPSIS
Install pinned official Windows SVT forks into Jesses' per-user tools directory.
.DESCRIPTION
Verifies published archive digests, extracted executable digests, and version
identities. Keeps licenses and download provenance. Existing matching installs
are reused; different existing files are never overwritten. PATH is unchanged.
The 5fish build requires x86-64-v3. The MSVC Patman HDR build requires the
Microsoft Visual C++ v14 Redistributable (x64) 14.51 or later.
#>
[CmdletBinding()]
param([ValidateSet('all', '5fish', 'hdr')][string]$Fork = 'all')

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'This installer requires x64 PowerShell 7 on Windows. Other platform assets are recorded in svt-forks.json.'
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
    if ($asset.architecture -eq 'x86-64-v3' -and (
        -not [System.Runtime.Intrinsics.X86.Avx2]::IsSupported -or
        -not [System.Runtime.Intrinsics.X86.Bmi2]::IsSupported -or
        -not [System.Runtime.Intrinsics.X86.Fma]::IsSupported)) {
        throw 'The selected encoder requires an x86-64-v3 CPU (including AVX2, BMI2, and FMA).'
    }
    if ($asset.runtimePrerequisite) {
        $runtime = $asset.runtimePrerequisite
        $runtimeDll = Join-Path $env:SystemRoot ('System32\' + $runtime.dll)
        $runtimeVersion = if (Test-Path -LiteralPath $runtimeDll -PathType Leaf) {
            [System.Diagnostics.FileVersionInfo]::GetVersionInfo($runtimeDll)
        }
        if (-not $runtimeVersion -or [version]::new($runtimeVersion.FileMajorPart, $runtimeVersion.FileMinorPart) -lt [version]$runtime.minimumVersion) {
            throw "Install $($runtime.name) $($runtime.minimumVersion) or later from $($runtime.url) and retry."
        }
    }
    $managedDirectory = $asset.managedDirectory ?? $encoder.id
    $destination = [System.IO.Path]::GetFullPath((Join-Path $toolsDirectory $managedDirectory))
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
    $archiveName = if ($asset.archiveFormat -eq '7z') { 'release.7z' } else { 'release.tar.xz' }
    $archive = Join-Path $staging $archiveName
    Invoke-WebRequest -Uri $asset.url -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $asset.sha256) {
        throw "Archive checksum mismatch; files retained for inspection at $staging"
    }
    $stagedBinary = Join-Path $staging $asset.entry
    if ($asset.archiveFormat -eq '7z') {
        $entries = @(& $tar -tvf $archive)
        $expected = $asset.members.PSObject.Properties
        if ($LASTEXITCODE -ne 0 -or $entries.Count -ne @($expected).Count) {
            throw "Unexpected archive entries; files retained for inspection at $staging"
        }
        $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($line in $entries) {
            $fields = $line -split '\s+', 9
            $member = if ($fields.Count -eq 9) { $expected | Where-Object Name -CEQ $fields[8] }
            if (-not $member -or -not $fields[0].StartsWith('-') -or
                $fields[4] -cne [string]$member.Value -or -not $seen.Add($fields[8])) {
                throw "Unexpected archive entries; files retained for inspection at $staging"
            }
        }
        # PowerShell pipelines may convert native output to text. Stream the
        # exact member as bytes and create a fresh file without archive paths.
        $start = [System.Diagnostics.ProcessStartInfo]::new($tar)
        foreach ($argument in @('-xOf', $archive, '--', $asset.entry)) { $start.ArgumentList.Add($argument) }
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $process = [System.Diagnostics.Process]::Start($start)
        try {
            $output = [System.IO.File]::Open($stagedBinary, [System.IO.FileMode]::CreateNew)
            try { $process.StandardOutput.BaseStream.CopyTo($output) }
            finally { $output.Dispose() }
            $process.WaitForExit()
            if ($process.ExitCode -ne 0 -or (Get-Item -LiteralPath $stagedBinary).Length -ne $asset.members.($asset.entry)) {
                throw "Archive extraction failed: $staging"
            }
        }
        finally { $process.Dispose() }
    } else {
        $entries = @(& $tar -tf $archive)
        if ($LASTEXITCODE -ne 0 -or $entries.Count -ne 1 -or $entries[0] -cne $asset.entry) {
            throw "Unexpected archive entries; files retained for inspection at $staging"
        }
        & $tar -xf $archive -C $staging $asset.entry
        if ($LASTEXITCODE -ne 0) { throw "Archive extraction failed: $staging" }
    }
    if ((Get-FileHash -LiteralPath $stagedBinary -Algorithm SHA256).Hash -ne $asset.executableSha256) {
        throw "Executable checksum mismatch: $staging"
    }
    $version = (& $stagedBinary --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $version.StartsWith($asset.versionMarker + ' ', [StringComparison]::Ordinal)) {
        throw "Encoder identity check failed: $version"
    }
    foreach ($license in $encoder.licenses) {
        Invoke-WebRequest -Uri ($asset.licenseBaseUrl + $license) -OutFile (Join-Path $staging $license)
    }
    [ordered]@{
        repository = $asset.repository
        release = $asset.release
        commit = $asset.commit
        assetUrl = $asset.url
        architecture = $asset.architecture
        archiveSha256 = $asset.sha256
        executableSha256 = $asset.executableSha256
        version = $version
        licenseBaseUrl = $asset.licenseBaseUrl
        runtimePrerequisite = $asset.runtimePrerequisite
        installedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $staging 'provenance.json') -Encoding utf8
    # Publish the complete directory with a no-overwrite rename. Keep the verified
    # archive as provenance; the installer performs no recursive cleanup.
    [System.IO.Directory]::Move($staging, $destination)
    Write-Output "Installed $version at $binary"
}
