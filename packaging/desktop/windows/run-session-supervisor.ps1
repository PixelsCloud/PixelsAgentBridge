$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$env:PAB_DATA_DIR = Join-Path $env:ProgramData 'PixelsAgentBridge'
$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = Join-Path $root 'pab-desktop.exe'
$startInfo.Arguments = '--session-supervisor'
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($startInfo)
if ($null -eq $process) {
    throw 'Could not start session supervisor'
}
$process.WaitForExit()
exit $process.ExitCode
