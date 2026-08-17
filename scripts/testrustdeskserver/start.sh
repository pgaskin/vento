#!/bin/bash
# hbbr, then hbbs, and the key printed once it exists.
set -e

# Where clients are told the relay is. It has to be an address the *client* can
# reach rather than one this container can, so it is the host's, passed in.
RELAY="${RELAY:-}"
# Y makes hbbs answer every punch with a relay instead, which is the only way
# to exercise that path on a network where every punch lands.
ALWAYS_USE_RELAY="${ALWAYS_USE_RELAY:-N}"
export ALWAYS_USE_RELAY
# Whether a client has to name the server's key to be answered at all. It does
# by default — their `-k` defaults to the server's own public key — and turning
# it off is what makes a client with the *wrong* key reach the introduction and
# fail to verify it, which is the only way to reach that path deliberately.
ENFORCE_KEY="${ENFORCE_KEY:-Y}"

hbbr &
HBBR_PID=$!

if [ "$ENFORCE_KEY" = Y ]; then
    hbbs ${RELAY:+-r "$RELAY"} &
else
    hbbs ${RELAY:+-r "$RELAY"} -k '' &
fi
HBBS_PID=$!

# Generated on first start, in the working directory, and it is what a client
# has to be given: the public half is what every peer key is checked against.
for _ in $(seq 100); do
    if [ -f /root/id_ed25519.pub ]; then
        echo "key: $(cat /root/id_ed25519.pub)"
        break
    fi
    sleep 0.1
done

wait "$HBBS_PID" "$HBBR_PID"
