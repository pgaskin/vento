#!/bin/bash
# Every key a keyboard has, through the playground's key trace.
#
# Opens the playground with the trace armed, walks the whole keyboard past it —
# the printing keys, the navigation block, the function row, the keypad with Num
# Lock both ways, the modifiers left against right, the media keys and the chords
# a phone takes for itself — and pulls the trace off. One line per key event,
# with what Android delivered and what keysym was made of it.
#
#   go run . &                 # the keyboard; this drives it over HTTP
#   ./walk.sh                  # ~200 keys, half a minute
#   ./walk.sh -o notes/data    # keep the trace somewhere
#
# POWER is the one key on the board that is deliberately not pressed: it would
# put the screen out and end the walk. AC_HOME and AC_BACK do end it, so they
# come last.
set -euo pipefail
cd "$(dirname "$0")"

HID=${HID:-localhost:8099}
PKG=net.pgaskin.remotedesktop
OUT=

while [ $# -gt 0 ]; do
    case "$1" in
        -o) OUT="$2"; shift 2 ;;
        -h) sed -n '2,/^set /p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 2 ;;
        *)  echo "unknown option: $1 (-h for usage)" >&2; exit 2 ;;
    esac
done

if ! curl -fsS "$HID/state" >/dev/null 2>&1; then
    echo "no keyboard on $HID — start one with: (cd $(pwd) && go run .) &" >&2
    exit 1
fi

tap() { curl -fsS "$HID/tap?k=$1&hold=${2:-40ms}" >/dev/null; sleep 0.05; }
media() { curl -fsS "$HID/consumer?k=$1" >/dev/null; sleep 0.05; }
leds() { curl -fsS "$HID/leds"; }

# The lock state is Android's, not ours, so it is read back rather than assumed;
# the first tap is what makes the phone write the LEDs at all.
numlock() {
    local want=$1 now
    now=$(leds | sed -n 's/^num=\([01]\).*/\1/p')
    if [ "$now" != "$want" ]; then
        tap NUMLOCK
        now=$(leds | sed -n 's/^num=\([01]\).*/\1/p')
    fi
    [ "$now" = "$want" ] || echo "warning: num lock is $now, wanted $want" >&2
}

# Two of the chords below are taken by the phone and send the playground to the
# background, where it sees nothing; everything after them would be lost.
foreground() {
    local top
    top=$(adb shell dumpsys activity activities | sed -n 's/.*topResumedActivity=.*u0 \([^ ]*\).*/\1/p' | head -1)
    case "$top" in
        "$PKG"/*) ;;
        *) adb shell am start -n "$PKG/.PlaygroundActivity" >/dev/null 2>&1; sleep 1 ;;
    esac
}

echo "opening the playground with the trace armed"
adb shell am force-stop "$PKG"
adb shell rm -f "/sdcard/Android/data/$PKG/files/keytrace/*.keys"
adb shell am start -n "$PKG/.PlaygroundActivity" --ez keys true >/dev/null
sleep 2
curl -fsS "$HID/release" >/dev/null   # a run starts with nothing held

echo "the printing keys"
for k in A B C D E F G H I J K L M N O P Q R S T U V W X Y Z \
         1 2 3 4 5 6 7 8 9 0 \
         MINUS EQUAL LEFTBRACE RIGHTBRACE BACKSLASH SEMICOLON APOSTROPHE \
         GRAVE COMMA DOT SLASH SPACE; do
    tap "$k"
done

echo "the same keys shifted, and the third level"
for k in A 1 2 3 SLASH GRAVE; do
    tap "LSHIFT,$k"
done
for k in A E Q; do
    tap "RALT,$k"        # AltGr, which a US layout has nothing on
done

echo "the modifiers, left against right"
for k in LCTRL RCTRL LSHIFT RSHIFT LALT RALT LGUI RGUI; do
    tap "$k"
done
echo "a key under each one held"
for m in LCTRL RCTRL LALT RALT LGUI RGUI; do
    tap "$m,C"
done

echo "the navigation block and the editing keys"
for k in UP DOWN LEFT RIGHT HOME END PAGEUP PAGEDOWN INSERT DELETE \
         BACKSPACE TAB ENTER ESCAPE; do
    tap "$k"
done

echo "the function row"
for n in $(seq 1 24); do
    tap "F$n"
done

echo "the three nobody agrees about, and the menu key"
for k in PRINTSCREEN SCROLLLOCK PAUSE APPLICATION; do
    tap "$k"
done

echo "the keypad, num lock on"
numlock 1
for k in KP_0 KP_1 KP_2 KP_3 KP_4 KP_5 KP_6 KP_7 KP_8 KP_9 KP_DOT \
         KP_SLASH KP_ASTERISK KP_MINUS KP_PLUS KP_ENTER KP_EQUAL KP_COMMA; do
    tap "$k"
done

echo "the keypad, num lock off"
numlock 0
for k in KP_0 KP_1 KP_2 KP_3 KP_4 KP_5 KP_6 KP_7 KP_8 KP_9 KP_DOT \
         KP_SLASH KP_ASTERISK KP_MINUS KP_PLUS KP_ENTER KP_EQUAL KP_COMMA; do
    tap "$k"
done
numlock 1

echo "the lock keys, and a letter on either side of caps lock"
tap A
tap CAPSLOCK
tap A
tap CAPSLOCK
tap A
tap SCROLLLOCK
tap SCROLLLOCK
echo "  $(leds)"

echo "the keys a US board does not have"
for k in NONUS_HASH NONUS_BACKSLASH INTL_RO INTL_YEN INTL_KANA \
         INTL_HENKAN INTL_MUHENKAN LANG_HANGUL LANG_HANJA \
         LANG_KATAKANA LANG_HIRAGANA; do
    tap "$k"
done

echo "the editing keys a keyboard can have of its own"
for k in EXECUTE HELP MENU SELECT STOP AGAIN UNDO CUT COPY PASTE FIND; do
    tap "$k"
done

echo "auto-repeat, held for a second and a half"
tap A 1500ms

echo "the chords a phone's window manager takes first"
for k in LGUI LGUI,ENTER LALT,TAB LALT,ESCAPE LCTRL,SPACE LCTRL,LALT,DELETE LGUI,SLASH; do
    tap "$k"
    foreground
done
# Meta+/ is the shortcut helper, which is an overlay rather than an activity, so
# nothing above notices it and every key after it would go to it.
tap ESCAPE
foreground

echo "the media keys, which are a page of their own"
for k in PLAYPAUSE SCANNEXT SCANPREV STOP EJECT MUTE VOLUMEUP VOLUMEDOWN \
         BRIGHTNESSUP BRIGHTNESSDOWN AC_SEARCH AC_REFRESH AC_FORWARD \
         AL_CALCULATOR; do
    media "$k"
    foreground   # some of these start an app rather than reaching one
done
# Also the keyboard page's own volume and mute keys, which are different keys.
for k in MUTE VOLUMEUP VOLUMEDOWN; do
    tap "$k"
done

echo "and last, the two that leave the app"
media AC_BACK
media AC_HOME

dir=$(mktemp -d)
adb pull -a "/sdcard/Android/data/$PKG/files/keytrace" "$dir" >/dev/null
file=$(ls -1 "$dir"/keytrace/*.keys | tail -1)
if [ -n "$OUT" ]; then
    cp "$file" "$OUT/keyboard-walk.txt"
    file="$OUT/keyboard-walk.txt"
fi
echo "$(grep -c '^[a-z]' "$file") events in $file"
