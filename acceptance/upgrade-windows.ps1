param(
 [Parameter(Mandatory=$true)][string]$Installer,
 [Parameter(Mandatory=$true)][string]$ExpectedSha256,
 [Parameter(Mandatory=$true)][string]$ExpectedVersion,
 [Parameter(Mandatory=$true)][string]$Report
)
$ErrorActionPreference='Stop'
# Durable no-replay guard. Run from a one-shot scheduled job, outside Executor.
$guard=[IO.File]::Open($Report+'.started',[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
$guard.Dispose()
$result=[ordered]@{state='starting';version=$ExpectedVersion;exit_code=$null;identity_preserved=$false;error=$null}
try {
 if((Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash -ine $ExpectedSha256) {throw 'Installer hash mismatch'}
 $key='C:\ProgramData\PixelsAgentBridge\device-endpoint.key'
 $before=(Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash
 $result.state='installing'
 $result | ConvertTo-Json | Set-Content -LiteralPath $Report -Encoding UTF8
 $process=Start-Process -FilePath $Installer -ArgumentList '/S' -WindowStyle Hidden -PassThru -Wait
 $result.exit_code=$process.ExitCode
 if($process.ExitCode -ne 0) {throw "Installer exit $($process.ExitCode)"}
 $result.identity_preserved=((Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash -eq $before)
 if(-not $result.identity_preserved) {throw 'Device identity changed'}
 $registry=Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge'
 if($registry.DisplayVersion -ne ($ExpectedVersion+' debug')) {throw 'Installed version mismatch'}
 if((Get-Service PixelsAgentBridgeExecutor).Status -ne 'Running') {throw 'Executor not running'}
 $hashes=[ordered]@{}
 foreach($name in @('pab-desktop.exe','pab-executor.exe','pab-mcp.exe')) {
  $hashes[$name]=(Get-FileHash -LiteralPath (Join-Path $registry.InstallLocation $name) -Algorithm SHA256).Hash.ToLowerInvariant()
 }
 $result['hashes']=$hashes
 $result.state='succeeded'
} catch { $result.state='failed';$result.error=$_.Exception.Message }
$result['finished_at']=[DateTime]::UtcNow.ToString('o')
$result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Report -Encoding UTF8
if($result.state -ne 'succeeded') {exit 1}
