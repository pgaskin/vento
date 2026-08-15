#!/usr/bin/env bash
#
# Records touch fixtures for FixtureReplayTest, one gesture at a time.
#
#   ./src/test/fixtures/record-fixtures.sh                 # walk the whole checklist
#   ./src/test/fixtures/record-fixtures.sh pinch-in taps:5 # just these (name[:count])
#
# For each entry it prompts, waits for you to perform the gesture on the
# device, then pulls whatever the app recorded and files it beside this script
# with the right label.
#
# Fixtures are raw touch streams, so they are independent of the PRESET
# toggle — each gesture only needs recording once.
#
# The recorder itself is TouchRecorder, driven by the playground view; PKG is
# whichever app is hosting that view, and is the one thing here that is not
# about this library.

set -uo pipefail

PKG=${PKG:-net.pgaskin.remotedesktop}
ACTIVITY=${ACTIVITY:-$PKG/.PlaygroundActivity}
REMOTE=/sdcard/Android/data/$PKG/files/touchlogs
OUT=$(cd "$(dirname "$0")" && pwd)

# name[:count] — count > 1 records that many in one go (label-01, label-02, ...)
CHECKLIST=(
    two-finger-tap
    three-finger-tap
    four-finger-tap
    two-finger-tap-then-drag
    three-finger-tap-then-drag
    pinch-out
    pinch-in
    pinch-with-rotation
    scroll-down-slow
    scroll-down-fast
    scroll-up
    scroll-left
    scroll-right
    two-finger-spread-and-translate
    lift-first-finger-continue-drag
    slow-drag-under-threshold
    tap-then-drag-to-edge
    tap:20
)

# What to actually do, for the ones where it matters.
declare -A HINT=(
    [four-finger-tap]="should do nothing at all"
    [two-finger-tap-then-drag]="two-finger tap, then within 250 ms put one finger down and drag — right-button drag"
    [three-finger-tap-then-drag]="same with three fingers — middle-button drag"
    [pinch-with-rotation]="pinch out while twisting, to see if rotation breaks the zoom/scroll ratio"
    [two-finger-spread-and-translate]="fingers moving apart AND sliding together — the ambiguous case at the 0.7/0.2 boundary"
    [lift-first-finger-continue-drag]="two fingers down, lift the FIRST one, keep dragging with the second"
    [slow-drag-under-threshold]="ONE touch, no tap first: put a finger down and scrub slowly and continuously, staying within ~1 cm of where you started"
    [tap-then-drag-to-edge]="tap, then tap-and-drag into the edge of the screen and hold there — bump scroll"
    [tap]="20 ordinary taps, as you would normally tap — this is the tap-path measurement"
)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }

remote_files() { adb shell "ls $REMOTE 2>/dev/null" | tr -d '\r' | grep '\.touch$' | sort; }

adb get-state >/dev/null 2>&1 || { warn "no device"; exit 1; }
mkdir -p "$OUT"

say "Clearing the device's recordings and starting the app with RECORD armed."
adb shell am force-stop $PKG
adb shell "rm -rf $REMOTE"
adb shell am start -n $ACTIVITY --ez record true >/dev/null || exit 1
sleep 2

[ $# -gt 0 ] && CHECKLIST=("$@")
seen=""
recorded=0

for entry in "${CHECKLIST[@]}"; do
    name=${entry%%:*}
    count=1
    [ "$entry" != "$name" ] && count=${entry##*:}

    while :; do
        echo
        if [ "$count" -gt 1 ]; then
            say "▶ $name  ×$count"
        else
            say "▶ $name"
        fi
        [ -n "${HINT[$name]:-}" ] && echo "  ${HINT[$name]}"
        read -rp "  perform it, then Enter  [s=skip, q=quit] " ans
        case "$ans" in
            s|S) break ;;
            q|Q) break 2 ;;
        esac

        new=$(comm -13 <(printf '%s\n' $seen | sort) <(remote_files))
        n=$(printf '%s\n' $new | grep -c '\.touch$')
        if [ "$n" -eq 0 ]; then
            warn "  nothing was recorded — is the app in the foreground with RECORD on?"
            continue
        fi
        if [ "$n" -ne "$count" ]; then
            warn "  got $n recording(s), expected $count"
            read -rp "  keep them anyway? [y/N/r=redo] " k
            case "$k" in
                y|Y) ;;
                r|R) adb shell "rm -f $(printf "$REMOTE/%s " $new)"; continue ;;
                *) adb shell "rm -f $(printf "$REMOTE/%s " $new)"; break ;;
            esac
        fi

        i=0
        for f in $new; do
            i=$((i + 1))
            if [ "$n" -gt 1 ]; then
                label=$(printf '%s-%02d' "$name" "$i")
            else
                label=$name
            fi
            adb pull "$REMOTE/$f" "$OUT/$label.touch" >/dev/null 2>&1 || continue
            sed -i "s/^# label .*/# label $label\n# source hand-recorded/" "$OUT/$label.touch"
            frames=$(grep -cv '^#' "$OUT/$label.touch")
            echo "  → $label.touch ($frames frames)"
            recorded=$((recorded + 1))
        done
        seen=$(printf '%s\n%s\n' "$seen" "$new")
        break
    done
done

echo
say "$recorded fixture(s) in src/test/fixtures"
cat <<'EOF'

Next, from the Gradle build this is a subproject of:
  ./gradlew :control:test   # writes a .expected golden per new fixture, then fails
  # read each golden — does it match what the gesture was meant to do?
  ./gradlew :control:test   # now green; commit the .touch and .expected together
EOF
