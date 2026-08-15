#!/bin/bash
# The keys the extension row has never sent, through a live backend, with the
# far end saying what arrived.
#
# 35b walked the control layer with no server: what Android delivers and what
# keysym this client makes of it. This is the other half — the keysym on the
# wire, and what the far end resolves it to. So it needs a session already up
# (scripts/session.sh) and a keyboard already registered (go run . in here).
#
#   go run . &                              # the HID keyboard, on :8099
#   PASSWORD=vncpass ../session.sh -f -b rfb 10.0.0.5:5901
#   ./farwalk.sh -c protovnc                # capture at an X far end with xev
#   ./farwalk.sh -w user@10.33.0.208        # capture at a Windows one, winkeys.ps1
#
# Two far ends, because one cannot stand in for the other. An X server (testvnc
# for the VNC backends, testrdp for the RDP ones) is reached in its container by
# name and read with xev, which names the keysym the server resolved — for the
# four VNC backends that is the whole answer and for the two RDP ones it is what
# their scancode became. A Windows RDP server has no xev and resolves a scancode
# to a virtual key rather than a keysym, so winkeys.ps1 is its instrument: it is
# already running in the session with focus, appending one line per key to a file
# this reads back over ssh.
#
# Either way the app sends keys and not focus, so whatever the far end last
# focused is where they go — under X every other window is closed first so
# openbox has only xev to give focus to.
#
# One key at a time, segmenting the log by how many lines it grew: a key that
# produces nothing (dropped) and one that produces a lock key of the server's
# own (the far end reconciling its Num Lock) are both then visible as such.
set -euo pipefail
cd "$(dirname "$0")"

HID=${HID:-localhost:8099}
CONTAINER=
DISP=:1
XAUTH=
WINHOST=
WINLOG='C:\Users\user\farkeys.txt'

while [ $# -gt 0 ]; do
    case "$1" in
        -c) CONTAINER="$2"; shift 2 ;;
        -d) DISP="$2"; shift 2 ;;
        -a) XAUTH="$2"; shift 2 ;;   # xrdp's per-session X wants its cookie
        -w) WINHOST="$2"; shift 2 ;;
        -l) WINLOG="$2"; shift 2 ;;  # winkeys.ps1's -Out, if it was given one
        -h) sed -n '2,/^set /p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 2 ;;
        *)  echo "unknown option: $1 (-h for usage)" >&2; exit 2 ;;
    esac
done
[ -z "$CONTAINER" ] || [ -z "$WINHOST" ] || { echo "-c and -w are two far ends, not one" >&2; exit 2; }
[ -n "$CONTAINER$WINHOST" ] || { echo "need -c <container> or -w [user@]host" >&2; exit 2; }

tap() { curl -fsS "$HID/tap?k=$1&hold=${2:-40ms}" >/dev/null; sleep 0.25; }
leds() { curl -fsS "$HID/leds"; }

numlock() {
    local want=$1 now
    now=$(leds | sed -n 's/^num=\([01]\).*/\1/p')
    [ "$now" = "$want" ] || { tap NUMLOCK; sleep 0.1; }
}

if [ -n "$CONTAINER" ]; then
    env="DISPLAY=$DISP; export DISPLAY;${XAUTH:+ XAUTHORITY=$XAUTH; export XAUTHORITY;}"
    far() { docker exec "$CONTAINER" sh -c "$env $*"; }
    # Detached, for the one thing that has to outlive the exec that starts it: a
    # backgrounded process is reaped when a plain `docker exec` returns.
    fard() { docker exec -d "$CONTAINER" sh -c "$env $*"; }

    # A fresh xev with nothing to compete for focus. Restarted rather than
    # truncated because xev holds the file open and a truncation it does not know
    # about leaves the fd writing past the old end.
    arm() {
        # openbox grabs some keys before any window sees them — Print runs scrot,
        # whose failure dialog then steals focus from xev and swallows the rest of
        # the walk. Strip every binding so the far end forwards the lot to xev.
        far 'perl -0777 -pi -e "s{<keyboard>.*?</keyboard>}{<keyboard></keyboard>}s" /etc/xdg/openbox/rc.xml;
             openbox --reconfigure 2>/dev/null; true'
        for app in xterm mousepad xcalc xclock feh; do
            far "pkill $app 2>/dev/null; true"
        done
        # -9 and a wait: a plain kill leaves the old xev alive long enough to keep
        # the log open, and the fresh one's truncation is then undone by its next
        # write — two writers, and a log that never resets.
        far 'pkill -9 xev 2>/dev/null; true'
        while [ "$(far 'pgrep -c xev || true')" != "0" ]; do sleep 0.2; done
        # Line-buffered, so a key read back right after it is sent is already in
        # the file: xev's stdout to a file is block-buffered by libc otherwise,
        # and a whole run's worth of events flushes only at the end.
        fard 'exec stdbuf -oL xev -event keyboard > /tmp/xev.log 2>&1'
        sleep 0.8
    }
    lines() { far 'wc -l < /tmp/xev.log'; }
    # The KeyPress keysyms that arrived after the mark, as "name" tokens. A
    # dropped key grows the log by nothing, so grep matches nothing and exits
    # non-zero — which under pipefail would end the walk at the first such key,
    # exactly the case worth recording. The guard keeps it a "-" instead.
    since() {
        far "tail -n +$(($1 + 1)) /tmp/xev.log" \
            | { grep -A2 KeyPress || true; } \
            | sed -n 's/.*keysym 0x[0-9a-f]*, \([^)]*\)).*/\1/p' \
            | paste -sd, -
    }
    note() { :; }   # the log is xev's own and restarted per run
else
    # Quote-free inside, so the same string survives whichever shell OpenSSH was
    # configured with: cmd and powershell both hand a double-quoted argument on
    # whole, and neither has anything to say about what is inside it.
    # One connection for the whole walk: this is two round trips per key, and
    # setting a session up costs more than either of them.
    ctl=/tmp/farwalk-ssh-$$
    trap 'ssh -O exit -o ControlPath=$ctl "$WINHOST" 2>/dev/null; true' EXIT
    winsh() {
        ssh -o BatchMode=yes -o ControlMaster=auto -o ControlPath="$ctl" -o ControlPersist=60 \
            "$WINHOST" "powershell -NoProfile -Command \"$*\"" | tr -d '\r'
    }

    # winkeys.ps1 is started by a person, in the session, with focus — nothing
    # reached over ssh can put a window on somebody else's desktop. So arming is
    # a probe rather than a restart: a key the far end always resolves, and a
    # run that captured nothing is a focus failure rather than a table of blanks.
    arm() { :; }
    lines() { winsh "@(Get-Content -LiteralPath $WINLOG).Count"; }
    # One line per key, "VKNAME 0xNN"; the marks below and winkeys' own header
    # are comments.
    since() {
        winsh "Get-Content -LiteralPath $WINLOG | Select-Object -Skip $1" \
            | { grep -v '^#' || true; } | awk 'NF {print $1}' | paste -sd, -
    }
    # Sections into the far end's own log, so the file read back off the machine
    # says what each run of lines was. Doubled, because a section title with an
    # apostrophe in it otherwise ends PowerShell's string early.
    note() { winsh "Add-Content -LiteralPath $WINLOG -Value '# ${*//\'/\'\'}'" >/dev/null; }
fi

# What one key produced, or "-" for a key the far end never saw.
#
# A key that produced nothing is checked rather than written down: the far end
# losing focus mid-walk looks exactly like every remaining key being dropped, and
# a dozen plausible dashes is the one failure that reads as a result. So a blank
# is followed by a key the far end always resolves, and a walk whose far end has
# stopped listening ends here rather than filling a table in.
sent() {
    local label=$1 key=$2 hold=${3:-40ms} pre got
    pre=$(lines)
    tap "$key" "$hold"
    got=$(since "$pre")
    if [ -z "$got" ]; then
        pre=$(lines)
        tap A
        [ -n "$(since "$pre")" ] || {
            echo "$label produced nothing, and so did a probe after it — focus?" >&2
            exit 1
        }
    fi
    printf '%-18s %s\n' "$label" "${got:--}"
}

section() { echo "# $*"; note "$*"; }

echo "arming ${CONTAINER:+$CONTAINER:$DISP}${WINHOST:+$WINHOST:$WINLOG}"
arm
pre=$(lines)
tap A
[ "$(lines)" != "$pre" ] || { echo "the far end saw nothing after a probe — focus?" >&2; exit 1; }

section "keypad, num lock on"
numlock 1
for k in KP_0 KP_1 KP_2 KP_3 KP_4 KP_5 KP_6 KP_7 KP_8 KP_9 \
         KP_DOT KP_SLASH KP_ASTERISK KP_MINUS KP_PLUS KP_ENTER KP_EQUAL KP_COMMA; do
    sent "on/$k" "$k"
done

section "keypad, num lock off"
numlock 0
for k in KP_0 KP_1 KP_2 KP_3 KP_4 KP_5 KP_6 KP_7 KP_8 KP_9 KP_DOT; do
    sent "off/$k" "$k"
done
numlock 1

section "the three nobody agrees about, and the menu key"
for k in PRINTSCREEN PAUSE APPLICATION SCROLLLOCK; do
    sent "$k" "$k"
done

section "the function row past twelve"
for n in $(seq 13 24); do
    sent "F$n" "F$n"
done

section "the right-hand modifiers, as keys of their own"
for k in RSHIFT RCTRL RGUI; do
    sent "$k" "$k"
done

section "AltGr over a key a US layout has nothing on the third level of"
sent "AltGr+A" "RALT,A"

section "a Japanese board's conversion keys — the three the phone gives a keycode"
for k in INTL_HENKAN INTL_MUHENKAN INTL_KANA; do
    sent "$k" "$k"
done

section "and the two kept empty on purpose: this phone hands these to a Korean"
section "board's Hangul and Hanja keys, so either keysym would be wrong for one"
for k in LANG_HANGUL LANG_HANJA; do
    sent "$k" "$k"
done

section "the keys a US board does not have — character keys, the layout's business"
for k in NONUS_HASH NONUS_BACKSLASH INTL_RO INTL_YEN; do
    sent "$k" "$k"
done

echo "done"
