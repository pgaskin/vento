#!/bin/bash
# Build, install and open a session, since doing it by hand is four commands
# and forgetting the force-stop silently reuses the running connection.
#
#   scripts/session.sh                       # the local test desktop (scripts/testvnc/)
#   scripts/session.sh 10.0.0.5:5900         # somewhere else
#   scripts/session.sh -n 10.0.0.5           # no rebuild
#   scripts/session.sh -d                    # with the debug HUD over the desktop
#   scripts/session.sh -t 256                # a mirror tile size other than the default
#   scripts/session.sh -l                    # follow the log of a running session
#   scripts/session.sh -b rfb                # same server, the other backend
#   scripts/session.sh -o Encoding=tight     # backend options, Key=value,Key=value
#   scripts/session.sh -b rdp                # the RDP test desktop (scripts/testrdp/)
#   scripts/session.sh -b freerdp            # the same desktop, the other RDP client
#   scripts/session.sh -b rustdesk           # the RustDesk desktop (scripts/testrustdesk/)
#   scripts/session.sh -b spice              # the QEMU rig's SPICE port (scripts/testqemu/)
#   scripts/session.sh -b rustdesk -o ConnectBy=id <peer-id>   # the same by ID
#   scripts/session.sh -6                    # the same containers over IPv6
#
# The default address is the test desktop's (5901), 3389 with -b rdp, 21118
# with -b rustdesk or 5930 with -b spice; the QEMU rig's VNC port and the H.264
# rig are 5902 and 5903 and have to be given. RustDesk is an add-on rather than part of the app, so -b rustdesk
# installs that APK as well.
#
# -6 takes this host's global v6 address instead of its v4 one and brackets it,
# which is the form the app requires of a literal. Docker publishes every
# container on [::] as well, so the far ends are the same ones.
set -euo pipefail
cd "$(dirname "$0")/.."   # the build is at the repository root

PKG=net.pgaskin.remotedesktop.debug
ACT="$PKG/net.pgaskin.remotedesktop.SessionActivity"
BUILD=1
HUD=false
BACKEND=
TILE=
OPTIONS=
FAMILY=-4

log() {
    # The core's own tags as well as ours: what it says about the connection is
    # usually more specific than what it tells the callbacks.
    exec adb logcat -v time \
        RealVnc:V CSession:V CSessionMgr:V CConnection:V CConn:V CProtoV4Down:V \
        CDesktop:V ConfigParameter:V Rfb:V Rdp:V FreeRdp:V LibVnc:V TigerVnc:V \
        RustDesk:V Spice:V Backends:V \
        AndroidRuntime:E DEBUG:F "$PKG:V" '*:S'
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n) BUILD=0; shift ;;
        -d) HUD=true; shift ;;
        -t) TILE="$2"; shift 2 ;;
        -o) OPTIONS="${OPTIONS:+$OPTIONS,}$2"; shift 2 ;;
        -b) BACKEND="$2"; shift 2 ;;
        -6) FAMILY=-6; shift ;;
        -l) log ;;
        -h) sed -n '2,/^set /p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 2 ;;
        -*) echo "unknown option: $1 (-h for usage)" >&2; exit 2 ;;
        *)  break ;;
    esac
done

ADDRESS="${1:-}"
if [ -z "$ADDRESS" ]; then
    ip="$(ip "$FAMILY" -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
    # A literal is bracketed, which is the app's rule and not this script's
    # preference: unbracketed there is no telling one from a host with a port.
    [ "$FAMILY" = -6 ] && ip="[$ip]"
    # Each test container's own port, since which one is meant follows from
    # which protocol was asked for.
    if [ "$BACKEND" = rdp ] || [ "$BACKEND" = freerdp ]; then
        ADDRESS="$ip:${PORT:-3389}"
    elif [ "$BACKEND" = rustdesk ]; then
        ADDRESS="$ip:${PORT:-21118}"
    elif [ "$BACKEND" = spice ]; then
        ADDRESS="$ip:${PORT:-5930}"
    else
        ADDRESS="$ip:${PORT:-5901}"
    fi
fi

# A backend that ships in an add-on needs that APK on the phone as well, and
# the app loads it at start rather than when a session asks for it.
INSTALL=(':app:installDebug')
[ "$BACKEND" = rustdesk ] && INSTALL+=(':plugins:rustdesk:installDebug')
[ "$BUILD" = 1 ] && ./gradlew --quiet "${INSTALL[@]}"

adb shell am force-stop "$PKG"
adb logcat -c
adb shell am start -n "$ACT" \
    `# quoted for the shell on the phone, which globs [::1] against its own /` \
    --es address "'$ADDRESS'" \
    --ez hud "$HUD" \
    ${BACKEND:+--es backend "$BACKEND"} \
    ${TILE:+--ei tile "$TILE"} \
    ${OPTIONS:+--es options "$OPTIONS"} \
    ${USERNAME:+--es user "$USERNAME"} \
    ${PASSWORD:+--es password "$PASSWORD"} >/dev/null

echo "session on $ADDRESS (${BACKEND:-default}${OPTIONS:+, $OPTIONS}) — scripts/session.sh -l to follow the log"
