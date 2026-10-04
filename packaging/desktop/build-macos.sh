#!/usr/bin/env bash
set -euo pipefail

[[ $(uname -s) == Darwin ]] || { echo 'Build on macOS.' >&2; exit 2; }
profile=${1:-debug}
architecture=${2:-native}
[[ $# -le 2 && ( $profile == debug || $profile == release ) ]] || {
    echo 'usage: bash packaging/desktop/build-macos.sh [debug|release] [native|aarch64|x86_64|all]' >&2; exit 2;
}
case $architecture in native|arm64|aarch64|x86_64|all) ;; *) echo 'Invalid architecture.' >&2; exit 2 ;; esac

# Non-interactive Executor sessions do not inherit the user's shell profile.
for directory in /usr/local/opt/rustup/bin /opt/homebrew/opt/rustup/bin /usr/local/bin /opt/homebrew/bin; do
    if [[ -d $directory ]]; then export PATH="$directory:$PATH"; fi
done
python=
for candidate in python3.14 python3.13 python3.12 python3; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 12))'; then
        python=$(command -v "$candidate")
        break
    fi
done
[[ -n $python ]] || { echo 'Python 3.12+ is required (system Python 3.9 is too old).' >&2; exit 2; }
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
exec "$python" scripts/build.py macos --profile "$profile" --macos-arch "$architecture" --package
