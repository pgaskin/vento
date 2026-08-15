#!/bin/bash
# Build (if needed) and run the test desktop, then print the address to put in
# the prototype. The phone connects over the LAN, so the port is published on
# every interface and the address printed is the one the phone can reach.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protovnc}"
PORT="${PORT:-5901}"
GEOMETRY="${GEOMETRY:-1920x1200}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
SECURITY_TYPES="${SECURITY_TYPES:-VncAuth}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    shot)    # what the far end currently looks like, without a viewer
             out="${2:-/tmp/$NAME.png}"
             docker exec -e DISPLAY=:1 "$NAME" xwd -root -silent > "$out.xwd"
             magick xwd:"$out.xwd" "$out" && rm -f "$out.xwd"
             echo "$out"; exit 0 ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
    -p "$PORT:5900" \
    -e GEOMETRY="$GEOMETRY" \
    -e VNC_PASSWORD="$VNC_PASSWORD" \
    -e SECURITY_TYPES="$SECURITY_TYPES" \
    -e ACCEPT_RESIZE="${ACCEPT_RESIZE:-1}" \
    -e PLAIN_USER="${PLAIN_USER:-root}" \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  ($GEOMETRY, $SECURITY_TYPES, password '$VNC_PASSWORD')"
