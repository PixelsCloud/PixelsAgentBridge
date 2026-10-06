#!/usr/bin/env bash
# Shared by the Linux installer/uninstaller; never matches processes by name alone.
has_systemd() {
    [[ -d /run/systemd/system ]] && command -v systemctl >/dev/null 2>&1
}

installed_processes() {
    local directory=$1 process executable
    for process in /proc/[0-9]*; do
        executable=$(readlink "$process/exe" 2>/dev/null || true)
        case $executable in
            "$directory"/pab-mcp|"$directory"/pab-executor|"$directory"/pab-bridge|"$directory"/pab-desktop)
                printf '%s\n' "${process##*/}" ;;
        esac
    done
}

stop_installed_processes() {
    local directory=$1 pid attempt
    while read -r pid; do
        [[ -z $pid ]] || kill -TERM "$pid" 2>/dev/null || true
    done < <(installed_processes "$directory")
    for ((attempt=0; attempt<30; attempt++)); do
        [[ -n $(installed_processes "$directory") ]] || return 0
        sleep 1
    done
    while read -r pid; do
        [[ -z $pid ]] || kill -KILL "$pid" 2>/dev/null || true
    done < <(installed_processes "$directory")
    for ((attempt=0; attempt<10; attempt++)); do
        [[ -n $(installed_processes "$directory") ]] || return 0
        sleep 1
    done
    echo 'Installed product processes are still running; files were not replaced/removed' >&2
    return 1
}
