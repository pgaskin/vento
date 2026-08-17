#!/bin/bash
# QEMU with its own VNC server on 5900, its SPICE server on 5930 and the same
# SPICE server's TLS port on 5931, and a KolibriOS guest on the screen of all
# three.
set -e

MEMORY="${MEMORY:-256}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
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

# SPICE's ticket, which is a password encrypted to a key the server sends and
# not a challenge over one. Ticketing is on by default and refuses every
# connection until a ticket is set, so an empty password has to say so.
# The two encodings a client cannot ask for: JPEG and zlib-over-GLZ are what
# the server reaches for when it thinks the link is a WAN, and `always` is the
# only way to see either of them from a container on this network.
wan=""
if [ -n "${SPICE_WAN:-}" ]; then
    wan=",jpeg-wan-compression=always,zlib-glz-wan-compression=always"
fi

spicesecret=(-object "secret,id=spicepw,data=${SPICE_PASSWORD}")
spiceauth="password-secret=spicepw"
if [ -z "$SPICE_PASSWORD" ]; then
    spicesecret=()
    spiceauth="disable-ticketing=on"
fi

# The certificate is the image's, made when it was built, so the fingerprint a
# client pins is the same one after a restart.
echo "accel=$accel pointer=$POINTER memory=${MEMORY}M password=${VNC_PASSWORD:-none} spice=${SPICE_PASSWORD:-none}"
openssl x509 -in /certs/server-cert.pem -noout -fingerprint -sha256

# Read only, so the container keeps no state and every run is the same desktop.
exec qemu-system-x86_64 \
    -machine "type=pc,accel=$accel" \
    -m "$MEMORY" \
    -drive file=/kolibri.img,format=raw,if=floppy,readonly=on \
    -boot a \
    -vga std \
    "${secret[@]}" \
    -vnc "0.0.0.0:0${auth:+,$auth}" \
    "${spicesecret[@]}" \
    -spice "port=5930,tls-port=5931,addr=0.0.0.0,x509-dir=/certs,$spiceauth$wan" \
    "${tablet[@]}" \
    -nographic \
    -serial none \
    -monitor none
