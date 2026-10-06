#!/usr/bin/env bash
# Run only inside a disposable Linux container; never on a user's installed host.
set -euo pipefail
[[ -f /.dockerenv ]] || { echo 'Disposable Docker container required' >&2; exit 2; }
[[ ! -e /opt/pixels-agent-bridge ]] || { echo 'Existing installation; refusing test' >&2; exit 2; }
payload=$(mktemp -d)
trap 'rm -rf -- "$payload"' EXIT
cp /src/packaging/desktop/unix/{install,uninstall,lifecycle,run-mcp,run-executor}.sh "$payload/"
cp /src/packaging/desktop/unix/INSTALL-LINUX.txt "$payload/"
cp /src/.build/linux-target/debug/{pab-executor,pab-mcp} "$payload/"
sed -i 's/\r$//' "$payload/"*.sh
for script in "$payload/"*.sh; do bash -n "$script"; done
control='wss://127.0.0.1:1/control?literal=$(touch /tmp/pab-injected)'
relay=https://127.0.0.1:1
if bash "$payload/install.sh" "$control" "$relay"; then
    echo 'Default install must reject missing systemd' >&2; exit 1
fi
[[ ! -e /opt/pixels-agent-bridge ]]
if bash "$payload/install.sh" --no-service http://invalid "$relay"; then exit 1; fi
bash "$payload/install.sh" --no-service "$control" "$relay"
installed=/opt/pixels-agent-bridge
[[ ! -e $installed/pab-desktop && ! -e $installed/run-app.sh ]]
[[ ! -e /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop ]]
[[ $(stat -c %a /var/lib/pixels-agent-bridge) == 700 ]]
source "$installed/settings.env"
[[ $PAB_CONTROL_URL == "$control" && ! -e /tmp/pab-injected ]]
printf preserved > /var/lib/pixels-agent-bridge/test-identity
# Forwarded CLI argument: must reject immediately instead of starting a service.
if timeout 10 "$installed/run-executor.sh" invalid-test-command > /tmp/cli-result 2>&1; then exit 1; fi
grep -q 'unknown executor command' /tmp/cli-result
# Verify the real binary loads in an image with no GUI packages/display/session.
[[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]
ldd "$installed/pab-executor" > /tmp/runtime-deps
! grep -E 'not found|webkit|gtk|X11' /tmp/runtime-deps
# Upgrade stops only executables from the installation, preserves persistent data,
# and removes obsolete GUI entry points. A same-named external process survives.
cp /bin/sleep "$installed/pab-desktop"
"$installed/pab-desktop" 300 & product_pid=$!
cp /bin/sleep "$payload/pab-desktop"
"$payload/pab-desktop" 300 & other_pid=$!
touch "$installed/run-app.sh"
mkdir -p /etc/xdg/autostart
touch /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop
bash "$payload/install.sh" --no-service "$control" "$relay"
! kill -0 "$product_pid" 2>/dev/null
kill -0 "$other_pid"
[[ $(cat /var/lib/pixels-agent-bridge/test-identity) == preserved ]]
[[ ! -e $installed/pab-desktop && ! -e $installed/run-app.sh ]]
[[ ! -e /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop ]]
# A product process ignoring SIGTERM must be forced down after the grace period.
cp /bin/bash "$installed/pab-desktop"
"$installed/pab-desktop" -c 'trap "" TERM; echo ready > /tmp/pab-stubborn-ready; while :; do read -t 1 -r line || true; done' < <(sleep 120) & stuck_pid=$!
for ((n=0; n<50; n++)); do [[ ! -f /tmp/pab-stubborn-ready ]] || break; sleep 0.1; done
[[ -f /tmp/pab-stubborn-ready ]]
"$installed/uninstall.sh"
! kill -0 "$stuck_pid" 2>/dev/null
[[ ! -e $installed/pab-executor && ! -e $installed/settings.env ]]
[[ $(cat /var/lib/pixels-agent-bridge/test-identity) == preserved ]]
kill "$other_pid"
wait "$other_pid" 2>/dev/null || true
printf 'PASS: Linux manual install, CLI, no GUI, upgrade, scoped stop, data preservation, uninstall\n'
