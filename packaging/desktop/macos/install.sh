#!/bin/bash
set -euo pipefail
[[ $(uname -s) == Darwin && $EUID -eq 0 && $# -eq 2 ]] || {
    echo 'usage (macOS): sudo bash install.sh WSS_CONTROL_URL HTTPS_RELAY_URL' >&2; exit 2;
}
control_url=$1
relay_url=$2
[[ $control_url == wss://?* && $relay_url == https://?* && $control_url != *[$'\r\n\t ']* && $relay_url != *[$'\r\n\t ']* ]] || {
    echo 'Use wss:// control and https:// relay URLs without whitespace' >&2; exit 2;
}
desktop_user=${SUDO_USER:-}
[[ -n $desktop_user && $desktop_user != root ]] || { echo 'Run sudo from the desktop user account' >&2; exit 2; }
desktop_uid=$(id -u "$desktop_user")
user_home=$(dscl . -read "/Users/$desktop_user" NFSHomeDirectory | sed 's/^NFSHomeDirectory: //')
[[ $user_home == /* && -d $user_home ]] || { echo 'Cannot resolve desktop user home' >&2; exit 2; }
source_dir=$(cd "$(dirname "$0")" && pwd)
install_dir='/Library/Application Support/PixelsAgentBridge'
app='/Applications/Pixels Agent Bridge.app'
data_dir='/Library/Application Support/PixelsAgentBridgeData'
user_data="$user_home/Library/Application Support/PixelsAgentBridge"
for name in pab-mcp pab-executor run-app.sh run-mcp.sh run-executor.sh uninstall.sh com.pixelsagentbridge.executor.plist com.pixelsagentbridge.session-helper.plist com.pixelsagentbridge.login-helper.plist; do
    [[ -f $source_dir/$name ]] || { echo "Missing package file: $name" >&2; exit 2; }
done
[[ -f "$source_dir/Pixels Agent Bridge.app/Contents/MacOS/pab-desktop" ]] || { echo 'Missing application bundle' >&2; exit 2; }
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$source_dir/Pixels Agent Bridge.app/Contents/Info.plist")
[[ $bundle_id == vip.rgaa.pab.desktop ]] || { echo 'Unexpected application identifier' >&2; exit 2; }
codesign --verify --deep --strict "$source_dir/Pixels Agent Bridge.app"
if [[ -e $app ]]; then
    installed_id=$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")
    [[ $installed_id == vip.rgaa.pab.desktop ]] || { echo 'Refusing to replace an unrelated application' >&2; exit 2; }
fi
# Every currently logged-in helper must stop before replacing the shared app.
if launchctl print loginwindow/com.pixelsagentbridge.login-helper >/dev/null 2>&1; then
    launchctl bootout loginwindow/com.pixelsagentbridge.login-helper
fi
while read -r uid; do
    if launchctl print "gui/$uid/com.pixelsagentbridge.session-helper" >/dev/null 2>&1; then
        launchctl bootout "gui/$uid/com.pixelsagentbridge.session-helper"
    fi
done < <(dscl . -list /Users UniqueID | awk '$2 >= 500 {print $2}')
if pgrep -f '^/Applications/Pixels Agent Bridge.app/Contents/MacOS/pab-desktop' >/dev/null \
    || pgrep -f '^/Library/Application Support/PixelsAgentBridge/pab-mcp' >/dev/null; then
    echo 'Quit Pixels Agent Bridge and its MCP clients before updating; helpers restart at next login.' >&2; exit 2;
fi
if launchctl print system/com.pixelsagentbridge.executor >/dev/null 2>&1; then
    launchctl bootout system/com.pixelsagentbridge.executor
fi
install -d -m 755 "$install_dir"
install -d -m 700 "$data_dir"
install -d -m 700 -o "$desktop_user" "$user_data"
for name in pab-mcp pab-executor run-app.sh run-mcp.sh run-executor.sh uninstall.sh; do
    install -m 755 "$source_dir/$name" "$install_dir/$name"
done
ditto "$source_dir/Pixels Agent Bridge.app" "$app"
chown -R root:wheel "$app"
chmod -R u=rwX,go=rX "$app"
{
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$install_dir/settings.env"
chmod 644 "$install_dir/settings.env"
settings_file=$(mktemp "$install_dir/operator-server.XXXXXX")
plutil -create xml1 "$settings_file"
plutil -insert controlUrl -string "$control_url" "$settings_file"
plutil -insert relayUrl -string "$relay_url" "$settings_file"
plutil -convert json "$settings_file"
chmod 644 "$settings_file"
mv -f "$settings_file" "$install_dir/operator-server.json"
PAB_DATA_DIR="$data_dir" "$install_dir/pab-executor" issue-local-access "$user_data/local-access.key"
chown "$desktop_user" "$user_data/local-access.key"
install -d -m 755 /Library/LaunchAgents /Library/LaunchDaemons
install -m 644 "$source_dir/com.pixelsagentbridge.executor.plist" /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
install -m 644 "$source_dir/com.pixelsagentbridge.session-helper.plist" /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist
install -m 644 "$source_dir/com.pixelsagentbridge.login-helper.plist" /Library/LaunchAgents/com.pixelsagentbridge.login-helper.plist
launchctl bootstrap system /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
if launchctl print "gui/$desktop_uid" >/dev/null 2>&1; then
    launchctl bootstrap "gui/$desktop_uid" /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist
fi
printf '%s\n' 'Installed Pixels Agent Bridge. Open it from Applications; grant Screen Recording and Accessibility in Settings.'
printf '%s\n' "MCP command: $install_dir/run-mcp.sh"
