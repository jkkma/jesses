param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'generated'),
    [string]$Ffmpeg = 'ffmpeg',
    [string]$Ffprobe = 'ffprobe'
)

$ErrorActionPreference = 'Stop'
$fixtureRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $fixtureRoot) {
    throw "Choose an output directory that does not exist: $fixtureRoot"
}
[System.IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null

function Invoke-FixtureVideo {
    param([string]$Name, [string]$Rate, [string]$Duration)
    $fixturePath = Join-Path $fixtureRoot $Name
    $arguments = @(
        '-hide_banner', '-loglevel', 'error', '-nostdin', '-n',
        '-f', 'lavfi', '-i', "testsrc2=size=320x180:rate=${Rate}:duration=${Duration}",
        '-f', 'lavfi', '-i', "sine=frequency=440:sample_rate=48000:duration=${Duration}",
        '-map', '0:v:0', '-map', '1:a:0',
        '-vf', 'setfield=prog', '-c:v', 'ffv1', '-level', '3', '-g', '1',
        '-threads:v', '1', '-pix_fmt', 'yuv420p', '-field_order', 'progressive',
        '-color_range', 'tv', '-colorspace', 'bt709', '-color_primaries', 'bt709',
        '-color_trc', 'bt709', '-c:a', 'pcm_s16le', '-ac', '2',
        '-metadata', 'title=Synthetic test pattern',
        '-metadata:s:a:0', 'language=eng', $fixturePath
    )
    & $Ffmpeg @arguments
    if ($LASTEXITCODE -ne 0) { throw "FFmpeg failed for $Name with exit code $LASTEXITCODE" }
}

Invoke-FixtureVideo -Name 'progressive.mkv' -Rate '24' -Duration '1'
Invoke-FixtureVideo -Name 'fractional.mkv' -Rate '24000/1001' -Duration '1.001'

# Construct Unicode explicitly so Windows PowerShell 5.1 can also run this ASCII script.
$unicodeName = "caf$([char]0x00e9) $([char]0x6771)$([char]0x4eac)'s clip.mkv"
[System.IO.File]::Copy((Join-Path $fixtureRoot 'fractional.mkv'), (Join-Path $fixtureRoot $unicodeName))
[System.IO.File]::WriteAllText((Join-Path $fixtureRoot 'malformed.mkv'), 'This is not a media container.', [System.Text.Encoding]::ASCII)

$toolReport = @(& $Ffmpeg '-version' | Select-Object -First 1)
$toolReport += @(& $Ffprobe '-version' | Select-Object -First 1)
[System.IO.File]::WriteAllLines((Join-Path $fixtureRoot 'tools.txt'), [string[]]$toolReport)

foreach ($name in @('progressive.mkv', 'fractional.mkv', $unicodeName, 'malformed.mkv')) {
    $fixturePath = Join-Path $fixtureRoot $name
    $errorPath = Join-Path $fixtureRoot "$name.stderr.txt"
    $probeArguments = @('-v', 'error', '-show_error', '-show_format', '-show_streams', '-count_frames', '-of', 'json', $fixturePath)
    # Windows PowerShell turns native stderr into error records, including expected probe failures.
    $previousErrorPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $probeJson = & $Ffprobe @probeArguments 2> $errorPath
        $probeExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorPreference
    }
    [System.IO.File]::WriteAllText((Join-Path $fixtureRoot "$name.ffprobe.json"), ($probeJson -join "`n"), [System.Text.UTF8Encoding]::new($false))
    if ($name -eq 'malformed.mkv') {
        if ($probeExitCode -eq 0) { throw 'Malformed fixture unexpectedly probed successfully.' }
    } elseif ($probeExitCode -ne 0) {
        throw "FFprobe failed for $name with exit code $probeExitCode"
    }
}

Write-Output "Created fixtures and probe reports in $fixtureRoot"
