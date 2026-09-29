param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'debug'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$dockerfile = Join-Path $PSScriptRoot 'Dockerfile.linux'
$packageScript = Join-Path $PSScriptRoot 'build.py'
$mount = "type=bind,source=$root,target=/src"
$image = 'pab-linux-build-deps:bookworm'

& docker build --target dependencies -t $image -f $dockerfile $root
if ($LASTEXITCODE -ne 0) {
    throw 'Linux dependency image build failed'
}

$cargoFlags = if ($Profile -eq 'release') { '--release' } else { '' }
$buildScript = @"
set -eu
cargo build --locked $cargoFlags -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp
cargo build --locked $cargoFlags --manifest-path apps/desktop/src-tauri/Cargo.toml
mkdir -p .build/guest-desktop-linux-$Profile
cp .build/linux-target/$Profile/pab-executor .build/linux-target/$Profile/pab-mcp .build/linux-target/$Profile/pab-desktop .build/guest-desktop-linux-$Profile/
"@
$buildScript = $buildScript.Replace("`r`n", "`n")

& docker run --rm --mount $mount --workdir /src `
    --env CARGO_HOME=/src/.build/linux-cargo-home `
    --env CARGO_TARGET_DIR=/src/.build/linux-target `
    --env CARGO_BUILD_JOBS=4 `
    --env PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin `
    $image sh -c $buildScript
if ($LASTEXITCODE -ne 0) {
    throw 'Linux program build failed'
}

& python $packageScript --platform linux --profile $Profile
if ($LASTEXITCODE -ne 0) {
    throw 'Linux archive creation failed'
}
