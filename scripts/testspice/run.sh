#!/bin/bash
# Build (if needed) and run the SPICE rig whose guest runs the agent, then
# print the address to put in the app. `testqemu`'s pattern, with the one
# difference this container is for: there is a `spice-vdagent` at the far end,
# so there is a clipboard, a resize and a monitor layout.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protospice}"
SPICE_PORT="${SPICE_PORT:-5940}"
SPICE_TLS_PORT="${SPICE_TLS_PORT:-5941}"
MEMORY="${MEMORY:-768}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
AGENT="${AGENT:-on}"
HEADS="${HEADS:-1}"
POINTER="${POINTER:-absolute}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
    # A command inside the guest, which is how this rig is checked: `xclip -o`
    # for what the clipboard holds, `xrandr` for what size the desktop is.
    guest)   shift; exec docker exec "$NAME" guest-cmd "$@" ;;
    # What the guest's own X server thinks is on the screen, without going
    # through SPICE at all — the reference for anything the app draws.
    shot)    out="${2:-/tmp/$NAME.png}"
             docker exec "$NAME" guest-cmd 'xwd -root -silent > /tmp/shot.xwd' >/dev/null
             docker exec "$NAME" guest-cmd 'cat /tmp/shot.xwd' > "${out%.png}.xwd"
             echo "${out%.png}.xwd"; exit 0 ;;
    console) exec docker exec -it "$NAME" socat -,raw,echo=0 UNIX-CONNECT:/run/guest-tty.sock ;;
esac

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true

kvm=()
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    kvm=(--device /dev/kvm)
fi

# NET_ADMIN only for SLOW, which puts a qdisc on the container's own interface.
net=()
[ -n "${SLOW:-}" ] && net=(--cap-add NET_ADMIN)

docker run -d --name "$NAME" "${kvm[@]}" "${net[@]}" \
    -p "$SPICE_PORT:5930" \
    -p "$SPICE_TLS_PORT:5931" \
    -e MEMORY="$MEMORY" \
    -e SPICE_PASSWORD="$SPICE_PASSWORD" \
    -e AGENT="$AGENT" \
    -e HEADS="$HEADS" \
    -e POINTER="$POINTER" \
    -e SPICE_WAN="${SPICE_WAN:-}" \
    -e SLOW="${SLOW:-}" \
    "$NAME:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$SPICE_PORT  (password '$SPICE_PASSWORD', agent $AGENT, pointer $POINTER), TLS on $ip:$SPICE_TLS_PORT"
echo "the guest takes a few seconds to reach its desktop"
