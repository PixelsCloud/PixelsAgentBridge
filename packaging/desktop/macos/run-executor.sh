#!/bin/bash
set -euo pipefail
install_dir=$(cd "$(dirname "$0")" && pwd)
export PATH="${PATH:-/usr/bin:/bin}:/opt/homebrew/bin:/usr/local/bin:/usr/sbin:/sbin"
exec "$install_dir/pab-executor" --config "$install_dir/config.toml" "$@"
