param(
 [ValidateSet('Inject','Watch')][string]$Action='Inject',
 [Parameter(Mandatory=$true)][string]$Directory,
 [int]$TargetProcessId,
 [string]$PeerAddress,
 [ValidateRange(100,3000)][int]$Milliseconds=500
)
$ErrorActionPreference='Stop'
$directoryPath=[IO.Path]::GetFullPath($Directory)
if(!$directoryPath.Contains('\smoke\') -or $directoryPath.StartsWith('\\')){throw 'Use an absolute local directory under smoke'}
$ownerFile=Join-Path $directoryPath 'owner.json'

function Remove-OwnedRules($owner){
 foreach($rule in $owner.rules){
  $existing=Get-NetFirewallRule -Name $rule -ErrorAction SilentlyContinue
  if($existing){
   if($existing.Group -ne $owner.id){throw 'Firewall ownership mismatch'}
   $existing|Remove-NetFirewallRule
  }
 }
}

if($Action -eq 'Watch'){
 $owner=Get-Content -LiteralPath $ownerFile -Raw -Encoding UTF8|ConvertFrom-Json
 if($owner.directory -ne $directoryPath -or $owner.id -notmatch '^PAB-Smoke-RD-Fault-[0-9a-f-]{36}$' -or
    @($owner.rules).Count -ne 2 -or $owner.rules[0] -ne ($owner.id+'-In') -or $owner.rules[1] -ne ($owner.id+'-Out')){throw 'Invalid owner state'}
 try {
  $deadline=[datetime]::Parse($owner.deadline).ToUniversalTime()
  while([datetime]::UtcNow -lt $deadline -and !(Test-Path -LiteralPath (Join-Path $directoryPath 'finished.json'))){Start-Sleep -Milliseconds 100}
  Remove-OwnedRules $owner
  @{watchdog_finished=$true;utc=[datetime]::UtcNow.ToString('o')}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $directoryPath 'watchdog.json') -Encoding UTF8
 } finally {Unregister-ScheduledTask -TaskName $owner.id -Confirm:$false -ErrorAction SilentlyContinue}
 return
}

if(!(Split-Path -IsAbsolute $Directory) -or (Test-Path -LiteralPath $directoryPath)){throw 'Use a fresh absolute directory'}
$peer=$null
if(![Net.IPAddress]::TryParse($PeerAddress,[ref]$peer) -or $peer.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork){throw 'Peer must be a private IPv4 address'}
$bytes=$peer.GetAddressBytes()
if(!($bytes[0] -eq 10 -or ($bytes[0] -eq 172 -and $bytes[1] -ge 16 -and $bytes[1] -le 31) -or ($bytes[0] -eq 192 -and $bytes[1] -eq 168))){throw 'Peer must be a private IPv4 address'}
$process=Get-CimInstance Win32_Process -Filter ('ProcessId='+$TargetProcessId)
if(!$process -or !$process.ExecutablePath){throw 'Target process unavailable'}
$image=[IO.Path]::GetFullPath($process.ExecutablePath)
if(!$image.Contains('\smoke\') -or [IO.Path]::GetFileName($image) -notin @('remote_desktop_host.exe','remote_desktop_stability.exe')){throw 'Only isolated smoke desktop processes are allowed'}
$ports=@(Get-NetUDPEndpoint -OwningProcess $TargetProcessId -ErrorAction Stop|Select-Object -ExpandProperty LocalPort -Unique)
if(!$ports.Count -or $ports.Count -gt 16 -or @($ports|Where-Object {$_ -lt 1024}).Count){throw 'Unexpected UDP ports'}
New-Item -ItemType Directory -Path $directoryPath -ErrorAction Stop|Out-Null
$id='PAB-Smoke-RD-Fault-'+[guid]::NewGuid().ToString()
$owner=@{id=$id;directory=$directoryPath;rules=@(($id+'-In'),($id+'-Out'));deadline=[datetime]::UtcNow.AddSeconds(15).ToString('o');process_id=$TargetProcessId;image=$image;image_sha256=(Get-FileHash -LiteralPath $image -Algorithm SHA256).Hash;ports=$ports;peer=$PeerAddress;milliseconds=$Milliseconds}
$owner|ConvertTo-Json -Depth 4|Set-Content -LiteralPath $ownerFile -Encoding UTF8
$watcher=Join-Path $directoryPath 'watch.ps1'
Copy-Item -LiteralPath $PSCommandPath -Destination $watcher
$actionTask=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'+$watcher+'" -Action Watch -Directory "'+$directoryPath+'"')
$principal=New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 2) -Priority 6
Register-ScheduledTask -TaskName $id -Action $actionTask -Principal $principal -Settings $settings|Out-Null
Start-ScheduledTask -TaskName $id
$timer=[Diagnostics.Stopwatch]::StartNew()
$result=@{applied=$false;rules_removed=$false;requested_ms=$Milliseconds}
try {
 # The original process must still exist before adding any rule. Rules also
 # bind to its exact binary path, private peer address and owned UDP ports.
 $current=Get-CimInstance Win32_Process -Filter ('ProcessId='+$TargetProcessId)
 if(!$current -or $current.CreationDate -ne $process.CreationDate -or $current.ExecutablePath -ne $image){throw 'Target process changed'}
 foreach($direction in @('Inbound','Outbound')){
  $suffix=if($direction -eq 'Inbound'){'-In'}else{'-Out'}
  New-NetFirewallRule -Name ($id+$suffix) -DisplayName ($id+$suffix) -Group $id -Direction $direction -Action Block -Program $image -RemoteAddress $PeerAddress -Protocol UDP -LocalPort $ports -Profile Any|Out-Null
 }
 $result.applied=$true
 $result.applied_utc=[datetime]::UtcNow.ToString('o')
 $hold=[Diagnostics.Stopwatch]::StartNew()
 Start-Sleep -Milliseconds $Milliseconds
 $result.hold_ms=$hold.Elapsed.TotalMilliseconds
} finally {
 Remove-OwnedRules $owner
 $result.removed_utc=[datetime]::UtcNow.ToString('o')
 $result.operation_ms=$timer.Elapsed.TotalMilliseconds
 $result.rules_removed=@(foreach($name in $owner.rules){Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue}).Count -eq 0
 $result|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $directoryPath 'finished.json') -Encoding UTF8
}
$result|ConvertTo-Json -Compress
if(!$result.applied -or !$result.rules_removed){throw 'Fault injection or cleanup incomplete'}
