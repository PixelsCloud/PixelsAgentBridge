# Windows PowerShell 5.1. Uses real Windows shortcuts in a disposable directory;
# never changes the installed app, common Startup folder, services or registry.
$ErrorActionPreference = 'Stop'
foreach ($entry in @(
    @{ File = 'install.ps1'; Name = 'Set-PabDesktopStartup' },
    @{ File = 'uninstall.ps1'; Name = 'Remove-PabDesktopStartup' }
)) {
    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $PSScriptRoot $entry.File), [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
    $definition = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $entry.Name
    }, $false)
    if (-not $definition) { throw "Missing function: $($entry.Name)" }
    . ([scriptblock]::Create($definition.Extent.Text))
}
$workspaceBuild = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..\.build'))
$testRoot = Join-Path $workspaceBuild ('desktop-startup-test-' + [Guid]::NewGuid().ToString('N'))
$startup = Join-Path $testRoot 'Startup'
$installOne = Join-Path $testRoot 'Program Files\Pixels One'
$installTwo = Join-Path $testRoot 'Program Files\Pixels Two'
try {
    foreach ($install in @($installOne, $installTwo)) {
        New-Item -ItemType Directory -Path $install -Force | Out-Null
        [IO.File]::WriteAllBytes((Join-Path $install 'pab-desktop.exe'), [byte[]]@(0))
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcutPath = Join-Path $startup 'Pixels Agent Bridge.lnk'
    Set-PabDesktopStartup -InstallRoot $installOne -StartupDirectory $startup
    Set-PabDesktopStartup -InstallRoot $installOne -StartupDirectory $startup
    $shortcut = $shell.CreateShortcut($shortcutPath)
    if ($shortcut.TargetPath -ine (Join-Path $installOne 'pab-desktop.exe') -or
        $shortcut.WorkingDirectory -ine $installOne -or $shortcut.Arguments -ne '' -or
        $shortcut.WindowStyle -ne 1 -or @(Get-ChildItem -LiteralPath $startup).Count -ne 1) {
        throw 'Expected one visible, normal-user desktop startup shortcut with intact spaced paths'
    }
    Set-PabDesktopStartup -InstallRoot $installTwo -StartupDirectory $startup
    $shortcut = $shell.CreateShortcut($shortcutPath)
    if ($shortcut.TargetPath -ine (Join-Path $installTwo 'pab-desktop.exe')) { throw 'Upgrade did not refresh target' }
    $missingRejected = $false
    try { Set-PabDesktopStartup -InstallRoot (Join-Path $testRoot 'Missing') -StartupDirectory $startup }
    catch { $missingRejected = $true }
    if (-not $missingRejected) { throw 'Missing application should not create a broken startup entry' }
    Remove-PabDesktopStartup -InstallRoot $installOne -StartupDirectory $startup
    if (-not (Test-Path -LiteralPath $shortcutPath)) { throw 'Old uninstaller removed another installation startup' }
    Remove-PabDesktopStartup -InstallRoot $installTwo -StartupDirectory $startup
    Remove-PabDesktopStartup -InstallRoot $installTwo -StartupDirectory $startup
    if (Test-Path -LiteralPath $shortcutPath) { throw 'Uninstall left a startup entry' }
    Write-Output 'PASS: visible GUI shortcut, paths with spaces, repeat install, upgrade target, missing binary rejection, ownership-safe and repeat uninstall.'
} finally {
    $resolvedTestRoot = [IO.Path]::GetFullPath($testRoot)
    if (-not $resolvedTestRoot.StartsWith($workspaceBuild + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Test cleanup escaped workspace build directory'
    }
    if (Test-Path -LiteralPath $resolvedTestRoot) { Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force }
}
