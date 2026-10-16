param([Parameter(Mandatory=$true)][int]$CallerPid, [string]$InstallerLogPath, [string]$PreviousSetupPath)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-logging.ps1')
Initialize-PabInstallerLog -Action 'recovery' -Path $InstallerLogPath
Set-PabInstallerStage 'recover_previous_setup'
try {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $processes = @(Get-CimInstance Win32_Process -OperationTimeoutSec 5)
    $protected = @{$PID=$true; $CallerPid=$true}
    # Preserve the current installer, its UAC bootstrapper and this helper.
    foreach ($origin in @($PID, $CallerPid)) {
        $ancestor = $origin
        $visited = @{}
        while ($ancestor) {
            if ($visited.ContainsKey($ancestor)) { break }
            $visited[$ancestor] = $true
            $entry = $processes | Where-Object ProcessId -eq $ancestor | Select-Object -First 1
            if (-not $entry -or $entry.ParentProcessId -eq $ancestor) { break }
            $ancestor = [int]$entry.ParentProcessId
            $protected[$ancestor] = $true
        }
    }
    $owners = @{}
    foreach ($entry in $processes) {
        if ($watch.Elapsed.TotalSeconds -gt 20) { throw 'Previous setup discovery exceeded 20 seconds' }
        if ($protected.ContainsKey([int]$entry.ProcessId) -or -not $entry.ExecutablePath -or
            $entry.ExecutablePath.StartsWith('\\')) { continue }
        if ($PreviousSetupPath -and $entry.ExecutablePath -ine $PreviousSetupPath) { continue }
        try {
            $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($entry.ExecutablePath)
            if ($version.ProductName -eq 'Pixels Agent Bridge' -and
                $version.FileDescription -match '\bInstaller$') {
                $owners[[int]$entry.ProcessId] = $entry
            }
        } catch { }
    }
    $targets = @{} + $owners
    # Snapshot already spawned helpers before terminating their parents, so old
    # PowerShell installers cannot continue replacing files after NSIS exits.
    do {
        $added = $false
        foreach ($entry in $processes) {
            $id = [int]$entry.ProcessId
            if (-not $protected.ContainsKey($id) -and -not $targets.ContainsKey($id) -and
                $targets.ContainsKey([int]$entry.ParentProcessId) -and
                $entry.CreationDate -ge $targets[[int]$entry.ParentProcessId].CreationDate) {
                $targets[$id] = $entry
                $added = $true
            }
        }
    } while ($added)
    foreach ($entry in @($targets.Values | Sort-Object CreationDate)) {
        if ($watch.Elapsed.TotalSeconds -gt 20) { throw 'Previous setup recovery exceeded 20 seconds' }
        $live = Get-Process -Id $entry.ProcessId -ErrorAction SilentlyContinue
        if (-not $live) { continue }
        # CIM timestamps have microsecond precision; process times have 100ns.
        if ([Math]::Abs(($live.StartTime.ToUniversalTime() - $entry.CreationDate.ToUniversalTime()).Ticks) -ge 10) {
            throw "Process identity changed; refusing to stop PID $($entry.ProcessId)"
        }
        Write-PabInstallerLog "terminate_verified_previous_setup pid=$($entry.ProcessId)"
        Stop-Process -InputObject $live -Force -ErrorAction Stop
        if (-not $live.WaitForExit(2000)) { throw "Previous setup did not exit: $($entry.ProcessId)" }
    }
    Write-PabInstallerLog "recovery_completed previous_setups=$($owners.Count)"
} catch {
    Write-PabInstallerFailure -Failure $_ -LegacyLog (Join-Path $env:ProgramData 'PixelsAgentBridge\logs\install-error.log')
    Stop-PabInstallerLog
    exit 1
}
Stop-PabInstallerLog
