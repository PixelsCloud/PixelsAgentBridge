#!/usr/bin/env bash
set -euo pipefail

mode=systemd
if [[ ${1:-} == --no-service ]]; then mode=manual; shift; fi
[[ $# -eq 2 ]] || { echo 'usage: install.sh [--no-service] WSS_CONTROL_URL HTTPS_RELAY_URL' >&2; exit 2; }
[[ $EUID -eq 0 ]] || { echo 'Installation requires root' >&2; exit 2; }
[[ $(uname -s) == Linux ]] || { echo 'This package requires Linux' >&2; exit 2; }
control_url=$1
relay_url=$2
[[ $control_url == wss://?* ]] || { echo 'Control URL must use wss://' >&2; exit 2; }
[[ $relay_url == https://?* ]] || { echo 'Relay URL must use https://' >&2; exit 2; }
source_dir=$(cd "$(dirname "$0")" && pwd)
install_dir=/opt/pixels-agent-bridge
unit=pixels-agent-bridge-executor.service
for name in pab-mcp pab-executor run-mcp.sh run-executor.sh uninstall.sh lifecycle.sh INSTALL-LINUX.txt; do
    [[ -f $source_dir/$name ]] || { echo "Package file is missing: $name" >&2; exit 2; }
done
source "$source_dir/lifecycle.sh"
if [[ $mode == systemd ]] && ! has_systemd; then
    echo 'systemd is not running; use --no-service and run-executor.sh with your process supervisor' >&2
    exit 2
fi
if has_systemd && systemctl cat "$unit" >/dev/null 2>&1; then
    systemctl stop "$unit"
    if [[ $mode == manual ]]; then systemctl disable "$unit"; fi
fi
stop_installed_processes "$install_dir"
install -d -m 755 "$install_dir"
install -d -m 700 /var/lib/pixels-agent-bridge
for name in pab-mcp pab-executor run-mcp.sh run-executor.sh uninstall.sh lifecycle.sh; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done
install -m 644 "$source_dir/INSTALL-LINUX.txt" "$install_dir/INSTALL-LINUX.txt"
{
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$install_dir/settings.env"
chmod 644 "$install_dir/settings.env"
# Remove only obsolete product entry points. Persistent device/user data is retained.
rm -f -- "$install_dir/run-app.sh" "$install_dir/run-ui.sh" "$install_dir/pab-bridge" "$install_dir/pab-desktop"
rm -f -- /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop
if [[ $mode == systemd ]]; then
    cat > "/etc/systemd/system/$unit" <<UNIT
[Unit]
Description=Pixels Agent Bridge Executor (headless)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$install_dir/run-executor.sh
Restart=always
RestartSec=3
TimeoutStopSec=60
KillMode=control-group
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable "$unit"
    systemctl restart "$unit"
    systemctl is-active --quiet "$unit"
else
    rm -f -- "/etc/systemd/system/$unit"
    if has_systemd; then systemctl daemon-reload; fi
fi
printf 'Installed Linux headless components to %s (%s).\n' "$install_dir" "$mode"
if [[ $mode == manual ]]; then printf 'Start: %s/run-executor.sh\n' "$install_dir"; fi
printf 'Device code/password: sudo %s/run-executor.sh show-access\n' "$install_dir"
printf 'MCP entry point: %s/run-mcp.sh\n' "$install_dir"
