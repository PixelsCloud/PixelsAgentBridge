$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$osInfo = Get-CimInstance Win32_OperatingSystem
$videoInfo = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, Status, VideoProcessor, CurrentHorizontalResolution, CurrentVerticalResolution)
$toolsFound = @{}
foreach ($toolName in @('cargo', 'rustc', 'cmake', 'clang', 'ffmpeg', 'nvidia-smi')) {
    $commandInfo = Get-Command $toolName -ErrorAction SilentlyContinue
    $toolsFound[$toolName] = [bool]$commandInfo
}
$compilers = @(Get-Process cargo,rustc,cmake,ninja,cl,msbuild -ErrorAction SilentlyContinue | Select-Object ProcessName,Id)
[ordered]@{
    schema_version = 1
    sampled_at = [DateTime]::UtcNow.ToString('o')
    os = $osInfo.Caption
    build = $osInfo.BuildNumber
    architecture = $osInfo.OSArchitecture
    gpus = $videoInfo
    tools = $toolsFound
    active_compilers = $compilers
    capture_tested = $false
    hardware_codec_tested = $false
    effective_fps = $null
} | ConvertTo-Json -Depth 6
