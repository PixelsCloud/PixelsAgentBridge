#!/usr/bin/env bash
set -euo pipefail

install_dir=$(cd "$(dirname "$0")" && pwd)
source "$install_dir/settings.env"
exec "$install_dir/pab-mcp" "$@"
