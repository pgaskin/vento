# SPICE, with an agent in the guest

The fifth rig, and the second QEMU: the same protocol `testqemu` serves, against
a guest that runs **`spice-vdagent`**. That is the whole difference and it is
most of what SPICE can do that RFB cannot — the clipboard, the resize, the
monitor layout and a pointer the client owns without a tablet being plugged in.
KolibriOS has no agent and never will, so the two rigs are the two halves of the
protocol rather than a fast one and a slow one.

```sh
./run.sh                        # build if needed, run, print the address
./run.sh logs                   # the guest's console and QEMU's own output
./run.sh guest 'xrandr'         # run a command inside the guest
./run.sh shot /tmp/guest.xwd    # what the guest's own X server has on screen
./run.sh console                # a shell on ttyS1, for debugging by hand
./run.sh stop
AGENT=off ./run.sh              # the same guest with no agent, for the A/B
```

Defaults: SPICE on `5940` and its TLS port on `5941`, ticket `spicepass`, 768 MB,
KVM if this machine has it. `SPICE_WAN=1` turns on the two encodings a client
cannot ask for, as in `testqemu`.

## The guest

**Alpine, built when the image is**: `apk --root` lays a root out of the build
container's own index, `mke2fs -d` turns it into an ext4 image without anybody
being root, and the kernel and an initramfs sit beside it. There is no installer
and no downloaded disk image to pin, because there is no disk image — what is
pinned is `alpine:3.22` and what the packages resolve to on the day it is built.
The disk is attached with `snapshot=on`, so a run writes to a temporary overlay
and every run is the same desktop.

In it: X with the **QXL driver**, jwm, two xterms and a clock, `spice-vdagent`
and its daemon, and an `apply-preferred-mode` loop. `guest-init` is PID 1's
whole init — udev, dbus, the agent, the command channel, X — because what this
guest has to bring up is a list and not a dependency graph.

**`apply-preferred-mode` is standing in for a desktop environment.** A client's
monitors config does not reach the agent at all when the guest's display is QXL:
the SPICE server intercepts it and hands it to the virtual hardware, so what
arrives inside is a new *preferred RandR mode* and something has to switch to
it. GNOME and KDE do that themselves; a bare window manager does not, and
without those four lines a resize looks from the client like a request that was
ignored.

## Running a command inside the guest

`./run.sh guest '<command>'` is a shell on a virtio-serial port — a second port
beside the agent's, with no SPICE in it — and it is what makes this rig
checkable from a script: `xclip -o` says what the guest's clipboard holds,
`xrandr` says what size the desktop is, and neither needs a person looking at a
screen.

Two things about it, both of which cost a run to find:

* **Redirect anything you leave running.** A background process inherits the
  port as its standard streams and a virtio port opens once — so an orphan
  holding it means every later command times out. `cmd >/dev/null 2>&1 &`.
* Data the host writes while nothing in the guest has the port open is
  **discarded**, not queued. The guest's shell announces itself when it has the
  port, and `guest-cmd` waits for that before sending — which is why a command
  can take up to a second to start.

## What this rig can be asked

- **The clipboard, both ways.** `SPICE_COPY=…` on the probe puts text on the
  guest's clipboard, and `./run.sh guest 'xclip -selection clipboard -o'` reads
  it back; `printf … | xclip -selection clipboard -i` in the guest sends it the
  other way, and the client prints what arrived.
- **The resize.** `SPICE_RESIZE=1280x800`, and then `xrandr` in the guest and
  the picture's own size at this end.
- **A pointer the client owns without a tablet.** `AGENT=off` takes the agent
  away and the same guest goes back to server mouse mode, which is the A/B
  `testqemu` cannot do because its guest has no agent to take away.
- **A guest that draws with QXL**, where KolibriOS drives plain VGA — a
  different set of draw commands out of the same server.
