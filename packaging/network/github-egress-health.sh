#!/bin/sh
set -eu
# Supply a private EnvironmentFile with PAB_GITHUB_PROXY_URL.
: "${PAB_GITHUB_PROXY_URL:?configure the GitHub egress proxy}"
for host in github.com api.github.com; do
    if ! curl --proxy "$PAB_GITHUB_PROXY_URL" --noproxy '' \
        --silent --fail --connect-timeout 5 --max-time 15 \
        --output /dev/null "https://$host"; then
        echo "GitHub egress health check failed for $host" >&2
        exit 1
    fi
done
