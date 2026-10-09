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
for name in pab-mcp pab-executor run-app.sh run-mcp.sh run-executor.sh uninstall.sh lifecycle.sh com.pixelsagentbridge.executor.plist com.pixelsagentbridge.session-helper.plist com.pixelsagentbridge.login-helper.plist; do
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
source "$source_dir/lifecycle.sh"
upgrade=$(mktemp -d '/Applications/.PixelsAgentBridge-upgrade.XXXXXX')
published_app=0
published_support=0
committed=0
stopped_domains=()
stopped_plists=()
started_jobs=()
plist_paths=(/Library/LaunchDaemons/com.pixelsagentbridge.executor.plist /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist /Library/LaunchAgents/com.pixelsagentbridge.login-helper.plist)
plists_written=0
upgrade_step='准备新版本文件 / staging new files'
finish_upgrade() {
    local status=$? job i restored=1
    trap - EXIT
    if [[ $committed != 1 ]]; then
        echo "失败步骤 / Failed step: $upgrade_step" >&2
        for job in ${started_jobs[@]+"${started_jobs[@]}"}; do launchctl bootout "$job" || true; done
        pab_restore_upgrade || restored=0
        if [[ $plists_written == 1 ]]; then
            for ((i=0; i<${#plist_paths[@]}; i++)); do
                if [[ -f $upgrade/plist-$i ]]; then
                    cp -p "$upgrade/plist-$i" "${plist_paths[$i]}" || restored=0
                else
                    rm -f -- "${plist_paths[$i]}" || restored=0
                fi
            done
        fi
        for ((i=0; i<${#stopped_domains[@]}; i++)); do
            launchctl bootstrap "${stopped_domains[$i]}" "${stopped_plists[$i]}" || restored=0
        done
        echo '升级未完成，已尝试恢复原程序和后台服务。/ Upgrade failed; restoration of the previous programs and services was attempted.' >&2
    fi
    if [[ $restored == 1 ]]; then
        rm -rf -- "$upgrade"
    else
        echo "恢复未完成，备份保留在 / Recovery incomplete; backup retained at: $upgrade" >&2
    fi
    exit "$status"
}
trap finish_upgrade EXIT
for ((i=0; i<${#plist_paths[@]}; i++)); do
    if [[ -f ${plist_paths[$i]} ]]; then cp -p "${plist_paths[$i]}" "$upgrade/plist-$i"; fi
done
# Stage and verify everything before stopping a running installation.
ditto "$source_dir/Pixels Agent Bridge.app" "$upgrade/new.app"
chown -R root:wheel "$upgrade/new.app"
chmod -R u=rwX,go=rX "$upgrade/new.app"
codesign --verify --deep --strict "$upgrade/new.app"
install -d -m 755 "$upgrade/new-support"
for name in pab-mcp pab-executor run-app.sh run-mcp.sh run-executor.sh uninstall.sh lifecycle.sh; do
    install -m 755 "$source_dir/$name" "$upgrade/new-support/$name"
done
{
    printf 'export PAB_CONTROL_URL=%q\n' "$control_url"
    printf 'export PAB_RELAY_URLS=%q\n' "$relay_url"
} > "$upgrade/new-support/settings.env"
chmod 644 "$upgrade/new-support/settings.env"
settings_file="$upgrade/new-support/operator-server.json"
plutil -create xml1 "$settings_file"
plutil -insert controlUrl -string "$control_url" "$settings_file"
plutil -insert relayUrl -string "$relay_url" "$settings_file"
plutil -convert json "$settings_file"
chmod 644 "$settings_file"

# Stop launchd respawn before retiring the old executable paths.
upgrade_step='停止旧版后台服务 / stopping old services'
if launchctl print loginwindow/com.pixelsagentbridge.login-helper >/dev/null 2>&1; then
    launchctl bootout loginwindow/com.pixelsagentbridge.login-helper
    stopped_domains+=(loginwindow)
    stopped_plists+=(/Library/LaunchAgents/com.pixelsagentbridge.login-helper.plist)
fi
while read -r uid; do
    if launchctl print "gui/$uid/com.pixelsagentbridge.session-helper" >/dev/null 2>&1; then
        launchctl bootout "gui/$uid/com.pixelsagentbridge.session-helper"
        stopped_domains+=("gui/$uid")
        stopped_plists+=(/Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist)
    fi
done < <(dscl . -list /Users UniqueID | awk '$2 >= 500 {print $2}')
if launchctl print system/com.pixelsagentbridge.executor >/dev/null 2>&1; then
    launchctl bootout system/com.pixelsagentbridge.executor
    stopped_domains+=(system)
    stopped_plists+=(/Library/LaunchDaemons/com.pixelsagentbridge.executor.plist)
fi
upgrade_step='停止旧程序并替换文件 / stopping old programs and replacing files'
pab_publish_upgrade
upgrade_step='配置本机访问权限 / configuring local access'
install -d -m 700 "$data_dir"
install -d -m 700 -o "$desktop_user" "$user_data"
PAB_DATA_DIR="$data_dir" "$install_dir/pab-executor" issue-local-access "$user_data/local-access.key"
chown "$desktop_user" "$user_data/local-access.key"
install -d -m 755 /Library/LaunchAgents /Library/LaunchDaemons
plists_written=1
upgrade_step='注册并启动后台服务 / registering and starting services'
install -m 644 "$source_dir/com.pixelsagentbridge.executor.plist" /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
install -m 644 "$source_dir/com.pixelsagentbridge.session-helper.plist" /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist
install -m 644 "$source_dir/com.pixelsagentbridge.login-helper.plist" /Library/LaunchAgents/com.pixelsagentbridge.login-helper.plist
launchctl bootstrap system /Library/LaunchDaemons/com.pixelsagentbridge.executor.plist
started_jobs+=(system/com.pixelsagentbridge.executor)
for ((i=0; i<${#stopped_domains[@]}; i++)); do
    if [[ ${stopped_domains[$i]} != system ]]; then
        launchctl bootstrap "${stopped_domains[$i]}" "${stopped_plists[$i]}"
        job_name=$(basename "${stopped_plists[$i]}" .plist)
        started_jobs+=("${stopped_domains[$i]}/$job_name")
    fi
done
if launchctl print "gui/$desktop_uid" >/dev/null 2>&1; then
    if ! launchctl print "gui/$desktop_uid/com.pixelsagentbridge.session-helper" >/dev/null 2>&1; then
        launchctl bootstrap "gui/$desktop_uid" /Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist
        started_jobs+=("gui/$desktop_uid/com.pixelsagentbridge.session-helper")
    fi
fi
committed=1
printf '%s\n' 'Installed Pixels Agent Bridge. Open it from Applications; grant Screen Recording and Accessibility in Settings.'
printf '%s\n' "MCP command: $install_dir/run-mcp.sh"
