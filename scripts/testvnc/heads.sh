#!/usr/bin/env python3
# Give the test desktop more than one screen, without a second graphics device.
#
#   ./heads.sh              # what the layout is now
#   ./heads.sh 2            # two screens of 1280x800, side by side
#   ./heads.sh 3 1024 768   # three of 1024x768
#   ./heads.sh 1 1920 1200  # back to one
#
# Xvnc grows its RandR outputs to match whatever screen list a client sends, so
# a layout is something a *client* creates and the server then keeps: this
# connects, sends a SetDesktopSize, prints what came back and disconnects. The
# desktop stays that shape for whatever connects next, which is the only way
# there is to point the app at a multi-head VNC desktop.
#
# Needs SECURITY_TYPES=None on the rig; it speaks no authentication.
"""Set or show the test desktop's screen layout."""
import os
import socket
import struct
import sys

HOST = os.environ.get("HOST", "127.0.0.1")
PORT = int(os.environ.get("PORT", "5901"))

count = int(sys.argv[1]) if len(sys.argv) > 1 else 0
width = int(sys.argv[2]) if len(sys.argv) > 2 else 1280
height = int(sys.argv[3]) if len(sys.argv) > 3 else 800

sock = socket.create_connection((HOST, PORT), timeout=10)


def read(n):
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise EOFError("the server closed the connection")
        buf += chunk
    return buf


read(12)
sock.sendall(b"RFB 003.008\n")
types = read(read(1)[0])
if 1 not in types:
    sys.exit("this server wants authentication; run the rig with SECURITY_TYPES=None")
sock.sendall(bytes([1]))
if struct.unpack(">I", read(4))[0] != 0:
    sys.exit("the server refused the connection")

sock.sendall(bytes([1]))  # shared
w, h = struct.unpack(">HH", read(4))
read(16)
read(struct.unpack(">I", read(4))[0])

# ExtendedDesktopSize, so the layout comes back; Raw, so there is something the
# reader below can skip past while it waits for it.
sock.sendall(struct.pack(">BBHii", 2, 0, 2, -308, 0))

if count > 0:
    layout = struct.pack(">BBHHBB", 251, 0, width * count, height, count, 0)
    for i in range(count):
        layout += struct.pack(">IHHHHI", i + 1, i * width, 0, width, height, 0)
    sock.sendall(layout)
    w, h = width * count, height

sock.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, w, h))

sock.settimeout(8)
while True:
    if read(1)[0] != 0:
        continue
    read(1)
    for _ in range(struct.unpack(">H", read(2))[0]):
        x, y, rw, rh, encoding = struct.unpack(">HHHHi", read(12))
        if encoding == -308:
            screens = read(1)[0]
            read(3)
            print(f"{rw}x{rh}, {screens} screen(s)"
                  + ("" if y == 0 else f" — the server said no (result {y})"))
            for _ in range(screens):
                sid, sx, sy, sw, sh, _flags = struct.unpack(">IHHHHI", read(16))
                print(f"  id={sid} {sw}x{sh}+{sx}+{sy}")
            raise SystemExit(0)
        elif encoding == 0:
            read(rw * rh * 4)
        else:
            sys.exit(f"unexpected encoding {encoding}")
