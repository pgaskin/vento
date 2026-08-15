#!/bin/bash
# Build (if needed) and run the RDP test desktop, then print the address to put
# in the prototype. testvnc/run.sh's pattern exactly, because the point of this
# container is that the same desktop is reachable two ways.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protordp}"
PORT="${PORT:-3389}"
RDP_USER="${RDP_USER:-proto}"
RDP_PASSWORD="${RDP_PASSWORD:-protopass}"
SECURITY_LAYER="${SECURITY_LAYER:-tls}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    shot)    # what the far end currently looks like, without a viewer
             out="${2:-/tmp/$NAME.png}"
             display="$(docker exec "$NAME" sh -c 'ls /tmp/.X11-unix | head -1 | tr -d X')"
             docker exec -e DISPLAY=":${display:-10}" -u "$RDP_USER" "$NAME" \
                 xwd -root -silent > "$out.xwd"
             magick xwd:"$out.xwd" "$out" && rm -f "$out.xwd"
             echo "$out"; exit 0 ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
    -p "$PORT:3389" \
    -e RDP_USER="$RDP_USER" \
    -e RDP_PASSWORD="$RDP_PASSWORD" \
    -e SECURITY_LAYER="$SECURITY_LAYER" \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  ($SECURITY_LAYER, user '$RDP_USER', password '$RDP_PASSWORD')"
