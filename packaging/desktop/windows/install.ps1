param(
    [Parameter(Mandatory = $true)]
    [string]$ControlUrl,

    [Parameter(Mandatory = $true)]
    [string]$RelayUrl,

    [string]$InstallRoot,
    [string]$InstallerLogPath
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-logging.ps1')
Initialize-PabInstallerLog -Action 'install' -Path $InstallerLogPath
$errorLog = Join-Path $env:ProgramData 'PixelsAgentBridge\logs\install-error.log'
trap {
    $failure = $_
    Write-PabInstallerFailure -Failure $failure -LegacyLog $errorLog
    Stop-PabInstallerLog
    break
}

function Set-PabDesktopStartup {
    param(
        [Parameter(Mandatory = $true)][string]$InstallRoot,
        [string]$StartupDirectory = [Environment]::GetFolderPath('CommonStartup')
    )
    $installFull = [IO.Path]::GetFullPath($InstallRoot)
    $desktop = Join-Path $installFull 'pab-desktop.exe'
    if (-not (Test-Path -LiteralPath $desktop -PathType Leaf)) {
        throw "Desktop application not found: $desktop"
    }
    if (-not $StartupDirectory) { throw 'Windows common Startup directory is unavailable' }
    New-Item -ItemType Directory -Path $StartupDirectory -Force | Out-Null
    # Explorer runs this as the signed-in user. No elevation or hidden/helper
    # arguments: the normal GUI shows its window and restores an existing one.
    $shortcutPath = Join-Path $StartupDirectory 'Pixels Agent Bridge.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $desktop
    $shortcut.Arguments = ''
    $shortcut.WorkingDirectory = $installFull
    $shortcut.IconLocation = "$desktop,0"
    $shortcut.Description = 'Pixels Agent Bridge'
    $shortcut.WindowStyle = 1
    $shortcut.Save()
}

function Set-PabServiceConfiguration {
    param([string]$Name, [string]$BinaryPath)
    $service = Get-CimInstance Win32_Service -Filter "Name = '$Name'" -ErrorAction Stop
    if ($null -eq $service) { throw "Windows service not found: $Name" }
    if ($service.PathName -eq $BinaryPath -and $service.StartName -eq 'LocalSystem' -and $service.StartMode -eq 'Auto') {
        return
    }
    # Windows PowerShell 5.1 strips embedded quotes when invoking sc.exe.
    # Pass the complete command as a CIM string, preserving quoted paths.
    $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{
        PathName = $BinaryPath
        StartName = 'LocalSystem'
        StartPassword = ''
        StartMode = 'Automatic'
    } -ErrorAction Stop
    if ($changed.ReturnValue -ne 0) {
        throw "Could not update the Executor Windows service (Win32_Service.Change code $($changed.ReturnValue))"
    }
    $actual = Get-CimInstance Win32_Service -Filter "Name = '$Name'" -ErrorAction Stop
    if ($actual.PathName -ne $BinaryPath -or $actual.StartName -ne 'LocalSystem' -or $actual.StartMode -ne 'Auto') {
        throw 'Executor Windows service configuration did not match the requested settings'
    }
}

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
    $names = @('pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe', 'pab-document-worker.exe')
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

function Stop-PabRuntimeProcesses {
    param([string]$InstallRoot)
    $node = [IO.Path]::GetFullPath((Join-Path $InstallRoot 'browser-runtime\node.exe'))
    Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -OperationTimeoutSec 5 |
        Where-Object { $_.ExecutablePath -ieq $node } |
        ForEach-Object {
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
            Wait-Process -Id $_.ProcessId -Timeout 5 -ErrorAction SilentlyContinue
        }
    Stop-PabInstalledBinary -Paths @((Join-Path $InstallRoot 'pab-document-worker.exe')) -WarnIfRunning
}

Set-PabInstallerStage 'validate_inputs'
if (-not $ControlUrl.StartsWith('wss://')) {
    throw 'ControlUrl must use wss://'
}
if (-not $RelayUrl.StartsWith('https://')) {
    throw 'RelayUrl must use https://'
}
Set-PabInstallerStage 'check_administrator'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Installation requires an elevated PowerShell window'
}
if (-not $InstallRoot) {
    $InstallRoot = Join-Path $env:ProgramFiles 'PixelsAgentBridge'
}
Set-PabInstallerStage 'check_sas_policy'
Enable-PabServiceSasPolicy

$source = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-PabInstallerStage 'validate_package'
$requiredFiles = @(
    'pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe', 'pab-document-worker.exe',
    'browser-runtime.zip', 'document-runtime.zip',
    'run-app.ps1', 'launch-app.ps1', 'run-session-supervisor.ps1', 'uninstall.ps1', 'installer-logging.ps1'
)
foreach ($name in $requiredFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $name) -PathType Leaf)) {
        throw "Package file is missing: $name"
    }
}
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
# Validate/create TOML with the new binary before stopping installed services.
Set-PabInstallerStage 'prepare_configuration'
$nativeExit = Invoke-PabInstallerNativeCommand -FilePath (Join-Path $source 'pab-executor.exe') -ArgumentList @(
    'configure', '--config', (Join-Path $InstallRoot 'config.toml'),
    '--previous', $InstallRoot, '--control-url', $ControlUrl, '--relay-url', $RelayUrl)
if ($nativeExit -ne 0) { throw "Could not prepare TOML configuration (exit code $nativeExit)" }
$installFull = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
$runtimeCurrent = Join-Path $installFull 'browser-runtime'
$runtimeNew = Join-Path $installFull ("browser-runtime." + [Guid]::NewGuid().ToString('N') + '.new')
$runtimeOld = Join-Path $installFull ("browser-runtime." + [Guid]::NewGuid().ToString('N') + '.old')
$documentCurrent = Join-Path $installFull 'document-runtime'
$documentNew = Join-Path $installFull ("document-runtime." + [Guid]::NewGuid().ToString('N') + '.new')
$documentOld = Join-Path $installFull ("document-runtime." + [Guid]::NewGuid().ToString('N') + '.old')
foreach ($path in @($runtimeCurrent, $runtimeNew, $runtimeOld, $documentCurrent, $documentNew, $documentOld)) {
    if (-not [IO.Path]::GetFullPath($path).StartsWith($installFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Browser runtime path escaped the install directory'
    }
}
Set-PabInstallerStage 'extract_runtimes'
Expand-Archive -LiteralPath (Join-Path $source 'browser-runtime.zip') -DestinationPath $runtimeNew
Expand-Archive -LiteralPath (Join-Path $source 'document-runtime.zip') -DestinationPath $documentNew
foreach ($name in @('node.exe', 'node_modules\@playwright\mcp\cli.js', 'manifest.json')) {
    if (-not (Test-Path -LiteralPath (Join-Path $runtimeNew $name) -PathType Leaf)) {
        throw "Browser runtime archive is incomplete: $name"
    }
}
foreach ($name in @('pdfium.dll', 'tessdata\eng.traineddata', 'tessdata\chi_sim.traineddata')) {
    if (-not (Test-Path -LiteralPath (Join-Path $documentNew $name) -PathType Leaf)) {
        throw "Document runtime archive is incomplete: $name"
    }
}
$taskName = 'PixelsAgentBridgeExecutor'
Set-PabInstallerStage 'stop_existing_services'
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
    try { $null = Invoke-PabInstallerNativeCommand -FilePath 'sc.exe' -ArgumentList @('stop', $taskName) -TimeoutSeconds 15 }
    catch { Write-Warning "Service stop request failed; checking for a stuck process: $($_.Exception.Message)" }
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
    Set-PabInstallerStage 'stop_runtime_processes'
    Stop-PabRuntimeProcesses -InstallRoot $InstallRoot
    Set-PabInstallerStage 'replace_program_files'
    if (Test-Path -LiteralPath $runtimeCurrent) {
        Move-Item -LiteralPath $runtimeCurrent -Destination $runtimeOld -ErrorAction Stop
    }
    try {
        Move-Item -LiteralPath $runtimeNew -Destination $runtimeCurrent -ErrorAction Stop
    } catch {
        if (Test-Path -LiteralPath $runtimeOld) {
            Move-Item -LiteralPath $runtimeOld -Destination $runtimeCurrent -ErrorAction SilentlyContinue
        }
        throw
    }
    if (Test-Path -LiteralPath $documentCurrent) {
        Move-Item -LiteralPath $documentCurrent -Destination $documentOld -ErrorAction Stop
    }
    Move-Item -LiteralPath $documentNew -Destination $documentCurrent -ErrorAction Stop
    Install-PabBinaries -SourceRoot $source -DestinationRoot $InstallRoot
} catch {
    $installFailure = $_
    if (Test-Path -LiteralPath $runtimeOld) {
        try {
            if (Test-Path -LiteralPath $runtimeCurrent) {
                Remove-Item -LiteralPath $runtimeCurrent -Recurse -Force -ErrorAction Stop
            }
            Move-Item -LiteralPath $runtimeOld -Destination $runtimeCurrent -ErrorAction Stop
        } catch {
            Write-Warning "Could not restore previous browser runtime: $($_.Exception.Message)"
        }
    }
    if (Test-Path -LiteralPath $documentOld) {
        try {
            if (Test-Path -LiteralPath $documentCurrent) { Remove-Item -LiteralPath $documentCurrent -Recurse -Force -ErrorAction Stop }
            Move-Item -LiteralPath $documentOld -Destination $documentCurrent -ErrorAction Stop
        } catch { Write-Warning "Could not restore previous document runtime: $($_.Exception.Message)" }
    }
    # The binary transaction restored the previous files. Bring the old
    # service and session helpers back before reporting the install failure.
    if ($serviceWasRunning) {
        try { $null = Invoke-PabInstallerNativeCommand -FilePath 'sc.exe' -ArgumentList @('start', $taskName) -TimeoutSeconds 20 }
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
foreach ($name in @('run-app.ps1', 'launch-app.ps1', 'run-session-supervisor.ps1', 'uninstall.ps1', 'installer-logging.ps1')) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination $InstallRoot -Force
}


Set-PabInstallerStage 'configure_local_access'
$dataRoot = Join-Path $env:ProgramData 'PixelsAgentBridge'
New-Item -ItemType Directory -Force -Path $dataRoot | Out-Null
$nativeExit = Invoke-PabInstallerNativeCommand -FilePath 'icacls.exe' -ArgumentList @(
    $dataRoot, '/inheritance:r', '/grant:r', '*S-1-5-18:(OI)(CI)F', '*S-1-5-32-544:(OI)(CI)F')
if ($nativeExit -ne 0) {
    throw "Could not protect the Executor data directory (exit code $nativeExit)"
}
$env:PAB_DATA_DIR = $dataRoot
$userToken = Join-Path $env:LOCALAPPDATA 'PixelsAgentBridge\local-access.key'
$nativeExit = Invoke-PabInstallerNativeCommand -FilePath (Join-Path $InstallRoot 'pab-executor.exe') -ArgumentList @(
    'issue-local-access', $userToken)
if ($nativeExit -ne 0) {
    throw "Could not grant this user local device access (exit code $nativeExit)"
}
$trigger = New-ScheduledTaskTrigger -AtStartup
$system = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -StartWhenAvailable -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$serviceCommand = '"' + (Join-Path $InstallRoot 'pab-executor.exe') + '" --service --config "' + (Join-Path $InstallRoot 'config.toml') + '"'
Set-PabInstallerStage 'register_executor_service'
if ($existingService) {
    Set-PabServiceConfiguration -Name $taskName -BinaryPath $serviceCommand
} else {
    New-Service -Name $taskName -BinaryPathName $serviceCommand `
        -DisplayName 'Pixels Agent Bridge Executor' -StartupType Automatic | Out-Null
}
Set-PabInstallerStage 'start_executor_service'
try {
    $startExit = Invoke-PabInstallerNativeCommand -FilePath 'sc.exe' -ArgumentList @('start', $taskName) -TimeoutSeconds 20
    $startedService = Get-Service -Name $taskName
    $startedService.WaitForStatus('Running', [TimeSpan]::FromSeconds(15))
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

Set-PabInstallerStage 'register_session_supervisor'
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

Set-PabInstallerStage 'configure_desktop_startup'
Set-PabDesktopStartup -InstallRoot $InstallRoot

if (Test-Path -LiteralPath $runtimeOld) {
    try { Remove-Item -LiteralPath $runtimeOld -Recurse -Force -ErrorAction Stop }
    catch { Write-Warning "Previous browser runtime remains in use: $runtimeOld" }
}
if (Test-Path -LiteralPath $documentOld) {
    try { Remove-Item -LiteralPath $documentOld -Recurse -Force -ErrorAction Stop }
    catch { Write-Warning "Previous document runtime remains in use: $documentOld" }
}

Write-Output "Installed to $InstallRoot"
Write-Output "App: powershell.exe -File `"$(Join-Path $InstallRoot 'run-app.ps1')`""
Set-PabInstallerStage 'completed'
Write-PabInstallerLog 'success'
Stop-PabInstallerLog
