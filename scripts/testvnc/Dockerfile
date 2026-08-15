# A throwaway remote desktop to point the prototype at.
#
# A real RFB server at the far end is the only thing that answers "what does
# the library actually do when a session connects"; a fake desktop cannot.
# Deliberately *not* RealVNC's own server — that needs a licence and, more to
# the point, a stock TigerVNC exercises the paths a third-party server takes:
# VncAuth rather than their identity check, and an unencrypted session, which
# is what makes the SecurityDlg prompt fire.
#
# Openbox and a handful of X clients, not a desktop environment. What is being
# judged is the input stack against real remote pixels: a cursor whose shape
# and hotspot change (the I-beam over text, openbox's resize corners), text to
# select and type into, buttons small enough to need the zoom, and a titlebar
# to drag.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        tigervnc-standalone-server \
        tigervnc-tools \
        openbox \
        xterm \
        x11-apps \
        `# xcalc and xclock are inside x11-apps in trixie, not packages of their own` \
        mousepad \
        dbus-x11 \
        xfonts-base \
        desktop-base \
        librsvg2-bin \
        feh \
        x11-xserver-utils \
        `# xev is how the far end answers "which button did you actually get?"` \
        `# — the question the ninth button turns on` \
        x11-utils \
        xauth \
        `# xclip makes the far end's clipboard settable and readable from a` \
        `# shell, which is the only way to test the clipboard without hands` \
        xclip \
        `# xdotool gives a window the focus a measurement's keys need, since` \
        `# the keys themselves come from the phone and land wherever it is` \
        xdotool \
        `# openssl makes the server's own certificate, and prints the` \
        `# fingerprint the client is checked against` \
        openssl \
        fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

# Debian's own wallpaper, rasterised at build time: feh cannot read the SVG the
# theme ships, and a smooth gradient is the worst case for a nearest-neighbour
# downscale, so whatever the framebuffer path does to one is visible on it.
RUN set -e; \
    src="$(readlink -f /usr/share/images/desktop-base/default 2>/dev/null || true)"; \
    if [ -z "$src" ]; then src="$(find /usr/share/desktop-base -name '*wallpaper*' \
        \( -name '*.svg' -o -name '*.png' \) | sort | head -1)"; fi; \
    case "$src" in \
      *.svg) rsvg-convert -w 1920 -h 1200 -o /usr/share/wallpaper.png "$src" ;; \
      *)     cp "$src" /usr/share/wallpaper.png ;; \
    esac

# Something with enough text to scroll through and select, for judging the
# wheel's gearing, which can only be judged against a real remote, and the
# axis lock.
RUN mkdir -p /root/Documents \
    && cat /usr/share/common-licenses/GPL-3 /usr/share/common-licenses/GPL-2 \
           /usr/share/common-licenses/LGPL-3 > /root/Documents/scrollme.txt

COPY start.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

EXPOSE 5900
ENTRYPOINT ["/usr/local/bin/start.sh"]
