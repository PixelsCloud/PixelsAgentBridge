#!/usr/bin/env bash
set -euo pipefail

install_dir=$(cd "$(dirname "$0")" && pwd)
source "$install_dir/settings.env"
export PAB_MCP_GUEST=1
exec "$install_dir/pab-mcp" --ui
