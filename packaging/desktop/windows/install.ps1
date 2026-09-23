param(
    [Parameter(Mandatory = $true)]
    [string]$DeploymentId,

    [Parameter(Mandatory = $true)]
    [string]$ControlUrl,

    [Parameter(Mandatory = $true)]
    [string]$RelayUrl,

    [string]$InstallRoot
)

$ErrorActionPreference = 'Stop'
if (-not $ControlUrl.StartsWith('wss://')) {
    throw 'ControlUrl must use wss://'
}
if (-not $RelayUrl.StartsWith('https://')) {
    throw 'RelayUrl must use https://'
}
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Installation requires an elevated PowerShell window'
}
if (-not $InstallRoot) {
    $InstallRoot = Join-Path $env:ProgramFiles 'PixelsAgentBridge'
}

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
foreach ($obsolete in @('run-ui.ps1', 'run-device-ui.ps1')) {
    $path = Join-Path $InstallRoot $obsolete
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
foreach ($name in @('pab-mcp.exe', 'pab-bridge.exe', 'pab-executor.exe', 'pab-desktop.exe')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}
foreach ($name in @('run-app.ps1', 'run-executor.ps1', 'uninstall.ps1')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}

@{
    deployment_id = $DeploymentId
    control_url = $ControlUrl
    relay_urls = $RelayUrl
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $InstallRoot 'settings.json') -Encoding UTF8

$dataRoot = Join-Path $env:ProgramData 'PixelsAgentBridge'
New-Item -ItemType Directory -Force -Path $dataRoot | Out-Null
& icacls.exe $dataRoot /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw 'Could not protect the Executor data directory'
}
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument (
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
    (Join-Path $InstallRoot 'run-executor.ps1') + '"'
)
$trigger = New-ScheduledTaskTrigger -AtStartup
$system = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -Action $action -Trigger $trigger -Principal $system -Force | Out-Null
Start-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor'

Write-Output "Installed to $InstallRoot"
Write-Output "App: powershell.exe -File `"$(Join-Path $InstallRoot 'run-app.ps1')`""
