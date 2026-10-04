#!/bin/bash
set -euo pipefail
install_dir=$(cd "$(dirname "$0")" && pwd)
source "$install_dir/settings.env"
export PAB_DATA_DIR='/Library/Application Support/PixelsAgentBridgeData'
export PATH="${PATH:-/usr/bin:/bin}:/opt/homebrew/bin:/usr/local/bin:/usr/sbin:/sbin"
exec "$install_dir/pab-executor" "$@"
