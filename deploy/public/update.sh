#!/bin/sh
set -eu

cd "$(dirname "$0")"

if [ "${COMMONWAKE_UPDATE_MODE:-registry}" = "source" ]; then
    echo "Commonwake is pinned to a locally built source image; registry update skipped"
    exit 0
fi

current_image="$(docker compose images -q commonwake 2>/dev/null || true)"
if [ -n "$current_image" ]; then
    docker image tag "$current_image" commonwake:rollback
fi

docker compose pull commonwake
if docker compose up -d --pull never --wait --wait-timeout 120 commonwake; then
    exit 0
fi

if [ -z "$current_image" ]; then
    echo "Commonwake failed its first health check and no prior image exists" >&2
    exit 1
fi

echo "Commonwake update failed its health check; restoring the prior image" >&2
COMMONWAKE_IMAGE=commonwake:rollback \
    docker compose up -d --pull never --wait --wait-timeout 120 commonwake
exit 1
