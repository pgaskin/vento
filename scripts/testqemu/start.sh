#!/bin/bash
# QEMU with its own VNC server on 5900, and a KolibriOS guest on the screen.
set -e

MEMORY="${MEMORY:-256}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
# Absolute is what a `usb-tablet` gives, and it is here only so that the two
# modes can be compared against the same guest — the point of the container is
# the default.
POINTER="${POINTER:-relative}"

accel="tcg"
if [ -e /dev/kvm ] && [ -w /dev/kvm ]; then
    accel="kvm"
fi

tablet=()
if [ "$POINTER" = "absolute" ]; then
    tablet=(-device usb-ehci -device usb-tablet)
fi

# A VNC password rather than none, so this rig exercises the same VncAuth path
# the others do. QEMU takes it as a secret object; eight characters is the
# protocol's limit, not ours.
secret=(-object "secret,id=vncpw,data=${VNC_PASSWORD}")
auth="password-secret=vncpw"
if [ -z "$VNC_PASSWORD" ]; then
    secret=()
    auth=""
fi

echo "accel=$accel pointer=$POINTER memory=${MEMORY}M password=${VNC_PASSWORD:-none}"

# Read only, so the container keeps no state and every run is the same desktop.
exec qemu-system-x86_64 \
    -machine "type=pc,accel=$accel" \
    -m "$MEMORY" \
    -drive file=/kolibri.img,format=raw,if=floppy,readonly=on \
    -boot a \
    -vga std \
    "${secret[@]}" \
    -vnc "0.0.0.0:0${auth:+,$auth}" \
    "${tablet[@]}" \
    -nographic \
    -serial none \
    -monitor none
