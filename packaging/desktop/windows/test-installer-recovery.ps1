# Elevated Windows PowerShell 5.1. Only disposable files, processes and one
# uniquely named service are used. Never updates the installed Pixels app.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-logging.ps1')
$build = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..\.build'))
$root = Join-Path $build ('installer-recovery-' + [Guid]::NewGuid().ToString('N'))
$name = 'PixelsRecoveryTest' + [Guid]::NewGuid().ToString('N')
$serviceCreated = $false
$runtimeProcesses = @()
New-Item -ItemType Directory -Path $root | Out-Null
try {
    Initialize-PabInstallerLog -Action 'test' -Path (Join-Path $root 'test.log')
    Set-PabInstallerStage 'test_policy'
    & {
        function Get-ItemProperty {
            param($LiteralPath, $Name, $ErrorAction)
            [pscustomobject]@{ SoftwareSASGeneration = $script:fixturePolicy }
        }
        function New-ItemProperty {
            param($LiteralPath, $Name, $PropertyType, $Value, [switch]$Force, $ErrorAction)
            $script:fixtureWrites++
            if ($script:fixtureDenied) { throw 'Fixture policy denied' }
            $script:fixturePolicy = $Value
        }
        foreach ($policy in @($null, 0, 1, 2, 3)) {
            $script:fixturePolicy = $policy
            $script:fixtureWrites = 0
            $script:fixtureDenied = $false
            Enable-PabServiceSasPolicy
            $expected = if ($policy -in @(1, 3)) { $policy } else { 1 }
            if ($script:fixturePolicy -ne $expected) { throw 'Policy was not prepared automatically' }
            if ($policy -in @(1, 3) -and $script:fixtureWrites) { throw 'Already valid policy was changed' }
        }
        $script:fixturePolicy = $null
        $script:fixtureDenied = $true
        Enable-PabServiceSasPolicy
        if ($script:fixturePolicy) { throw 'A denied policy was reported as enabled' }
        Write-Output 'PASS: missing/disabled SAS policy repaired; values 1/3 retained; denied policy does not abort.'
    }
    $exe = Join-Path $root 'pab-executor.exe'
    $source = @'
using System;
using System.IO;
using System.ServiceProcess;
using System.Text;
using System.Threading;
class Fixture : ServiceBase {
    string marker;
    Fixture(string name, string marker) { ServiceName=name; CanStop=true; this.marker=marker; }
    protected override void OnStop() { File.WriteAllText(marker,"stopping"); Thread.Sleep(60000); }
    static void Main(string[] args) {
        if(args[0]=="--wait") { Thread.Sleep(60000); return; }
        if(args[0]=="--args") {
            Console.OutputEncoding=new UTF8Encoding(false);
            Console.WriteLine("argc="+(args.Length-1));
            for(int i=1;i<args.Length;i++) Console.WriteLine(Convert.ToBase64String(Encoding.UTF8.GetBytes(args[i])));
            return;
        }
        ServiceBase.Run(new Fixture(args[0],args[1]));
    }
}
'@
    Add-Type -TypeDefinition $source -ReferencedAssemblies 'System.ServiceProcess.dll' -OutputAssembly $exe -OutputType ConsoleApplication
    $arguments = @('', 'a b', 'a"b', 'C:\path with spaces\', 'slashes\\"quote', '中文')
    $exit = Invoke-PabInstallerNativeCommand -FilePath $exe -ArgumentList (@('--args') + $arguments)
    if ($exit -ne 0) { throw 'Argument fixture failed' }
    $log = [IO.File]::ReadAllText((Join-Path $root 'test.log'), [Text.Encoding]::Unicode)
    if (-not $log.Contains('native_output=argc=6')) { throw 'An empty or quoted argument was lost' }
    # Empty output is intentionally not logged. The other arguments must survive
    # exactly, including embedded quotes, Unicode and trailing backslashes.
    foreach ($argument in $arguments | Where-Object { $_ }) {
        $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($argument))
        if (-not $log.Contains('native_output=' + $encoded)) { throw "Argument changed during process launch: $argument" }
    }
    Write-Output 'PASS: native process argument quoting, spaces, quotes, backslashes and Unicode.'

    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'install.ps1'), [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
    $stopBlock = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.IfStatementAst] -and
        $node.Extent.Text.StartsWith('if ($existingService)') -and $node.Extent.Text.Contains("'sc.exe'")
    }, $true)
    if (-not $stopBlock) { throw 'Production service stop block not found' }
    $marker = Join-Path $root 'stop-requested.txt'
    $binaryPath = '"' + $exe + '" ' + $name + ' "' + $marker + '"'
    New-Service -Name $name -BinaryPathName $binaryPath -StartupType Manual | Out-Null
    $serviceCreated = $true
    Start-Service -Name $name
    $taskName = $name
    $InstallRoot = $root
    $existingService = Get-Service -Name $name
    $processIdBefore = (Get-CimInstance Win32_Service -Filter "Name = '$name'").ProcessId
    $watch = [Diagnostics.Stopwatch]::StartNew()
    & ([scriptblock]::Create($stopBlock.Extent.Text))
    if ($watch.Elapsed.TotalSeconds -gt 40) { throw 'Service recovery exceeded its time budget' }
    if (-not (Test-Path -LiteralPath $marker)) { throw 'Stuck OnStop fixture was never requested to stop' }
    if ((Get-Service -Name $name).Status -ne 'Stopped') { throw 'Unresponsive service was not stopped' }
    if (Get-Process -Id $processIdBefore -ErrorAction SilentlyContinue) { throw 'Unresponsive verified service process survived' }
    Write-Output 'PASS: real unresponsive Windows service forcibly stopped within a bounded deadline.'
    foreach ($functionName in @('Stop-PabInstalledBinary', 'Stop-PabRuntimeProcesses')) {
        $definition = $ast.Find({ param($node)
            $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
        }, $true)
        . ([scriptblock]::Create($definition.Extent.Text))
    }
    $node = Join-Path $root 'browser-runtime\node.exe'
    $otherNode = Join-Path $root 'other\browser-runtime\node.exe'
    $worker = Join-Path $root 'pab-document-worker.exe'
    foreach ($path in @($node, $otherNode, $worker)) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        Copy-Item -LiteralPath $exe -Destination $path
        $runtimeProcesses += Start-Process -FilePath $path -ArgumentList '--wait' -PassThru -WindowStyle Hidden
    }
    Stop-PabRuntimeProcesses -InstallRoot $root
    foreach ($process in $runtimeProcesses) { $process.Refresh() }
    if (-not $runtimeProcesses[0].HasExited -or -not $runtimeProcesses[2].HasExited) { throw 'Runtime process still holds upgrade files' }
    if ($runtimeProcesses[1].HasExited) { throw 'Another installation runtime was terminated' }
    Write-Output 'PASS: browser/document runtime processes stopped; another installation remains running.'
} finally {
    foreach ($process in $runtimeProcesses) {
        if (-not $process.HasExited) { Stop-Process -InputObject $process -Force -ErrorAction SilentlyContinue }
        $null = $process.WaitForExit(2000)
        $process.Dispose()
    }
    if ($serviceCreated) {
        $entry = Get-CimInstance Win32_Service -Filter "Name = '$name'" -ErrorAction SilentlyContinue
        if ($entry -and $entry.ProcessId -gt 0) {
            $live = Get-CimInstance Win32_Process -Filter "ProcessId = $($entry.ProcessId)"
            if ($live.ExecutablePath -ieq (Join-Path $root 'pab-executor.exe')) {
                Stop-Process -Id $entry.ProcessId -Force -ErrorAction SilentlyContinue
            }
        }
        if ($entry) { $null = Invoke-CimMethod -InputObject $entry -MethodName Delete }
    }
    Stop-PabInstallerLog
    $resolved = [IO.Path]::GetFullPath($root)
    if (-not $resolved.StartsWith($build + '\installer-recovery-', [StringComparison]::OrdinalIgnoreCase)) { throw 'Test cleanup escaped workspace' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
