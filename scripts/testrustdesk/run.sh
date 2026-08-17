#!/bin/bash
# Build (if needed) and run the RustDesk test desktop, then print what a client
# needs to reach it: the address for direct IP access, and the peer id if the
# container has registered with a rendezvous network.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protorustdesk}"
PORT="${PORT:-21118}"
GEOMETRY="${GEOMETRY:-1920x1200}"
PASSWORD="${PASSWORD:-rustdeskpass}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    # Their own binary, because the id is in the config in their obfuscated
    # form and nothing else can read it. Once, and not while the server is
    # starting: it is a second process against the same config, and one racing
    # the server rewrote the file with the permanent password gone — which
    # looks from a client like a peer that has started refusing a password
    # nobody changed.
    id)      exec docker exec "$NAME" rustdesk --get-id ;;
    shot)    # what the far end currently looks like, without a viewer
             out="${2:-/tmp/$NAME.png}"
             docker exec -e DISPLAY=:1 -e XAUTHORITY=/root/.Xauthority "$NAME" \
                 sh -c 'xwd -root -silent | convert xwd:- png:-' > "$out" 2>/dev/null ||
             docker exec -e DISPLAY=:1 -e XAUTHORITY=/root/.Xauthority "$NAME" \
                 xwd -root -silent > "${out%.png}.xwd"
             echo "${out}"; exit 0 ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
# The host's GPU, where there is one: their server asks VA-API what it can
# encode, and with nothing to ask it settles for VP9, VP8 and AV1 in software.
# It is the only far end here that can be made to send H.264 or H.265, so the
# codec row has something to be checked against — and it is the host's hardware
# rather than the rig's, which is why a run says which machine it was on.
DRI=()
[ -e /dev/dri ] && DRI=(--device /dev/dri)

docker run -d --name "$NAME" \
    -p "$PORT:21118" \
    "${DRI[@]}" \
    -e GEOMETRY="$GEOMETRY" \
    -e PASSWORD="$PASSWORD" \
    ${RENDEZVOUS:+-e RENDEZVOUS="$RENDEZVOUS"} \
    ${KEY:+-e KEY="$KEY"} \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  ($GEOMETRY, password $PASSWORD)"
# After the server has settled, and asked once: see the `id` case above.
sleep 3
echo "peer id: $(docker exec "$NAME" rustdesk --get-id 2>/dev/null | grep -E '^[0-9]+$' || echo '(none yet)')"
