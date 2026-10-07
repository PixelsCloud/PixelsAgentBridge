#!/usr/bin/env bash
# Disposable systemd container only. No external control server is contacted.
set -euo pipefail
[[ -f /.dockerenv && -d /run/systemd/system ]] || exit 2
[[ ! -e /opt/pixels-agent-bridge ]] || exit 2
payload=$(mktemp -d)
trap 'rm -rf -- "$payload"' EXIT
if [[ $# -eq 1 ]]; then
    tar -xzf "$1" -C "$payload"
else
    cp /src/packaging/desktop/unix/{install,uninstall,lifecycle,run-mcp,run-executor}.sh "$payload/"
    cp /src/packaging/desktop/unix/INSTALL-LINUX.txt "$payload/"
    cp /src/.build/linux-target/debug/{pab-executor,pab-mcp} "$payload/"
fi
sed -i 's/\r$//' "$payload/"*.sh
unit=pixels-agent-bridge-executor.service
installed=/opt/pixels-agent-bridge
bash "$payload/install.sh" wss://127.0.0.1:1/control https://127.0.0.1:1
systemctl is-enabled --quiet "$unit"
systemctl is-active --quiet "$unit"
[[ $(systemctl show "$unit" -p KillMode --value) == control-group ]]
[[ $(systemctl show "$unit" -p TimeoutStopUSec --value) == 1min ]]
printf retained > /var/lib/pixels-agent-bridge/test-identity
systemctl stop "$unit"
[[ $(systemctl show "$unit" -p MainPID --value) == 0 ]]
# Reinstall retains data and starts the service again.
bash "$payload/install.sh" wss://127.0.0.1:1/control https://127.0.0.1:1
systemctl is-active --quiet "$unit"
[[ $(cat /var/lib/pixels-agent-bridge/test-identity) == retained ]]
# Switching to external supervision must disable/remove the old service.
bash "$payload/install.sh" --no-service wss://127.0.0.1:1/control https://127.0.0.1:1
! systemctl is-enabled --quiet "$unit" 2>/dev/null
[[ ! -e /etc/systemd/system/$unit ]]
bash "$payload/install.sh" wss://127.0.0.1:1/control https://127.0.0.1:1
"$installed/uninstall.sh"
[[ ! -e $installed/pab-executor && ! -e /etc/systemd/system/$unit ]]
! systemctl is-enabled --quiet "$unit" 2>/dev/null
[[ $(cat /var/lib/pixels-agent-bridge/test-identity) == retained ]]
printf 'PASS: real systemd install, enable/start/stop, reinstall, manual-mode transition, uninstall\n'
