# A RustDesk far end that is not somebody's Windows machine.
#
# The rig this stage started against is the Windows box, which is the honest
# one — it is what people run — and the wrong one to develop against: it cannot
# be reset, its display geometry changes under you, and a session on it is
# visible to whoever is sitting there. This is the reproducible half.
#
# Their released .deb rather than a build from source, pinned by SHA-256, which
# is `third_party/`'s rule for a tarball and for the same reason: building
# RustDesk needs flutter, vcpkg, libvpx, libaom, libyuv and opus, and none of
# that is what is being tested here.
#
# The desktop is scripts/testvnc's, deliberately: the same wallpaper, clock,
# terminal and document, so a picture out of this rig and a picture out of that
# one are of the same thing and a codec comparison has something to compare.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

ARG RUSTDESK_VERSION=1.4.9
ARG RUSTDESK_SHA256=7244ba47c40e804172044bfbe659467c54ce46554c98e78c8c0406f1d612fda3

RUN apt-get update && apt-get install -y --no-install-recommends \
        xvfb \
        openbox \
        xterm \
        x11-apps \
        mousepad \
        feh \
        dbus-x11 \
        xfonts-base \
        desktop-base \
        librsvg2-bin \
        x11-utils \
        x11-xserver-utils \
        xclip \
        xdotool \
        `# xwd photographs the far end and convert makes it a png, as run.sh` \
        `# does for the other rigs` \
        imagemagick \
        fonts-dejavu-core \
        procps \
        `# their --server spawns a tray, which panics in a loop without this` \
        libayatana-appindicator3-1 \
        `# what their hwcodec asks VA-API for, and the drivers behind it: with` \
        `# /dev/dri forwarded this peer can encode H.264 and H.265, which no` \
        `# other far end here can, and without it their server finds nothing` \
        `# and falls back to the software codecs` \
        va-driver-all \
        intel-media-va-driver \
        vainfo \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl -fsSLo /tmp/rustdesk.deb \
        "https://github.com/rustdesk/rustdesk/releases/download/${RUSTDESK_VERSION}/rustdesk-${RUSTDESK_VERSION}-x86_64.deb"; \
    echo "${RUSTDESK_SHA256}  /tmp/rustdesk.deb" | sha256sum -c -; \
    apt-get update; \
    apt-get install -y --no-install-recommends /tmp/rustdesk.deb; \
    rm -rf /tmp/rustdesk.deb /var/lib/apt/lists/*

# The same wallpaper as the other rigs, rasterised at build time.
RUN set -e; \
    src="$(readlink -f /usr/share/images/desktop-base/default 2>/dev/null || true)"; \
    if [ -z "$src" ]; then src="$(find /usr/share/desktop-base -name '*wallpaper*' \
        \( -name '*.svg' -o -name '*.png' \) | sort | head -1)"; fi; \
    case "$src" in \
      *.svg) rsvg-convert -w 1920 -h 1200 -o /usr/share/wallpaper.png "$src" ;; \
      *)     cp "$src" /usr/share/wallpaper.png ;; \
    esac

RUN mkdir -p /root/Documents \
    && cat /usr/share/common-licenses/GPL-3 /usr/share/common-licenses/GPL-2 \
           /usr/share/common-licenses/LGPL-3 > /root/Documents/scrollme.txt

COPY start.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

EXPOSE 21118
ENTRYPOINT ["/usr/local/bin/start.sh"]
