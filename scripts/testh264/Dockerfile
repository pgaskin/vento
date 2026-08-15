# A remote desktop that sends H.264, which nothing else here does.
#
# The Open H.264 encoding (RFB encoding 50) has one server implementation worth
# pointing a phone at — neatvnc, through wayvnc — and it will only use it for
# frames that arrive as GPU buffers. So this container is a wlroots compositor
# rather than the Xvnc of scripts/testvnc: sway is what makes the frames
# DMA-BUFs and wayvnc's --gpu is what lets neatvnc hand them to VAAPI.
#
# It nests in the host's own compositor, which is not a convenience: a headless
# wlroots has no DRM file descriptor, so its linux-dmabuf never starts and every
# frame reaches neatvnc as software — where it refuses H.264 outright.
#
# The desktop content is scripts/testvnc's, through Xwayland, because what is
# being judged is a decoder rather than the input stack: the same wallpaper (a
# smooth gradient is where a lossy codec shows first), the same clock (one small
# region repainting once a second, which is the worst case for a codec that
# sends whole frames) and the same document to scroll (a full-screen change,
# which is the best one).
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        sway \
        wayvnc \
        `# Xwayland, so the same X clients scripts/testvnc uses run here` \
        xwayland \
        xterm \
        x11-apps \
        mousepad \
        swaybg \
        `# grim is this rig's xwd: what the far end looks like, without a viewer` \
        grim \
        `# the VAAPI driver for the host's render node, and the GL that wlroots` \
        `# renders through — without either, sway starts and no frame is ever` \
        `# encodable as H.264` \
        mesa-va-drivers \
        libgl1-mesa-dri \
        libglx-mesa0 \
        va-driver-all \
        vainfo \
        dbus-x11 \
        xfonts-base \
        desktop-base \
        librsvg2-bin \
        x11-utils \
        xclip \
        `# xdotool gives a window the focus a measurement's keys need, since` \
        `# the keys themselves come from the phone and land wherever it is` \
        xdotool \
        fonts-dejavu-core \
        procps \
    && rm -rf /var/lib/apt/lists/*

# The same wallpaper as the other rig, rasterised at build time.
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

EXPOSE 5900
ENTRYPOINT ["/usr/local/bin/start.sh"]
