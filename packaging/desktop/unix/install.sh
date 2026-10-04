#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo 'usage: install.sh WSS_CONTROL_URL HTTPS_RELAY_URL' >&2
    exit 2
fi
[[ $EUID -eq 0 ]] || { echo 'Installation requires root' >&2; exit 2; }

control_url=$1
relay_url=$2
source_dir=$(cd "$(dirname "$0")" && pwd)
platform=$(uname -s)

[[ $control_url == wss://* ]] || { echo 'Control URL must use wss://' >&2; exit 2; }
[[ $relay_url == https://* ]] || { echo 'Relay URL must use https://' >&2; exit 2; }

case $platform in
    Linux) install_dir=/opt/pixels-agent-bridge ;;
    Darwin) install_dir='/Library/Application Support/PixelsAgentBridge' ;;
    *) echo "unsupported platform: $platform" >&2; exit 2 ;;
esac

for name in pab-mcp pab-executor pab-desktop run-app.sh run-mcp.sh run-executor.sh uninstall.sh; do
    [[ -f $source_dir/$name ]] || { echo "Package file is missing: $name" >&2; exit 2; }
done

if [[ $platform == Linux ]]; then
    if systemctl cat pixels-agent-bridge-executor.service >/dev/null 2>&1; then
        systemctl stop pixels-agent-bridge-executor.service
    fi
    for process in /proc/[0-9]*; do
        executable=$(readlink "$process/exe" 2>/dev/null || true)
        case $executable in
            "$install_dir"/pab-mcp|"$install_dir"/pab-bridge|"$install_dir"/pab-executor|"$install_dir"/pab-desktop)
                kill "${process##*/}" 2>/dev/null || true
                ;;
        esac
    done
    sleep 2
    for process in /proc/[0-9]*; do
        executable=$(readlink "$process/exe" 2>/dev/null || true)
        case $executable in
            "$install_dir"/pab-mcp|"$install_dir"/pab-bridge|"$install_dir"/pab-executor|"$install_dir"/pab-desktop)
                echo "Cannot replace running program: $executable" >&2
                exit 2
                ;;
        esac
    done
elif launchctl print system/com.pixelsagentbridge.executor >/dev/null 2>&1; then
    launchctl bootout system/com.pixelsagentbridge.executor
fi

install -d -m 755 "$install_dir"
rm -f -- "$install_dir/run-ui.sh"
rm -f -- "$install_dir/pab-bridge"
for name in pab-mcp pab-executor pab-desktop; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done
for name in run-app.sh run-mcp.sh run-executor.sh uninstall.sh; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done

{
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$install_dir/settings.env"
chmod 644 "$install_dir/settings.env"

if [[ $platform == Linux ]]; then
    install -d -m 700 /var/lib/pixels-agent-bridge
    desktop_user=${SUDO_USER:-root}
    user_home=$(getent passwd "$desktop_user" | cut -d: -f6)
    if [[ -z $user_home ]]; then
        echo "Cannot find home directory for $desktop_user" >&2
        exit 2
    fi
    user_data="$user_home/.local/share/pixels-agent-bridge"
    install -d -m 700 -o "$desktop_user" "$user_data"
    PAB_DATA_DIR=/var/lib/pixels-agent-bridge \
        "$install_dir/pab-executor" issue-local-access "$user_data/local-access.key"
    chown "$desktop_user" "$user_data/local-access.key"
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
    systemctl enable pixels-agent-bridge-executor.service
    systemctl restart pixels-agent-bridge-executor.service
    install -d -m 755 /etc/xdg/autostart
    cat > /etc/xdg/autostart/pixels-agent-bridge-session-helper.desktop <<DESKTOP
[Desktop Entry]
Type=Application
Name=Pixels Agent Bridge Session Helper
Exec=$install_dir/pab-desktop --session-helper
NoDisplay=true
X-GNOME-Autostart-enabled=true
DESKTOP
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
