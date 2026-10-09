#!/bin/bash
# Installer hides component-script stderr behind a generic failure page. Show
# the actionable reason in the signed-in user's GUI as well as install.log.
pab_report_failure() {
    local message=$1 desktop_user desktop_uid
    printf '%s\n' "$message" >&2
    desktop_user=$(stat -f '%Su' /dev/console)
    case $desktop_user in root|loginwindow|_mbsetupuser|'') return ;; esac
    desktop_uid=$(id -u "$desktop_user") || return
    /bin/launchctl asuser "$desktop_uid" /usr/bin/sudo -u "$desktop_user" \
        /usr/bin/osascript - "$message" < "$package_dir/failure-dialog.applescript" || true
}
