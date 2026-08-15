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
#   scripts/session.sh -f                    # the free flavour: our own RFB client only
#   scripts/session.sh -b rfb                # same server, the other backend
#   scripts/session.sh -o Encoding=tight     # backend options, Key=value,Key=value
#   scripts/session.sh -f -b rdp             # the RDP test desktop (scripts/testrdp/)
#   scripts/session.sh -f -b freerdp         # the same desktop, the other RDP client
#
# The default address is the test desktop's (5901), or 3389 with -b rdp; the
# QEMU and H.264 rigs are 5902 and 5903 and have to be given. -f sets the
# backend as well, so it goes before -b rather than after it.
set -euo pipefail
cd "$(dirname "$0")/.."   # the build is at the repository root

PKG=net.pgaskin.remotedesktop
ACT="$PKG/.SessionActivity"
BUILD=1
HUD=false
FLAVOUR=Nonfree
BACKEND=
TILE=
OPTIONS=

log() {
    # The core's own tags as well as ours: what it says about the connection is
    # usually more specific than what it tells the callbacks.
    exec adb logcat -v time \
        RealVnc:V CSession:V CSessionMgr:V CConnection:V CConn:V CProtoV4Down:V \
        CDesktop:V ConfigParameter:V Rfb:V Rdp:V FreeRdp:V LibVnc:V TigerVnc:V \
        AndroidRuntime:E DEBUG:F "$PKG:V" '*:S'
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n) BUILD=0; shift ;;
        -d) HUD=true; shift ;;
        -t) TILE="$2"; shift 2 ;;
        -o) OPTIONS="${OPTIONS:+$OPTIONS,}$2"; shift 2 ;;
        -f) FLAVOUR=Free; BACKEND=rfb; shift ;;
        -b) BACKEND="$2"; shift 2 ;;
        -l) log ;;
        -h) sed -n '2,/^set /p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 2 ;;
        -*) echo "unknown option: $1 (-h for usage)" >&2; exit 2 ;;
        *)  break ;;
    esac
done

ADDRESS="${1:-}"
if [ -z "$ADDRESS" ]; then
    ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
    # Each test container's own port, since which one is meant follows from
    # which protocol was asked for.
    if [ "$BACKEND" = rdp ] || [ "$BACKEND" = freerdp ]; then
        ADDRESS="$ip:${PORT:-3389}"
    else
        ADDRESS="$ip:${PORT:-5901}"
    fi
fi

[ "$BUILD" = 1 ] && ./gradlew --quiet ":app:install${FLAVOUR}Debug"

adb shell am force-stop "$PKG"
adb logcat -c
adb shell am start -n "$ACT" \
    --es address "$ADDRESS" \
    --ez hud "$HUD" \
    ${BACKEND:+--es backend "$BACKEND"} \
    ${TILE:+--ei tile "$TILE"} \
    ${OPTIONS:+--es options "$OPTIONS"} \
    ${USERNAME:+--es user "$USERNAME"} \
    ${PASSWORD:+--es password "$PASSWORD"} >/dev/null

echo "session on $ADDRESS (${FLAVOUR,,}${BACKEND:+, $BACKEND}${OPTIONS:+, $OPTIONS}) — scripts/session.sh -l to follow the log"
