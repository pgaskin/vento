# hidkbd

A keyboard the phone believes is real.

`adb shell input keyevent` synthesises an event from the virtual device with an
empty `metaState`, so everything that makes a keyboard a keyboard is out of
reach through it: a modifier held across another key, a lock state, the layout
a key is looked up in. This registers a USB HID keyboard over `uhid` on the
device and drives it from HTTP, so a script can hold a chord, toggle Num Lock,
and read back what Android thinks the lock state is.

```sh
go run . &                                  # register, serve on :8099
curl -s 'localhost:8099/tap?k=NUMLOCK'
curl -s localhost:8099/leds                 # num=1 caps=0 scroll=0 raw=0x01
curl -s 'localhost:8099/tap?k=LCTRL,LSHIFT,ESCAPE'
curl -s 'localhost:8099/press?k=LALT'       # held until released
curl -s 'localhost:8099/release'            # everything
```

The device exists for exactly as long as the process does.

`./walk.sh` is what it was built for: the whole keyboard pressed past the
playground's key trace — the printing keys, the navigation block, the function
row, the keypad with Num Lock both ways, the modifiers left against right, the
media keys and the chords a phone takes for itself — pulled off as one table of
what Android delivered against what keysym was made of it.

`./farwalk.sh` is the other half: the same special keys driven through a *live
session* of a backend, with the far end saying what arrived. It reads an X far
end with `xev` — `-c protovnc` for the VNC test desktop, `-c protordp -d :10 -a
/home/proto/.Xauthority` for the RDP one on its own display — and strips the far
end's window-manager key bindings first so every key reaches the probe.

A Windows RDP far end has no `xev`, and resolves a position to a virtual key
rather than to a keysym: `winkeys.ps1` is its instrument, a key-layout debugger
that says which one. Run it in the session with focus, then `-w user@host` reads
its log back over ssh, one key at a time, the same way. Two things about that far
end are not obvious and are in the script's own header: it is ASCII on purpose,
and nothing reached over ssh can put a window on the desktop somebody is looking
at — a scheduled task with an interactive principal is what starts it.

| | |
|---|---|
| `/press?k=…` | hold a comma-separated key list |
| `/release[?k=…]` | let go of those keys, or of everything |
| `/tap?k=…[&hold=40ms]` | press in order, release in reverse — a chord |
| `/consumer?k=PLAYPAUSE` | the media keys, which are a page of their own |
| `/report?b=1,0,…` | a raw report, for what this vocabulary has no name for |
| `/leds`, `/state`, `/keys` | what Android set, what is held, what can be named |

**The lock state comes back for free**, which is the point: Android writes a
keyboard's LEDs, and a uhid output report arrives on the `hid` command's stdout,
so "is Num Lock on" is answered by the phone rather than assumed.

## Two things that decide whether any of this works

**The vendor and product id.** A phone carries about 170 per-device key layouts
matched on vid:pid, and the one that matches decides whether the device is a
keyboard at all: Android reads the layout, asks it for the scancode behind `Q`,
and marks the device alphabetic only if it has one. A non-alphabetic keyboard
gets no lock tracking, no LED writes and no US layout. 18d1:4f80 — Google's own
vendor id and a plausible product — is `Vendor_18d1_Product_4f80.kl`, "Android
Stylus", under which this arrives as a stylus. The pair has to be one nothing on
the phone claims.

**The consumer page as a bitmap.** Declared as an array field over the whole
usage range, the kernel has no key for the usages it does not recognise and the
field becomes an absolute axis — the device then reaches Android as a keyboard,
a stylus and a joystick at once. Sixteen named usages as sixteen bits do not
have that problem.

Both faults are silent: the device registers, `getevent` shows the right scan
codes, and only `dumpsys input` says the keyboard is not a keyboard.

## Requirements

`/system/bin/hid` (AOSP, on every build) and write access to `/dev/uhid`, which
means an `adb root` device or a shell user in the `uhid` group. The ADB host
server is reached directly rather than through the `adb` binary, so nothing here
shells out.
