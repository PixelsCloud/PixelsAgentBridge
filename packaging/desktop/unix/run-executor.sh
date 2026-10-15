#!/usr/bin/env bash
set -euo pipefail

install_dir=$(cd "$(dirname "$0")" && pwd)
exec "$install_dir/pab-executor" --config /etc/pixels-agent-bridge/config.toml "$@"
