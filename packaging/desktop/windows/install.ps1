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
$sasPolicy = (Get-ItemProperty -Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Policies\System' `
    -Name SoftwareSASGeneration -ErrorAction SilentlyContinue).SoftwareSASGeneration
if ($sasPolicy -notin @(1, 3)) {
    throw 'Windows policy SoftwareSASGeneration must allow Services (value 1 or 3) for remote Ctrl+Alt+Delete'
}

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
$requiredFiles = @(
    'pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe',
    'run-app.ps1', 'launch-app.ps1', 'run-session-supervisor.ps1', 'uninstall.ps1'
)
foreach ($name in $requiredFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $name) -PathType Leaf)) {
        throw "Package file is missing: $name"
    }
}
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
$taskName = 'PixelsAgentBridgeExecutor'
$supervisorTaskName = 'PixelsAgentBridgeSessionSupervisor'
$existingService = Get-Service -Name $taskName -ErrorAction SilentlyContinue
if ($existingService) {
    Stop-Service -Name $taskName -ErrorAction SilentlyContinue
    $existingService.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(15))
}
$existingSupervisorTask = Get-ScheduledTask -TaskName $supervisorTaskName -ErrorAction SilentlyContinue
if ($existingSupervisorTask) {
    Stop-ScheduledTask -TaskName $supervisorTaskName -ErrorAction SilentlyContinue
}
$existingTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($existingTask) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
$binaryNames = @('pab-mcp', 'pab-bridge', 'pab-executor', 'pab-desktop')
foreach ($name in $binaryNames) {
    $binary = Join-Path $InstallRoot "$name.exe"
    Get-CimInstance Win32_Process -Filter "Name = '$name.exe'" |
        Where-Object { $_.ExecutablePath -eq $binary } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}
Start-Sleep -Seconds 2
foreach ($name in $binaryNames) {
    $binary = Join-Path $InstallRoot "$name.exe"
    $running = @(Get-CimInstance Win32_Process -Filter "Name = '$name.exe'" |
        Where-Object { $_.ExecutablePath -eq $binary })
    if ($running.Count -gt 0) {
        throw "Cannot replace running program: $binary"
    }
}
foreach ($obsolete in @('run-ui.ps1', 'run-device-ui.ps1', 'run-executor.ps1', 'pab-bridge.exe')) {
    $path = Join-Path $InstallRoot $obsolete
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
foreach ($name in @('pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}
foreach ($name in @('run-app.ps1', 'launch-app.ps1', 'run-session-supervisor.ps1', 'uninstall.ps1')) {
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
$env:PAB_DATA_DIR = $dataRoot
$userToken = Join-Path $env:LOCALAPPDATA 'PixelsAgentBridge\local-access.key'
& (Join-Path $InstallRoot 'pab-executor.exe') issue-local-access $userToken
if ($LASTEXITCODE -ne 0) {
    throw 'Could not grant this user local device access'
}
$trigger = New-ScheduledTaskTrigger -AtStartup
$system = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -StartWhenAvailable -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$serviceCommand = '"' + (Join-Path $InstallRoot 'pab-executor.exe') + '" --service'
if ($existingService) {
    $serviceConfiguration = Get-CimInstance Win32_Service -Filter "Name = '$taskName'"
    $needsConfigurationUpdate = (
        $null -eq $serviceConfiguration -or
        $serviceConfiguration.PathName -ne $serviceCommand -or
        $serviceConfiguration.StartName -ne 'LocalSystem' -or
        $serviceConfiguration.StartMode -ne 'Auto'
    )
    if ($needsConfigurationUpdate) {
        $configOutput = & sc.exe config $taskName binPath= $serviceCommand obj= LocalSystem start= auto 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "Could not update the Executor Windows service: $($configOutput -join ' ')"
        }
    }
} else {
    New-Service -Name $taskName -BinaryPathName $serviceCommand `
        -DisplayName 'Pixels Agent Bridge Executor' -StartupType Automatic | Out-Null
}
Start-Service -Name $taskName

$supervisorAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument (
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
    (Join-Path $InstallRoot 'run-session-supervisor.ps1') + '"'
)
$supervisorTask = @{
    TaskName = $supervisorTaskName
    Action = $supervisorAction
    Trigger = $trigger
    Principal = $system
    Settings = $settings
    Force = $true
}
Register-ScheduledTask @supervisorTask | Out-Null
Start-ScheduledTask -TaskName $supervisorTaskName

Write-Output "Installed to $InstallRoot"
Write-Output "App: powershell.exe -File `"$(Join-Path $InstallRoot 'run-app.ps1')`""
