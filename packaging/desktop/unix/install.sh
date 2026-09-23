#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
    echo 'usage: install.sh operator|executor DEPLOYMENT_ID WSS_CONTROL_URL HTTPS_RELAY_URL' >&2
    exit 2
fi

mode=$1
deployment_id=$2
control_url=$3
relay_url=$4
source_dir=$(cd "$(dirname "$0")" && pwd)
platform=$(uname -s)

[[ $control_url == wss://* ]] || { echo 'Control URL must use wss://' >&2; exit 2; }
[[ $relay_url == https://* ]] || { echo 'Relay URL must use https://' >&2; exit 2; }

if [[ $mode == operator ]]; then
    case $platform in
        Linux) install_dir="$HOME/.local/opt/pixels-agent-bridge" ;;
        Darwin) install_dir="$HOME/Applications/PixelsAgentBridge" ;;
        *) echo "unsupported platform: $platform" >&2; exit 2 ;;
    esac
elif [[ $mode == executor ]]; then
    [[ $EUID -eq 0 ]] || { echo 'Executor installation requires root' >&2; exit 2; }
    case $platform in
        Linux) install_dir=/opt/pixels-agent-bridge ;;
        Darwin) install_dir=/Library/Application\ Support/PixelsAgentBridge ;;
        *) echo "unsupported platform: $platform" >&2; exit 2 ;;
    esac
else
    echo 'mode must be operator or executor' >&2
    exit 2
fi

install -d -m 755 "$install_dir"
install -m 755 "$source_dir/pab-mcp" "$install_dir/pab-mcp"
install -m 755 "$source_dir/pab-bridge" "$install_dir/pab-bridge"
install -m 755 "$source_dir/pab-executor" "$install_dir/pab-executor"
install -m 755 "$source_dir/run-ui.sh" "$install_dir/run-ui.sh"
install -m 755 "$source_dir/run-executor.sh" "$install_dir/run-executor.sh"
install -m 755 "$source_dir/uninstall.sh" "$install_dir/uninstall.sh"

{
    printf 'export PAB_DEPLOYMENT_ID=%q\n' "$deployment_id"
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$install_dir/settings.env"
chmod 600 "$install_dir/settings.env"

if [[ $mode == executor && $platform == Linux ]]; then
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
elif [[ $mode == executor && $platform == Darwin ]]; then
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
printf 'Operator UI: %s/run-ui.sh\n' "$install_dir"
