param(
 [Parameter(Mandatory=$true)][ValidateSet('Start','Stop','Watch')][string]$Action,
 [Parameter(Mandatory=$true)][string]$Directory,
 [string]$LocalAddress,
 [string]$PeerAddress,
 [int[]]$Ports,
 [ValidateRange(30,600)][int]$Seconds=240,
 [ValidateSet('all','nics')][string]$Components='all'
)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$root=[IO.Path]::GetFullPath($Directory)
if(![IO.Path]::IsPathRooted($Directory) -or $root -notmatch '[\\/]smoke[\\/]' -or $root.StartsWith('\\')){throw 'Use a new absolute local directory under smoke'}
$statePath=Join-Path $root 'owner.json'
$logPath=Join-Path $root 'control.jsonl'
function Native([string]$Program,[string[]]$Arguments,[switch]$AllowFailure){
 $info=[Diagnostics.ProcessStartInfo]::new()
 $info.FileName=$Program
 $quoted=@($Arguments|ForEach-Object {'"'+[regex]::Replace([regex]::Replace($_,'(\\*)"','$1$1\"'),'(\\+)$','$1$1')+'"'})
 $info.Arguments=[string]::Join(' ',$quoted)
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true
 $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $process=[Diagnostics.Process]::new();$process.StartInfo=$info
 $out=[IO.MemoryStream]::new();$err=[IO.MemoryStream]::new()
 try {
  $null=$process.Start()
  $outTask=$process.StandardOutput.BaseStream.CopyToAsync($out)
  $errTask=$process.StandardError.BaseStream.CopyToAsync($err)
  if(!$process.WaitForExit(30000)){$process.Kill();throw 'Own native capture command exceeded 30 seconds'}
  $outTask.GetAwaiter().GetResult();$errTask.GetAwaiter().GetResult()
  $code=$process.ExitCode
  $bytes=$out.ToArray()+$err.ToArray()
  try {$output=[Text.UTF8Encoding]::new($false,$true).GetString($bytes)} catch {$output=[Text.Encoding]::GetEncoding([Globalization.CultureInfo]::CurrentCulture.TextInfo.OEMCodePage).GetString($bytes)}
 } finally {$out.Dispose();$err.Dispose();$process.Dispose()}
 if(Test-Path -LiteralPath $root){@{program=$Program;arguments=$Arguments;exit_code=$code;output=$output;utc=(Get-Date).ToUniversalTime().ToString('o')}|ConvertTo-Json -Depth 4 -Compress|Add-Content -LiteralPath $logPath -Encoding UTF8}
 if($code -ne 0 -and !$AllowFailure){throw ('Native command failed: '+$Program+' '+($Arguments -join ' ')+' ('+$code+')')}
 return @{code=$code;output=[string]$output}
}
function FilterNames($text){
 $rows=@([regex]::Matches($text,'(?m)^\s*\d+\s+(\S+)')|ForEach-Object {$_.Groups[1].Value})
 if(!$rows.Count -and $text -notmatch '(?m)^\s*(None|\u65e0)[.\u3002]?\s*$'){throw 'Unrecognized PktMon filter listing; leave filters unchanged'}
 return ,$rows
}
function Save($value){$value|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $statePath -Encoding UTF8}
function StopOwned {
 $state=Get-Content -LiteralPath $statePath -Raw -Encoding UTF8|ConvertFrom-Json
 $mutex=[Threading.Mutex]::new($false,('Global\PAB-Smoke-Pktmon-'+$state.id))
 $held=$false
 try {
  $held=$mutex.WaitOne(10000)
  if(!$held){throw 'Own capture cleanup lock timed out'}
  if(Test-Path -LiteralPath (Join-Path $root 'stopped.json')){return}
  $status=Native 'pktmon.exe' @('status')
  $session=Native 'logman.exe' @('query','PktMon','-ets') -AllowFailure
  if($session.code -eq 0){
   if(!$status.output.Contains($state.etl)){throw 'A different PktMon capture is active; leave it unchanged'}
   $counters=Native 'pktmon.exe' @('counters','--json') -AllowFailure
   try {$null=$counters.output|ConvertFrom-Json;$counterText=$counters.output} catch {
    [IO.File]::WriteAllText((Join-Path $root 'counters.txt'),$counters.output,[Text.UTF8Encoding]::new($false))
    $counterText=@{unavailable=$true;raw_file='counters.txt'}|ConvertTo-Json
   }
   [IO.File]::WriteAllText((Join-Path $root 'counters.json'),$counterText,[Text.UTF8Encoding]::new($false))
   $null=Native 'pktmon.exe' @('stop')
  }
  $listed=Native 'pktmon.exe' @('filter','list')
  $names=FilterNames $listed.output
  $foreign=@($names|Where-Object {$_ -notin $state.filters})
  # PktMon only supports clearing ALL filters, not removal by name.
  if(!$foreign.Count -and $names.Count){$null=Native 'pktmon.exe' @('filter','remove')}
  $status=Native 'pktmon.exe' @('status')
  @{stopped=$true;filters_removed=(!$foreign.Count);foreign_filters=$foreign;etl_exists=(Test-Path -LiteralPath $state.etl);status=$status.output;utc=(Get-Date).ToUniversalTime().ToString('o')}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $root 'stopped.json') -Encoding UTF8
 } finally {if($held){$mutex.ReleaseMutex()};$mutex.Dispose()}
}
if($Action -eq 'Start'){
 foreach($value in @($LocalAddress,$PeerAddress)){
  $ip=[Net.IPAddress]::Parse($value)
  $b=$ip.GetAddressBytes()
  if($b.Length -ne 4 -or !($b[0] -eq 10 -or ($b[0] -eq 172 -and $b[1] -ge 16 -and $b[1] -le 31) -or ($b[0] -eq 192 -and $b[1] -eq 168))){throw 'Explicit private IPv4 test endpoints required'}
 }
 $Ports=@($Ports|Sort-Object -Unique)
 if(!$Ports.Count -or $Ports.Count -gt 16 -or @($Ports|Where-Object {$_ -lt 1024 -or $_ -gt 65535}).Count){throw 'Provide 1-16 explicit unprivileged test UDP ports'}
 if(Test-Path -LiteralPath $root){throw 'Use a fresh capture directory'}
 $session=Native 'logman.exe' @('query','PktMon','-ets') -AllowFailure
 if($session.code -eq 0){throw 'PktMon already active; leave it unchanged'}
 $filters=Native 'pktmon.exe' @('filter','list')
 if((FilterNames $filters.output).Count){throw 'Existing PktMon filters; leave them unchanged'}
 New-Item -ItemType Directory -Path $root|Out-Null
 $id=[guid]::NewGuid().ToString('N')
 $state=@{id=$id;task=('PAB-Smoke-Pktmon-'+$id);filters=@();etl=(Join-Path $root 'capture.etl');deadline=(Get-Date).ToUniversalTime().AddSeconds($Seconds).ToString('o');local=$LocalAddress;peer=$PeerAddress;ports=$Ports;packet_bytes=96;max_mebibytes=128}
 Save $state
 Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $root 'capture.ps1')
 try {
  $principal=New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
  $settings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 12) -Priority 6
  $argsText='-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'+(Join-Path $root 'capture.ps1')+'" -Action Watch -Directory "'+$root+'"'
  $job=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $argsText
  Register-ScheduledTask -TaskName $state.task -Action $job -Principal $principal -Settings $settings|Out-Null
  Start-ScheduledTask -TaskName $state.task
  foreach($port in $Ports){
   $name='SMOKE-'+$id.Substring(0,12)+'-'+$port
   $null=Native 'pktmon.exe' @('filter','add',$name,'-i',$LocalAddress,$PeerAddress,'-t','UDP','-p',"$port")
   $state.filters+=@($name);Save $state
  }
  $state.component_selection=$Components;Save $state
  $null=Native 'pktmon.exe' @('start','--capture','--comp',$Components,'--flags','0x013','--pkt-size','96','--file-name',$state.etl,'--file-size','128','--log-mode','circular')
  $status=Native 'pktmon.exe' @('status')
  if(!$status.output.Contains($state.etl)){throw 'Capture ownership could not be verified'}
  Set-Content -LiteralPath (Join-Path $root 'started') -Value 'ready'
  @{started=$true;directory=$root;ports=$Ports;deadline=$state.deadline}|ConvertTo-Json -Compress
 } catch {StopOwned;throw}
} elseif($Action -eq 'Watch'){
 $state=Get-Content -LiteralPath $statePath -Raw -Encoding UTF8|ConvertFrom-Json
 $deadline=[datetime]::Parse($state.deadline).ToUniversalTime()
 while((Get-Date).ToUniversalTime() -lt $deadline -and !(Test-Path -LiteralPath (Join-Path $root 'stop.request')) -and !(Test-Path -LiteralPath (Join-Path $root 'stopped.json'))){Start-Sleep -Milliseconds 500}
 try {StopOwned} finally {Unregister-ScheduledTask -TaskName $state.task -Confirm:$false -ErrorAction SilentlyContinue}
} else {
 Set-Content -LiteralPath (Join-Path $root 'stop.request') -Value 'complete'
 StopOwned
 $state=Get-Content -LiteralPath $statePath -Raw -Encoding UTF8|ConvertFrom-Json
 $deadline=(Get-Date).AddSeconds(10)
 do {$task=Get-ScheduledTask -TaskName $state.task -ErrorAction SilentlyContinue;if($task -and $task.State -eq 'Running'){Start-Sleep -Milliseconds 200}} while($task -and $task.State -eq 'Running' -and (Get-Date) -lt $deadline)
 if($task -and $task.State -eq 'Running'){throw 'Own capture watchdog has not finished'}
 if($task){Unregister-ScheduledTask -TaskName $state.task -Confirm:$false}
 $stopped=Get-Content -LiteralPath (Join-Path $root 'stopped.json') -Raw -Encoding UTF8|ConvertFrom-Json
 if(!$stopped.filters_removed){throw 'Own capture stopped and watchdog removed; foreign filters appeared, so all filters were preserved'}
 @{stopped=$true;directory=$root;etl_bytes=if(Test-Path -LiteralPath $state.etl){(Get-Item -LiteralPath $state.etl).Length}else{0};task_removed=$true}|ConvertTo-Json -Compress
}
