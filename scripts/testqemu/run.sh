#!/bin/bash
# Build (if needed) and run the relative-pointer test server, then print the
# address to put in the prototype. testvnc/run.sh's pattern, with the one
# difference this container is for: the pointer at the far end is relative.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protoqemu}"
PORT="${PORT:-5902}"
SPICE_PORT="${SPICE_PORT:-5930}"
SPICE_TLS_PORT="${SPICE_TLS_PORT:-5931}"
MEMORY="${MEMORY:-512}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
POINTER="${POINTER:-relative}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true

# KVM if this machine has it and will lend it out; Tiny Core boots under plain
# TCG too, just slower.
kvm=()
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    kvm=(--device /dev/kvm)
fi

docker run -d --name "$NAME" "${kvm[@]}" \
    -p "$PORT:5900" \
    -p "$SPICE_PORT:5930" \
    -p "$SPICE_TLS_PORT:5931" \
    -e MEMORY="$MEMORY" \
    -e VNC_PASSWORD="$VNC_PASSWORD" \
    -e SPICE_PASSWORD="$SPICE_PASSWORD" \
    -e POINTER="$POINTER" \
    -e SPICE_WAN="${SPICE_WAN:-}" \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  (pointer $POINTER, password '$VNC_PASSWORD')"
echo "     spice: $ip:$SPICE_PORT  (password '$SPICE_PASSWORD'), TLS on $ip:$SPICE_TLS_PORT"
echo "the guest takes a few seconds to reach its desktop"
