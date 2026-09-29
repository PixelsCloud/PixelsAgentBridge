#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 0 ]] || { echo 'usage: uninstall.sh' >&2; exit 2; }
[[ $EUID -eq 0 ]] || { echo 'Uninstallation requires root' >&2; exit 2; }

platform=$(uname -s)
case $platform in
    Linux)
        install_dir=/opt/pixels-agent-bridge
        systemctl disable --now pixels-agent-bridge-executor.service
        rm -f -- /etc/systemd/system/pixels-agent-bridge-executor.service
        rm -f -- /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop
        systemctl daemon-reload
        ;;
    Darwin)
        install_dir='/Library/Application Support/PixelsAgentBridge'
        launchctl bootout system /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
        rm -f -- /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
        ;;
    *) echo "unsupported platform: $platform" >&2; exit 2 ;;
esac

if [[ $platform == Linux ]]; then
    for process in /proc/[0-9]*; do
        executable=$(readlink "$process/exe" 2>/dev/null || true)
        case $executable in
            "$install_dir"/pab-mcp|"$install_dir"/pab-bridge|"$install_dir"/pab-executor|"$install_dir"/pab-desktop)
                kill "${process##*/}" 2>/dev/null || true
                ;;
        esac
    done
fi

for name in pab-mcp pab-bridge pab-executor pab-desktop run-app.sh run-mcp.sh run-ui.sh run-executor.sh uninstall.sh settings.env; do
    rm -f -- "$install_dir/$name"
done
printf 'Application files removed from %s. Persistent data retained.\n' "$install_dir"
