# scripts/testrustdesk

A RustDesk far end in a container: their released 1.4.9 `.deb`, pinned by
SHA-256, over Xvfb and Openbox, answering **direct IP access on 21118** and
registered with the public rendezvous network so the id path works too.

The desktop is [`../testvnc`](../testvnc)'s — the same wallpaper, clock,
terminal and document — so a picture from this rig and a picture from that one
are of the same thing.

```sh
scripts/testrustdesk/run.sh              # build if needed, run, print address and id
scripts/testrustdesk/run.sh id           # the peer id, which is generated on first start
scripts/testrustdesk/run.sh shot         # what the far end looks like, without a viewer
scripts/testrustdesk/run.sh logs
scripts/testrustdesk/run.sh stop
```

**The host's GPU is forwarded where there is one** (`/dev/dri`), and the image
carries `va-driver-all` and `intel-media-va-driver`. Their server asks VA-API
what it can encode: with the device it finds `h264_vaapi` and this becomes the
only far end here that sends **H.264 over RustDesk**, which is what the codec
row is checked against; without it, it settles for VP9, VP8 and AV1 in software.
The hardware is the host's, so a measurement says which machine it was taken on.

`PASSWORD` sets the permanent password (`rustdeskpass`), `GEOMETRY` the screen,
`PORT` where 21118 lands on the host. `RENDEZVOUS=host:port` and `KEY=…` point
it at a self-hosted `hbbs` instead of the public network — which is
[`../testrustdeskserver`](../testrustdeskserver), and it prints the pair of them
to paste; `RENDEZVOUS=none` registers with nothing at all, leaving direct IP as
the only way in.

**The peer inside this container is never reachable by the address a rendezvous
server hands out for it**, since that address is on Docker's bridge. That is not
a fault to work around: it is the only far end here that forces the relay, and
it is how both relay paths were driven.

Two things the container has to do that a person does not, both in `start.sh`:

- **The password is written into the config rather than set with their CLI.**
  `rustdesk --password` talks over an ipc socket to a service and refuses
  unless the binary believes it is installed. A plain value in `RustDesk.toml`
  is accepted and is rewritten in their obfuscated form on first start.
- **The background is set after the window manager, not before it.** Xvfb has no
  persistent root pixmap, so the other rigs' order leaves a black screen.

What this rig answered is [45a](../../notes/77-rustdesk-checks.md)'s check 2:
**their X11 capture works under Xvfb**, unprompted, first try — which is the one
[49](../../notes/49-h264.md) lost a day to with a headless compositor.
