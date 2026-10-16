#!/usr/bin/env bash
set -euo pipefail
if [[ $EUID -eq 0 ]]; then
    echo 'Open Pixels from your normal desktop account; the background service runs separately.' >&2
    exit 1
fi
# VMware's virtual GPU can corrupt WebKitGTK's DMABUF presentation on Wayland.
# Keep this workaround specific to that environment and honor explicit WebKit
# overrides. Other systems retain their normal accelerated rendering path.
if [[ -z ${WEBKIT_DISABLE_DMABUF_RENDERER+x} && -r /sys/class/dmi/id/sys_vendor ]] \
    && grep -qi vmware /sys/class/dmi/id/sys_vendor; then
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
fi
exec /opt/pixels-agent-bridge/pab-desktop "$@"
