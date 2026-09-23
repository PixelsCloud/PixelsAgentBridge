$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$settings = Get-Content -LiteralPath (Join-Path $root 'settings.json') -Raw | ConvertFrom-Json
$env:PAB_DEPLOYMENT_ID = $settings.deployment_id
$env:PAB_CONTROL_URL = $settings.control_url
$env:PAB_RELAY_URLS = $settings.relay_urls
& (Join-Path $root 'pab-desktop.exe')
exit $LASTEXITCODE
