# Real Windows image-lock / immediate respawn regression. No installed services
# or applications are touched; every executable lives in an isolated directory.
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'install.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
foreach ($name in @('Stop-PabInstalledBinary', 'Install-PabBinaries')) {
    $definition = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $false)
    . ([scriptblock]::Create($definition.Extent.Text))
}
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..\'))
$testRoot = Join-Path $workspace ('.build\installer-respawn-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$testRoot = (Resolve-Path -LiteralPath $testRoot).Path
$watcher = $null
$unrelated = $null
try {
    $sourceRoot = Join-Path $testRoot 'source'
    $installedRoot = Join-Path $testRoot 'installed'
    $otherRoot = Join-Path $testRoot 'other'
    New-Item -ItemType Directory -Path $sourceRoot, $installedRoot, $otherRoot | Out-Null
    $fixtureSource = Join-Path $testRoot 'fixture.cs'
    @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
class Fixture {
    static void Main(string[] args) {
        if (args.Length == 1) File.AppendAllText(args[0], Process.GetCurrentProcess().Id + "\n");
        Thread.Sleep(120000);
    }
}
'@ | Set-Content -LiteralPath $fixtureSource
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    $fixture = Join-Path $sourceRoot 'pab-mcp.exe'
    & $compiler /nologo /target:exe "/out:$fixture" $fixtureSource
    if ($LASTEXITCODE -ne 0) { throw 'Fixture compilation failed' }
    foreach ($name in @('pab-executor.exe', 'pab-desktop.exe')) {
        Copy-Item -LiteralPath $fixture -Destination (Join-Path $sourceRoot $name)
    }
    foreach ($name in @('pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe')) {
        Copy-Item -LiteralPath (Join-Path $sourceRoot $name) -Destination $installedRoot
    }
    Copy-Item -LiteralPath $fixture -Destination (Join-Path $otherRoot 'pab-mcp.exe')
    $unrelated = Start-Process -FilePath (Join-Path $otherRoot 'pab-mcp.exe') -PassThru -WindowStyle Hidden
    $watchScript = Join-Path $testRoot 'watch.ps1'
    @'
param([string]$Binary, [string]$Marker, [string]$StopFile)
while (-not (Test-Path -LiteralPath $StopFile)) {
    if (Test-Path -LiteralPath $Binary) {
        try {
            $p = Start-Process -FilePath $Binary -ArgumentList ('"' + $Marker + '"') -PassThru -WindowStyle Hidden
            $p.WaitForExit()
        } catch {}
    }
    Start-Sleep -Milliseconds 10
}
'@ | Set-Content -LiteralPath $watchScript
    $binary = Join-Path $installedRoot 'pab-mcp.exe'
    $marker = Join-Path $testRoot 'started.txt'
    $stopFile = Join-Path $testRoot 'stop'
    $arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + $watchScript + '" "' + $binary + '" "' + $marker + '" "' + $stopFile + '"'
    $watcher = Start-Process powershell.exe -ArgumentList $arguments -PassThru -WindowStyle Hidden
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path -LiteralPath $marker)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Watchdog did not start fixture' }
        Start-Sleep -Milliseconds 100
    }
    $firstId = [int](Get-Content -LiteralPath $marker | Select-Object -First 1)
    # Reproduce the old installer: kill once, then allow immediate respawn.
    Stop-Process -Id $firstId -Force
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (@(Get-Content -LiteralPath $marker).Count -lt 2) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Watchdog did not respawn fixture' }
        Start-Sleep -Milliseconds 100
    }
    $oldId = [int](Get-Content -LiteralPath $marker | Select-Object -Last 1)
    # Different bytes prove the old locked executable was actually replaced.
    $append = [IO.File]::Open($fixture, [IO.FileMode]::Append)
    $append.WriteByte(0)
    $append.Dispose()
    Install-PabBinaries -SourceRoot $sourceRoot -DestinationRoot $installedRoot
    if ((Get-FileHash -LiteralPath $binary).Hash -ne (Get-FileHash -LiteralPath $fixture).Hash) {
        throw 'Installed file does not match new source'
    }
    if (Get-Process -Id $oldId -ErrorAction SilentlyContinue) { throw 'Old process still running' }
    $unrelated.Refresh()
    if ($unrelated.HasExited) { throw 'Unrelated same-name executable was stopped' }
    if (Get-ChildItem -LiteralPath $installedRoot -Filter '*.old') { throw 'Retired image left behind' }
    Write-Output 'PASS: immediate MCP respawn upgrade; new file hash; old process exited; other installation untouched'

    # Force a publication failure after retirement and verify rollback preserves
    # the original bytes. Override only Move-Item inside this test scope.
    New-Item -ItemType File -Path $stopFile | Out-Null
    Stop-Process -Id $watcher.Id -Force -ErrorAction SilentlyContinue
    Stop-PabInstalledBinary -Paths @($binary)
    $before = @{}
    foreach ($name in @('pab-mcp.exe', 'pab-executor.exe', 'pab-desktop.exe')) {
        $before[$name] = (Get-FileHash -LiteralPath (Join-Path $installedRoot $name)).Hash
        $append = [IO.File]::Open((Join-Path $sourceRoot $name), [IO.FileMode]::Append)
        $append.WriteByte(1)
        $append.Dispose()
    }
    $script:publications = 0
    function Move-Item {
        param([string]$LiteralPath, [string]$Destination, [string]$ErrorAction)
        if ($LiteralPath.EndsWith('.new')) {
            $script:publications++
            if ($script:publications -eq 2) { throw 'Injected publication failure' }
        }
        Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination -ErrorAction Stop
    }
    $failed = $false
    try { Install-PabBinaries -SourceRoot $sourceRoot -DestinationRoot $installedRoot }
    catch { $failed = $_.Exception.Message -match 'Injected publication failure' }
    finally { Remove-Item Function:\Move-Item }
    if (-not $failed) { throw 'Expected publication failure' }
    foreach ($name in $before.Keys) {
        if ((Get-FileHash -LiteralPath (Join-Path $installedRoot $name)).Hash -ne $before[$name]) {
            throw "Failed installation did not restore original binary: $name"
        }
    }
    Write-Output 'PASS: publication failure restores previous binaries'
} finally {
    if ($watcher) { Stop-Process -Id $watcher.Id -Force -ErrorAction SilentlyContinue }
    Get-CimInstance Win32_Process -Filter "Name LIKE 'pab-%'" |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    if (-not $testRoot.StartsWith($workspace.TrimEnd('\') + '\.build\installer-respawn-', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Unexpected test cleanup path'
    }
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
