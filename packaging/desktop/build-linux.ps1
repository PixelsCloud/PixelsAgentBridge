param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'debug',
    [switch]$ComponentsOnly
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $ComponentsOnly) {
    & python (Join-Path $root 'scripts\build.py') linux --profile $Profile --package
    exit $LASTEXITCODE
}
$dockerfile = Join-Path $PSScriptRoot 'Dockerfile.linux'
$mount = "type=bind,source=$root,target=/src"
$image = 'pab-linux-headless-build-deps:bullseye'
$cargoJobs = if ($env:CARGO_BUILD_JOBS) { [Math]::Min(10, [Math]::Max(1, [int]$env:CARGO_BUILD_JOBS)) } else { 10 }

& docker build --target dependencies -t $image -f $dockerfile $root
if ($LASTEXITCODE -ne 0) {
    throw 'Linux dependency image build failed'
}

$cargoFlags = if ($Profile -eq 'release') { '--release' } else { '' }
$buildScript = @"
set -eu
# Linux Makefile builds share Cargo's jobserver, including native dependencies.
# An explicit CMake parallel limit overrides that pool and serializes OCR builds.
unset CMAKE_BUILD_PARALLEL_LEVEL
cargo build --locked $cargoFlags -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp -p pab-document --bin pab-document-worker -j $cargoJobs
mkdir -p .build/guest-desktop-linux-$Profile
cp .build/linux-target-bullseye/$Profile/pab-executor .build/linux-target-bullseye/$Profile/pab-mcp .build/linux-target-bullseye/$Profile/pab-document-worker .build/guest-desktop-linux-$Profile/
"@
$buildScript = $buildScript.Replace("`r`n", "`n")

$defaultsArguments = @()
if (Test-Path Env:PAB_CLIENT_DEFAULTS_FILE) {
    if (-not $env:PAB_CLIENT_DEFAULTS_FILE) { throw 'PAB_CLIENT_DEFAULTS_FILE must name a TOML file' }
    $defaultsFile = [IO.Path]::GetFullPath($env:PAB_CLIENT_DEFAULTS_FILE)
    if (-not (Test-Path -LiteralPath $defaultsFile -PathType Leaf)) { throw 'Private client defaults TOML not found' }
    $defaultsArguments = @('--mount', "type=bind,source=$defaultsFile,target=/run/pab-client-defaults.toml,readonly",
                          '--env', 'PAB_CLIENT_DEFAULTS_FILE=/run/pab-client-defaults.toml')
}
# The default .env/client-defaults.toml is available through the existing source
# bind mount; an explicit host path is mounted read-only, never copied into an image.
& docker run --rm --mount $mount --workdir /src `
    --env CARGO_HOME=/src/.build/linux-cargo-home `
    --env CARGO_TARGET_DIR=/src/.build/linux-target-bullseye `
    --cpus 10 `
    --env "CARGO_BUILD_JOBS=$cargoJobs" `
    --env PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin `
    @defaultsArguments $image sh -c $buildScript
if ($LASTEXITCODE -ne 0) {
    throw 'Linux program build failed'
}
