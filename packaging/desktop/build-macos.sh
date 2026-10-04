#!/usr/bin/env bash
set -euo pipefail

[[ $(uname -s) == Darwin ]] || { echo 'Build on macOS.' >&2; exit 2; }
profile=${1:-release}
architecture=${2:-$(uname -m)}
case $architecture in
    arm64|aarch64) architecture=aarch64 ;;
    x86_64) ;;
    *) echo 'Architecture must be aarch64 (arm64) or x86_64.' >&2; exit 2 ;;
esac
[[ $# -le 2 && ( $profile == debug || $profile == release ) ]] || {
    echo 'usage: bash packaging/desktop/build-macos.sh [debug|release] [aarch64|x86_64]' >&2; exit 2;
}
target="$architecture-apple-darwin"
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
cargo_profile=dev
if [[ $profile == release ]]; then cargo_profile=release; fi
cargo build --locked --target "$target" --profile "$cargo_profile" -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp
cd "$root/apps/desktop"
npm ci
if [[ $profile == release ]]; then
    npm run tauri -- build --target "$target" --bundles app
else
    npm run tauri -- build --target "$target" --debug --bundles app
fi
cd "$root"
python3 packaging/desktop/build.py --platform macos --profile "$profile" \
    --macos-arch "$architecture" --macos-bin-dir "$root/target/$target/$profile" \
    --macos-app "$root/apps/desktop/src-tauri/target/$target/$profile/bundle/macos/Pixels Agent Bridge.app"
