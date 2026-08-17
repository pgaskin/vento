# A SPICE server with a guest that runs the **agent**, which is the half of the
# protocol `testqemu` cannot serve: the clipboard, the resize and the monitor
# layout all travel over `spice-vdagent` inside the guest, and KolibriOS has no
# such thing.
#
# So this is the second QEMU rig, and the guest is a Linux built here rather
# than downloaded: Alpine's packages laid into a root of their own, an ext4
# image made out of that root without root privileges (`mke2fs -d`), and the
# kernel and initramfs beside it. That is a guest with an X server, a window
# manager, `spice-vdagent`, and a serial console the host can run commands down
# — which is what makes a clipboard test something a script can check rather
# than something a person has to look at.
#
# The disk is attached with `snapshot=on`, so a run writes to a temporary
# overlay and the image itself is the same desktop every time.

FROM alpine:3.22 AS guest

# `apk --root` builds somebody else's root out of this one's index; the keys
# and the repository list have to be put there first, since --initdb makes
# neither.
RUN apk add --no-cache e2fsprogs mkinitfs imagemagick

RUN mkdir -p /guest/etc/apk \
    && cp -r /etc/apk/keys /guest/etc/apk/ \
    && cp /etc/apk/repositories /guest/etc/apk/ \
    && apk add --root /guest --initdb --no-cache \
        alpine-baselayout busybox busybox-suid musl-utils \
        linux-virt \
        eudev dbus \
        xorg-server xinit xauth xf86-video-qxl xf86-input-libinput \
        spice-vdagent \
        xrandr xdpyinfo xsetroot setxkbmap xclip xwd xdotool \
        font-misc-misc font-cursor-misc \
        jwm xterm feh

# Something photographic to look at, which is the only way to reach the two
# encodings a server keeps for a link it thinks is a WAN: QEMU decides between
# JPEG and a lossless codec by how gradual an image is, and a desktop of flat
# panels and dithered edges is never a photograph.
RUN magick -size 1024x768 plasma:fractal -blur 0x2 /guest/root/photo.png

# The guest's own files: what PID 1 runs, and what X starts.
COPY guest/ /guest/
RUN chmod +x /guest/sbin/guest-init /guest/root/.xinitrc \
        /guest/usr/local/bin/apply-preferred-mode /guest/usr/local/bin/show-photo

# The kernel out of the guest root, and an initramfs made for it: virtio for
# the disk, ext4 to mount it, and nothing else — this guest has one device and
# one filesystem.
RUN kver="$(ls /guest/lib/modules)" \
    && cp "/guest/boot/vmlinuz-virt" /vmlinuz \
    && mkinitfs -b /guest -F "base virtio ext4" -o /initramfs "$kver"

# 1.5 GB is the image; what is in it is about 500 MB, and the rest is where a
# session writes. `-d` populates from a directory, which is the one way to make
# a filesystem image without being root.
RUN mke2fs -q -t ext4 -d /guest -F /guest.img 1500M


FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        qemu-system-x86 \
        qemu-system-modules-spice \
        seabios \
        ipxe-qemu \
        openssl \
        python3 \
        socat \
        iproute2 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=guest /guest.img /vmlinuz /initramfs /guest/

# The same certificate arrangement `testqemu` has, and for the same reason: made
# when the image is built, so the fingerprint a phone pins survives a restart
# and changes only when somebody rebuilds on purpose.
RUN mkdir -p /certs \
    && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -keyout /certs/ca-key.pem -out /certs/ca-cert.pem \
        -subj "/CN=protospice ca" 2>/dev/null \
    && openssl req -newkey rsa:2048 -nodes \
        -keyout /certs/server-key.pem -out /certs/server-req.pem \
        -subj "/CN=protospice" 2>/dev/null \
    && openssl x509 -req -in /certs/server-req.pem -days 3650 \
        -CA /certs/ca-cert.pem -CAkey /certs/ca-key.pem -set_serial 1 \
        -out /certs/server-cert.pem 2>/dev/null \
    && rm -f /certs/server-req.pem \
    && openssl x509 -in /certs/server-cert.pem -noout -fingerprint -sha256

COPY start.sh guest-cmd /usr/local/bin/
RUN chmod +x /usr/local/bin/start.sh /usr/local/bin/guest-cmd

EXPOSE 5930 5931
ENTRYPOINT ["/usr/local/bin/start.sh"]
