#!/usr/bin/env bash
set -euo pipefail
[[ $# -eq 0 ]] || { echo 'usage: uninstall.sh' >&2; exit 2; }
[[ $EUID -eq 0 ]] || { echo 'Uninstallation requires root' >&2; exit 2; }
[[ $(uname -s) == Linux ]] || { echo 'This package requires Linux' >&2; exit 2; }
install_dir=/opt/pixels-agent-bridge
unit=pixels-agent-bridge-executor.service
source "$(dirname "$0")/lifecycle.sh"
if has_systemd && systemctl cat "$unit" >/dev/null 2>&1; then
    systemctl disable --now "$unit"
fi
stop_installed_processes "$install_dir"
rm -f -- "/etc/systemd/system/$unit" /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop
if has_systemd; then systemctl daemon-reload; fi
for name in pab-mcp pab-bridge pab-executor pab-desktop run-app.sh run-mcp.sh run-ui.sh run-executor.sh uninstall.sh lifecycle.sh settings.env INSTALL-LINUX.txt; do
    rm -f -- "$install_dir/$name"
done
printf 'Application files removed from %s. Persistent data retained in /var/lib/pixels-agent-bridge and user homes.\n' "$install_dir"
