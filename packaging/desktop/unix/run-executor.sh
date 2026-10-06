#!/usr/bin/env bash
set -euo pipefail

install_dir=$(cd "$(dirname "$0")" && pwd)
source "$install_dir/settings.env"
case $(uname -s) in
    Linux) export PAB_DATA_DIR=/var/lib/pixels-agent-bridge ;;
    Darwin) export PAB_DATA_DIR='/Library/Application Support/PixelsAgentBridgeData' ;;
    *) echo 'unsupported platform' >&2; exit 2 ;;
esac
exec "$install_dir/pab-executor" "$@"
