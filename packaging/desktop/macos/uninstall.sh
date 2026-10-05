#!/bin/bash
set -euo pipefail
[[ $(uname -s) == Darwin && $EUID -eq 0 && $# -eq 0 ]] || { echo 'usage: sudo bash uninstall.sh (macOS)' >&2; exit 2; }
install_dir='/Library/Application Support/PixelsAgentBridge'
app='/Applications/Pixels Agent Bridge.app'
if [[ -e $app ]]; then
    bundle_id=$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")
    [[ $bundle_id == vip.rgaa.pab.desktop ]] || { echo 'Refusing to remove an unrelated application' >&2; exit 2; }
fi
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
    echo 'Quit the app and MCP clients, then run uninstall again.' >&2; exit 2;
fi
if launchctl print system/com.pixelsagentbridge.executor >/dev/null 2>&1; then
    launchctl bootout system/com.pixelsagentbridge.executor
fi
rm -f -- /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist
rm -f -- /Library/LaunchAgents/com.pixelsagentbridge.login-helper.plist
rm -rf -- '/Applications/Pixels Agent Bridge.app'
for name in pab-mcp pab-executor run-app.sh run-mcp.sh run-executor.sh uninstall.sh settings.env operator-server.json; do
    rm -f -- "$install_dir/$name"
done
printf '%s\n' 'Application and launchd jobs removed. Machine and user data retained.'
