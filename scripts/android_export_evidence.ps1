param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][Guid]$RunId,
    [Parameter(Mandatory = $true)][Guid]$ExportId
)
$ErrorActionPreference = 'Stop'
$resolved = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\')
if ((Split-Path -Leaf $resolved) -ne $RunId.ToString()) {
    throw 'The evidence root does not match the isolated run.'
}
$allowed = [System.Collections.Generic.List[System.IO.FileInfo]]::new()
foreach ($name in @('network.json', 'operator-environment.json', 'session.json', 'old-password-check.json', 'upgrade-proof-v14.json')) {
    $path = Join-Path $resolved $name
    if (Test-Path -LiteralPath $path -PathType Leaf) { $allowed.Add((Get-Item -LiteralPath $path)) }
}
Get-ChildItem -LiteralPath $resolved -Filter 'artifact-transition-*.json' -File | ForEach-Object {
    $allowed.Add($_)
}
Get-ChildItem -LiteralPath $resolved -Directory | Where-Object {
    $_.Name -match '^(campaign|soak)-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
} | ForEach-Object {
    foreach ($name in @('result.json', 'progress.json', 'operation-trace.jsonl', 'standby-trace.jsonl')) {
        $path = Join-Path $_.FullName $name
        if (Test-Path -LiteralPath $path -PathType Leaf) { $allowed.Add((Get-Item -LiteralPath $path)) }
    }
}
$entries = @($allowed | ForEach-Object {
    if (-not $_.FullName.StartsWith($resolved + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'An evidence file escaped the isolated root.'
    }
    @{ path = $_.FullName.Substring($resolved.Length + 1).Replace('\', '/');
       size = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
})
$manifest = @{ schema_version = 1; run_id = $RunId.ToString(); export_id = $ExportId.ToString();
    observed_at_utc = [DateTime]::UtcNow.ToString('o'); hostname = $env:COMPUTERNAME;
    installed_application_updated = $false; credential_files_included = $false;
    mcp_sha256 = (Get-FileHash -LiteralPath (Join-Path $resolved 'pab-mcp.exe') -Algorithm SHA256).Hash.ToLowerInvariant();
    entries = $entries }
$manifestPath = Join-Path $resolved ('export-' + $ExportId.ToString() + '.json')
[IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$archive = Join-Path $resolved ('export-' + $ExportId.ToString() + '.zip')
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.IO.Compression
$zip = [IO.Compression.ZipFile]::Open($archive, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in $allowed) {
        $relative = $file.FullName.Substring($resolved.Length + 1).Replace('\', '/')
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $file.FullName, $relative) | Out-Null
    }
    [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $manifestPath, 'export-manifest.json') | Out-Null
} finally { $zip.Dispose() }
@{ status = 'passed'; archive = $archive; entries = $entries.Count;
   sha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() } | ConvertTo-Json -Compress
