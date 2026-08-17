#!/bin/bash
# Build (if needed) and run a rendezvous server and relay of our own, then
# print what a client needs to use it: the address and the server's key.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protorustdeskserver}"
# A volume, so the key survives a restart: a client that has pinned a peer
# against one key would see every restart as the machine having changed.
VOLUME="${VOLUME:-$NAME-data}"

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    key)     exec docker exec "$NAME" cat /root/id_ed25519.pub ;;
    forget)  docker rm -f "$NAME" >/dev/null 2>&1 || true
             docker volume rm "$VOLUME" >/dev/null 2>&1 || true
             echo "the server's identity and its peer list are gone"; exit 0 ;;
    build)   docker build -t "$NAME:latest" . ; exit 0 ;;
esac

# Two modes of the same server rather than two containers. `relay` tells every
# client to relay rather than punch, which is the only way to reach that path on
# a network where a punch always lands; `open` stops the server insisting a
# client names its key, which is what lets a client arrive with the wrong one
# and fail to verify the introduction it is given.
RELAY_ONLY=N
ENFORCE_KEY=Y
[ "$1" = relay ] && RELAY_ONLY=Y
[ "$1" = open ] && ENFORCE_KEY=N

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"

docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
    -v "$VOLUME:/root" \
    -p 21115:21115 \
    -p 21116:21116 \
    -p 21116:21116/udp \
    -p 21117:21117 \
    -e RELAY="$ip" \
    -e ALWAYS_USE_RELAY="$RELAY_ONLY" \
    -e ENFORCE_KEY="$ENFORCE_KEY" \
    "$NAME:latest" >/dev/null

key="$(docker exec "$NAME" sh -c 'for i in $(seq 100); do [ -f /root/id_ed25519.pub ] && cat /root/id_ed25519.pub && exit 0; sleep 0.1; done')"
echo "$NAME up: $ip:21116  (relay $ip:21117$([ "$RELAY_ONLY" = Y ] && echo ", every connection")$([ "$ENFORCE_KEY" = N ] && echo ", any key"))"
echo "key: $key"
echo
echo "a peer:   RENDEZVOUS=$ip KEY=$key scripts/testrustdesk/run.sh"
echo "a client: cargo run --example rustdesk-probe -- id <peer-id> --server $ip --key $key"
