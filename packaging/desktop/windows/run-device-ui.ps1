$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Open the local device window from an elevated PowerShell window'
}

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$settings = Get-Content -LiteralPath (Join-Path $root 'settings.json') -Raw | ConvertFrom-Json
$env:PAB_DATA_DIR = Join-Path $env:ProgramData 'PixelsAgentBridge'
$env:PAB_DEPLOYMENT_ID = $settings.deployment_id
$env:PAB_CONTROL_URL = $settings.control_url
$env:PAB_RELAY_URLS = $settings.relay_urls
& (Join-Path $root 'pab-desktop.exe')
exit $LASTEXITCODE
