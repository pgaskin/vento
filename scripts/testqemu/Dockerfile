# A VNC server whose pointer is *relative*, which nothing else in this
# repository can be.
#
# A far end that owns the cursor has exactly one free implementation in RFB:
# QEMU's built-in
# VNC server, which offers pseudo-encoding −257 and then reads a PointerEvent's
# coordinates as deltas biased by 0x7FFF. TigerVNC has none, so `testvnc`
# cannot answer the question this container exists to answer.
#
# The guest is **KolibriOS**, from its own 1.4 MB floppy image: it boots to a
# graphical desktop in about three seconds, at 1024×768, with windows to drag
# and a pointer **drawn by the guest into the framebuffer** — which is exactly
# the world a relative client lives in, since a PS/2 mouse has no hardware
# cursor for QEMU to forward as a `Cursor` rectangle.
#
# Tiny Core was tried first and is not usable here: its Xvesa reaches the VESA
# BIOS through /dev/mem, which a modern kernel refuses, so X never starts and
# the guest has no pointer to look at.
#
# No `usb-tablet`, deliberately: that is the device that would make the pointer
# absolute and turn this container into a slower `testvnc`.
#
# It also serves **SPICE**, on the same QEMU and the same guest, which is the
# only place here where one far end speaks two protocols. Debian splits the
# server out of `qemu-system-x86` into `qemu-system-modules-spice`, so without
# that package `-spice` is not an option group at all.
#
# SPICE's TLS is a **second port** rather than a negotiation, so it is a second
# port here too: 5930 plain and 5931 with a certificate generated when this
# image is built. Built rather than started, so that the fingerprint the phone
# pins survives a restart of the container and a *changed* one is something the
# rig can be made to do on purpose by rebuilding.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        qemu-system-x86 \
        qemu-system-modules-spice \
        seabios \
        ipxe-qemu \
        ca-certificates \
        openssl \
        curl \
        libarchive-tools \
    && rm -rf /var/lib/apt/lists/*

# Pinned by hash like everything else here. This one is a *nightly* build and
# upstream publishes no dated URL, so the pin will eventually fail — which is
# the point: a changed image is something to look at rather than something to
# discover on a phone. When it does, download it, check it boots, and put the
# new hash here.
ARG KOLIBRI_URL=https://builds.kolibrios.org/en_US/latest-img.7z
ARG KOLIBRI_SHA256=d90f8fdb574f3ab6be98f899f96244433bf0e1ff071c58736d4e55fa1aa73f54
RUN curl -fsSL -o /kolibri.7z "${KOLIBRI_URL}" \
    && echo "${KOLIBRI_SHA256}  /kolibri.7z" | sha256sum -c - \
    && bsdtar -xf /kolibri.7z -C / kolibri.img \
    && rm -f /kolibri.7z

# QEMU wants its own names: a CA it made up, and a certificate under it. There
# is nothing for the CA to be worth here — this app pins the leaf and verifies
# no chain — so it is one command and the shortest life a certificate can have
# without being a nuisance.
RUN mkdir -p /certs \
    && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -keyout /certs/ca-key.pem -out /certs/ca-cert.pem \
        -subj "/CN=protoqemu ca" 2>/dev/null \
    && openssl req -newkey rsa:2048 -nodes \
        -keyout /certs/server-key.pem -out /certs/server-req.pem \
        -subj "/CN=protoqemu" 2>/dev/null \
    && openssl x509 -req -in /certs/server-req.pem -days 3650 \
        -CA /certs/ca-cert.pem -CAkey /certs/ca-key.pem -set_serial 1 \
        -out /certs/server-cert.pem 2>/dev/null \
    && rm -f /certs/server-req.pem \
    && openssl x509 -in /certs/server-cert.pem -noout -fingerprint -sha256

COPY start.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

EXPOSE 5900 5930 5931
ENTRYPOINT ["/usr/local/bin/start.sh"]
