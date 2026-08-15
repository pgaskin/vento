#!/bin/bash
# Build (if needed) and run the H.264 test desktop, then print the address to
# put in the app. The render node is passed through, since the encoding this
# rig exists for is done on it.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protoh264}"
PORT="${PORT:-5903}"
GEOMETRY="${GEOMETRY:-1920x1200}"
DRM_DEVICE="${DRM_DEVICE:-/dev/dri/renderD128}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    shot)    # what the far end currently looks like, without a viewer
             out="${2:-/tmp/$NAME.png}"
             docker exec -e XDG_RUNTIME_DIR=/tmp/xdg -e WAYLAND_DISPLAY=wayland-1 \
                 "$NAME" grim -o HEADLESS-1 - > "$out"
             echo "$out"; exit 0 ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
# The render group is the host's, by number: the container's own /etc/group
# knows nothing about it, and without membership the node opens read-only and
# every encode fails.
render_gid="$(stat -c %g "$DRM_DEVICE")"
# The host compositor's socket, which this rig nests in: a headless wlroots has
# no DRM file descriptor to export DMA-BUFs with, and without those neatvnc
# will not encode H.264 at all.
host_sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_DISPLAY:-wayland-0}"
if [ ! -S "$host_sock" ]; then
    echo "error: no Wayland compositor at $host_sock — this rig nests in one" >&2
    exit 1
fi
docker run -d --name "$NAME" \
    -p "$PORT:5900" \
    --device "$DRM_DEVICE" \
    --group-add "$render_gid" \
    -v "$host_sock:/tmp/xdg/wayland-host" \
    -e GEOMETRY="$GEOMETRY" \
    -e DRM_DEVICE="$DRM_DEVICE" \
    ${LIBVA_DRIVER_NAME:+-e LIBVA_DRIVER_NAME="$LIBVA_DRIVER_NAME"} \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  ($GEOMETRY, no authentication, nested in ${WAYLAND_DISPLAY:-wayland-0})"
