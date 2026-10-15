# Run elevated with Windows PowerShell 5.1, as used by the NSIS installer.
# Registers only a disposable stopped service; never touches installed Pixels.
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'install.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$definition = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Set-PabServiceConfiguration'
}, $false)
. ([scriptblock]::Create($definition.Extent.Text))
$name = 'PabInstallerConfigTest' + [Guid]::NewGuid().ToString('N')
$oldCommand = '"C:\Program Files\Pixels Test\pab-executor.exe" --service'
$newCommand = $oldCommand + ' --config "C:\Program Files\Pixels Test\config.toml"'
try {
    New-Service -Name $name -BinaryPathName $oldCommand -StartupType Manual | Out-Null
    # Reproduce the exact previously shipped invocation without running a service.
    $legacyOutput = & sc.exe config $name binPath= $newCommand obj= LocalSystem start= auto 2>&1
    $legacyExit = $LASTEXITCODE
    if ($PSVersionTable.PSVersion.Major -eq 5 -and $legacyExit -eq 0) {
        throw 'Expected the legacy sc.exe quoting regression under Windows PowerShell 5.1'
    }
    Set-PabServiceConfiguration -Name $name -BinaryPath $newCommand
    Set-PabServiceConfiguration -Name $name -BinaryPath $newCommand
    $actual = Get-CimInstance Win32_Service -Filter "Name = '$name'"
    if ($actual.PathName -cne $newCommand -or $actual.StartMode -ne 'Auto' -or $actual.StartName -ne 'LocalSystem' -or $actual.State -ne 'Stopped') {
        throw 'Service command, quoting, startup mode, account, or state was not preserved'
    }
    # Also cover an existing service registered under another built-in account.
    $changed = Invoke-CimMethod -InputObject $actual -MethodName Change -Arguments @{
        StartName = 'NT AUTHORITY\LocalService'; StartPassword = ''; StartMode = 'Manual'
    }
    if ($changed.ReturnValue -ne 0) { throw 'Could not prepare account-change fixture' }
    Set-PabServiceConfiguration -Name $name -BinaryPath $newCommand
    $missingRejected = $false
    try { Set-PabServiceConfiguration -Name ($name + 'Missing') -BinaryPath $newCommand }
    catch { $missingRejected = $true }
    if (-not $missingRejected) { throw 'Missing service was not rejected' }
    Write-Output "PASS: legacy sc.exe exit=$legacyExit; quoted path migration, repeat install, account/startup migration, and missing-service rejection."
} finally {
    $service = Get-CimInstance Win32_Service -Filter "Name = '$name'" -ErrorAction SilentlyContinue
    if ($null -ne $service) {
        $deleted = Invoke-CimMethod -InputObject $service -MethodName Delete
        if ($deleted.ReturnValue -ne 0) { throw "Could not remove test service $name" }
    }
}
