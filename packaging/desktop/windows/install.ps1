param(
    [Parameter(Mandatory = $true)]
    [string]$DeploymentId,

    [Parameter(Mandatory = $true)]
    [string]$ControlUrl,

    [Parameter(Mandatory = $true)]
    [string]$RelayUrl,

    [switch]$Executor,

    [string]$InstallRoot
)

$ErrorActionPreference = 'Stop'
if (-not $ControlUrl.StartsWith('wss://')) {
    throw 'ControlUrl must use wss://'
}
if (-not $RelayUrl.StartsWith('https://')) {
    throw 'RelayUrl must use https://'
}

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
if ($Executor) {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Installing the Executor requires an elevated PowerShell window'
    }
    if (-not $InstallRoot) {
        $InstallRoot = Join-Path $env:ProgramFiles 'PixelsAgentBridge'
    }
} else {
    if (-not $InstallRoot) {
        $InstallRoot = Join-Path $env:LOCALAPPDATA 'Programs\PixelsAgentBridge'
    }
}

New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
foreach ($name in @('pab-mcp.exe', 'pab-bridge.exe', 'pab-executor.exe', 'pab-desktop.exe')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}
foreach ($name in @('run-ui.ps1', 'run-executor.ps1', 'run-device-ui.ps1', 'uninstall.ps1')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}

@{
    deployment_id = $DeploymentId
    control_url = $ControlUrl
    relay_urls = $RelayUrl
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $InstallRoot 'settings.json') -Encoding UTF8

if ($Executor) {
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
    $taskParameters = @{
        TaskName = 'PixelsAgentBridgeExecutor'
        Action = $action
        Trigger = $trigger
        Principal = $system
        Force = $true
    }
    Register-ScheduledTask @taskParameters | Out-Null
    Start-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor'
}

Write-Output "Installed to $InstallRoot"
Write-Output "Operator UI: powershell.exe -File `"$(Join-Path $InstallRoot 'run-ui.ps1')`""
if ($Executor) {
    Write-Output "Device access: set PAB_DATA_DIR=$dataRoot, then run pab-executor.exe show-access"
    Write-Output "Local device window: open an elevated PowerShell and run $(Join-Path $InstallRoot 'run-device-ui.ps1')"
}
