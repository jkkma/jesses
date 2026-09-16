param(
    [Parameter(Mandatory = $true)]
    [string] $PortableRuntime,
    [string] $Destination = (Join-Path $env:LOCALAPPDATA 'jesses\tools\vship'),
    [switch] $Activate
)

$ErrorActionPreference = 'Stop'

function Get-CheckedChildPath {
    param([string] $Parent, [string] $Child, [string] $Label)
    $parentFull = [IO.Path]::GetFullPath($Parent)
    $childFull = [IO.Path]::GetFullPath($Child)
    $relative = [IO.Path]::GetRelativePath($parentFull, $childFull)
    if ([IO.Path]::IsPathRooted($relative) -or $relative -eq '..' -or
        $relative.StartsWith("..$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::Ordinal)) {
        throw "$Label escapes its intended directory."
    }
    return $childFull
}

function Assert-NoReparsePath {
    param([string] $Path, [string] $Label)
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Label cannot contain or target a redirected filesystem entry: $current"
            }
        }
        $parent = [IO.Directory]::GetParent($current)
        if ($null -eq $parent) { break }
        $current = $parent.FullName
    }
}

function Remove-ManagedStaging {
    param([string] $DestinationRoot, [string] $Staging)
    $checked = Get-CheckedChildPath $DestinationRoot $Staging 'The staging directory'
    Assert-NoReparsePath $checked 'The staging directory'
    Remove-Item -LiteralPath $checked -Recurse
}

$lockPath = Join-Path $PSScriptRoot 'vship-windows-lock.json'
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
if ($lock.schemaVersion -ne 1 -or !$lock.version -or !$lock.binary -or !$lock.source) {
    throw 'The Vship lock has an unsupported schema.'
}
$runtimeInput = [IO.Path]::GetFullPath($PortableRuntime)
Assert-NoReparsePath $runtimeInput 'PortableRuntime'
$runtime = (Resolve-Path -LiteralPath $runtimeInput).Path
$vspipe = Join-Path $runtime 'VSPipe.exe'
$plugins = Join-Path $runtime 'vs-plugins'
if (!(Test-Path -LiteralPath $vspipe -PathType Leaf) -or !(Test-Path -LiteralPath $plugins -PathType Container)) {
    $bundledVspipe = Join-Path $runtime 'python\Lib\site-packages\vapoursynth\vspipe.exe'
    if ((Test-Path -LiteralPath $bundledVspipe -PathType Leaf) -or
        (Test-Path -LiteralPath (Join-Path $runtime '..\manifest.json') -PathType Leaf)) {
        throw 'Manifest-verified application bundles cannot be mutated by this installer. They retain their packaged CPU scorers; add Vship through the source-complete package recipe instead.'
    }
    throw 'PortableRuntime must contain VSPipe.exe and its private vs-plugins directory.'
}
Assert-NoReparsePath $vspipe 'The selected VSPipe executable'
Assert-NoReparsePath $plugins 'The selected plugin directory'
if (!(Test-Path -LiteralPath "$env:WINDIR\System32\vulkan-1.dll" -PathType Leaf)) {
    throw 'The managed Vulkan scorer needs the GPU driver Vulkan loader. CPU vszip and Julek remain available.'
}

$destinationRoot = [IO.Path]::GetFullPath($Destination)
Assert-NoReparsePath $destinationRoot 'The managed tools destination'
if (!(Test-Path -LiteralPath $destinationRoot)) {
    New-Item -ItemType Directory -Path $destinationRoot | Out-Null
}
Assert-NoReparsePath $destinationRoot 'The managed tools destination'
$versionRoot = Get-CheckedChildPath $destinationRoot (Join-Path $destinationRoot $lock.version) 'The version directory'
$staging = Get-CheckedChildPath $destinationRoot "$versionRoot.staging-$PID" 'The staging directory'
if (Test-Path -LiteralPath $staging) {
    throw "The staging directory already exists: $staging"
}
New-Item -ItemType Directory -Path $staging | Out-Null
try {
    foreach ($record in @($lock.binary, $lock.source)) {
        $path = Get-CheckedChildPath $staging (Join-Path $staging $record.path) 'A locked download path'
        Invoke-WebRequest -Headers @{'User-Agent' = 'jesses-vship-installer'} -Uri $record.url -OutFile $path
        $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($hash -ne $record.sha256) {
            throw "Downloaded Vship input has SHA256 $hash; expected $($record.sha256)."
        }
        if ($record.size -and (Get-Item -LiteralPath $path).Length -ne $record.size) {
            throw 'Downloaded Vship binary has an unexpected length.'
        }
    }
    $sourceDirectory = Join-Path $staging 'source'
    New-Item -ItemType Directory -Path $sourceDirectory | Out-Null
    $sourceArchive = Get-CheckedChildPath $staging (Join-Path $staging $lock.source.path) 'The source archive'
    $archiveEntries = @(tar -tzf $sourceArchive)
    if ($LASTEXITCODE -ne 0 -or !$archiveEntries.Count -or
        @($archiveEntries | Where-Object {
            [IO.Path]::IsPathRooted($_) -or $_ -match '(^|[\\/])\.\.([\\/]|$)' -or $_ -match ':'
        }).Count) {
        throw 'The pinned Vship source archive contains an unsafe path.'
    }
    tar -xzf $sourceArchive -C $sourceDirectory
    if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath (Join-Path $sourceDirectory 'vship\LICENSE') -PathType Leaf)) {
        throw 'The pinned Vship source archive or license could not be retained.'
    }
    foreach ($entry in Get-ChildItem -LiteralPath $sourceDirectory -Force -Recurse) {
        if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "The pinned Vship source archive contains a redirected entry: $($entry.FullName)"
        }
    }

    $binary = Join-Path $staging $lock.binary.path
    $binaryLiteral = ConvertTo-Json ([IO.Path]::GetFullPath($binary)) -Compress
    $active = Join-Path $plugins 'libvship.dll'
    $activeAlreadyMatches = $false
    if (Test-Path -LiteralPath $active) {
        Assert-NoReparsePath $active 'The active plugin'
        $activeHash = (Get-FileHash -LiteralPath $active -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($activeHash -ne $lock.binary.sha256) {
            throw "The portable runtime already contains a different libvship.dll: $active"
        }
        $activeAlreadyMatches = $true
    }
    $load = if ($activeAlreadyMatches) { '' } else { "core.std.LoadPlugin(path=$binaryLiteral)" }
    $probe = @"
import math
import vapoursynth as vs
core = vs.core
$load
reference = core.std.BlankClip(width=192, height=128, format=vs.YUV444P10, length=2, color=[256, 512, 512])
distorted = core.std.BlankClip(reference, color=[264, 520, 520])
ssimu2 = core.vship.SSIMULACRA2(reference, distorted, numStream=1)
butteraugli = core.vship.BUTTERAUGLI(reference, distorted, distmap=1, intensity_multiplier=203.0, numStream=1)
with ssimu2.get_frame(0) as first, butteraugli.get_frame(0) as second:
    values = [float(first.props['_SSIMULACRA2']), float(second.props['_BUTTERAUGLI_INFNorm'])]
    assert all(math.isfinite(value) for value in values), values
    print('JESSES_VSHIP_OK', values)
reference.set_output()
"@
    $probePath = Join-Path $staging 'capability.vpy'
    Set-Content -LiteralPath $probePath -Value $probe -Encoding utf8NoBOM
    $diagnostic = (& $vspipe --info $probePath - 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0 -or !$diagnostic.Contains('JESSES_VSHIP_OK')) {
        throw "The pinned x64 Vulkan plugin did not pass its actual VapourSynth/GPU ABI probe. CPU vszip and Julek remain compatible. $diagnostic"
    }
    Remove-Item -LiteralPath $probePath

    if (Test-Path -LiteralPath $versionRoot) {
        Assert-NoReparsePath $versionRoot 'The managed version directory'
        $existing = Join-Path $versionRoot $lock.binary.path
        if (!(Test-Path -LiteralPath $existing -PathType Leaf) -or
            (Get-FileHash -LiteralPath $existing -Algorithm SHA256).Hash.ToLowerInvariant() -ne $lock.binary.sha256) {
            throw "A different managed Vship installation already exists: $versionRoot"
        }
        Remove-ManagedStaging $destinationRoot $staging
    } else {
        Get-CheckedChildPath $destinationRoot $staging 'The staging directory' | Out-Null
        Get-CheckedChildPath $destinationRoot $versionRoot 'The version directory' | Out-Null
        Assert-NoReparsePath $staging 'The staging directory'
        Move-Item -LiteralPath $staging -Destination $versionRoot
    }

    if ($Activate) {
        $installed = Join-Path $versionRoot $lock.binary.path
        if (Test-Path -LiteralPath $active) {
            Assert-NoReparsePath $active 'The active plugin'
            $activeHash = (Get-FileHash -LiteralPath $active -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($activeHash -ne $lock.binary.sha256) {
                throw "The portable runtime already contains a different libvship.dll: $active"
            }
        } else {
            $active = Get-CheckedChildPath $plugins $active 'The active plugin'
            $sourceStream = $null
            $activeStream = $null
            $created = $false
            try {
                try {
                    $sourceStream = [IO.File]::Open($installed, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
                    $activeStream = [IO.File]::Open($active, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
                    $created = $true
                    $sourceStream.CopyTo($activeStream)
                    $activeStream.Flush($true)
                } finally {
                    if ($null -ne $activeStream) { $activeStream.Dispose() }
                    if ($null -ne $sourceStream) { $sourceStream.Dispose() }
                }
            } catch {
                if ($created -and (Test-Path -LiteralPath $active -PathType Leaf)) {
                    Remove-Item -LiteralPath $active
                }
                throw
            }
            if ((Get-FileHash -LiteralPath $active -Algorithm SHA256).Hash.ToLowerInvariant() -ne $lock.binary.sha256) {
                if ($created) { Remove-Item -LiteralPath $active }
                throw 'The plugin changed while it was activated.'
            }
        }
        $av1an = Join-Path (Split-Path $runtime -Parent) 'av1an.exe'
        Assert-NoReparsePath $av1an 'The selected av1an executable'
        $version = (& $av1an --version 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0 -or !$version.Contains('com.lumen.vship : Found')) {
            throw 'Vship passed the direct GPU probe but the selected av1an runtime did not discover the activated plugin.'
        }
    }
    Write-Output "Managed Vship $($lock.version) passed SSIMULACRA2 and Butteraugli on this GPU."
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-ManagedStaging $destinationRoot $staging
    }
}
