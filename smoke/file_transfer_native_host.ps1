<#
Isolated Windows candidate lifecycle for file_transfer_native_cross.mjs.
Prepare runs as SYSTEM with explicitly sampled UserSid and two existing,
SHA-256-verified candidate EXEs. Root must be a NEW ft-native-<RunId> directory.
Transfer candidate files between machines using Pixels before calling Prepare.
LaunchExecutor runs as SYSTEM; LaunchDesktop as exactly UserSid. Neither
registers a Windows service or updates an installed application. Invoke launch
commands with a task timeout longer than LifetimeSeconds. They remain live and
bounded until that process exits, Stop is called, or the lifetime expires.
Stop requires the caller to have reconciled original batches and restored any
fixture ACL boundaries. It never recursively deletes profiles or payloads.
All configuration is TOML; JSON files below are private runtime observations.
#>
param(
  [Parameter(Mandatory=$true)][ValidateSet('Prepare','LaunchExecutor','LaunchDesktop','Inspect','Stop')][string]$Phase,
  [Parameter(Mandatory=$true)][string]$Root,
  [string]$RunId,
  [string]$UserSid,
  [string]$ExecutorSource,
  [string]$DesktopSource,
  [string]$ExecutorSha256,
  [string]$DesktopSha256,
  [ValidateRange(1024,65535)][int]$IpcPort=39256,
  [ValidateRange(1024,65535)][int]$CdpPort=39255,
  [ValidateRange(30,86400)][int]$LifetimeSeconds=3600,
  [string]$ControlUrl='',
  [string[]]$RelayUrls=@(),
  [switch]$BatchesReconciled
)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$utf8=New-Object Text.UTF8Encoding($false)
function WriteJson([string]$Path,$Value) {
  [IO.File]::WriteAllText($Path,($Value|ConvertTo-Json -Depth 8 -Compress)+"`n",$utf8)
}
function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function NativeIdentity {
  $native=[Security.Principal.WindowsIdentity]::GetCurrent()
  [ordered]@{name=[string]$native.Name;sid=[string]$native.User.Value;owner=[string]$native.Owner.Value;session=[int](Get-Process -Id $PID).SessionId}
}
function CheckOrdinaryPath([string]$Path) {
  $part=[IO.Path]::GetFullPath($Path)
  while($part) {
    if(Test-Path -LiteralPath $part) {
      $item=Get-Item -LiteralPath $part -Force
      if($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Candidate path must not traverse reparse points' }
    }
    $parent=[IO.Path]::GetDirectoryName($part)
    if($parent -eq $part) { break };$part=$parent
  }
}
function InstalledService {
  $service=Get-CimInstance Win32_Service -Filter "Name='PixelsAgentBridgeExecutor'"
  [ordered]@{state=if($service){[string]$service.State}else{'Absent'};pid=if($service){[int]$service.ProcessId}else{0}}
}
function StopExact($Record) {
  $process=Get-Process -Id ([int]$Record.pid) -ErrorAction SilentlyContinue
  if(!$process) { return }
  if([string]$process.Path -ne [string]$Record.exe -or $process.StartTime.ToUniversalTime().Ticks -ne [long]$Record.started_ticks) { throw 'Recorded candidate PID was reused or path changed; refusing to stop it' }
  $process|Stop-Process -Force
}
if($env:OS -ne 'Windows_NT') { throw 'Windows adapter required' }
if($Root -notmatch '^[A-Za-z]:\\' -or $Root.Substring(2).Contains(':') -or $Root -match "['`r`n*?\[\]]") { throw 'An ordinary absolute local candidate directory is required' }
$Root=[IO.Path]::GetFullPath($Root).TrimEnd('\')
CheckOrdinaryPath $Root
$markerPath=Join-Path $Root 'native-lab.json'
$config=Join-Path $Root 'config.toml'
$identity=NativeIdentity
if($Phase -eq 'Prepare') {
  $parsedId=[Guid]::Empty
  if(![Guid]::TryParseExact($RunId,'D',[ref]$parsedId) -or $RunId -ne $parsedId.ToString() -or (Split-Path $Root -Leaf) -ne ('ft-native-'+$RunId)) { throw 'Root leaf must equal ft-native-<explicit canonical RunId UUID>' }
  if($identity.sid -ne 'S-1-5-18') { throw 'Prepare requires an explicitly selected SYSTEM process' }
  $sidObject=New-Object Security.Principal.SecurityIdentifier($UserSid)
  if($UserSid -ne $sidObject.Value -or $UserSid -eq 'S-1-5-18' -or $IpcPort -eq $CdpPort) { throw 'An explicit native user SID and distinct ports are required' }
  foreach($pair in @(@($ExecutorSource,$ExecutorSha256),@($DesktopSource,$DesktopSha256))) {
    if(!$pair[0] -or $pair[1] -notmatch '^[0-9a-f]{64}$' -or !(Test-Path -LiteralPath $pair[0] -PathType Leaf)) { throw 'Two explicit candidate paths and lowercase SHA-256 values required' }
    CheckOrdinaryPath $pair[0]
    if((Hash $pair[0]) -ne $pair[1]) { throw 'Candidate source hash mismatch' }
  }
  foreach($url in @($ControlUrl)+$RelayUrls) {
    if(!$url) { continue }
    $uri=$null
    if($url -match "['`r`n]" -or ![Uri]::TryCreate($url,[UriKind]::Absolute,[ref]$uri) -or $uri.Scheme -notin @('https','wss') -or $uri.UserInfo) { throw 'Use explicit HTTPS/WSS URLs without credentials' }
  }
  if(Test-Path -LiteralPath $Root) { throw 'Prepare never overwrites or adopts an existing directory' }
  if(!(Test-Path -LiteralPath (Split-Path $Root -Parent) -PathType Container)) { throw 'Explicit candidate parent directory must already exist' }
  [void](New-Item -ItemType Directory -Path $Root -ErrorAction Stop)
  & icacls.exe $Root /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' (('*'+$UserSid)+':(OI)(CI)F') | Out-Null
  if($LASTEXITCODE -ne 0) { throw 'Private candidate ACL setup failed' }
  foreach($name in @('executor-data','operator-data','webview','payload')) { [void][IO.Directory]::CreateDirectory((Join-Path $Root $name)) }
  Copy-Item -LiteralPath $ExecutorSource -Destination (Join-Path $Root 'pab-executor.exe') -ErrorAction Stop
  Copy-Item -LiteralPath $DesktopSource -Destination (Join-Path $Root 'pab-desktop.exe') -ErrorAction Stop
  if((Hash (Join-Path $Root 'pab-executor.exe')) -ne $ExecutorSha256 -or (Hash (Join-Path $Root 'pab-desktop.exe')) -ne $DesktopSha256) { throw 'Copied candidate bytes differ' }
  $relayToml=(@($RelayUrls|ForEach-Object { "'"+$_+"'" }) -join ', ')
  $toml="control_url = '$ControlUrl'`nrelay_urls = [$relayToml]`n[executor]`ndata_dir = '$Root\executor-data'`ndevice_name = 'FT isolated $RunId'`n[operator]`ndata_dir = '$Root\operator-data'`n"
  [IO.File]::WriteAllText($config,$toml,$utf8)
  [IO.File]::WriteAllText((Join-Path $Root 'operator-data\agent-integrations.toml'),"auto_connect = false`n",$utf8)
  $marker=[ordered]@{schema=1;run_id=$RunId;root=$Root;machine=[string]$env:COMPUTERNAME;user_sid=$UserSid;ipc_port=$IpcPort;cdp_port=$CdpPort;config_sha256=(Hash $config);executor_sha256=$ExecutorSha256;desktop_sha256=$DesktopSha256;installed_before=(InstalledService);prepared=[DateTime]::UtcNow.ToString('o')}
  WriteJson $markerPath $marker
  [ordered]@{phase='prepared';root=$Root;machine=$marker.machine;run_id=$RunId}|ConvertTo-Json -Compress
  exit 0
}
if(!(Test-Path -LiteralPath $markerPath -PathType Leaf)) { throw 'Prepare this isolated candidate first' }
$marker=Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
if($marker.schema -ne 1 -or [string]$marker.root -ne $Root -or [string]$marker.machine -ne [string]$env:COMPUTERNAME -or (Hash $config) -ne [string]$marker.config_sha256) { throw 'Candidate root, host or configuration changed' }
$desktopExe=Join-Path $Root 'pab-desktop.exe';$executorExe=Join-Path $Root 'pab-executor.exe'
if((Hash $desktopExe) -ne [string]$marker.desktop_sha256 -or (Hash $executorExe) -ne [string]$marker.executor_sha256) { throw 'Prepared candidate bytes changed' }
if($Phase -eq 'LaunchExecutor' -or $Phase -eq 'LaunchDesktop') {
  $isDesktop=$Phase -eq 'LaunchDesktop'
  if($isDesktop -and $identity.sid -ne [string]$marker.user_sid) { throw 'Desktop must run as the explicitly selected native user' }
  if(!$isDesktop -and $identity.sid -ne 'S-1-5-18') { throw 'Executor must run as explicitly selected SYSTEM' }
  $exe=if($isDesktop){$desktopExe}else{$executorExe}
  $port=if($isDesktop){[int]$marker.cdp_port}else{[int]$marker.ipc_port}
  if(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue) { throw 'Candidate test port is already occupied' }
  $recordPath=Join-Path $Root $(if($isDesktop){'desktop-process.json'}else{'service-process.json'})
  if(Test-Path -LiteralPath $recordPath) {
    $old=Get-Content -LiteralPath $recordPath -Raw|ConvertFrom-Json
    if(Get-Process -Id ([int]$old.pid) -ErrorAction SilentlyContinue) { throw 'An original recorded process is still alive; inspect it first' }
    Copy-Item -LiteralPath $recordPath -Destination ($recordPath+'.'+[Guid]::NewGuid().ToString()+'.history') -ErrorAction Stop
  }
  Get-ChildItem Env:|Where-Object {$_.Name -like 'PAB_*'}|ForEach-Object {[Environment]::SetEnvironmentVariable($_.Name,$null,'Process')}
  $env:PAB_LOCAL_IPC_PORT=[string]$marker.ipc_port
  $launchArgs=@('--config',('"'+$config+'"'))
  $logTag=[Guid]::NewGuid().ToString()
  if($isDesktop) {
    $env:PAB_DESKTOP_TERMINAL_ACCEPTANCE='1'
    $env:WEBVIEW2_USER_DATA_FOLDER=Join-Path $Root 'webview'
    $launchArgs+=('--acceptance-cdp-port='+[string]$marker.cdp_port)
  } else {
    $access=Join-Path $Root 'operator-data\local-access.key'
    if(!(Test-Path -LiteralPath $access)) {
      & $exe --config $config issue-local-access $access 1>$null 2>(Join-Path $Root ('issue-local-'+$logTag+'.stderr.log'))
      if($LASTEXITCODE -ne 0) { throw 'Candidate local access setup failed; inspect its private error log' }
      & icacls.exe $access /inheritance:r /grant:r '*S-1-5-18:F' (('*'+[string]$marker.user_sid)+':R')|Out-Null
      if($LASTEXITCODE -ne 0) { throw 'Candidate local token ACL setup failed' }
    }
  }
  $process=Start-Process -FilePath $exe -ArgumentList $launchArgs -WorkingDirectory $Root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $Root ($Phase+'-'+$logTag+'.stdout.log')) -RedirectStandardError (Join-Path $Root ($Phase+'-'+$logTag+'.stderr.log')) -PassThru
  $record=[ordered]@{pid=[int]$process.Id;exe=$exe;config=$config;cdp=[int]$marker.cdp_port;local_ipc=[int]$marker.ipc_port;identity=$identity;started=[DateTime]::UtcNow.ToString('o');started_ticks=$process.StartTime.ToUniversalTime().Ticks;lifetime_seconds=$LifetimeSeconds}
  WriteJson $recordPath $record
  $record|ConvertTo-Json -Depth 4 -Compress
  try {
    $exited=$process.WaitForExit($LifetimeSeconds*1000)
    if(!$exited) { StopExact $record }
    [ordered]@{phase='exited';lifetime_expired=(!$exited)}|ConvertTo-Json -Compress
  } finally { if(!$process.HasExited) { StopExact $record } }
  exit 0
}
if($Phase -eq 'Stop') {
  if(!$BatchesReconciled) { throw 'Reconcile original batches and restore owned receiver ACLs before stopping candidates' }
  foreach($name in @('desktop-process.json','service-process.json')) {
    $recordPath=Join-Path $Root $name
    if(Test-Path -LiteralPath $recordPath) {
      $record=Get-Content -LiteralPath $recordPath -Raw|ConvertFrom-Json
      if([string]$record.exe -notin @($desktopExe,$executorExe) -or [string]$record.config -ne $config) { throw 'Candidate launch record points outside its owned paths' }
      StopExact $record
    }
  }
  # A SYSTEM command wrapper may have exited before its exact candidate helpers.
  foreach($helper in @(Get-CimInstance Win32_Process|Where-Object {[string]$_.ExecutablePath -eq $executorExe})) {
    $process=Get-Process -Id ([int]$helper.ProcessId) -ErrorAction SilentlyContinue
    if($process -and [string]$process.Path -eq $executorExe) { $process|Stop-Process -Force }
  }
}
for($attempt=0;$attempt -lt 20;$attempt++) {
  $remaining=@(Get-CimInstance Win32_Process|Where-Object {[string]$_.ExecutablePath -in @($desktopExe,$executorExe) -or ([string]$_.Name -eq 'msedgewebview2.exe' -and [string]$_.CommandLine -like ('*'+$Root+'*'))})
  $ports=@(Get-NetTCPConnection -LocalPort @([int]$marker.cdp_port,[int]$marker.ipc_port) -State Listen -ErrorAction SilentlyContinue)
  if($Phase -ne 'Stop' -or (!$remaining.Count -and !$ports.Count)) { break }
  Start-Sleep -Milliseconds 300
}
$installed=InstalledService
$pass=(!$remaining.Count -and !$ports.Count -and ($marker.installed_before.state -ne 'Running' -or $installed.state -eq 'Running'))
$report=[ordered]@{phase=$Phase;root=$Root;machine=[string]$env:COMPUTERNAME;remaining_owned_processes=[int]$remaining.Count;remaining_test_listeners=[int]$ports.Count;installed_executor=$installed;pass=$pass;at=[DateTime]::UtcNow.ToString('o')}
WriteJson (Join-Path $Root ($Phase.ToLowerInvariant()+'-result.json')) $report
$report|ConvertTo-Json -Depth 4 -Compress
if($Phase -eq 'Stop' -and !$pass) { throw 'Owned cleanup or installed service verification incomplete' }
