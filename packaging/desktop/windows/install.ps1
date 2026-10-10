param(
    [Parameter(Mandatory = $true)]
    [string]$ControlUrl,

    [Parameter(Mandatory = $true)]
    [string]$RelayUrl,

    [string]$InstallRoot
)

$ErrorActionPreference = 'Stop'

function Stop-PabInstalledBinary {
    param([string[]]$Paths, [switch]$WarnIfRunning)
    # Compare full paths, never kill the Agent client or another installation.
    Get-CimInstance Win32_Process -Filter "Name LIKE 'pab-%'" |
        Where-Object { $_.ExecutablePath -and $Paths -icontains $_.ExecutablePath } |
        ForEach-Object {
            $processIdToStop = $_.ProcessId
            $createdAt = $_.CreationDate
            try {
                Stop-Process -Id $processIdToStop -Force -ErrorAction Stop
                Wait-Process -Id $processIdToStop -Timeout 5 -ErrorAction SilentlyContinue
            } catch {
                if ($WarnIfRunning) { Write-Warning "Could not stop old program (PID ${processIdToStop}): $($_.Exception.Message)" }
            }
            # A renamed image can remain in use without blocking publication of
            # the replacement. Do not roll back merely because its process takes
            # longer than five seconds to exit. Check creation time to avoid
            # confusing a reused PID with the process we tried to stop.
            $remaining = Get-CimInstance Win32_Process -Filter "ProcessId = $processIdToStop" -ErrorAction SilentlyContinue
            if ($WarnIfRunning -and $remaining -and $remaining.CreationDate -eq $createdAt) {
                Write-Warning "Old program (PID $processIdToStop) is still running from its retired image; replacement can continue"
            }
        }
}

function Install-PabBinaries {
    param([string]$SourceRoot, [string]$DestinationRoot)
    $names = @('pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe')
    $transaction = [Guid]::NewGuid().ToString('N')
    $staged = @{}
    $retired = @{}
    $published = @()
    $committed = $false
    try {
        # Finish all copying before interrupting the installed programs.
        foreach ($name in $names) {
            $binary = Join-Path $DestinationRoot $name
            $staged[$binary] = "$binary.$transaction.new"
            Copy-Item -LiteralPath (Join-Path $SourceRoot $name) -Destination $staged[$binary] -Force
        }
        foreach ($name in ($names + @('pab-bridge.exe'))) {
            $binary = Join-Path $DestinationRoot $name
            $old = "$binary.$transaction.old"
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while (Test-Path -LiteralPath $binary) {
                try {
                    # A running Windows image can normally be renamed. Removing
                    # its launch path BEFORE killing it prevents client respawn
                    # from reopening the old binary during replacement.
                    Move-Item -LiteralPath $binary -Destination $old -ErrorAction Stop
                    $retired[$binary] = $old
                    break
                } catch {
                    if ([DateTime]::UtcNow -ge $deadline) {
                        throw "Could not retire installed program after 30 seconds: $binary. $($_.Exception.Message)"
                    }
                    Stop-PabInstalledBinary -Paths @($binary)
                    Start-Sleep -Milliseconds 200
                }
            }
            # Windows may report either the original or renamed executable path.
            Stop-PabInstalledBinary -Paths @($binary, $old) -WarnIfRunning
        }
        foreach ($binary in $staged.Keys) {
            Move-Item -LiteralPath $staged[$binary] -Destination $binary -ErrorAction Stop
            $published += $binary
        }
        $committed = $true
    } catch {
        $failure = $_
        # Restore the previous files if staging/retirement/publication failed.
        foreach ($binary in $published) {
            try {
                Move-Item -LiteralPath $binary -Destination $staged[$binary] -ErrorAction Stop
                Stop-PabInstalledBinary -Paths @($binary, $staged[$binary])
            } catch { Write-Warning "Could not withdraw new program: $binary" }
        }
        foreach ($binary in $retired.Keys) {
            try {
                Move-Item -LiteralPath $retired[$binary] -Destination $binary -ErrorAction Stop
            } catch { Write-Warning "Previous program retained at $($retired[$binary]); could not restore $binary" }
        }
        throw $failure
    } finally {
        $cleanupPaths = @($staged.Values)
        if ($committed) { $cleanupPaths += @($retired.Values) }
        foreach ($path in $cleanupPaths) {
            if (Test-Path -LiteralPath $path) {
                # Only remove exact files allocated by this invocation. A locked
                # old image may be left behind without blocking the new install.
                try { Remove-Item -LiteralPath $path -Force -ErrorAction Stop }
                catch { Write-Warning "Upgrade temporary file is still in use: $path" }
            }
        }
    }
}

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
$existingSupervisorTask = Get-ScheduledTask -TaskName $supervisorTaskName -ErrorAction SilentlyContinue
$supervisorWasRunning = $existingSupervisorTask -and $existingSupervisorTask.State -eq 'Running'
if ($existingSupervisorTask) {
    Stop-ScheduledTask -TaskName $supervisorTaskName -ErrorAction SilentlyContinue
}
$existingTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
$taskWasRunning = $existingTask -and $existingTask.State -eq 'Running'
if ($existingTask) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
}
$existingService = Get-Service -Name $taskName -ErrorAction SilentlyContinue
$serviceWasRunning = $existingService -and $existingService.Status -eq 'Running'
if ($existingService) {
    Stop-Service -Name $taskName -ErrorAction SilentlyContinue
    try {
        $existingService.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(10))
    } catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [System.ServiceProcess.TimeoutException]) { throw }
    }
    $existingService.Refresh()
    if ($existingService.Status -ne 'Stopped') {
        $serviceProcessId = (Get-CimInstance Win32_Service -Filter "Name = '$taskName'").ProcessId
        $serviceProcess = if ($serviceProcessId -gt 0) {
            Get-CimInstance Win32_Process -Filter "ProcessId = $serviceProcessId"
        }
        if (-not $serviceProcess) {
            $existingService.Refresh()
            if ($existingService.Status -ne 'Stopped') {
                throw "Executor service did not stop, and its process could not be found: $taskName (PID $serviceProcessId)"
            }
        } elseif ($serviceProcess.ExecutablePath -ine (Join-Path $InstallRoot 'pab-executor.exe')) {
            throw "Executor service did not stop, and its process could not be verified: $taskName (PID $serviceProcessId)"
        } else {
            Write-Output "Executor service did not stop within 10 seconds; terminating its verified process (PID $serviceProcessId)."
            Stop-Process -Id $serviceProcessId -Force
            try {
                $existingService.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(10))
            } catch [System.Management.Automation.MethodInvocationException] {
                if ($_.Exception.InnerException -isnot [System.ServiceProcess.TimeoutException]) { throw }
                throw "Executor service is still not stopped after its process was terminated: $taskName (PID $serviceProcessId)"
            }
        }
    }
}
try {
    Install-PabBinaries -SourceRoot $source -DestinationRoot $InstallRoot
} catch {
    $installFailure = $_
    # The binary transaction restored the previous files. Bring the old
    # service and session helpers back before reporting the install failure.
    if ($serviceWasRunning) {
        try { Start-Service -Name $taskName -ErrorAction Stop }
        catch { Write-Warning "Could not restart previous Executor service: $($_.Exception.Message)" }
    }
    if ($taskWasRunning) {
        try { Start-ScheduledTask -TaskName $taskName -ErrorAction Stop }
        catch { Write-Warning "Could not restart previous Executor task: $($_.Exception.Message)" }
    }
    if ($supervisorWasRunning) {
        try { Start-ScheduledTask -TaskName $supervisorTaskName -ErrorAction Stop }
        catch { Write-Warning "Could not restart previous session supervisor: $($_.Exception.Message)" }
    }
    throw $installFailure
}
if ($existingTask) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
foreach ($obsolete in @('run-ui.ps1', 'run-device-ui.ps1', 'run-executor.ps1', 'pab-bridge.exe')) {
    $path = Join-Path $InstallRoot $obsolete
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
foreach ($name in @('run-app.ps1', 'launch-app.ps1', 'run-session-supervisor.ps1', 'uninstall.ps1')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}

@{
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
try {
    Start-Service -Name $taskName
} catch {
    $startupFailure = $_
    Write-Output 'Executor startup failed. Recent startup diagnostics:'
    # Surface the actual cause instead of only the generic Start-Service error.
    # Do not print arbitrary command output, credentials or the whole log.
    $executorLog = Join-Path $dataRoot 'logs\executor.log'
    try {
        if (Test-Path -LiteralPath $executorLog) {
            Get-Content -LiteralPath $executorLog -Tail 80 |
                Select-String 'Executor service stopped with an error|Windows Executor service failed|process panicked' |
                Select-Object -Last 3 |
                ForEach-Object { Write-Output $_.Line }
        }
    } catch {
        Write-Output "Could not read startup log: $executorLog"
    }
    Write-Output "Full Executor log: $executorLog"
    throw $startupFailure
}

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
