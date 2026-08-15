# A throwaway RDP desktop to point the prototype at — testvnc's twin.
#
# "RDP is a different protocol, not a different server" is a claim about the
# control stack, and it is only tested by driving the same desktop through
# both.
#
# So this is deliberately the *same* desktop as testvnc — same wallpaper, same
# four X clients, same scrollable document — with xrdp in front of it instead of
# Xvnc. What differs between the two sessions on the phone is then the protocol
# and nothing else.
#
# xrdp rather than a Windows machine because it is the server anybody can run,
# it authenticates through PAM, and it presents a certificate it generated for
# itself — which is exactly the trust question the pin store was built
# for.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        xrdp \
        xorgxrdp \
        xserver-xorg-core \
        xserver-xorg-video-dummy \
        openbox \
        xterm \
        x11-apps \
        `# xcalc and xclock are inside x11-apps in trixie, not packages of their own` \
        x11-utils \
        `# xev is how the far end answers "which key did you actually get?" — the` \
        `# question a physical keyboard's special keys turn on` \
        mousepad \
        dbus-x11 \
        xfonts-base \
        desktop-base \
        librsvg2-bin \
        feh \
        x11-xserver-utils \
        xauth \
        `# xclip makes the far end's clipboard settable and readable from a` \
        `# shell, which is the only way to test the clipboard without hands` \
        xclip \
        `# openssl makes the server's own certificate, and prints the` \
        `# fingerprint the client is checked against` \
        openssl \
        fonts-dejavu-core \
        procps \
        iproute2 \
    && rm -rf /var/lib/apt/lists/*

# Same wallpaper as testvnc, and for the same reason: a smooth gradient is the
# worst case for a nearest-neighbour downscale, so artefacts show on it.
RUN set -e; \
    src="$(readlink -f /usr/share/images/desktop-base/default 2>/dev/null || true)"; \
    if [ -z "$src" ]; then src="$(find /usr/share/desktop-base -name '*wallpaper*' \
        \( -name '*.svg' -o -name '*.png' \) | sort | head -1)"; fi; \
    case "$src" in \
      *.svg) rsvg-convert -w 1920 -h 1200 -o /usr/share/wallpaper.png "$src" ;; \
      *)     cp "$src" /usr/share/wallpaper.png ;; \
    esac

# Something with enough text to scroll through and select, for judging the
# wheel's gearing and the axis lock against a document that really scrolls.
RUN mkdir -p /etc/skel/Documents \
    && cat /usr/share/common-licenses/GPL-3 /usr/share/common-licenses/GPL-2 \
           /usr/share/common-licenses/LGPL-3 > /etc/skel/Documents/scrollme.txt

# Xorg has to start as an ordinary user with no seat and no console, which is
# what xorgxrdp's virtual driver is for; Debian's wrapper refuses that by
# default.
RUN printf 'allowed_users=anybody\nneeds_root_rights=no\n' > /etc/X11/Xwrapper.config

COPY start.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

EXPOSE 3389
ENTRYPOINT ["/usr/local/bin/start.sh"]
