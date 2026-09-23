#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ( $1 != operator && $1 != executor ) ]]; then
    echo 'usage: uninstall.sh operator|executor' >&2
    exit 2
fi

mode=$1
platform=$(uname -s)
if [[ $mode == executor ]]; then
    [[ $EUID -eq 0 ]] || { echo 'Executor removal requires root' >&2; exit 2; }
    case $platform in
        Linux)
            install_dir=/opt/pixels-agent-bridge
            systemctl disable --now pixels-agent-bridge-executor.service
            rm -f /etc/systemd/system/pixels-agent-bridge-executor.service
            systemctl daemon-reload
            ;;
        Darwin)
            install_dir='/Library/Application Support/PixelsAgentBridge'
            launchctl bootout system /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
            rm -f /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
            ;;
        *) echo "unsupported platform: $platform" >&2; exit 2 ;;
    esac
else
    case $platform in
        Linux) install_dir="$HOME/.local/opt/pixels-agent-bridge" ;;
        Darwin) install_dir="$HOME/Applications/PixelsAgentBridge" ;;
        *) echo "unsupported platform: $platform" >&2; exit 2 ;;
    esac
fi

for name in pab-mcp pab-bridge pab-executor run-ui.sh run-executor.sh uninstall.sh settings.env; do
    rm -f "$install_dir/$name"
done
printf 'Application files removed from %s. Persistent data retained.\n' "$install_dir"
