#!/usr/bin/env bash
set -euo pipefail
[[ $EUID -eq 0 && $# -eq 1 ]] || { echo 'Usage: sudo grant-desktop-user.sh USER' >&2; exit 2; }
target=$1
uid=$(id -u -- "$target")
[[ $uid -ne 0 ]] || { echo 'Choose a normal desktop account.' >&2; exit 2; }
root=$(runuser -u "$target" -- /opt/pixels-agent-bridge/pab-desktop --local-data-path)
[[ $root == /* && $root != / ]] || { echo 'Invalid user data directory.' >&2; exit 2; }
install -d -m 700 /var/lib/pixels-agent-bridge
token=$(mktemp /var/lib/pixels-agent-bridge/.desktop-access.XXXXXX)
trap 'rm -f -- "$token"' EXIT
/opt/pixels-agent-bridge/run-executor.sh issue-local-access "$token"
# Create user-owned files as the user, never follow a user-controlled path as root.
runuser -u "$target" -- bash -c '
    set -euo pipefail
    umask 077
    mkdir -p -- "$1"
    chmod 700 -- "$1"
    cat > "$1/local-access.key"
    chmod 600 -- "$1/local-access.key"
' bash "$root" < "$token"
printf 'Local service access configured for %s.\n' "$target"
