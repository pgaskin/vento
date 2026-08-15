# Test desktop, with a relative pointer

The third rig, and the only one where **the far end owns the cursor**. QEMU's
built-in VNC server is the one free implementation of a relative pointer in RFB
— pseudo-encoding −257, `PointerTypeChange` — and this rig is how to see what
that does to a control stack built entirely on owning the cursor here. TigerVNC has no such thing, so `testvnc` cannot be asked this question.

```sh
./run.sh                 # build if needed, run, print the address for the phone
./run.sh logs            # QEMU's own output
./run.sh stop
POINTER=absolute ./run.sh   # the same guest with a usb-tablet, for the A/B
```

Defaults: port `5902`, 256 MB, `VncAuth` with password `vncpass`, KVM if this
machine has it. There is no `shot` command: the guest's screen is not an X
display anybody can reach from the host — use the client under test.

```
$ cargo run -q --example rfb-probe -- 127.0.0.1:5902 vncpass /tmp/f.ppm
connected: 1024x768
pointer mode: relative
…
pointer:      relative
buttons:      8
```

## The guest

**KolibriOS**, from its own 1.4 MB floppy image: a graphical desktop at
1024×768 in about three seconds, with windows to drag, a text editor, a
calculator and a dozen small games — enough to judge a pointer by. It is
downloaded at build time and pinned by SHA-256; upstream publishes nightly
builds and no dated URL, so the pin will fail one day, which is the intended
behaviour and the Dockerfile says what to do about it.

The important property is not that it is small: **the pointer is drawn by the
guest, into the framebuffer.** A PS/2 mouse has no hardware cursor for QEMU to
forward, so there is no `Cursor` rectangle and no hotspot — the client is not
told the shape of the pointer or where it is, which is exactly the world a
relative client lives in.

Tiny Core was tried first and does not work here: its Xvesa reaches the VESA
BIOS through `/dev/mem`, which a current kernel refuses (`Interrupt pointer …
doesn't point at ROM`, then `no screens found`), so X never starts and the
console has no pointer at all. The container booted, the pointer mode
negotiated, and there was nothing to look at.

## What this rig can be asked

- **Which end owns the cursor**, and that our client changes its behaviour when
  told: `pointer mode: relative` arrives unprompted, right after `SetEncodings`.
- **Whether the deltas mean what we think.** `RFB_MOVE=dx,dy,count` walks the
  pointer, and two frames either side of the walk locate the cursor to the
  pixel — the guest draws it, so it is in the picture.
- **Whose acceleration.** The answer is visible in the walk: 40 × (10, 5) moves
  this guest's pointer by (187, 64), not (400, 200), because the transfer curve
  at the far end is now the one that counts.
- **The A/B against the same guest.** `POINTER=absolute` plugs in a USB tablet,
  QEMU announces absolute, and the client goes back to owning the cursor with
  nothing else about the session changing.

`RFB_TYPE='ls /usr/local/bin\n'` types into the guest through the same client,
which is the only way into a virtual machine that has no shell of its own on
this side. Note that **QEMU does not synthesise Shift**: it maps a keysym to a
scancode, so `*` sent with nothing held arrives as `8` — the probe holds Shift
itself for the characters a US keyboard puts behind it, which is the same
thing the RDP client has to do for a layout that needs Shift.
