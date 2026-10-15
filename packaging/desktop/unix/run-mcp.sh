#!/usr/bin/env bash
set -euo pipefail

install_dir=$(cd "$(dirname "$0")" && pwd)
# The executable loads installed TOML and the user's TOML server preferences.
exec "$install_dir/pab-mcp" "$@"
