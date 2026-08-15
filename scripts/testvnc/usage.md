# Test desktop

A container with TigerVNC, Openbox and four X clients, for pointing the app
at. Everything of the form "what does `libvncviewer.so` actually do when a
session connects" needs one, and the playground's fake desktop answers none of
them.

```sh
./run.sh                 # build if needed, run, print the address for the phone
./run.sh logs            # Xvnc's own log
./run.sh shot [out.png]  # what the far end looks like now, without a viewer
./run.sh stop
./heads.sh [n [w h]]     # show or set the screen layout (SECURITY_TYPES=None)
```

Defaults: `1920x1200`, port `5901` on the host, `VncAuth` with password
`vncpass`, and a desktop the viewer may resize. Override with `GEOMETRY=`,
`PORT=`, `VNC_PASSWORD=`, `SECURITY_TYPES=`, `ACCEPT_RESIZE=` in the
environment.

`ACCEPT_RESIZE=0` makes the server refuse a `SetDesktopSize` with
`resultProhibited`, which is the only way to reach the branch a client takes
when the answer is no.

## Why this, rather than RealVNC's own server

RealVNC Server is what the viewer is built for, and it is the only thing that
exercises their identity check and encryption negotiation. But it needs a
licence, and — more usefully — a stock TigerVNC exercises the *other* path: the
one every third-party server takes. `VncAuth` makes the password prompt fire on
every connection, and an unencrypted session makes `SecurityDlg` fire with
`EncUnencryptedWarn`. Both prompts are mandatory (a session blocks until they
are answered), so they are the first thing the backend has to get right.

`SECURITY_TYPES=None` turns the password prompt off, which is how to tell "the
prompt hung" apart from "the connection failed".

## More than one screen

`./heads.sh 2` makes this a two-headed desktop and `./heads.sh 1 1920 1200` puts
it back. It is the only server available here that reports a screen layout at
all — a Windows machine with three monitors has none of its VNC servers say so
— so it is where anything about multi-monitor gets exercised.

There is no second graphics device involved and no option to set. Xvnc grows its
RandR outputs to match whatever screen list a *client* sends, so a layout is
something a client creates and the server then keeps: `heads.sh` connects, sends
one `SetDesktopSize`, prints what came back and disconnects, and the desktop
stays that shape for whatever connects next.

## What is on it, and why

| | |
|---|---|
| Debian's own wallpaper | a smooth gradient is the worst case for the framebuffer path's nearest-neighbour downscale, so artefacts show on it |
| xterm | an I-beam cursor over the text and an arrow over the scrollbar — `setCursor` shape *and* hotspot changing under the pointer |
| mousepad, on 2500 lines of licence text | text to select (the axis lock), and a document to scroll (the wheel gearing, which only a real document can settle) |
| xcalc | buttons small enough that hitting one needs the zoom, and a real double-click target |
| xclock | a region that repaints once a second and nothing else does — `drawRegion`/`framebufferUpdateEnd` traffic with the desktop otherwise idle |
| openbox | titlebars to drag and window corners to resize: long button-held drags, which is what bump scroll is for |
| xclip | the clipboard, from a shell rather than by hand |

The clipboard is the one feature with no visible surface on either side, so it
is worth writing down how to watch it:

```sh
docker exec -d protovnc sh -c 'DISPLAY=:1 xclip -selection clipboard -i /etc/hostname'
docker exec    protovnc sh -c 'DISPLAY=:1 xclip -selection clipboard -o'
```

`-i` has to be detached (`-d`): whichever process wrote the selection has to
stay alive to own it. On the phone, `adb logcat RealVnc:V` shows every
`getClipboard`/`setClipboard`, and `CConn:V` shows `sending client cut text
length N` when one actually leaves.
