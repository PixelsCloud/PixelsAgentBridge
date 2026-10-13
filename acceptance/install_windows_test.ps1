param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$ExpectedSha256,
    [Parameter(Mandatory = $true)][string]$ExpectedExecutorSha256,
    [Parameter(Mandatory = $true)][string]$ExpectedMcpSha256,
    [Parameter(Mandatory = $true)][string]$ResultFile,
    [int]$TimeoutSeconds = 300
)

$ErrorActionPreference = 'Stop'
$result = [ordered]@{
    started_at = (Get-Date).ToString('o')
    installer = $Installer
    installer_sha256 = $null
    old_version = $null
    new_version = $null
    executor_sha256 = $null
    mcp_sha256 = $null
    installer_exit_code = $null
    service_state = $null
    success = $false
    error = $null
}
$binary = 'C:\Program Files\PixelsAgentBridge\pab-executor.exe'
$mcpBinary = 'C:\Program Files\PixelsAgentBridge\pab-mcp.exe'
try {
    if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
        throw "Installer missing: $Installer"
    }
    $result.installer_sha256 = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($result.installer_sha256 -ne $ExpectedSha256.ToLowerInvariant()) {
        throw 'Installer SHA-256 mismatch'
    }
    if (Test-Path -LiteralPath $binary) {
        $result.old_version = (Get-Item -LiteralPath $binary).VersionInfo.ProductVersion
    }
    $existing = @(Get-CimInstance Win32_Process | Where-Object {
        $_.ProcessId -ne $PID -and $_.CommandLine -and (
            $_.CommandLine -like "*$Installer*" -or
            ($_.Name -eq 'Un.exe' -and $_.CommandLine -like '*PixelsAgentBridge*')
        )
    })
    if ($existing.Count -gt 0) {
        throw "An installer or uninstaller is already running: $($existing.ProcessId -join ',')"
    }
    $process = Start-Process -FilePath $Installer -ArgumentList '/S' -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "Installer timed out after $TimeoutSeconds seconds (PID $($process.Id))"
    }
    $result.installer_exit_code = $process.ExitCode
    if ($process.ExitCode -ne 0) {
        throw "Installer exited with code $($process.ExitCode)"
    }
    if (Test-Path -LiteralPath $binary) {
        $result.new_version = (Get-Item -LiteralPath $binary).VersionInfo.ProductVersion
        $result.executor_sha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if ($result.executor_sha256 -ne $ExpectedExecutorSha256.ToLowerInvariant()) {
        throw 'Installed Executor SHA-256 does not match the tested binary'
    }
    if (Test-Path -LiteralPath $mcpBinary) {
        $result.mcp_sha256 = (Get-FileHash -LiteralPath $mcpBinary -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if ($result.mcp_sha256 -ne $ExpectedMcpSha256.ToLowerInvariant()) {
        throw 'Installed MCP SHA-256 does not match the tested binary'
    }
    $service = Get-Service PixelsAgentBridgeExecutor -ErrorAction SilentlyContinue
    $result.service_state = if ($service) { $service.Status.ToString() } else { 'Missing' }
    if ($result.service_state -ne 'Running') {
        throw "Service is $($result.service_state) after setup"
    }
    $result.success = $true
} catch {
    $result.error = $_.Exception.Message
    try {
        $service = Get-Service PixelsAgentBridgeExecutor -ErrorAction SilentlyContinue
        if ($service -and $service.Status -ne 'Running' -and (Test-Path -LiteralPath $binary)) {
            Start-Service PixelsAgentBridgeExecutor -ErrorAction Stop
        }
        $service = Get-Service PixelsAgentBridgeExecutor -ErrorAction SilentlyContinue
        $result.service_state = if ($service) { $service.Status.ToString() } else { 'Missing' }
    } catch {
        $result.error += "; recovery: $($_.Exception.Message)"
    }
} finally {
    $result.finished_at = (Get-Date).ToString('o')
    New-Item -ItemType Directory -Path (Split-Path -Parent $ResultFile) -Force | Out-Null
    $result | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $ResultFile -Encoding UTF8
}
