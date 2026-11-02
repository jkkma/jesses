param([string]$OutputRoot = 'target/windows-console-qualification')
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'Use PowerShell 7: pwsh -NoProfile -File scripts/qualify-windows-console.ps1' }
if (-not $IsWindows) { throw 'Run this qualification natively on Windows. Cross-compilation does not qualify console behavior.' }
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
  $destinationRoot = [System.IO.Path]::GetFullPath($OutputRoot)
  [System.IO.Directory]::CreateDirectory($destinationRoot) | Out-Null
  $runName = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '-' + [Guid]::NewGuid().ToString('N')
  $destination = Join-Path $destinationRoot $runName
  [System.IO.Directory]::CreateDirectory($destination) | Out-Null
  $commandLog = Join-Path $destination 'native-console-run.log'
  & cargo run -p media-runtime --locked --target x86_64-pc-windows-msvc --features console-fixture --example windows_console_fixture -- $destination 2>&1 | Tee-Object -FilePath $commandLog
  if ($LASTEXITCODE -ne 0) { throw "Native console fixture failed with exit $LASTEXITCODE. See $commandLog" }
  $receipt = Get-Content -LiteralPath (Join-Path $destination 'console-qualification.json') -Raw | ConvertFrom-Json
  if ($receipt.Count -ne 12 -or @($receipt | Where-Object { $_.passed -ne $true }).Count -ne 0) { throw 'The complete twelve-case native receipt was not produced.' }
  Write-Output "Native Windows console qualification passed: $destination"
  if ($env:GITHUB_OUTPUT) { "receipt-directory=$destination" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8 }
} finally { Pop-Location }
