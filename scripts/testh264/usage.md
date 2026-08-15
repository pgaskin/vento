# H.264 test desktop

A container that serves a desktop over the **Open H.264 encoding** (RFB encoding
50), which nothing else in this repository can produce and only the TigerVNC
backend can decode. sway under wayvnc, with neatvnc doing the encoding on the
host's GPU.

```sh
./run.sh                 # build if needed, run, print the address for the phone
./run.sh logs            # sway's and wayvnc's output
./run.sh shot [out.png]  # what the far end looks like now, without a viewer
./run.sh stop
```

Defaults: `1920x1200`, port `5903` on the host, **no authentication**. Override
with `GEOMETRY=`, `PORT=`, `DRM_DEVICE=`.

## What it needs, and why it is shaped like this

It **nests in the host's Wayland session** and passes through the host's render
node. Neither is a convenience:

- neatvnc will only encode a frame that arrived as a GPU buffer object — *"h264
  is useless for sw frames"* — so wayvnc has to capture into a DMA-BUF, which
  needs a DRM file descriptor from the compositor's backend. wlroots' headless
  backend has none, and its linux-dmabuf global fails to start; its Wayland
  backend takes one from the compositor it is nested in.
- The encoding itself is VAAPI on the host's GPU, so `vainfo` must report an
  `H264 … EncSlice` entry for the render node. `start.sh` prints what it finds
  before anything else starts.

So sway runs on **two** backends at once. The Wayland one is there for its file
descriptor and its window stays blank on purpose; the desktop being served is a
headless output beside it, which is what gives this rig a size of its own — a
tiling compositor decides how big a window is and will not be told.

## Two things about this server worth knowing before believing a measurement

**A width that is not a multiple of 16 shears the picture.** At 952 pixels wide
every frame arrives sheared into diagonal ribbons, and ffmpeg decodes it exactly
as the phone does, so it is the encoder rather than any client: it is fed a
buffer whose rows are padded to 960 and told the picture is 952. Keep `GEOMETRY`
a multiple of 16.

**The first client after the server starts may get Tight instead.** The encoding
is chosen per frame from the client's list, and the very first frame is captured
before the buffer pool has been reconfigured for DMA-BUFs — so H.264 is skipped
for that frame and Tight, if the client offered it, is chosen and kept.
Reconnecting is enough.

## What is on it

The four X clients `scripts/testvnc` runs, through Xwayland, for the same
reasons — except that here the clock earns its place twice over: one small
region repainting once a second is the worst case for a codec that sends a whole
frame whatever changed, and the document scrolling is the best one.
