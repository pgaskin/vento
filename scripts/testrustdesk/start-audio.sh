#!/bin/bash
# The audio image's entry point: a PulseAudio server whose default sink is a
# null sink, the tone playing into it, and then start.sh with their server's
# audio switched on. See Dockerfile.audio.
set -e

export XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# A user server for root, which is who their server runs as here: no
# configuration file of its own (-n), only the socket and the sink, and it
# never exits for being idle.
pulseaudio --daemonize=yes -n --exit-idle-time=-1 \
    --load=module-native-protocol-unix \
    --load="module-null-sink sink_name=desktop sink_properties=device.description=desktop"
for _ in $(seq 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl set-default-sink desktop
echo "pulseaudio: default sink $(pactl get-default-sink)"

# The guest's serial line, as a pty that socat connects to one TCP client at
# a time; the tone is started again with each, since the pty is new. The
# silence is a 1-LSB square rather than zeros, because their server sends
# nothing at all while its desktop is silent (tone.c).
(
    while :; do
        rm -f /tmp/tone-tty
        socat PTY,link=/tmp/tone-tty,raw,echo=0 TCP-LISTEN:4555,reuseaddr &
        bridge=$!
        for _ in $(seq 50); do [ -e /tmp/tone-tty ] && break; sleep 0.1; done
        tone pulse /tmp/tone-tty 1 &
        player=$!
        wait "$bridge" || true
        kill "$player" 2>/dev/null || true
        wait "$player" 2>/dev/null || true
    done
) &

# Their connection manager records the sound and hands it to their server
# over the `_pa` socket. Their server starts it (`--cm`) for each session, as
# on a desktop, through sudo as the user loginctl reports logged in
# (Dockerfile.audio), and it exits a few seconds after the session ends.

export AUDIO_ENABLE=Y
exec /usr/local/bin/start.sh
