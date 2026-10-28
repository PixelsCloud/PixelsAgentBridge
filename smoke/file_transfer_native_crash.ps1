<# Force-stop ONLY a hash-verified isolated candidate desktop after a recorded
partial transfer. No installed service, other app, or payload is stopped/deleted.
Pair with file_transfer_native_restart.mjs and retain original request evidence.
#>
param(
  [Parameter(Mandatory=$true)][string]$Root,
  [Parameter(Mandatory=$true)][string]$RunId
)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
if($env:OS -ne 'Windows_NT' -or $Root -notmatch '^[A-Za-z]:\\' -or $Root -match "['`r`n*?\[\]]") { throw 'Ordinary absolute Windows fixture required' }
$Root=[IO.Path]::GetFullPath($Root).TrimEnd('\')
if($RunId -notmatch '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$') { throw 'Explicit case UUID required' }
$ancestor=$Root
while($ancestor) {
  if((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse point in fixture path' }
  $ancestor=[IO.Path]::GetDirectoryName($ancestor)
}
$marker=Get-Content -LiteralPath (Join-Path $Root 'native-lab.json') -Raw|ConvertFrom-Json
$record=Get-Content -LiteralPath (Join-Path $Root 'desktop-process.json') -Raw|ConvertFrom-Json
$base=Join-Path $Root ('payload\native-restart-'+$RunId)
$start=Get-Content -LiteralPath (Join-Path $base 'start-download.json') -Raw|ConvertFrom-Json
$resultPath=Join-Path $base 'forced-stop.json'
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
if([string]$marker.root -ne $Root -or [string]$marker.machine -ne $env:COMPUTERNAME -or $identity.User.Value -ne [string]$marker.user_sid) { throw 'Exact fixture native user required' }
if($start.run_id -ne $RunId -or $start.status -ne 'awaiting_desktop_crash' -or !$start.active_item.attempt -or [long]$start.active_item.confirmed -le 0 -or [long]$start.active_item.confirmed -ge [long]$start.active_item.spec.size) { throw 'Original partially confirmed transfer required' }
if(Test-Path -LiteralPath $resultPath) { throw 'Original crash outcome already exists; do not repeat it' }
$expected=Join-Path $Root 'pab-desktop.exe'
if([string]$record.exe -ne $expected -or [string]$record.config -ne (Join-Path $Root 'config.toml') -or [int]$start.desktop.pid -ne [int]$record.pid) { throw 'Original desktop record changed' }
if((Get-FileHash -LiteralPath $expected -Algorithm SHA256).Hash.ToLowerInvariant() -ne [string]$marker.desktop_sha256) { throw 'Desktop candidate hash changed' }
$process=Get-Process -Id ([int]$record.pid) -ErrorAction Stop
if([string]$process.Path -ne $expected -or $process.StartTime.ToUniversalTime().Ticks -ne [long]$record.started_ticks) { throw 'Candidate PID reused or process changed' }
$process|Stop-Process -Force
if(!$process.WaitForExit(10000)) { throw 'Original desktop has not stopped' }
$result=[ordered]@{run_id=$RunId;root=$Root;phase='forced_desktop_stop';pass=$true;pid=[int]$record.pid;exe=$expected;batch_id=[string]$start.original_batch_id;item_id=[string]$start.active_item.spec.id;attempt=[string]$start.active_item.attempt;confirmed=[string]$start.active_item.confirmed;at=[DateTime]::UtcNow.ToString('o')}
$bytes=(New-Object Text.UTF8Encoding($false)).GetBytes(($result|ConvertTo-Json -Compress)+"`n")
$stream=[IO.File]::Open($resultPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
try { $stream.Write($bytes,0,$bytes.Length);$stream.Flush($true) } finally { $stream.Dispose() }
$result|ConvertTo-Json -Compress
