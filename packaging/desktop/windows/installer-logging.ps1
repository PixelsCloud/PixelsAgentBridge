# Shared by EXE and ZIP installation. Never log credentials or configuration contents.
function Initialize-PabInstallerLog {
    param([string]$Action, [string]$Path)
    $script:PabInstallerStage = 'initializing'
    $script:PabInstallerLog = $null
    $script:PabInstallerTranscript = $false
    $roots = @((Join-Path $env:ProgramData 'PixelsAgentBridge\logs\setup'),
        (Join-Path ([IO.Path]::GetTempPath()) 'PixelsAgentBridge-InstallerLogs'))
    foreach ($root in $roots) {
        try {
            $candidate = $Path
            if (-not $candidate) {
                $name = '{0}-{1}-{2}.log' -f $Action, [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ'), [Guid]::NewGuid().ToString('N')
                $candidate = Join-Path $root $name
            }
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $candidate) | Out-Null
            [IO.File]::AppendAllText($candidate, '', [Text.Encoding]::Unicode)
            $script:PabInstallerLog = $candidate
            break
        } catch { $Path = $null }
    }
    if (-not $script:PabInstallerLog) {
        Write-Warning 'Could not create an installer log in ProgramData or TEMP.'
        return
    }
    Write-Output "Installer log: $script:PabInstallerLog"
    Write-PabInstallerLog "powershell_start action=$Action pid=$PID"
    try {
        Start-Transcript -LiteralPath ($script:PabInstallerLog + '.powershell.log') -Append -Force | Out-Null
        $script:PabInstallerTranscript = $true
    } catch { Write-PabInstallerLog 'powershell_transcript_unavailable' }
}

function Write-PabInstallerLog {
    param([string]$Message)
    if (-not $script:PabInstallerLog) { return }
    try {
        $line = '{0} [powershell] stage={1} {2}{3}' -f [DateTime]::UtcNow.ToString('o'), $script:PabInstallerStage, $Message, [Environment]::NewLine
        [IO.File]::AppendAllText($script:PabInstallerLog, $line, [Text.Encoding]::Unicode)
    } catch { Write-Warning 'Could not append to the installer log.' }
}

function Set-PabInstallerStage {
    param([string]$Stage)
    $script:PabInstallerStage = $Stage
    Write-PabInstallerLog 'started'
}

function Invoke-PabInstallerNativeCommand {
    param([string]$FilePath, [string[]]$ArgumentList, [ValidateRange(1, 600)][int]$TimeoutSeconds = 60)
    $command = Get-Command -Name $FilePath -CommandType Application -ErrorAction Stop
    $quoted = foreach ($argument in $ArgumentList) {
        # Windows CommandLineToArgvW quoting, including quotes and trailing slashes.
        $escaped = [regex]::Replace($argument, '(\\*)"', '$1$1\"')
        $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
        '"' + $escaped + '"'
    }
    $process = New-Object Diagnostics.Process
    $process.StartInfo.FileName = $command.Source
    $process.StartInfo.Arguments = $quoted -join ' '
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    $process.StartInfo.RedirectStandardOutput = $true
    $process.StartInfo.RedirectStandardError = $true
    # Executor emits UTF-8. Windows service/ACL utilities use the console code page.
    $encoding = if ([IO.Path]::GetFileName($command.Source) -like 'pab-*') { [Text.Encoding]::UTF8 } else { [Console]::OutputEncoding }
    $process.StartInfo.StandardOutputEncoding = $encoding
    $process.StartInfo.StandardErrorEncoding = $encoding
    try {
        if (-not $process.Start()) { throw "Could not start installer command: $FilePath" }
        # Drain both streams concurrently; waiting before reading can deadlock.
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
            try { $process.Kill() } catch { }
            $null = $process.WaitForExit(2000)
        }
        foreach ($stream in @($stdout, $stderr)) {
            if ($stream.Wait(2000)) {
                foreach ($line in ($stream.Result -split '\r?\n')) {
                    if ($line) { Write-PabInstallerLog ('native_output=' + $line); Write-Host $line }
                }
            } elseif (-not $timedOut) {
                throw "Installer command output did not close: $FilePath"
            }
        }
        if ($timedOut) {
            Write-PabInstallerLog "native_timeout_seconds=$TimeoutSeconds"
            throw "Installer command exceeded $TimeoutSeconds seconds and was terminated: $FilePath"
        }
        Write-PabInstallerLog "native_exit_code=$($process.ExitCode)"
        return [int]$process.ExitCode
    } finally { $process.Dispose() }
}

function Enable-PabServiceSasPolicy {
    $path = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Policies\System'
    try {
        $current = (Get-ItemProperty -LiteralPath $path -Name SoftwareSASGeneration -ErrorAction SilentlyContinue).SoftwareSASGeneration
        if ($current -in @(1, 3)) { return }
        New-ItemProperty -LiteralPath $path -Name SoftwareSASGeneration -PropertyType DWord -Value 1 -Force -ErrorAction Stop | Out-Null
        $current = (Get-ItemProperty -LiteralPath $path -Name SoftwareSASGeneration -ErrorAction Stop).SoftwareSASGeneration
        if ($current -notin @(1, 3)) { throw 'The policy change was not retained' }
        Write-PabInstallerLog 'sas_policy_enabled value=1'
    } catch {
        # This policy controls remote Ctrl+Alt+Delete, not installation or normal
        # remote operation. A domain policy must not block the entire upgrade.
        Write-PabInstallerLog ('sas_policy_unavailable installation_continues error=' + $_.Exception.Message)
        Write-Warning 'Remote Ctrl+Alt+Delete is unavailable because Windows policy could not be enabled. Installation will continue.'
    }
}

function Stop-PabInstallerLog {
    if ($script:PabInstallerTranscript) {
        try { Stop-Transcript | Out-Null } catch { }
        $script:PabInstallerTranscript = $false
    }
}

function Write-PabInstallerFailure {
    param($Failure, [string]$LegacyLog)
    Write-PabInstallerLog ('failed error=' + $Failure.ToString())
    Write-PabInstallerLog ('stack=' + $Failure.ScriptStackTrace)
    # Preserve the legacy troubleshooting path as an append-only error history.
    try {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LegacyLog) | Out-Null
        @([DateTime]::UtcNow.ToString('o'), "stage=$script:PabInstallerStage log=$script:PabInstallerLog",
            $Failure.ToString(), $Failure.ScriptStackTrace) | Add-Content -LiteralPath $LegacyLog -Encoding UTF8
    } catch { }
    Write-Output "Installation diagnostics: $script:PabInstallerLog"
}
