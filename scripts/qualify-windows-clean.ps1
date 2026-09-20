<#
.SYNOPSIS
Qualifies a Jesses Windows portable archive without using user-installed media tools.

.DESCRIPTION
The runner verifies and extracts one portable archive into a Scoop-shaped disposable
tree, puts the program files behind a read/execute-only ACL, and keeps application
data in a separate writable persist directory. A native package discovery probe and
small generated x264/SVT-AV1 media fixtures run with a cleared environment and a
system-only PATH. The original archive and all package payloads are hash-checked.

FreshImage is a procedural claim: run it only in a newly provisioned disposable
Windows image and identify that image with -ImageReference. RestrictedHost exercises
the same package boundary but records that it is not clean-machine evidence.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Archive,

    [Parameter(Mandatory = $true)]
    [string]$Probe,

    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,

    [ValidateSet("FreshImage", "RestrictedHost")]
    [string]$Scope = "FreshImage",

    [string]$ImageReference,

    [ValidatePattern("^[0-9a-fA-F]{64}$")]
    [string]$ExpectedArchiveSha256,

    [ValidatePattern("^[0-9a-fA-F]{64}$")]
    [string]$ExpectedProbeSha256
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        return Get-StreamSha256 $stream
    }
    finally {
        $stream.Dispose()
    }
}

function Get-StreamSha256 {
    param([Parameter(Mandatory = $true)][System.IO.Stream]$Stream)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $algorithm.ComputeHash($Stream)
        return ([System.BitConverter]::ToString($bytes)).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $algorithm.Dispose()
    }
}

function Resolve-ExistingFile {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Description)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if (-not $item.PSIsContainer -and (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0)) {
        return $item.FullName
    }
    throw "$Description must be an ordinary file: $Path"
}

function Get-SafeArchiveName {
    param([Parameter(Mandatory = $true)][string]$Name)
    if ([string]::IsNullOrWhiteSpace($Name) -or $Name.Contains("\") -or $Name.StartsWith("/") -or
        $Name.EndsWith("/") -or $Name.Contains(":") -or [System.IO.Path]::IsPathRooted($Name)) {
        throw "Unsafe or non-file archive entry: $Name"
    }
    $parts = $Name.Split("/")
    foreach ($part in $parts) {
        if ([string]::IsNullOrWhiteSpace($part) -or $part -eq "." -or $part -eq "..") {
            throw "Unsafe archive entry component: $Name"
        }
    }
    return ($parts -join "/")
}

function ConvertTo-ProcessArgument {
    param([AllowEmptyString()][string]$Value)
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') {
        return $Value
    }
    $escaped = [System.Text.RegularExpressions.Regex]::Replace($Value, '(\\*)"', '$1$1\"')
    $escaped = [System.Text.RegularExpressions.Regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

function ConvertTo-PowerShellLiteral {
    param([AllowEmptyString()][string]$Value)
    return "'" + $Value.Replace("'", "''") + "'"
}

function Get-CleanEnvironment {
    param([Parameter(Mandatory = $true)][string]$ProfileRoot)
    $systemRoot = [Environment]::GetEnvironmentVariable("SystemRoot")
    if ([string]::IsNullOrWhiteSpace($systemRoot)) {
        throw "SystemRoot is unavailable."
    }
    $system32 = Join-Path $systemRoot "System32"
    $temporary = Join-Path $ProfileRoot "temp"
    foreach ($directory in @($ProfileRoot, $temporary, (Join-Path $ProfileRoot "appdata-roaming"), (Join-Path $ProfileRoot "appdata-local"))) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }
    return [ordered]@{
        "APPDATA" = Join-Path $ProfileRoot "appdata-roaming"
        "COMSPEC" = Join-Path $system32 "cmd.exe"
        "HOME" = $ProfileRoot
        "LOCALAPPDATA" = Join-Path $ProfileRoot "appdata-local"
        "NUMBER_OF_PROCESSORS" = [Environment]::ProcessorCount.ToString([Globalization.CultureInfo]::InvariantCulture)
        "OS" = "Windows_NT"
        "PATH" = "$system32;$systemRoot"
        "PATHEXT" = ".COM;.EXE;.BAT;.CMD"
        "PROCESSOR_ARCHITECTURE" = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
        "SystemDrive" = [Environment]::GetEnvironmentVariable("SystemDrive")
        "SystemRoot" = $systemRoot
        "TEMP" = $temporary
        "TMP" = $temporary
        "USERPROFILE" = $ProfileRoot
        "WINDIR" = $systemRoot
        "XDG_CACHE_HOME" = Join-Path $ProfileRoot "cache"
        "XDG_CONFIG_HOME" = Join-Path $ProfileRoot "config"
        "XDG_DATA_HOME" = Join-Path $ProfileRoot "data"
    }
}

function Stop-QualificationProcessTree {
    param([Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process)
    $taskkill = Join-Path $env:SystemRoot "System32\taskkill.exe"
    $output = & $taskkill "/PID" ([string]$Process.Id) "/T" "/F" 2>&1 | Out-String
    $taskkillExitCode = $LASTEXITCODE
    if (-not $Process.WaitForExit(10000)) {
        try { $Process.Kill() } catch { }
        $Process.WaitForExit(5000) | Out-Null
    }
    return [ordered]@{ taskkillExitCode = $taskkillExitCode; output = $output.Trim() }
}

function New-QualificationJob {
    if (-not ("JessesQualification.ProcessJob" -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

namespace JessesQualification
{
    public sealed class ProcessJob : IDisposable
    {
        private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
        private const int JobObjectExtendedLimitInformation = 9;
        private IntPtr handle;

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_BASIC_LIMIT_INFORMATION
        {
            public long PerProcessUserTimeLimit;
            public long PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize;
            public UIntPtr MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass;
            public uint SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct IO_COUNTERS
        {
            public ulong ReadOperationCount;
            public ulong WriteOperationCount;
            public ulong OtherOperationCount;
            public ulong ReadTransferCount;
            public ulong WriteTransferCount;
            public ulong OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
        {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
            public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit;
            public UIntPtr JobMemoryLimit;
            public UIntPtr PeakProcessMemoryUsed;
            public UIntPtr PeakJobMemoryUsed;
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateJobObject(IntPtr securityAttributes, string name);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool SetInformationJobObject(IntPtr job, int informationClass, IntPtr information, uint length);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool TerminateJobObject(IntPtr job, uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr handle);

        public ProcessJob()
        {
            handle = CreateJobObject(IntPtr.Zero, null);
            if (handle == IntPtr.Zero || handle == new IntPtr(-1))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not create the qualification process job.");

            var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            int size = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
            IntPtr value = Marshal.AllocHGlobal(size);
            try
            {
                Marshal.StructureToPtr(limits, value, false);
                if (!SetInformationJobObject(handle, JobObjectExtendedLimitInformation, value, (uint)size))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not configure kill-on-close for the qualification process job.");
            }
            catch
            {
                CloseHandle(handle);
                handle = IntPtr.Zero;
                throw;
            }
            finally
            {
                Marshal.FreeHGlobal(value);
            }
        }

        public void Assign(IntPtr processHandle)
        {
            if (!AssignProcessToJobObject(handle, processHandle))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not assign the qualification process to its job.");
        }

        public void Terminate(uint exitCode)
        {
            if (handle != IntPtr.Zero && !TerminateJobObject(handle, exitCode))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not terminate the qualification process job.");
        }

        public void Dispose()
        {
            if (handle != IntPtr.Zero)
            {
                CloseHandle(handle);
                handle = IntPtr.Zero;
            }
            GC.SuppressFinalize(this);
        }

        ~ProcessJob()
        {
            Dispose();
        }
    }
}
'@
    }
    return New-Object JessesQualification.ProcessJob
}

function Invoke-CleanProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][System.Collections.IDictionary]$Environment,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [int]$TimeoutSeconds = 120
    )
    $wrapperId = [Guid]::NewGuid().ToString("N")
    $wrapperPath = Join-Path $WorkingDirectory (".qualification-launch-{0}.ps1" -f $wrapperId)
    $signalPath = Join-Path $WorkingDirectory (".qualification-launch-{0}.go" -f $wrapperId)
    $argumentLiterals = (($Arguments | ForEach-Object { ConvertTo-PowerShellLiteral ([string]$_) }) -join ", ")
    $wrapperLines = @(
        '$ErrorActionPreference = ''Stop''',
        ('$signal = {0}' -f (ConvertTo-PowerShellLiteral $signalPath)),
        '$deadline = [DateTime]::UtcNow.AddSeconds(30)',
        'while (-not [System.IO.File]::Exists($signal)) {',
        '    if ([DateTime]::UtcNow -ge $deadline) { throw ''Qualification launch gate timed out.'' }',
        '    [System.Threading.Thread]::Sleep(10)',
        '}',
        'try {',
        ('    & {0} @({1})' -f (ConvertTo-PowerShellLiteral $Executable), $argumentLiterals),
        '    exit $LASTEXITCODE',
        '}',
        'catch {',
        '    [Console]::Error.WriteLine($_.Exception.Message)',
        '    exit 9009',
        '}'
    )
    [System.IO.File]::WriteAllLines($wrapperPath, $wrapperLines, (New-Object System.Text.UTF8Encoding($false)))
    $launcher = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
    if (-not [System.IO.File]::Exists($launcher)) {
        throw "Windows PowerShell launcher is unavailable: $launcher"
    }
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $launcher
    $start.Arguments = ((@("-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", $wrapperPath) |
        ForEach-Object { ConvertTo-ProcessArgument ([string]$_) }) -join " ")
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables.Clear()
    foreach ($entry in $Environment.GetEnumerator()) {
        if ($null -ne $entry.Value -and -not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
            $start.EnvironmentVariables[[string]$entry.Key] = [string]$entry.Value
        }
    }
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    $started = $false
    $job = New-QualificationJob
    try {
        if (-not $process.Start()) {
            throw "Could not start $Executable"
        }
        $started = $true
        try {
            $job.Assign($process.Handle)
        }
        catch {
            Stop-QualificationProcessTree $process | Out-Null
            throw
        }
        [System.IO.File]::WriteAllText($signalPath, "go`n", [System.Text.Encoding]::ASCII)
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $job.Terminate(1460)
            $process.WaitForExit(10000) | Out-Null
            $stdout = if ($stdoutTask.Wait(5000)) { $stdoutTask.Result } else { "<stdout did not close after process-tree cleanup>" }
            $stderr = if ($stderrTask.Wait(5000)) { $stderrTask.Result } else { "<stderr did not close after process-tree cleanup>" }
            $stopwatch.Stop()
            $timeoutLog = "TIMEOUT after $TimeoutSeconds seconds`r`nCLEANUP Windows Job Object terminated`r`nSTDOUT`r`n$stdout`r`nSTDERR`r`n$stderr"
            [System.IO.File]::WriteAllText($LogPath, $timeoutLog, [System.Text.Encoding]::UTF8)
            throw "Process timed out after $TimeoutSeconds seconds and its process tree was terminated: $Executable (see $LogPath)"
        }
        $stdoutClosed = $stdoutTask.Wait(5000)
        $stderrClosed = $stderrTask.Wait(5000)
        if (-not $stdoutClosed -or -not $stderrClosed) {
            $job.Terminate(1460)
            $stdout = if ($stdoutTask.Wait(5000)) { $stdoutTask.Result } else { "<stdout remained open after job termination>" }
            $stderr = if ($stderrTask.Wait(5000)) { $stderrTask.Result } else { "<stderr remained open after job termination>" }
            $stopwatch.Stop()
            $pipeLog = "ROOT EXITED but an owned descendant kept redirected output open`r`nCLEANUP Windows Job Object terminated`r`nSTDOUT`r`n$stdout`r`nSTDERR`r`n$stderr"
            [System.IO.File]::WriteAllText($LogPath, $pipeLog, [System.Text.Encoding]::UTF8)
            throw "The root process exited while an owned descendant kept output open; its process job was terminated: $Executable (see $LogPath)"
        }
        $stdout = $stdoutTask.Result
        $stderr = $stderrTask.Result
        $stopwatch.Stop()
        [System.IO.File]::WriteAllText($LogPath, "STDOUT`r`n$stdout`r`nSTDERR`r`n$stderr", [System.Text.Encoding]::UTF8)
        $result = [ordered]@{
            executable = [System.IO.Path]::GetFileName($Executable)
            arguments = @($Arguments)
            exitCode = $process.ExitCode
            elapsedMilliseconds = $stopwatch.ElapsedMilliseconds
            log = [System.IO.Path]::GetFileName($LogPath)
            stdout = $stdout
            stderr = $stderr
        }
        if ($result.exitCode -ne 0) {
            throw "Process failed with exit code $($result.exitCode): $Executable (see $LogPath)"
        }
        return $result
    }
    finally {
        if ($null -ne $job) {
            $job.Dispose()
        }
        if ($started -and -not $process.HasExited) {
            $process.WaitForExit(5000) | Out-Null
        }
        if ($started -and -not $process.HasExited) {
            Stop-QualificationProcessTree $process | Out-Null
        }
        $stopwatch.Stop()
        $process.Dispose()
        foreach ($temporary in @($signalPath, $wrapperPath)) {
            try { [System.IO.File]::Delete($temporary) } catch { }
        }
    }
}

function Get-PeMachine {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    $reader = New-Object System.IO.BinaryReader($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Not a PE executable: $Path" }
        $stream.Position = 0x3C
        $offset = $reader.ReadUInt32()
        if ($offset -gt ($stream.Length - 6)) { throw "Invalid PE header offset: $Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw "Invalid PE signature: $Path" }
        $machine = $reader.ReadUInt16()
        if ($machine -ne 0x8664) { throw ("Expected an x64 PE executable, found 0x{0:x4}: {1}" -f $machine, $Path) }
        return ("0x{0:x4}" -f $machine)
    }
    finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Get-AmbientToolHits {
    $names = @("ffmpeg.exe", "ffprobe.exe", "x264.exe", "SvtAv1EncApp.exe", "av1an.exe")
    $hits = @()
    $seen = @{}
    foreach ($directory in ([Environment]::GetEnvironmentVariable("PATH") -split ";")) {
        if ([string]::IsNullOrWhiteSpace($directory) -or -not [System.IO.Path]::IsPathRooted($directory)) { continue }
        foreach ($name in $names) {
            $candidate = Join-Path $directory $name
            if ([System.IO.File]::Exists($candidate)) {
                $full = [System.IO.Path]::GetFullPath($candidate)
                $key = $full.ToLowerInvariant()
                if (-not $seen.ContainsKey($key)) {
                    $seen[$key] = $true
                    $hits += [ordered]@{ name = $name; path = $full }
                }
            }
        }
    }
    return @($hits)
}

function Get-WebView2Runtime {
    $candidates = @()
    $roots = @()
    if (${env:ProgramFiles(x86)}) { $roots += (Join-Path ${env:ProgramFiles(x86)} "Microsoft\EdgeWebView\Application") }
    if ($env:ProgramFiles) { $roots += (Join-Path $env:ProgramFiles "Microsoft\EdgeWebView\Application") }
    if ($env:LOCALAPPDATA) { $roots += (Join-Path $env:LOCALAPPDATA "Microsoft\EdgeWebView\Application") }
    foreach ($root in $roots) {
        if (-not [System.IO.Directory]::Exists($root)) { continue }
        foreach ($directory in [System.IO.Directory]::GetDirectories($root)) {
            $executable = Join-Path $directory "msedgewebview2.exe"
            if ([System.IO.File]::Exists($executable)) {
                $version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($executable).FileVersion
                $candidates += [ordered]@{ version = $version; executable = $executable; scope = $(if ($root.StartsWith($env:LOCALAPPDATA, [StringComparison]::OrdinalIgnoreCase)) { "user" } else { "machine" }) }
            }
        }
    }
    if ($candidates.Count -eq 0) {
        return [ordered]@{ available = $false; candidates = @() }
    }
    return [ordered]@{ available = $true; candidates = @($candidates) }
}

function Read-AndExtractPackage {
    param(
        [Parameter(Mandatory = $true)][string]$ArchivePath,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @{}
        foreach ($entry in $zip.Entries) {
            $name = Get-SafeArchiveName $entry.FullName
            $folded = $name.ToLowerInvariant()
            if ($entries.ContainsKey($folded)) { throw "Duplicate or case-colliding archive entry: $name" }
            $attributes = [System.BitConverter]::ToUInt32(
                [System.BitConverter]::GetBytes([int32]$entry.ExternalAttributes), 0
            )
            $unixMode = ($attributes -shr 16) -band 0xF000
            if ($unixMode -eq 0xA000) { throw "Archive links are forbidden: $name" }
            $entries[$folded] = $entry
        }
        if (-not $entries.ContainsKey("package-manifest.json")) { throw "The archive has no package-manifest.json." }
        $manifestEntry = $entries["package-manifest.json"]
        if ($manifestEntry.Length -gt 16777216) { throw "The package manifest is unreasonably large." }
        $reader = New-Object System.IO.StreamReader($manifestEntry.Open(), [System.Text.Encoding]::UTF8, $true)
        try { $manifestText = $reader.ReadToEnd() } finally { $reader.Dispose() }
        $manifest = $manifestText | ConvertFrom-Json
        if ($manifest.schemaVersion -ne 1 -or $manifest.product -ne "jesses" -or
            $manifest.target -ne "x86_64-pc-windows-msvc" -or $manifest.storage -ne "portable") {
            throw "The archive is not a supported Jesses Windows x64 portable package."
        }
        $expected = @{}
        foreach ($record in $manifest.files) {
            $name = Get-SafeArchiveName ([string]$record.path)
            $folded = $name.ToLowerInvariant()
            if ($folded -eq "package-manifest.json" -or $expected.ContainsKey($folded)) {
                throw "Duplicate or reserved package manifest entry: $name"
            }
            if ($folded -eq "jesses-data" -or $folded.StartsWith("jesses-data/")) {
                throw "User data must not be present in the portable archive."
            }
            if ([string]$record.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or [int64]$record.size -lt 0) {
                throw "Invalid package manifest size or hash: $name"
            }
            $expected[$folded] = $record
        }
        if ($entries.Count -ne ($expected.Count + 1)) { throw "Archive contents differ from package-manifest.json." }
        foreach ($required in @("jesses.exe", "jesses.portable", "resources/runtime-contract.json", "resources/tools/manifest.json")) {
            if (-not $expected.ContainsKey($required)) { throw "Required package entry is missing: $required" }
        }
        [System.IO.Directory]::CreateDirectory($Destination) | Out-Null
        foreach ($key in $entries.Keys) {
            $entry = $entries[$key]
            $name = Get-SafeArchiveName $entry.FullName
            $destinationPath = Join-Path $Destination ($name.Replace("/", [System.IO.Path]::DirectorySeparatorChar))
            $fullDestination = [System.IO.Path]::GetFullPath($destinationPath)
            $destinationPrefix = [System.IO.Path]::GetFullPath($Destination).TrimEnd("\") + "\"
            if (-not $fullDestination.StartsWith($destinationPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Archive extraction would escape its destination: $name"
            }
            $parent = [System.IO.Path]::GetDirectoryName($fullDestination)
            [System.IO.Directory]::CreateDirectory($parent) | Out-Null
            $source = $entry.Open()
            $target = New-Object System.IO.FileStream($fullDestination, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
            try { $source.CopyTo($target) } finally { $target.Dispose(); $source.Dispose() }
            if ($key -eq "package-manifest.json") {
                continue
            }
            $record = $expected[$key]
            $item = Get-Item -LiteralPath $fullDestination -Force
            if ($item.Length -ne [int64]$record.size -or (Get-Sha256 $fullDestination) -ne ([string]$record.sha256).ToLowerInvariant()) {
                throw "Extracted package payload differs from its manifest: $name"
            }
        }
        if ([System.IO.File]::ReadAllText((Join-Path $Destination "jesses.portable"), [System.Text.Encoding]::UTF8).Trim() -ne "1") {
            throw "The portable storage marker is invalid."
        }
        return [ordered]@{
            manifest = $manifest
            records = @($manifest.files)
            entryCount = $entries.Count
        }
    }
    finally {
        $zip.Dispose()
    }
}

function Assert-PackageUnchanged {
    param([Parameter(Mandatory = $true)][string]$PackageRoot, [Parameter(Mandatory = $true)][object[]]$Records)
    foreach ($record in $Records) {
        $name = Get-SafeArchiveName ([string]$record.path)
        $path = Join-Path $PackageRoot ($name.Replace("/", [System.IO.Path]::DirectorySeparatorChar))
        $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
        if ($item.PSIsContainer -or (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) -or
            $item.Length -ne [int64]$record.size -or (Get-Sha256 $path) -ne ([string]$record.sha256).ToLowerInvariant()) {
            throw "Program payload changed during qualification: $name"
        }
    }
}

function Set-ProgramReadOnly {
    param([Parameter(Mandatory = $true)][string]$PackageRoot, [Parameter(Mandatory = $true)][object[]]$Records, [Parameter(Mandatory = $true)][string]$AclLog)
    foreach ($record in $Records) {
        $path = Join-Path $PackageRoot (([string]$record.path).Replace("/", [System.IO.Path]::DirectorySeparatorChar))
        (Get-Item -LiteralPath $path -Force).IsReadOnly = $true
    }
    (Get-Item -LiteralPath (Join-Path $PackageRoot "package-manifest.json") -Force).IsReadOnly = $true
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $icacls = Join-Path $env:SystemRoot "System32\icacls.exe"
    $output = & $icacls $PackageRoot "/inheritance:r" "/grant:r" ("*{0}:(OI)(CI)(RX)" -f $identity) "/grant:r" "*S-1-5-18:(OI)(CI)(F)" 2>&1
    $exitCode = $LASTEXITCODE
    [System.IO.File]::WriteAllText($AclLog, ($output -join "`r`n"), [System.Text.Encoding]::UTF8)
    if ($exitCode -ne 0) { throw "icacls could not make the program tree read/execute-only (see $AclLog)." }
    return $identity
}

# Dot-sourcing is supported only so the focused test suite can exercise helper
# functions (especially timeout cleanup) without running a qualification.
if ($MyInvocation.InvocationName -eq ".") {
    return
}

if ($env:OS -ne "Windows_NT") {
    throw "This qualification runner requires Windows."
}
if ($Scope -eq "FreshImage" -and [string]::IsNullOrWhiteSpace($ImageReference)) {
    throw "FreshImage scope requires -ImageReference identifying the disposable base image or snapshot."
}
if ($Scope -eq "FreshImage" -and
    ([string]::IsNullOrWhiteSpace($ExpectedArchiveSha256) -or [string]::IsNullOrWhiteSpace($ExpectedProbeSha256))) {
    throw "FreshImage scope requires the externally recorded archive and probe SHA-256 values."
}

$archivePath = Resolve-ExistingFile $Archive "Archive"
$probePath = Resolve-ExistingFile $Probe "Native discovery probe"
$evidencePath = [System.IO.Path]::GetFullPath($EvidenceDirectory)
if ([System.IO.Directory]::Exists($evidencePath) -or [System.IO.File]::Exists($evidencePath)) {
    throw "EvidenceDirectory must not already exist: $evidencePath"
}
$archiveHashBefore = Get-Sha256 $archivePath
if ($ExpectedArchiveSha256 -and $archiveHashBefore -ne $ExpectedArchiveSha256.ToLowerInvariant()) {
    throw "Archive SHA-256 does not match -ExpectedArchiveSha256."
}
$probeHash = Get-Sha256 $probePath
if ($ExpectedProbeSha256 -and $probeHash -ne $ExpectedProbeSha256.ToLowerInvariant()) {
    throw "Native discovery probe SHA-256 does not match -ExpectedProbeSha256."
}
$ambientHits = @(Get-AmbientToolHits)
$inheritedJessesVariables = @((Get-ChildItem Env: | Where-Object { $_.Name.StartsWith("JESSES_", [StringComparison]::OrdinalIgnoreCase) } | Select-Object -ExpandProperty Name))
$ambientManagedRoot = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "jesses\tools" } else { $null }
$ambientManagedPresent = $ambientManagedRoot -and [System.IO.Directory]::Exists($ambientManagedRoot)

[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null
$logs = Join-Path $evidencePath "logs"
[System.IO.Directory]::CreateDirectory($logs) | Out-Null
$receiptPath = Join-Path $evidencePath "qualification.json"

$receipt = [ordered]@{
    schemaVersion = 1
    status = "running"
    scope = $Scope
    qualifiesFreshImage = $false
    imageReference = $ImageReference
    startedAt = [DateTime]::UtcNow.ToString("o")
    host = [ordered]@{
        windowsVersion = [Environment]::OSVersion.VersionString
        osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        processArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
        computerName = $env:COMPUTERNAME
        userName = $env:USERNAME
    }
    isolation = [ordered]@{
        childPath = "$env:SystemRoot\System32;$env:SystemRoot"
        environmentCleared = $true
        isolatedProfile = $true
        ambientToolHits = $ambientHits
        ambientManagedToolDirectoryPresent = [bool]$ambientManagedPresent
        inheritedJessesVariables = $inheritedJessesVariables
    }
    archive = [ordered]@{
        fileName = [System.IO.Path]::GetFileName($archivePath)
        bytes = (Get-Item -LiteralPath $archivePath).Length
        sha256Before = $archiveHashBefore
    }
    probe = [ordered]@{ fileName = [System.IO.Path]::GetFileName($probePath); sha256 = $probeHash }
    runner = [ordered]@{ fileName = [System.IO.Path]::GetFileName($PSCommandPath); sha256 = Get-Sha256 $PSCommandPath }
    checks = @()
}

try {
    if ($Scope -eq "FreshImage" -and ($ambientHits.Count -gt 0 -or $ambientManagedPresent -or $inheritedJessesVariables.Count -gt 0)) {
        throw "FreshImage scope requires no ambient media tools, managed Jesses tools, or JESSES_* overrides. Use RestrictedHost for a sanitized host run."
    }

    $scoopRoot = Join-Path $evidencePath "scoop"
    $packageRoot = Join-Path $scoopRoot "apps\jesses\clean-image"
    $persistRoot = Join-Path $scoopRoot "persist\jesses\jesses-data"
    foreach ($directory in @(
        (Join-Path $scoopRoot "apps\jesses"),
        (Join-Path $scoopRoot "persist\jesses"),
        $persistRoot,
        (Join-Path $persistRoot "config"),
        (Join-Path $persistRoot "data"),
        (Join-Path $persistRoot "data\jobs"),
        (Join-Path $persistRoot "cache"),
        (Join-Path $persistRoot "cache\webview"),
        (Join-Path $persistRoot "logs"),
        (Join-Path $persistRoot "logs\jobs"),
        (Join-Path $persistRoot "work")
    )) { [System.IO.Directory]::CreateDirectory($directory) | Out-Null }

    $package = Read-AndExtractPackage $archivePath $packageRoot
    $receipt.package = [ordered]@{
        product = $package.manifest.product
        version = $package.manifest.version
        target = $package.manifest.target
        profile = $package.manifest.profile
        storage = $package.manifest.storage
        archiveEntryCount = $package.entryCount
        manifestPayloadCount = $package.records.Count
    }
    $dataLink = Join-Path $packageRoot "jesses-data"
    New-Item -ItemType Junction -Path $dataLink -Target $persistRoot | Out-Null
    if (((Get-Item -LiteralPath $dataLink -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) {
        throw "The Scoop-shaped data link is not a directory junction."
    }

    foreach ($binary in @(
        (Join-Path $packageRoot "jesses.exe"),
        $probePath,
        (Join-Path $packageRoot "resources\tools\ffmpeg\ffmpeg.exe"),
        (Join-Path $packageRoot "resources\tools\ffmpeg\ffprobe.exe"),
        (Join-Path $packageRoot "resources\tools\x264\x264.exe"),
        (Join-Path $packageRoot "resources\tools\svt-av1\SvtAv1EncApp.exe"),
        (Join-Path $packageRoot "resources\tools\svt-av1-5fish\SvtAv1EncApp.exe"),
        (Join-Path $packageRoot "resources\tools\svt-av1-hdr\SvtAv1EncApp.exe"),
        (Join-Path $packageRoot "resources\tools\av1an\av1an.exe")
    )) { Get-PeMachine $binary | Out-Null }

    $identity = Set-ProgramReadOnly $packageRoot $package.records (Join-Path $logs "program-acl.log")
    $rootCreateDenied = $false
    try {
        [System.IO.File]::WriteAllText((Join-Path $packageRoot ".write-test"), "unexpected")
    }
    catch [System.UnauthorizedAccessException] { $rootCreateDenied = $true }
    if (-not $rootCreateDenied) { throw "The program directory remained writable after its ACL was restricted." }

    $nestedCreateDenied = $false
    $nestedWrite = Join-Path $packageRoot "resources\.write-test"
    try {
        [System.IO.Directory]::CreateDirectory($nestedWrite) | Out-Null
        [System.IO.Directory]::Delete($nestedWrite)
    }
    catch [System.UnauthorizedAccessException] { $nestedCreateDenied = $true }
    if (-not $nestedCreateDenied) { throw "A nested program resource directory remained writable after its ACL was restricted." }

    $existingFileWriteDenied = $false
    $existingStream = $null
    try {
        $existingStream = [System.IO.File]::Open(
            (Join-Path $packageRoot "resources\runtime-contract.json"),
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
    }
    catch [System.UnauthorizedAccessException] { $existingFileWriteDenied = $true }
    finally { if ($null -ne $existingStream) { $existingStream.Dispose() } }
    if (-not $existingFileWriteDenied) { throw "An existing program resource remained writable after its ACL was restricted." }

    foreach ($relative in @("config", "data", "cache", "logs", "cache\webview")) {
        $check = Join-Path $dataLink ($relative + "\.write-test")
        [System.IO.File]::WriteAllText($check, "jesses`n", [System.Text.Encoding]::UTF8)
        [System.IO.File]::Delete($check)
    }
    $receipt.permissions = [ordered]@{
        principalSid = $identity
        rootCreateDenied = $rootCreateDenied
        nestedResourceCreateDenied = $nestedCreateDenied
        existingResourceWriteDenied = $existingFileWriteDenied
        programReadExecuteAcl = $true
        scoopPersistJunction = $true
        writableDataDirectories = @("config", "data", "cache", "logs", "cache/webview")
    }

    $webView = Get-WebView2Runtime
    $receipt.webView2 = $webView
    if (-not $webView.available) { throw "Microsoft Edge WebView2 Runtime is unavailable." }

    $profileRoot = Join-Path $evidencePath "isolated-profile"
    $cleanEnvironment = Get-CleanEnvironment $profileRoot
    if ([System.IO.Directory]::Exists((Join-Path $profileRoot "appdata-local\jesses\tools"))) {
        throw "The isolated profile unexpectedly contains managed tools."
    }
    $work = Join-Path $persistRoot "work"
    $commands = @()
    $discovery = Invoke-CleanProcess $probePath @($packageRoot, "--require-media") $work $cleanEnvironment (Join-Path $logs "discovery.log") 120
    $commands += $discovery
    $parsedDiscovery = $discovery.stdout | ConvertFrom-Json
    $discovered = @()
    foreach ($entry in $parsedDiscovery) { $discovered += $entry }
    $requiredTools = @("ffmpeg", "ffprobe", "x264", "svt-av1", "av1an", "svt-av1-5fish", "svt-av1-hdr")
    $discoverySummary = @()
    $packagePrefix = [System.IO.Path]::GetFullPath($packageRoot).TrimEnd("\") + "\"
    foreach ($toolId in $requiredTools) {
        $tool = @($discovered | Where-Object { $_.id -eq $toolId })
        if ($tool.Count -ne 1 -or -not $tool[0].available -or [string]::IsNullOrWhiteSpace([string]$tool[0].path)) {
            throw "The native discovery probe did not report bundled capability: $toolId"
        }
        $selectedTool = $tool[0]
        [string]$reported = [string]$selectedTool.path
        if ($reported.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
            $reported = '\\' + $reported.Substring(8)
        }
        elseif ($reported.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase)) {
            $reported = $reported.Substring(4)
        }
        try {
            $reported = [System.IO.Path]::GetFullPath($reported)
        }
        catch {
            throw "The native discovery probe returned an invalid path for $toolId`: $reported"
        }
        if (-not $reported.StartsWith($packagePrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "The native discovery probe resolved $toolId outside the package: $reported"
        }
        $discoverySummary += [ordered]@{ id = $toolId; version = $selectedTool.version; packagedPath = $reported.Substring($packagePrefix.Length).Replace("\", "/") }
    }
    $receipt.discovery = [ordered]@{ required = $requiredTools; bundled = $discoverySummary }

    $ffmpeg = Join-Path $packageRoot "resources\tools\ffmpeg\ffmpeg.exe"
    $ffprobe = Join-Path $packageRoot "resources\tools\ffmpeg\ffprobe.exe"
    $x264 = Join-Path $packageRoot "resources\tools\x264\x264.exe"
    $svt = Join-Path $packageRoot "resources\tools\svt-av1\SvtAv1EncApp.exe"
    $source = Join-Path $work "generated.y4m"
    $h264 = Join-Path $work "x264.h264"
    $av1 = Join-Path $work "svt-av1.ivf"
    $h264Frames = Join-Path $work "x264.framemd5"
    $av1Frames = Join-Path $work "svt-av1.framemd5"

    $commands += Invoke-CleanProcess $ffmpeg @("-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=64x64:rate=24", "-frames:v", "8", "-pix_fmt", "yuv420p", $source) $work $cleanEnvironment (Join-Path $logs "fixture-generate.log") 120
    $commands += Invoke-CleanProcess $x264 @("--demuxer", "y4m", "--frames", "8", "--preset", "ultrafast", "--output", $h264, $source) $work $cleanEnvironment (Join-Path $logs "x264-encode.log") 120
    $commands += Invoke-CleanProcess $svt @("--input", $source, "--output", $av1, "--frames", "8", "--preset", "11", "--lp", "1") $work $cleanEnvironment (Join-Path $logs "svt-av1-encode.log") 120
    $h264Probe = Invoke-CleanProcess $ffprobe @("-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height,nb_read_frames", "-of", "json", $h264) $work $cleanEnvironment (Join-Path $logs "x264-probe.log") 120
    $commands += $h264Probe
    $av1Probe = Invoke-CleanProcess $ffprobe @("-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height,nb_read_frames", "-of", "json", $av1) $work $cleanEnvironment (Join-Path $logs "svt-av1-probe.log") 120
    $commands += $av1Probe
    $commands += Invoke-CleanProcess $ffmpeg @("-hide_banner", "-loglevel", "error", "-i", $h264, "-f", "framemd5", $h264Frames) $work $cleanEnvironment (Join-Path $logs "x264-decode.log") 120
    $commands += Invoke-CleanProcess $ffmpeg @("-hide_banner", "-loglevel", "error", "-i", $av1, "-f", "framemd5", $av1Frames) $work $cleanEnvironment (Join-Path $logs "svt-av1-decode.log") 120

    $h264Info = @((($h264Probe.stdout | ConvertFrom-Json).streams))[0]
    $av1Info = @((($av1Probe.stdout | ConvertFrom-Json).streams))[0]
    if ($h264Info.codec_name -ne "h264" -or [int]$h264Info.width -ne 64 -or [int]$h264Info.height -ne 64 -or [int]$h264Info.nb_read_frames -ne 8) {
        throw "The x264 generated fixture did not decode as eight 64x64 H.264 frames."
    }
    if ($av1Info.codec_name -ne "av1" -or [int]$av1Info.width -ne 64 -or [int]$av1Info.height -ne 64 -or [int]$av1Info.nb_read_frames -ne 8) {
        throw "The SVT-AV1 generated fixture did not decode as eight 64x64 AV1 frames."
    }
    $h264FrameCount = @([System.IO.File]::ReadAllLines($h264Frames) | Where-Object { -not $_.StartsWith("#") -and -not [string]::IsNullOrWhiteSpace($_) }).Count
    $av1FrameCount = @([System.IO.File]::ReadAllLines($av1Frames) | Where-Object { -not $_.StartsWith("#") -and -not [string]::IsNullOrWhiteSpace($_) }).Count
    if ($h264FrameCount -ne 8 -or $av1FrameCount -ne 8) { throw "Decoded frame checksums did not contain eight frames per output." }
    $receipt.media = [ordered]@{
        source = [ordered]@{ generated = $true; width = 64; height = 64; frames = 8; sha256 = Get-Sha256 $source }
        x264 = [ordered]@{ codec = "h264"; framesDecoded = $h264FrameCount; outputSha256 = Get-Sha256 $h264; decodedFramesSha256 = Get-Sha256 $h264Frames }
        svtAv1 = [ordered]@{ codec = "av1"; framesDecoded = $av1FrameCount; outputSha256 = Get-Sha256 $av1; decodedFramesSha256 = Get-Sha256 $av1Frames }
    }
    $receipt.commands = @($commands | ForEach-Object {
        [ordered]@{ executable = $_.executable; arguments = $_.arguments; exitCode = $_.exitCode; elapsedMilliseconds = $_.elapsedMilliseconds; log = $_.log }
    })

    Assert-PackageUnchanged $packageRoot $package.records
    $archiveHashAfter = Get-Sha256 $archivePath
    if ($archiveHashAfter -ne $archiveHashBefore) { throw "The source archive changed during qualification." }
    $receipt.archive.sha256After = $archiveHashAfter
    $receipt.checks = @(
        "archive-and-manifest-verified",
        "x64-pe-inputs-verified",
        "program-tree-read-execute-only",
        "scoop-persist-data-writable",
        "webview2-runtime-present",
        "bundled-discovery-with-cleared-environment",
        "generated-x264-encode-decode",
        "generated-svt-av1-encode-decode",
        "package-payloads-unchanged",
        "source-archive-unchanged"
    )
    $receipt.qualifiesFreshImage = ($Scope -eq "FreshImage")
    $receipt.status = "passed"
    $receipt.boundary = if ($Scope -eq "FreshImage") {
        "Fresh-image receipt for the supplied image reference. Native GUI launch and interaction are outside this runner."
    } else {
        "Restricted-host evidence only. The child environment excludes inherited tools and profiles, but this is not a fresh Windows installation. Native GUI launch and interaction are outside this runner."
    }
}
catch {
    $receipt.status = "failed"
    $receipt.qualifiesFreshImage = $false
    $receipt.error = $_.Exception.Message
    throw
}
finally {
    $receipt.completedAt = [DateTime]::UtcNow.ToString("o")
    try {
        $receipt | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $receiptPath -Encoding UTF8
    }
    catch {
        Write-Warning "Could not write qualification receipt: $($_.Exception.Message)"
    }
}

Write-Host "Windows package qualification passed ($Scope)."
Write-Host "Receipt: $receiptPath"
