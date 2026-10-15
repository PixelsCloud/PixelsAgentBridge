[CmdletBinding()]
param(
    [ValidateSet('Install','Update','Rollback','Restore','Remove','Status')][string]$Action='Install',
    [Parameter(Mandatory)][ValidateSet('Server','Relay')][string]$Component,
    [Parameter(Mandatory)][string]$InstallDirectory,
    [string]$SourceDirectory,
    [string]$ConfigFile,
    [ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedSha256,
    [ValidatePattern('^[a-zA-Z0-9_.-]{1,80}$')][string]$ServiceName
)
$ErrorActionPreference='Stop'
# Local WMI can hang even while the Service Control Manager is responsive.
# Deployment must not depend on WMI providers or an unbounded CIM query.
if (-not ('Pixels.ServerDeployment.ServiceApi' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
namespace Pixels.ServerDeployment {
    public sealed class ServiceInfo {
        public string PathName { get; set; }
        public string State { get; set; }
        public uint ProcessId { get; set; }
    }
    public static class ServiceApi {
        private const uint ManagerConnect = 1, QueryConfig = 1, ChangeConfig = 2, QueryStatus = 4;
        private const uint NoChange = 0xffffffff, AutomaticStart = 2;
        private const int ServiceDoesNotExist = 1060, InsufficientBuffer = 122;
        [StructLayout(LayoutKind.Sequential)] private struct Status {
            public uint Type, State, Controls, Win32Exit, ServiceExit, Checkpoint, WaitHint, ProcessId, Flags;
        }
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] private struct Config {
            public uint Type, StartType, ErrorControl;
            public IntPtr BinaryPath, Group;
            public uint Tag;
            public IntPtr Dependencies, Account, DisplayName;
        }
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        private static extern IntPtr OpenSCManager(string machine, string database, uint access);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        private static extern IntPtr OpenService(IntPtr manager, string name, uint access);
        [DllImport("advapi32.dll", SetLastError=true)] private static extern bool CloseServiceHandle(IntPtr handle);
        [DllImport("advapi32.dll", SetLastError=true)]
        private static extern bool QueryServiceStatusEx(IntPtr service, int level, out Status status, int size, out int needed);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        private static extern bool QueryServiceConfig(IntPtr service, IntPtr config, int size, out int needed);
        [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        private static extern bool ChangeServiceConfig(IntPtr service, uint type, uint start, uint error,
            string binary, string group, IntPtr tag, string dependencies, string account, string password, string display);
        [DllImport("kernel32.dll", SetLastError=true)] private static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        private static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder path, ref int size);
        [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
        private static IntPtr Open(string name, uint access, bool allowMissing) {
            IntPtr manager = OpenSCManager(null, null, ManagerConnect);
            if (manager == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            try {
                IntPtr service = OpenService(manager, name, access);
                if (service == IntPtr.Zero) {
                    int error = Marshal.GetLastWin32Error();
                    if (!allowMissing || error != ServiceDoesNotExist) throw new Win32Exception(error);
                }
                return service;
            } finally { CloseServiceHandle(manager); }
        }
        public static ServiceInfo Read(string name) {
            IntPtr service = Open(name, QueryConfig | QueryStatus, true);
            if (service == IntPtr.Zero) return null;
            try {
                Status status; int needed;
                if (!QueryServiceStatusEx(service, 0, out status, Marshal.SizeOf(typeof(Status)), out needed))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                QueryServiceConfig(service, IntPtr.Zero, 0, out needed);
                int error = Marshal.GetLastWin32Error();
                if (error != InsufficientBuffer || needed <= 0 || needed > 65536) throw new Win32Exception(error);
                IntPtr buffer = Marshal.AllocHGlobal(needed);
                try {
                    if (!QueryServiceConfig(service, buffer, needed, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
                    Config config = (Config)Marshal.PtrToStructure(buffer, typeof(Config));
                    string[] states = { "Unknown", "Stopped", "Start Pending", "Stop Pending", "Running", "Continue Pending", "Pause Pending", "Paused" };
                    return new ServiceInfo { PathName = Marshal.PtrToStringUni(config.BinaryPath), ProcessId = status.ProcessId,
                        State = status.State < states.Length ? states[status.State] : "Unknown" };
                } finally { Marshal.FreeHGlobal(buffer); }
            } finally { CloseServiceHandle(service); }
        }
        public static void SetImage(string name, string image) {
            IntPtr service = Open(name, ChangeConfig, false);
            try {
                if (!ChangeServiceConfig(service, NoChange, AutomaticStart, NoChange, image, null, IntPtr.Zero, null, null, null, null))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
            } finally { CloseServiceHandle(service); }
        }
        public static string ProcessImage(uint pid) {
            // MainModule/PowerShell's Process.Path need broader process rights
            // on .NET Framework. Limited query also works for a service account.
            IntPtr process = OpenProcess(0x1000, false, pid);
            if (process == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            try {
                int size = 32768; StringBuilder path = new StringBuilder(size);
                if (!QueryFullProcessImageName(process, 0, path, ref size)) throw new Win32Exception(Marshal.GetLastWin32Error());
                return path.ToString();
            } finally { CloseHandle(process); }
        }
    }
}
'@
}
if (-not $ServiceName) { $ServiceName=if($Component -eq 'Server'){'PixelsAgentBridgeServer'}else{'PixelsAgentBridgeRelay'} }
if ($ServiceName -ieq 'PixelsAgentBridgeExecutor') { throw 'The existing Executor service must never be modified by this installer.' }
$taskInstallRoot=[IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\')
if ($taskInstallRoot -eq [IO.Path]::GetPathRoot($taskInstallRoot).TrimEnd('\')) { throw 'A dedicated installation directory is required.' }
function Assert-OwnedPath([string]$Path) {
    $resolved=[IO.Path]::GetFullPath($Path)
    if (-not $resolved.StartsWith($taskInstallRoot+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Deployment path is outside the named installation directory.' }
    return $resolved
}
function File-Sha256([string]$Path) {
    $stream=[IO.File]::OpenRead($Path)
    $algorithm=[Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-','').ToLowerInvariant() }
    finally { $stream.Dispose();$algorithm.Dispose() }
}
function Image-Binary([string]$ImagePath) {
    if ($ImagePath -match '^"([^"]+)"') { return Assert-OwnedPath $Matches[1] }
    if ($ImagePath -match '^([^\s"]+\.exe)(?:\s|$)') { return Assert-OwnedPath $Matches[1] }
    throw 'Existing service has an unrecognized image path; refusing to modify it.'
}
$statePath=Assert-OwnedPath (Join-Path $taskInstallRoot ($ServiceName+'.deployment.json'))
$state=if(Test-Path -LiteralPath $statePath){Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json}else{$null}
$service=[Pixels.ServerDeployment.ServiceApi]::Read($ServiceName)
if ($service) {
    $null=Image-Binary $service.PathName
}
if ($Action -eq 'Status') {
    $busy=$false;$probeMutex=$null;$acquired=$false
    try {
        $probeMutex=[Threading.Mutex]::OpenExisting('Global\PixelsServerDeploy-'+$ServiceName)
        try {$acquired=$probeMutex.WaitOne(0)} catch [Threading.AbandonedMutexException] {$acquired=$true}
        $busy=-not $acquired
    } catch [Threading.WaitHandleCannotBeOpenedException] {} finally {
        if ($acquired) {$probeMutex.ReleaseMutex()}
        if ($probeMutex) {$probeMutex.Dispose()}
    }
    $runningBinary=$null;$runningHash=$null;$releaseId=$null
    if ($service -and $service.ProcessId -gt 0) {
        $processPath=[Pixels.ServerDeployment.ServiceApi]::ProcessImage($service.ProcessId)
        if ($processPath) {
            $runningBinary=Assert-OwnedPath $processPath
            $runningHash=File-Sha256 $runningBinary
            $releaseId=Split-Path -Leaf (Split-Path -Parent $runningBinary)
        }
    }
    [pscustomobject]@{service=$ServiceName;status=if($service){$service.State}else{'NotInstalled'};deployment=$state;installer_active=$busy;running_binary=$runningBinary;running_sha256=$runningHash;release_id=$releaseId} | ConvertTo-Json -Depth 6
    return
}
$mutex=[Threading.Mutex]::new($false,('Global\PixelsServerDeploy-'+$ServiceName))
$lockAcquired=$false
try { $lockAcquired=$mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $lockAcquired=$true }
if (-not $lockAcquired) { $mutex.Dispose();throw 'Another installer is already working on this service.' }
function Invoke-Sc([string[]]$Arguments) {
    $text=& sc.exe @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw ('Service manager operation failed: '+($text -join ' ')) }
}
function Stop-OwnedService {
    $current=[Pixels.ServerDeployment.ServiceApi]::Read($ServiceName)
    if (-not $current -or $current.State -eq 'Stopped') { return }
    $controller=Get-Service -Name $ServiceName
    $controller.Stop()
    try { (Get-Service -Name $ServiceName).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(10)) }
    catch {
        $current=[Pixels.ServerDeployment.ServiceApi]::Read($ServiceName)
        if ($current.ProcessId -gt 0) {
            $processPath=[Pixels.ServerDeployment.ServiceApi]::ProcessImage($current.ProcessId)
            if (-not $processPath) { throw 'Cannot verify the stuck service process.' }
            $null=Assert-OwnedPath $processPath
            Stop-Process -Id $current.ProcessId -Force
            (Get-Service -Name $ServiceName).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(3))
        }
    }
}
function Restore-Web([string]$ImagePath) {
    if ($Component -ne 'Server') { return }
    $binaryPath=Image-Binary $ImagePath
    $webSource=Join-Path (Split-Path -Parent $binaryPath) 'web'
    $webTarget=Assert-OwnedPath (Join-Path $taskInstallRoot 'web')
    if (-not (Test-Path -LiteralPath (Join-Path $webSource 'index.html'))) { throw 'Versioned Web resources are missing.' }
    New-Item -ItemType Directory -Path $webTarget -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $webSource 'index.html') -Destination $webTarget -Force
    Get-ChildItem -LiteralPath $webSource -Force | Where-Object Name -ne 'index.html' | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $webTarget -Recurse -Force
    }
}
function Start-Image([string]$ImagePath) {
    $null=Image-Binary $ImagePath
    Restore-Web $ImagePath
    $existing=[Pixels.ServerDeployment.ServiceApi]::Read($ServiceName)
    if ($existing) {
        [Pixels.ServerDeployment.ServiceApi]::SetImage($ServiceName,$ImagePath)
    } else {
        New-Service -Name $ServiceName -BinaryPathName $ImagePath -StartupType Automatic -DisplayName ('Pixels '+$Component+' Server') | Out-Null
    }
    Invoke-Sc -Arguments @('failure',$ServiceName,'reset=','86400','actions=','restart/5000/restart/15000/restart/30000')
    Invoke-Sc -Arguments @('failureflag',$ServiceName,'1')
    Start-Service -Name $ServiceName
    (Get-Service -Name $ServiceName).WaitForStatus('Running',[TimeSpan]::FromSeconds(10))
}
try {
    if ($Action -eq 'Remove') {
        Stop-OwnedService
        if ($service) { Invoke-Sc -Arguments @('delete',$ServiceName) }
        Write-Output 'Service removed. Configuration, data, logs and versioned files were retained.'
        return
    }
    if ($Action -in @('Rollback','Restore')) {
        $selected=$state.previous_image
        if ($Action -eq 'Restore') {
            if (-not $ExpectedSha256) {throw 'Restore requires ExpectedSha256.'}
            $selected=$null
            foreach ($candidate in @($state.current_image,$state.previous_image)) {
                if ($candidate -and (File-Sha256 (Image-Binary $candidate)) -eq $ExpectedSha256) {$selected=$candidate;break}
            }
        }
        if (-not $selected) { throw 'The requested recovery version is not recorded.' }
        Stop-OwnedService
        Start-Image $selected
        if ($selected -ne $state.current_image) {$previous=$state.current_image;$state.current_image=$selected;$state.previous_image=$previous}
        $state.sha256=File-Sha256 (Image-Binary $selected)
        $state.release_id=Split-Path -Leaf (Split-Path -Parent (Image-Binary $selected))
        $state | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8
        return
    }
    if (-not $SourceDirectory -or -not $ConfigFile) { throw 'SourceDirectory and ConfigFile are required before installing or updating.' }
    if ($Action -eq 'Update' -and -not $service) { throw 'Update requires an installed service.' }
    $sourceRoot=(Resolve-Path -LiteralPath $SourceDirectory).Path
    $configPath=(Resolve-Path -LiteralPath $ConfigFile).Path
    $binaryName=if($Component -eq 'Server'){'pab-server.exe'}else{'pab-relay-server.exe'}
    $sourceBinary=Join-Path $sourceRoot $binaryName
    if (-not (Test-Path -LiteralPath $sourceBinary -PathType Leaf)) { throw 'The selected server executable is missing.' }
    $digest=File-Sha256 $sourceBinary
    $binaryDigest=$digest
    if ($Component -eq 'Server') {
        $assetIndex=Join-Path $sourceRoot 'web\index.html'
        if (-not (Test-Path -LiteralPath $assetIndex)) { throw 'Production Web resources are missing.' }
        $digest+='-'+(File-Sha256 $assetIndex).Substring(0,12)
    }
    $release=Assert-OwnedPath (Join-Path $taskInstallRoot ('releases\'+$digest))
    New-Item -ItemType Directory -Path $release -Force | Out-Null
    $installedBinary=Assert-OwnedPath (Join-Path $release $binaryName)
    if (Test-Path -LiteralPath $installedBinary) {
        if ((File-Sha256 $installedBinary) -ne (File-Sha256 $sourceBinary)) { throw 'The versioned executable has changed unexpectedly.' }
    } else { Copy-Item -LiteralPath $sourceBinary -Destination $installedBinary }
    if ($Component -eq 'Server') { Copy-Item -LiteralPath (Join-Path $sourceRoot 'web') -Destination $release -Recurse -Force }
    $preflightLog=Assert-OwnedPath (Join-Path $taskInstallRoot ($ServiceName+'.preflight.log'))
    & $installedBinary check --config $configPath *> $preflightLog
    if ($LASTEXITCODE -ne 0) { throw ('Configuration preflight failed; service was not stopped. See '+$preflightLog) }
    $arguments=if($Component -eq 'Server'){'serve '}else{''}
    $image='"'+$installedBinary+'" '+$arguments+'--config "'+$configPath+'" --service --service-name '+$ServiceName
    $oldImage=if($service){$service.PathName}else{$null}
    Stop-OwnedService
    try { Start-Image $image }
    catch {
        Stop-OwnedService
        if ($oldImage) { Start-Image $oldImage }
        elseif (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) { Invoke-Sc -Arguments @('delete',$ServiceName) }
        throw
    }
    [ordered]@{service=$ServiceName;component=$Component;current_image=$image;previous_image=$oldImage;sha256=$binaryDigest;release_id=$digest;updated_at=[DateTime]::UtcNow.ToString('o')} |
        ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $statePath -Encoding utf8
    Write-Output ('Installed '+$ServiceName+'; health must be verified through its control connection and a real transfer probe.')
} finally { $mutex.ReleaseMutex();$mutex.Dispose() }
