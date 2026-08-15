#!/bin/bash
# Xvnc plus an Openbox session, on :1, served on 5900.
set -e

GEOMETRY="${GEOMETRY:-1920x1200}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
# For X509Plain, which authenticates a *system* user through PAM. `root` with
# the same password is the whole of the setup, and it is a throwaway container.
PLAIN_USER="${PLAIN_USER:-root}"
# VncAuth alone by default, so a connection always exercises the password
# prompt. Set SECURITY_TYPES=None for a server that needs no answer at all —
# useful for isolating "did the prompt hang?" from "did the connection fail?".
# X509Vnc / X509None / X509Plain put VeNCrypt in front of that, which is what
# the client's TLS was developed against; RA2 and RA2ne are RSA-AES, which is
# what a RealVNC Server offers and the one security type only the TigerVNC
# backend can do.
SECURITY_TYPES="${SECURITY_TYPES:-VncAuth}"
# Whether the desktop may be resized from the viewer. It could not until the
# viewer could ask, and both answers are worth having: 0 makes the server
# refuse, which is the branch a client only ever exercises against a server that
# says no.
ACCEPT_RESIZE="${ACCEPT_RESIZE:-1}"

# The server's own certificate, made once per container and self-signed,
# because that is what a VNC server anywhere actually presents. Its fingerprint
# is printed here so that what the phone shows can be checked against something
# that is not the phone — the whole point of pinning a key nobody vouches for.
mkdir -p /root/.vnc
if [ ! -f /root/.vnc/cert.pem ]; then
    openssl req -new -x509 -days 3650 -nodes -batch \
        -subj "/CN=prototype test desktop" \
        -keyout /root/.vnc/key.pem -out /root/.vnc/cert.pem >/dev/null 2>&1
    chmod 600 /root/.vnc/key.pem
fi
echo "certificate: $(openssl x509 -in /root/.vnc/cert.pem -noout -fingerprint -sha256 \
    | sed 's/.*=//')"

# The RSA key the RSA-AES security types authenticate the *server* with, which
# is a different key from the certificate above and a different check: no
# certificate, no authority, just a key whose fingerprint the client shows. The
# eight bytes printed here are TigerVNC's own fingerprint of it — SHA-1 over the
# key length and then the modulus and exponent, each padded to the key size —
# so that what the phone shows can again be checked against something else.
if [ ! -f /root/.vnc/rsa_key ]; then
    openssl genrsa -out /root/.vnc/rsa_key -traditional 2048 >/dev/null 2>&1
    chmod 600 /root/.vnc/rsa_key
fi
echo "server key: $(python3 - <<'EOF'
import hashlib, subprocess
modulus = subprocess.run(["openssl", "rsa", "-in", "/root/.vnc/rsa_key", "-noout", "-modulus"],
                         capture_output=True, text=True).stdout.strip()
n = bytes.fromhex(modulus.split("=")[1])
e = (65537).to_bytes(len(n), "big")
digest = hashlib.sha1((len(n) * 8).to_bytes(4, "big") + n + e).digest()[:8]
print("-".join("%02x" % b for b in digest))
EOF
)"

printf '%s\n%s\n\n' "$VNC_PASSWORD" "$VNC_PASSWORD" | vncpasswd -f > /root/.vnc/passwd
chmod 600 /root/.vnc/passwd

# X509Plain goes through PAM, so the user it names needs a password to check.
echo "root:$VNC_PASSWORD" | chpasswd

echo "geometry=$GEOMETRY security=$SECURITY_TYPES password=$VNC_PASSWORD"

Xvnc :1 \
    -geometry "$GEOMETRY" \
    -depth 24 \
    -rfbport 5900 \
    -rfbauth /root/.vnc/passwd \
    -SecurityTypes "$SECURITY_TYPES" \
    -X509Cert /root/.vnc/cert.pem \
    -X509Key /root/.vnc/key.pem \
    -RSAKey /root/.vnc/rsa_key \
    -PlainUsers "$PLAIN_USER" \
    -AlwaysShared \
    -AcceptSetDesktopSize="$ACCEPT_RESIZE" \
    -desktop "prototype test desktop" \
    -localhost=0 &
XVNC_PID=$!

export DISPLAY=:1
for _ in $(seq 50); do
    if xdpyinfo >/dev/null 2>&1; then break; fi
    sleep 0.1
done

# A cursor that is not the X root's black X: setCursor's shape and hotspot are
# part of what this rig is for.
xsetroot -cursor_name left_ptr
feh --no-fehbg --bg-fill /usr/share/wallpaper.png || true

openbox &

xterm -geometry 100x30+60+80 -fa 'DejaVu Sans Mono' -fs 11 \
      -bg '#101418' -fg '#e8f0ff' -sb -sl 5000 &
mousepad /root/Documents/scrollme.txt &
xclock -geometry 200x200-60+80 -update 1 &
xcalc -geometry -60-80 &

wait "$XVNC_PID"
