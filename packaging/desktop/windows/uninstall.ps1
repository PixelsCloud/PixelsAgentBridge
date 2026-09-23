param([string]$InstallRoot)

$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Uninstallation requires an elevated PowerShell window'
}
if (-not $InstallRoot) {
    $InstallRoot = Join-Path $env:ProgramFiles 'PixelsAgentBridge'
}
$task = Get-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -Confirm:$false
}

foreach ($processName in @('pab-desktop', 'pab-mcp', 'pab-bridge', 'pab-executor')) {
    Get-Process -Name $processName -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq (Join-Path $InstallRoot "$processName.exe") } |
        Stop-Process -Force
}

foreach ($name in @(
    'pab-mcp.exe',
    'pab-bridge.exe',
    'pab-executor.exe',
    'pab-desktop.exe',
    'settings.json',
    'run-app.ps1',
    'run-ui.ps1',
    'run-device-ui.ps1',
    'run-executor.ps1',
    'uninstall.ps1'
)) {
    $path = Join-Path $InstallRoot $name
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
Write-Output 'Application files removed. Persistent user and machine data were retained.'
