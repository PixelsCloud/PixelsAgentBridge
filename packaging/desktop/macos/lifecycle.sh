#!/bin/bash
# Sourced by install.sh. Match executable paths, never an Agent host's name.
pab_matching_processes() {
    local pid executable candidate snapshot
    snapshot=$(ps -axo pid=,comm=) || return
    while read -r pid executable; do
        for candidate in "$@"; do
            if [[ $executable == "$candidate" ]]; then
                printf '%s\n' "$pid"
                break
            fi
        done
    done <<< "$snapshot"
}

pab_stop_binaries() {
    local pid attempt signal pids
    for signal in TERM KILL; do
        pids=$(pab_matching_processes "$@") || return
        for pid in $pids; do
            # Recheck immediately before signalling in case a process exited.
            local executable
            executable=$(ps -p "$pid" -o comm= 2>/dev/null || true)
            local candidate
            for candidate in "$@"; do
                if [[ $executable == "$candidate" ]]; then
                    kill -"$signal" "$pid" 2>/dev/null || true
                    break
                fi
            done
        done
        for ((attempt=0; attempt<25; attempt++)); do
            pids=$(pab_matching_processes "$@") || return
            [[ -n $pids ]] || return 0
            sleep 0.2
        done
    done
    echo '无法停止旧版 PAB 进程。请退出使用 Pixels MCP 的客户端后重试。/ Could not stop the old PAB processes; quit the Pixels MCP client and retry.' >&2
    return 1
}

# Call only after launchd jobs have stopped. Removing the public launch paths
# first prevents a still-running Codex/editor from respawning the OLD binaries.
# Keep backups until the complete installer (including service restart) succeeds.
pab_publish_upgrade() {
    if [[ -e $app ]]; then
        mv "$app" "$upgrade/previous.app" || return
    fi
    if [[ -e $install_dir ]]; then
        mv "$install_dir" "$upgrade/previous-support" || return
    fi
    pab_stop_binaries \
        "$app/Contents/MacOS/pab-desktop" \
        "$upgrade/previous.app/Contents/MacOS/pab-desktop" \
        "$install_dir/pab-mcp" "$upgrade/previous-support/pab-mcp" \
        "$install_dir/pab-executor" "$upgrade/previous-support/pab-executor" || return
    mv "$upgrade/new.app" "$app" || return
    published_app=1
    mv "$upgrade/new-support" "$install_dir" || return
    published_support=1
}

pab_restore_upgrade() {
    # Withdraw the new launch paths before stopping any respawned processes.
    if [[ $published_support == 1 ]]; then
        mv "$install_dir" "$upgrade/failed-support" || return
    fi
    if [[ $published_app == 1 ]]; then
        mv "$app" "$upgrade/failed.app" || return
    fi
    if [[ $published_support == 1 || $published_app == 1 ]]; then
        pab_stop_binaries \
            "$app/Contents/MacOS/pab-desktop" "$upgrade/failed.app/Contents/MacOS/pab-desktop" \
            "$install_dir/pab-mcp" "$upgrade/failed-support/pab-mcp" \
            "$install_dir/pab-executor" "$upgrade/failed-support/pab-executor" || return
    fi
    if [[ -e $upgrade/previous.app ]]; then
        mv "$upgrade/previous.app" "$app" || return
    fi
    if [[ -e $upgrade/previous-support ]]; then
        mv "$upgrade/previous-support" "$install_dir" || return
    fi
}
