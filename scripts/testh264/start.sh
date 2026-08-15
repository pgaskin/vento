#!/bin/bash
# Sway nested in the host's compositor, with wayvnc serving it on 5900.
set -e

GEOMETRY="${GEOMETRY:-1920x1200}"
DRM_DEVICE="${DRM_DEVICE:-/dev/dri/renderD128}"
HOST_DISPLAY="${HOST_DISPLAY:-wayland-host}"

export XDG_RUNTIME_DIR=/tmp/xdg
export XDG_SESSION_TYPE=wayland
export LIBVA_DRIVER_NAME="${LIBVA_DRIVER_NAME:-iHD}"
chmod 700 "$XDG_RUNTIME_DIR"

# Nested rather than headless, and it has to be. neatvnc will only encode a
# frame that arrived as a GBM buffer object — "h264 is useless for sw frames" —
# so wayvnc has to capture into a DMA-BUF, which needs a DRM file descriptor
# from the compositor's backend. wlroots' headless backend has no such
# descriptor at all and its linux-dmabuf global fails to start; its Wayland
# backend takes one from the compositor it is nested in. So this rig runs
# inside the desktop session of whatever machine it is started on, and the
# desktop being served appears in a window there.
if [ ! -S "$XDG_RUNTIME_DIR/$HOST_DISPLAY" ]; then
    echo "error: no host compositor at $XDG_RUNTIME_DIR/$HOST_DISPLAY." >&2
    echo "This rig nests in one; a headless wlroots cannot export DMA-BUFs" >&2
    echo "and so can never send H.264. Start it from a Wayland session." >&2
    exit 1
fi
if [ ! -e "$DRM_DEVICE" ]; then
    echo "error: no $DRM_DEVICE — run with --device /dev/dri" >&2
    exit 1
fi
# What the encoder will actually be able to do, printed before anything starts:
# an H264 EncSlice entry here is the difference between this rig sending H.264
# and it quietly falling back to Tight.
vainfo --display drm --device "$DRM_DEVICE" 2>&1 | grep -E 'driver version|H264.*Enc' || true

mkdir -p /root/.config/sway
cat > /root/.config/sway/config <<EOF
# Two backends, and the desktop being served is the headless one. The Wayland
# backend is here for its DRM file descriptor, which is what the compositor
# allocates GBM buffers with and so what makes any output of it capturable as a
# DMA-BUF; the headless output is what gives this rig a size of its own, since
# the compositor the window lives in decides how big a window is and a tiling
# one will not be told. Its own window stays blank on purpose — nothing is ever
# put on that output — and it can be ignored.
output HEADLESS-1 resolution ${GEOMETRY/x/ } position 0 0
output WL-1 position 4000 0
output HEADLESS-1 bg /usr/share/wallpaper.png fill
default_border none
xwayland enable

# The same four clients scripts/testvnc runs, for the same reasons: text to
# select, a document to scroll, small targets, and one region that repaints on
# its own while nothing else does — which is the worst case for a codec that
# sends whole frames and the reason this rig has a clock at all.
exec xterm -geometry 100x30+60+80 -fa 'DejaVu Sans Mono' -fs 11 \
     -bg '#101418' -fg '#e8f0ff' -sb -sl 5000
exec mousepad /root/Documents/scrollme.txt
exec xclock -geometry 200x200-60+80 -update 1
exec xcalc -geometry -60-80
EOF

echo "geometry=$GEOMETRY nested in $HOST_DISPLAY device=$DRM_DEVICE"

WAYLAND_DISPLAY="$HOST_DISPLAY" WLR_BACKENDS=wayland,headless sway &
SWAY_PID=$!

export WAYLAND_DISPLAY=wayland-1
for _ in $(seq 100); do
    if [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]; then break; fi
    sleep 0.1
done

# --gpu is what makes wayvnc capture into a DMA-BUF rather than shared memory,
# and so the whole of why this rig exists. Without it the server is a perfectly
# ordinary Tight one.
wayvnc --gpu --verbose -o HEADLESS-1 0.0.0.0 5900 &
WAYVNC_PID=$!

wait "$SWAY_PID" "$WAYVNC_PID"
