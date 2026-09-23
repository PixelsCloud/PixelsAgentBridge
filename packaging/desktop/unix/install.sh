#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo 'usage: install.sh DEPLOYMENT_ID WSS_CONTROL_URL HTTPS_RELAY_URL' >&2
    exit 2
fi
[[ $EUID -eq 0 ]] || { echo 'Installation requires root' >&2; exit 2; }

deployment_id=$1
control_url=$2
relay_url=$3
source_dir=$(cd "$(dirname "$0")" && pwd)
platform=$(uname -s)

[[ $control_url == wss://* ]] || { echo 'Control URL must use wss://' >&2; exit 2; }
[[ $relay_url == https://* ]] || { echo 'Relay URL must use https://' >&2; exit 2; }

case $platform in
    Linux) install_dir=/opt/pixels-agent-bridge ;;
    Darwin) install_dir='/Library/Application Support/PixelsAgentBridge' ;;
    *) echo "unsupported platform: $platform" >&2; exit 2 ;;
esac

install -d -m 755 "$install_dir"
rm -f -- "$install_dir/run-ui.sh"
for name in pab-mcp pab-bridge pab-executor pab-desktop; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done
for name in run-app.sh run-executor.sh uninstall.sh; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done

{
    printf 'export PAB_DEPLOYMENT_ID=%q\n' "$deployment_id"
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$install_dir/settings.env"
chmod 644 "$install_dir/settings.env"

if [[ $platform == Linux ]]; then
    install -d -m 700 /var/lib/pixels-agent-bridge
    cat > /etc/systemd/system/pixels-agent-bridge-executor.service <<UNIT
[Unit]
Description=Pixels Agent Bridge Executor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$install_dir/run-executor.sh
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable --now pixels-agent-bridge-executor.service
else
    install -d -m 700 '/Library/Application Support/PixelsAgentBridgeData'
    cat > /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.pixelsagentbridge.executor</string>
    <key>ProgramArguments</key>
    <array><string>$install_dir/run-executor.sh</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
</dict>
</plist>
PLIST
    launchctl bootstrap system /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
fi

printf 'Installed to %s\n' "$install_dir"
printf 'App: %s/run-app.sh\n' "$install_dir"
