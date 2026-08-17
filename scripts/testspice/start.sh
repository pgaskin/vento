#!/bin/bash
# QEMU with a SPICE server on 5930, its TLS port on 5931, and a Linux guest
# that runs `spice-vdagent` on the screen of both.
set -e

MEMORY="${MEMORY:-768}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
# The agent is what this rig is for, so it is on by default; off is how to see
# the same guest as a far end with no clipboard, no resize and a server-mode
# pointer, which is what `testqemu`'s guest always is.
#
# There is deliberately no `usb-tablet` here in either case: with one, the
# server offers client mouse mode whether or not an agent is running, and the
# thing this rig is for is that the *agent* is what makes the pointer absolute.
AGENT="${AGENT:-on}"
# Which end owns the cursor, and on this guest that is decided by a *driver*
# rather than by a device: Linux's psmouse binds QEMU's VMMouse, a PS/2 mouse
# with an absolute mode behind the VMware port, so QEMU has an absolute pointer
# and SPICE offers client mode — with no tablet attached and no agent running.
# `vmport=off` takes the port away and leaves a plain PS/2 mouse; `agent-mouse=off`
# stops the agent supplying an absolute pointer instead, which leaves a session
# with a relative pointer and a clipboard.
POINTER="${POINTER:-absolute}"
# How many heads the one QXL device has. More than one is the layout SPICE
# announces where every other protocol here leaves it to be guessed at, and the
# guest has to be told to use the second one (`xrandr --output Virtual-2`).
HEADS="${HEADS:-1}"

accel="tcg"
if [ -e /dev/kvm ] && [ -w /dev/kvm ]; then
    accel="kvm"
fi

# The two encodings a client cannot ask for, as in `testqemu`: a server reaches
# for them when it decides the link is a WAN.
wan=""
if [ -n "${SPICE_WAN:-}" ]; then
    wan=",jpeg-wan-compression=always,zlib-glz-wan-compression=always"
fi

vmport=""
agentmouse=""
if [ "$POINTER" = "relative" ]; then
    vmport=",vmport=off"
    agentmouse=",agent-mouse=off"
fi

spicesecret=(-object "secret,id=spicepw,data=${SPICE_PASSWORD}")
spiceauth="password-secret=spicepw"
if [ -z "$SPICE_PASSWORD" ]; then
    spicesecret=()
    spiceauth="disable-ticketing=on"
fi

# The agent's own port: a virtio-serial device the SPICE server writes into,
# under the one name `spice-vdagentd` looks for.
agent=()
if [ "$AGENT" = "on" ]; then
    agent=(-chardev spicevmc,id=vdagent,name=vdagent
           -device virtserialport,chardev=vdagent,name=com.redhat.spice.0)
fi

# A second port, which is how the host runs a command inside the guest — see
# `guest-cmd`. Nothing about SPICE goes down it.
rm -f /run/guest-cmd.sock /run/guest-tty.sock

# A link the server thinks is a WAN, which is the only way to reach JPEG and
# zlib-over-GLZ: the two are not a switch but a *measurement*, taken by the
# server off the main channel's own ping at connect time, and a container on
# this network is never slow enough to be one.
if [ -n "${SLOW:-}" ]; then
    tc qdisc add dev eth0 root netem rate "${SLOW}" delay 40ms 2>/dev/null \
        && echo "link throttled to ${SLOW} with 40 ms of delay" \
        || echo "cannot throttle the link: the container needs NET_ADMIN"
fi

echo "accel=$accel memory=${MEMORY}M agent=$AGENT pointer=$POINTER heads=$HEADS spice=${SPICE_PASSWORD:-none}"
openssl x509 -in /certs/server-cert.pem -noout -fingerprint -sha256

# `snapshot=on`: writes go to a temporary overlay, so the image is the same
# desktop on every run and the container keeps no state.
exec qemu-system-x86_64 \
    -machine "type=q35,accel=$accel$vmport" \
    -m "$MEMORY" \
    -smp 2 \
    -kernel /guest/vmlinuz \
    -initrd /guest/initramfs \
    -append "root=/dev/vda rootfstype=ext4 rw console=ttyS0,115200" \
    -drive file=/guest/guest.img,format=raw,if=virtio,snapshot=on \
    -device "qxl-vga,ram_size=134217728,vram_size=134217728,max_outputs=$HEADS" \
    "${spicesecret[@]}" \
    -spice "port=5930,tls-port=5931,addr=0.0.0.0,x509-dir=/certs,$spiceauth$wan$agentmouse" \
    -device virtio-serial \
    "${agent[@]}" \
    -chardev socket,id=guestcmd,path=/run/guest-cmd.sock,server=on,wait=off \
    -device virtserialport,chardev=guestcmd,name=net.pgaskin.guestcmd \
    -serial file:/dev/stdout \
    -chardev socket,id=guesttty,path=/run/guest-tty.sock,server=on,wait=off \
    -serial chardev:guesttty \
    -display none \
    -monitor none
