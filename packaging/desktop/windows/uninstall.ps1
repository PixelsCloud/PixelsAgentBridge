param([string]$InstallRoot)

$ErrorActionPreference = 'Stop'
function Remove-PabDesktopStartup {
    param(
        [Parameter(Mandatory = $true)][string]$InstallRoot,
        [string]$StartupDirectory = [Environment]::GetFolderPath('CommonStartup')
    )
    if (-not $StartupDirectory) { throw 'Windows common Startup directory is unavailable' }
    $shortcutPath = Join-Path $StartupDirectory 'Pixels Agent Bridge.lnk'
    if (-not (Test-Path -LiteralPath $shortcutPath -PathType Leaf)) { return }
    $desktop = Join-Path ([IO.Path]::GetFullPath($InstallRoot)) 'pab-desktop.exe'
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
    # An old installation's uninstaller must not remove a newer install's entry.
    if ($shortcut.TargetPath -ieq $desktop) {
        Remove-Item -LiteralPath $shortcutPath -Force
    }
}

$serviceRemoved = $false
$retiredBinaries = [ordered]@{}
$errorLog = Join-Path $env:ProgramData 'PixelsAgentBridge\logs\uninstall-error.log'
trap {
    $failure = $_
    try {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $errorLog) | Out-Null
        @(
            (Get-Date).ToString('o')
            $failure.ToString()
            $failure.ScriptStackTrace
        ) | Set-Content -LiteralPath $errorLog -Encoding UTF8
    } catch { }
    foreach ($binary in $retiredBinaries.Keys) {
        $retired = $retiredBinaries[$binary]
        if ((Test-Path -LiteralPath $retired -PathType Leaf) -and
            -not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            try { Move-Item -LiteralPath $retired -Destination $binary -ErrorAction Stop }
            catch {
                Add-Content -LiteralPath $errorLog -Value "Could not restore $binary`: $($_.Exception.Message)" -ErrorAction SilentlyContinue
            }
        }
    }
    # A failed removal must not strand a previously working remote device.
    if ($serviceRemoved -and (Test-Path -LiteralPath (Join-Path $InstallRoot 'pab-executor.exe'))) {
        try {
            if (-not (Get-Service -Name 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue)) {
                $serviceCommand = '"' + (Join-Path $InstallRoot 'pab-executor.exe') + '" --service'
                New-Service -Name 'PixelsAgentBridgeExecutor' -BinaryPathName $serviceCommand `
                    -DisplayName 'Pixels Agent Bridge Executor' -StartupType Automatic | Out-Null
            }
            Start-Service -Name 'PixelsAgentBridgeExecutor' -ErrorAction Stop
        } catch {
            Add-Content -LiteralPath $errorLog -Value "Service recovery failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue
        }
    }
    break
}
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Uninstallation requires an elevated PowerShell window'
}
if (-not $InstallRoot) {
    $InstallRoot = Join-Path $env:ProgramFiles 'PixelsAgentBridge'
}
$supervisorTask = Get-ScheduledTask -TaskName 'PixelsAgentBridgeSessionSupervisor' -ErrorAction SilentlyContinue
if ($supervisorTask) {
    Stop-ScheduledTask -TaskName 'PixelsAgentBridgeSessionSupervisor' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'PixelsAgentBridgeSessionSupervisor' -Confirm:$false
}
$task = Get-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'PixelsAgentBridgeExecutor' -Confirm:$false
}
$executorService = Get-Service -Name 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
if ($executorService) {
    Stop-Service -Name 'PixelsAgentBridgeExecutor' -ErrorAction SilentlyContinue
    try {
        $executorService.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(10))
    } catch [System.Management.Automation.MethodInvocationException] {
        if ($_.Exception.InnerException -isnot [System.ServiceProcess.TimeoutException]) { throw }
    }
    $executorService.Refresh()
    if ($executorService.Status -ne 'Stopped') {
        $serviceProcessId = (Get-CimInstance Win32_Service -Filter "Name = 'PixelsAgentBridgeExecutor'").ProcessId
        $serviceProcess = if ($serviceProcessId -gt 0) {
            Get-CimInstance Win32_Process -Filter "ProcessId = $serviceProcessId"
        }
        if (-not $serviceProcess) {
            $executorService.Refresh()
            if ($executorService.Status -ne 'Stopped') {
                throw "Executor service did not stop, and its process could not be found: PixelsAgentBridgeExecutor (PID $serviceProcessId)"
            }
        } elseif ($serviceProcess.ExecutablePath -ine (Join-Path $InstallRoot 'pab-executor.exe')) {
            throw "Executor service did not stop, and its process could not be verified: PixelsAgentBridgeExecutor (PID $serviceProcessId)"
        } else {
            Write-Output "Executor service did not stop within 10 seconds; terminating its verified process (PID $serviceProcessId)."
            Stop-Process -Id $serviceProcessId -Force
            try {
                $executorService.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(10))
            } catch [System.Management.Automation.MethodInvocationException] {
                if ($_.Exception.InnerException -isnot [System.ServiceProcess.TimeoutException]) { throw }
                throw "Executor service is still not stopped after its process was terminated: PixelsAgentBridgeExecutor (PID $serviceProcessId)"
            }
        }
    }
    & sc.exe delete 'PixelsAgentBridgeExecutor' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Could not remove the Executor Windows service' }
    $serviceRemoved = $true
}

foreach ($processName in @('pab-desktop', 'pab-mcp', 'pab-bridge', 'pab-executor', 'pab-document-worker')) {
    Get-Process -Name $processName -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq (Join-Path $InstallRoot "$processName.exe") } |
        Stop-Process -Force
}

# A desktop helper can restart pab-mcp while the uninstaller is stopping the
# running processes. Retire each launch path first so the helper cannot reopen
# it, then wait for Windows to release the old image before removing it.
$retirementId = [Guid]::NewGuid().ToString('N')
foreach ($name in @('pab-mcp.exe', 'pab-bridge.exe', 'pab-desktop.exe', 'pab-executor.exe', 'pab-document-worker.exe')) {
    $binary = Join-Path $InstallRoot $name
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { continue }
    $retired = "$binary.$retirementId.uninstall"
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($true) {
        try {
            Move-Item -LiteralPath $binary -Destination $retired -ErrorAction Stop
            $retiredBinaries[$binary] = $retired
            break
        } catch {
            if ([DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 200
        }
    }
}
foreach ($binary in $retiredBinaries.Keys) {
    $retired = $retiredBinaries[$binary]
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (Test-Path -LiteralPath $retired -PathType Leaf) {
        Get-CimInstance Win32_Process -Filter "Name LIKE 'pab-%'" -ErrorAction SilentlyContinue |
            Where-Object { $_.ExecutablePath -ieq $binary -or $_.ExecutablePath -ieq $retired } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        try {
            Remove-Item -LiteralPath $retired -Force -ErrorAction Stop
        } catch {
            if ([DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 200
        }
    }
}

$installFull = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
$runtime = Join-Path $installFull 'browser-runtime'
if (-not [IO.Path]::GetFullPath($runtime).StartsWith($installFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Browser runtime path escaped installation directory'
}
if (Test-Path -LiteralPath $runtime) {
    $runtimeNode = Join-Path $runtime 'node.exe'
    Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -ieq $runtimeNode } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $runtime -Recurse -Force
}
$documentRuntime = Join-Path $installFull 'document-runtime'
if (-not [IO.Path]::GetFullPath($documentRuntime).StartsWith($installFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Document runtime path escaped installation directory'
}
if (Test-Path -LiteralPath $documentRuntime) { Remove-Item -LiteralPath $documentRuntime -Recurse -Force }

foreach ($name in @(
    'settings.json',
    'run-app.ps1',
    'run-ui.ps1',
    'run-device-ui.ps1',
    'run-executor.ps1',
    'run-session-supervisor.ps1',
    'uninstall.ps1'
)) {
    $path = Join-Path $InstallRoot $name
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
Remove-PabDesktopStartup -InstallRoot $InstallRoot
Write-Output 'Application files removed. Persistent user and machine data were retained.'
Remove-Item -LiteralPath $errorLog -Force -ErrorAction SilentlyContinue
