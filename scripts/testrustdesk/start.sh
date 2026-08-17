#!/bin/bash
# Xvfb plus an Openbox session on :1, and RustDesk's own server over it,
# answering direct IP access on 21118.
set -e

GEOMETRY="${GEOMETRY:-1920x1200}"
PASSWORD="${PASSWORD:-rustdeskpass}"
DIRECT_PORT="${DIRECT_PORT:-21118}"
# Empty means their default, the public network, which is what makes the id
# path testable with nothing stood up — the server only introduces the two ends
# and a punch on one LAN keeps the session off it. A self-hosted hbbs goes here
# as host:port; `none` registers with nothing and leaves direct IP as the only
# way in.
RENDEZVOUS="${RENDEZVOUS:-}"
KEY="${KEY:-}"
if [ "$RENDEZVOUS" = none ]; then
    RENDEZVOUS=127.0.0.1
fi

export DISPLAY=:1
export XAUTHORITY=/root/.Xauthority
touch "$XAUTHORITY"

Xvfb :1 -screen 0 "${GEOMETRY}x24" -nolisten tcp -auth "$XAUTHORITY" &
XVFB_PID=$!

for _ in $(seq 50); do
    if xdpyinfo >/dev/null 2>&1; then break; fi
    sleep 0.1
done

xsetroot -cursor_name left_ptr
openbox &
# After openbox rather than before it, which is the other rigs' order and the
# wrong one here: this X server has no persistent root pixmap, so a background
# set before the window manager starts is the black root again a moment later.
sleep 1
feh --no-fehbg --bg-fill /usr/share/wallpaper.png || true

xterm -geometry 100x30+60+80 -fa 'DejaVu Sans Mono' -fs 11 \
      -bg '#101418' -fg '#e8f0ff' -sb -sl 5000 &
mousepad /root/Documents/scrollme.txt &
xclock -geometry 200x200-60+80 -update 1 &
xcalc -geometry -60-80 &

# Written rather than set through their CLI: `--password` goes over an ipc
# socket to a running service and refuses unless the binary thinks it is
# installed, which in a container it is not. A value that is not their
# obfuscated form is read back as itself, so the plain one is accepted.
mkdir -p /root/.config/rustdesk
if [ ! -f /root/.config/rustdesk/RustDesk.toml ]; then
    printf "password = '%s'\n" "$PASSWORD" > /root/.config/rustdesk/RustDesk.toml
fi
# verification-method, or a permanent password is set and connections are still
# asked about on a desktop with nobody at it; approve-mode goes with it.
{
    printf "rendezvous_server = '%s'\n" "$RENDEZVOUS"
    printf "nat_type = 0\nserial = 0\n\n[options]\n"
    printf "direct-server = 'Y'\n"
    printf "direct-access-port = '%s'\n" "$DIRECT_PORT"
    printf "verification-method = 'use-permanent-password'\n"
    printf "approve-mode = 'password'\n"
    printf "enable-audio = 'N'\n"
    [ -n "$RENDEZVOUS" ] && printf "custom-rendezvous-server = '%s'\n" "$RENDEZVOUS"
    [ -n "$KEY" ] && printf "key = '%s'\n" "$KEY"
} > /root/.config/rustdesk/RustDesk2.toml

echo "geometry=$GEOMETRY password=$PASSWORD direct-port=$DIRECT_PORT"

rustdesk --server &
SERVER_PID=$!

# The id is not printed here, and that is deliberate. It is generated on first
# start and written in their obfuscated form, so the only way to read it is to
# ask their own binary — which is a second process against the same config, and
# one racing the server as it settles rewrote the file with the permanent
# password gone. `run.sh id` asks once, later, which is what a person would do.

wait "$SERVER_PID" "$XVFB_PID"
