#!/bin/bash
# xrdp plus xrdp-sesman, serving an Openbox session on 3389.
#
# The desktop itself is testvnc's, line for line, so that the same machine
# through two protocols is the only difference between the two sessions.
set -e

RDP_USER="${RDP_USER:-proto}"
RDP_PASSWORD="${RDP_PASSWORD:-protopass}"
# tls  — TLS with the certificate below, and the password in the Client Info
#        PDU. What xrdp anywhere is set up as, and what IronRDP's connector
#        does when should_perform_credssp() is false.
# rdp  — legacy RDP encryption, no TLS at all. Here so that "the client
#        refuses to talk to a server that offers no TLS" is testable.
# negotiate — xrdp's own default: whatever the client asks for.
SECURITY_LAYER="${SECURITY_LAYER:-tls}"

id -u "$RDP_USER" >/dev/null 2>&1 || useradd -m -s /bin/bash "$RDP_USER"
echo "$RDP_USER:$RDP_PASSWORD" | chpasswd

# The server's own certificate, made once per container and self-signed,
# because that is what an xrdp anywhere actually presents. Its fingerprint is
# printed here so that what the phone shows can be checked against something
# that is not the phone — the whole point of pinning a key nobody vouches for.
if [ ! -f /etc/xrdp/proto-cert.pem ]; then
    openssl req -new -x509 -days 3650 -nodes -batch \
        -subj "/CN=prototype test desktop (rdp)" \
        -keyout /etc/xrdp/proto-key.pem -out /etc/xrdp/proto-cert.pem >/dev/null 2>&1
    chmod 600 /etc/xrdp/proto-key.pem
    chown xrdp:xrdp /etc/xrdp/proto-key.pem /etc/xrdp/proto-cert.pem 2>/dev/null || true
fi
echo "certificate: $(openssl x509 -in /etc/xrdp/proto-cert.pem -noout -fingerprint -sha256 \
    | sed 's/.*=//')"

sed -i \
    -e "s%^security_layer=.*%security_layer=$SECURITY_LAYER%" \
    -e "s%^certificate=.*%certificate=/etc/xrdp/proto-cert.pem%" \
    -e "s%^key_file=.*%key_file=/etc/xrdp/proto-key.pem%" \
    -e "s%^crypt_level=.*%crypt_level=high%" \
    -e "s%^tls_ciphers=.*%tls_ciphers=HIGH%" \
    -e "s%^; *ssl_protocols=.*%ssl_protocols=TLSv1.2, TLSv1.3%" \
    -e "s%^ssl_protocols=.*%ssl_protocols=TLSv1.2, TLSv1.3%" \
    -e "s%^new_cursors=.*%new_cursors=true%" \
    /etc/xrdp/xrdp.ini

# The session sesman starts. Openbox and the same four X clients testvnc runs,
# for the same reasons (see its usage.md): an I-beam and an arrow to change the
# cursor shape under the pointer, a document to scroll, buttons small enough to
# need the zoom, and one region that repaints on its own.
cat > /etc/xrdp/startwm.sh <<'EOF'
#!/bin/sh
if [ -r /etc/profile ]; then . /etc/profile; fi
mkdir -p "$HOME/Documents"
[ -f "$HOME/Documents/scrollme.txt" ] || cp /etc/skel/Documents/scrollme.txt "$HOME/Documents/" 2>/dev/null || true

xsetroot -cursor_name left_ptr
feh --no-fehbg --bg-fill /usr/share/wallpaper.png || true

xterm -geometry 100x30+60+80 -fa 'DejaVu Sans Mono' -fs 11 \
      -bg '#101418' -fg '#e8f0ff' -sb -sl 5000 &
mousepad "$HOME/Documents/scrollme.txt" &
xclock -geometry 200x200-60+80 -update 1 &
xcalc -geometry -60-80 &

exec openbox
EOF
chmod +x /etc/xrdp/startwm.sh

mkdir -p /var/run/xrdp /var/log/xrdp
chown xrdp:xrdp /var/run/xrdp 2>/dev/null || true

echo "user=$RDP_USER password=$RDP_PASSWORD security_layer=$SECURITY_LAYER"

# Both in the foreground, logging to stdout, so `run.sh logs` shows the whole
# story: sesman is what authenticates and starts the X server, xrdp is what
# speaks the protocol.
/usr/sbin/xrdp-sesman --nodaemon &
SESMAN_PID=$!
sleep 1
/usr/sbin/xrdp --nodaemon &
XRDP_PID=$!

trap 'kill $XRDP_PID $SESMAN_PID 2>/dev/null' TERM INT
wait -n "$XRDP_PID" "$SESMAN_PID"
