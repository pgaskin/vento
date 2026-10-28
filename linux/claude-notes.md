<!--
SPDX-FileCopyrightText: 2026 Patrick Gaskin
SPDX-License-Identifier: GPL-3.0-or-later
-->

# linux

A native Linux client for the backends under `backends/`. One process that can
hold a session on any of the eight protocols — the four Rust ones directly, the
four C/C++ ones through the shared libraries this crate's `build.rs` has Meson
build — written against the same `Backend` trait the Android app is. It is an
egui window over that trait, and a headless probe for proving a backend
without one.

## What is here

The `Backend` trait is not this crate's: it is in `backends/api`
(`remotedesktop-api`), which the phone holds every session but RealVNC's
through too, and every backend's adapter to it is in that backend's own
crate. On the phone the same adapters sit behind one JNI layer
(`backends/platform/android`), so a backend is written once for both
clients, and what differs between them is each client's own code.

- **`backends/api`** — the `Backend` trait and the value types around it
  (`Facts`, `ConnectionFact`, `Monitor`, `Traffic`, `Picture`); the `Event`
  enum (`runtime::Callbacks` plus the C API's `question`, `security_needed`,
  `message`) and the `Mailbox` a session pushes them onto, with a wake hook
  the window hands its repaint request to; the option descriptions and the
  catalog of the eight backends, so `-o key=value` and the dialog's rows are
  generic; `Kind` and `Connect`; the credentials and trust rules — how a
  prompt's answer is read, and `trust::decide`, which meets a far end's
  identity with the pins in whatever `trust::Store` the client keeps — which
  the phone's JNI layer applies too, held to their cases
  (`tests/vectors.rs`); and, under its `c` feature, the one adapter over the
  vtable in `vento/backend.h`.
  Damage is coalesced into a dirty-rect list behind a mutex and delivered one
  `Event::Frame` per update, never a message per rectangle; a window that
  falls behind finds one unread `Frame` per desktop size and the latest
  cursor, pointer mode and clipboard, not a backlog. This crate re-exports
  its `backend`, `events` and `options` under those names.
- **The adapters** — `src/backend.rs` in each Rust protocol crate
  (`backends/modules/{rfb,ironrdp,rustdesk,spice}`), and a small crate
  beside each C library (`backends/modules/{libvnc,tigervnc,freerdp,realvnc}`,
  the C in its `cpp/`) over the `api` crate's C adapter, linked on the
  phone into the library Meson makes of the C — but for RealVNC's, which is
  this client's alone, since its library hosts the viewer's own core (the
  phone's RealVNC backend is its own Java over the viewer's JNI), and which
  opens that library at run time rather than linking it (see
  [RealVNC](#realvnc)). None is this crate's.
- **`src/backends/`** — the list of the eight as this binary has them, in the
  catalog's order, and what a session here is told about this machine.
- **`src/gui/`** — the window: `mod.rs` (the winit loop; one GPU device, queue
  and egui renderer shared by every window, each window a `Surface` of its own
  with its wgpu surface, egui-winit state and egui viewport, all in one egui
  context; the focus resolved between the windows once per turn of the loop,
  with tests), `session.rs` (a live session, and what of it is the same in
  every window: the backend and its events, the desktop, the keyboard and the
  capture, the prompts, the far end's cursor; the windows it asks for and
  closes, with tests), `pane.rs` (one window's part of a session: its view,
  which monitor it shows, its toolbar, the pointer over it and its cursor;
  the main window's or a pop-out's, with tests), `desktop.rs` (the desktop as one
  texture, or as tiles where it is bigger than the device's largest, updated
  a damaged rectangle at a time, or a picture on the GPU's texture),
  `zerocopy.rs` and `zerocopy.wgsl` (a hardware picture imported from its
  DMA-BUFs and converted on the GPU, with tests), `view.rs` (scale modes, panning and the
  pointer's mapping, pure arithmetic with tests), `toolbar.rs` (where the
  toolbar sits and which strip of the edge brings it out, with tests),
  `icons.rs` (the toolbar's font and the symbols it draws, tested against
  the font and its codepoints file), `follow.rs` (the desktop
  following the windows' sizes, a monitor per window where the far end takes
  a layout: the phone's `WindowFollower`, pure logic with tests), `keys.rs` (winit keys to X11 keysyms), `display_server.rs` (which display
  server the window is on),
  `wayland.rs` (the Wayland keyboard shortcuts inhibitor), `x11.rs` (the X11
  keyboard grab that stands in for it), `cursor.rs` (the far end's cursor as
  the window's own, with tests), `hud.rs` (the debug overlay's box and rates,
  with tests), `settings.rs` (the client's own settings and their rows),
  `dialog.rs` (the connection dialog), `panel.rs` (the connection-info panel,
  with the live controls), `prompts.rs` (the modal prompts).
- **`src/known_hosts.rs`** — this client's `api::trust::Store`: where
  it keeps its pins (what they mean, and what is asked, is `api::trust`)
  and RealVNC's identities, as files in the state directory
  (`--state-dir`, `$XDG_STATE_HOME/vento` by default), with tests.
- **`src/main.rs`** — the `vento` binary: the window, `--help`,
  `--version`, `--list`, `--forget`, and the headless probe.
  **`src/cli.rs`** — the command line's GNU-style splitting, the help text
  and the version shown, the formatting of the help's `backends:` section
  and of `--list`, and `--probe`'s spec, with tests.
- **`metadata/icon.png`** — the window icon, a copy of the app's
  (`app/fastlane/metadata/android/en-US/images/icon.png`, 512×512), which
  `build.rs` decodes, scales to 112×112, rounds at the corners (radius 20%
  of the side) and sets over a soft drop shadow in a 128×128 RGBA image for
  `gui/mod.rs` to embed (see [Display servers](#display-servers)).

## Building and running

This crate is a Cargo workspace of its own, not a member of the repository's
(the root `Cargo.toml` says why: a desktop GUI stack's optional features must
not unify into the phone's dependency graph and its licence page). Build it
from this directory, or with `--manifest-path linux/Cargo.toml` from the root;
its binary lands in `linux/target/`.

Host packages (the same the C host build needs, plus a libclang for bindgen):
Meson 1.11 or later (`meson` on the path, or `MESON`; `mesonVersion` in
`backends/build.gradle` is the version releases are built with), ninja, make,
m4, perl (with its core modules: Fedora's perl-core), python3, pkg-config,
nasm (on x86, for the video decoders'
assembly; without it they are built in plain C, with a build note, and decode
several times slower), a C/C++ compiler — **clang** for the RealVNC backend
(libc-shim does not build under GCC; without clang that one backend's library
is not built, with a build note, and the backend says it is not available) —
and the development files for zlib, plus libclang. Not
libjpeg-turbo or pixman: both are built from their submodules
(`third_party/libjpeg-turbo`, `third_party/pixman`, as the phone's are) and
linked into the backends that use them, so that a built tree does not need a
distribution's (whose libjpeg is `libjpeg.so.62` on Fedora and Debian and
`libjpeg.so.8` on Ubuntu). NASM gives libjpeg-turbo its SIMD too. Not GnuTLS
or nettle either: the phone's release tarballs of both (GnuTLS 3.8, nettle 4.0
with its mini-gmp) are configured and built by their own autotools build in
the Meson tree, once (about a minute and a half), and linked into the libvnc
and TigerVNC libraries, since nettle's soname changes with nearly every
release. The tarballs are not submodules: the first build downloads them
from the mirrors in `backends/crypto-sources.txt`, checks them against the
SHA-256 hashes there and unpacks them into `third_party/gnutls` and
`third_party/nettle`, so it needs the network once. `./gradlew
:backends:fetchCryptoSources` unpacks the same trees, and the build's error
without the network says how to unpack them by hand. Nettle's assembly
needs m4; without it, nettle is
built in C, with a build note. GnuTLS takes its CA certificates from
`/etc/ssl/certs/ca-certificates.crt` (Debian, Ubuntu and Fedora all have
it), which decides nothing here: every certificate goes to the known hosts
(**Known hosts**, below). Not OpenSSL either: `third_party/openssl` (3.5,
the submodule the phone's port builds) is configured by its own perl
Configure and built in the Meson tree, once (about half a minute), and
linked into the FreeRDP library, since its soname differs between
distributions (`libssl.so.1.1` on Ubuntu 18.04, `libssl.so.3` on current
ones). It reads no `openssl.cnf` (the system's is written for the system's
OpenSSL, and on Fedora includes its crypto policy), and the CA certificates
it would find under `/etc/ssl` decide nothing either: FreeRDP is told the
client manages certificates, so every one goes to the known hosts. RealVNC's hosting layer links the compiler's static
`libatomic.a` where there is one (on Fedora, `libatomic-static`), and the
shared `libatomic.so.1` otherwise.
Nothing of PipeWire or ALSA: their libraries are loaded at run time where
they are installed (**Sound**, below).
The window needs a Vulkan (or GLES) driver wgpu can use and a Wayland or X11
session.

**No FFmpeg of the system's**, neither its development files nor its
libraries: the client's FFmpeg is built from `third_party/ffmpeg` (n8.1.3) and
`third_party/dav1d` (1.5.4) by `backends/platform/linux/build.rs`, once per
target directory and profile (about a minute and a half; a stamp beside it
says what it was built from, the compiler's version among it, and it is built
again only when that changes), with the compiler as `CC` runs it, a launcher
such as `CC="ccache gcc"` included,
as static libraries that the Rust decoders (RustDesk's video, and TigerVNC's
and FreeRDP's H.264; FreeRDP's AAC and Opus sound) and FreeRDP's scaling
link — FreeRDP through `ffmpeg_prefix`, which `build.rs` hands the
Meson build, and only libswscale and libavutil of it. For a target other
than the build machine, FFmpeg's configure is told so
(`--enable-cross-compile`, the arch and OS, and cc-rs's compiler and archiver
for the target: `CC_<target>`, `AR_<target>`) and dav1d gets a Meson cross
file, both in the stamp; only their making is tested (`cross.rs`), as no
cross toolchain has been at hand to build with. TigerVNC links none of
it: its H.264 decoder context hands each rectangle to the Rust decoder
through a table of function pointers in its config, and FreeRDP's H.264
subsystem and its rdpsnd device hand it each frame and each wave the same
way. It is configured `--disable-everything`, `--disable-autodetect`,
without programs, network or the format, filter and device libraries, with
exactly these decoders: H.264, H.265, VP8, VP9, AV1 (dav1d's), AAC and Opus,
libswscale and libswresample (which libavcodec's Opus decoder needs), and
the hardware decoders of those video codecs: VAAPI's for each (with libavcodec's
own AV1 decoder, which decodes only on hardware), built against headers from
`third_party/libva` (2.23.0) and nothing else of it: FFmpeg's VAAPI code
reaches libva through `cpp/va_shim.c`, which loads `libva.so.2` and
`libva-drm.so.2` on first use, so the binary does not depend on it. Its own code
(`backends/platform/linux/cpp/vento_video.c` and `vento_ffaudio.c`) is the
only C that includes FFmpeg's headers for the Rust decoders, which call a
few dozen functions of it declared by hand.

**Hardware decoding is off by default**, and off it is the software decoder
it always was: no device is opened, and libva is not loaded. Asked for — the
**Hardware decoding** setting, or `--hw-decode`, decoding as `--video` says
(`auto`, the default, or `vaapi`, which is the same; `off` disables the
setting) — the Rust decoder (RustDesk's picture, and TigerVNC's H.264)
decodes on VAAPI, stream by stream, where the device has a decoder for the
stream's profile, and in software where it has not, with the reason kept:
**VAAPI first, then software**. One device is opened for the whole process,
by the first decoder that asks, on the first DRM render node. Where there
is no device (`vaapi: no device`, `vaapi: no libva (...)`) or no decoder for
the profile (iHD has none for H.264 High 10 or AV1 4:4:4: `vaapi: H.264 High
10 is not supported`), the stream is software's. Which codecs a machine
decodes is its libva driver's: Intel's **iHD** (`intel-media-driver`, what
this was measured on), Mesa's for AMD; a Mesa built without the
patent-encumbered codecs (Fedora's) has no H.264 or H.265 decoder, so those
streams are software's there, and NVIDIA's own driver has no VAAPI at all.

Unless it is drawn from the GPU (below), the picture comes off the device
as planes in memory (NV12, P010 or XV30) and is converted like a software
one, so the two look the same: VAAPI's
surface mapped for reading — each decoder measures, on its first pictures,
whether mapping the surface in place or as an image the driver copies it
into is faster on this machine, and keeps to the faster. A hardware failure
mid-stream is a software decoder in its place and a key frame asked for,
not a lost picture. What it gains is CPU more than time: on Lunar Lake at
1080p (release build, a full-motion clip), H.264 on VAAPI took 10-11 ms a
frame decoded and converted against software's 7 (slice threads on eight
cores), at half the CPU; AV1 on VAAPI 9-10 ms against dav1d's 12. Most of
VAAPI's time is the GPU's
decode (about 3 ms) and the read back of the whole picture, which a mostly
still desktop costs as much as a moving one, where software's decode is
cheap. RustDesk's, TigerVNC's and FreeRDP's **Encoding** row says which,
after the codec: `AV1 (software)`, `H.264 (hardware)`, `EGFX 10.6, H.264
(software)`, nothing before the first picture; and the debug overlay's `dec` line says how and what a frame costs
(`dec H.264 vaapi map 2.1 ms`, `dec H.264 sw 3.2 ms (vaapi: no device)`).

**RustDesk's hardware pictures are drawn from the GPU** where the window
can, with nothing read back (zero-copy): on wgpu's Vulkan, with a device
that imports DMA-BUFs (`VK_EXT_external_memory_dma_buf`, which the window
asks for at start where the adapter has it) and samples the surface's
layout. A VAAPI picture in NV12 or P010 is then not mapped at all: the
decoder waits for the GPU to finish it and hands it out held, its surface
exported as two DMA-BUFs (Y, and U and V interleaved; Intel's 4-tiled
layout on Lunar Lake). The window imports a surface once and keeps the
import while the decoder reuses the surface, converts each picture with one
render pass — `runtime::media::video::convert`'s integer arithmetic, P010
narrowed as in software — into a texture of the picture's size, and draws that
texture where the desktop's tiles would be: the same view and filter, every
window, the cursor on top, and the same pixels (a screenshot of an H.264
session differs from the read-back one in no pixel; the tests hold the pass
to `convert` for every pair of chroma samples, and an imported picture to
its read-back on the fixtures). The decoder makes more surfaces as the
window holds some, so nothing waits; a held picture is let go of once the
GPU has read it. The framebuffer is then behind the picture, and caught up
(read back once) only for whoever reads it — `read_region`, a thumbnail;
the probe never asks for pictures on the GPU. Where the window cannot —
the GL backend (`WGPU_BACKEND=gl`), an X server without DRI3, where wgpu's
Vulkan is lavapipe and has only the linear layout, a P010 stream on a
device without 16-bit textures, an import or a pass that fails — pictures
are read back as before, which the log says once and the overlay says
(`dec H.264 vaapi map 9.5 ms  zero-copy off (wgpu is on Gl, and only Vulkan
imports DMA-BUFs here)`); a failure turns it off for the rest of the
session, the next picture read back. Measured on Lunar Lake (debug build,
1920x1200, a terminal of random text redrawn ten times a second, 30 s of
`/proc/PID/stat`): H.264 drawn from the GPU took 4.1 ms a frame on the
protocol thread (the decode and the wait for it) and 9.7% of a core for the
whole process, where read back it took 13.4 ms and 16.3%, and in software
8.6 ms and 16.2%; VP9 3.7 ms and 8.9%, AV1 3.6 ms and 9.3%, VP8 4.2 ms and
7.2%. The window's pass costs it 2.3 ms a picture in a debug build, most of
it wgpu's own unoptimised code; in a release build 0.7 ms, and the same
H.264 comparison is 4.0 ms and 2.9% from the GPU, 11.9 ms and 9.0% read
back, 8.3 ms and 10.8% in software. **TigerVNC and FreeRDP stay
on read-back**: their H.264 is rectangles composed, with every other
encoding, into a framebuffer in memory, which a picture on the GPU would
have to be composed with there. H.265 4:4:4's XV30 is read back too.

The setting reaches every backend: the session's decoders carry it.
**RustDesk** asks them each frame and takes a change at once — a new
decoder, and a key frame asked for, on the same session. **TigerVNC** decodes
its H.264 rectangles through the same decoders (handed to the C as
`vento/video.h`'s table) and takes a change at once too: each rectangle's
decoder is made again in the new mode and the whole screen asked for
(a non-incremental update request, the nearest thing RFB has to a key frame
request, which wayvnc answers with one); a picture or two in between is
refused and nothing wrong is drawn. TigerVNC keeps a decoder per rectangle,
up to 64, and at most four of them hold a hardware decoder; the rest decode
in software. Against wayvnc at 1920x1200 with a terminal scrolling (debug
build), a rectangle took 3.7-4.0 ms in software where upstream's libavcodec
context, which this replaced, took 5.3 — it converted the whole picture
every frame, where only the rows that moved are converted now, straight
into the framebuffer — and 10-12 ms on VAAPI, whose read back of the whole
picture is the cost here as it is for RustDesk.

**FreeRDP** decodes its H.264 through the same decoders, on both
platforms: `patches/freerdp/0001` adds a slot to the library's table of
H.264 subsystems for one the application links in (`WITH_H264_EXTERNAL`),
and the module's `h264_vento.c` is it. A frame goes to a vento decoder
(`vento/video.h`'s `decode_planes`), which hands back its picture as I420
planes, and FreeRDP's own code does the rest as it does with the decoders it
ships: the rectangles, AVC444's combine of its two pictures (two frames of
one stream through the surface's one decoder), the conversion into the
surface. None of FreeRDP's own H.264 decoders is built (no
`WITH_VIDEO_FFMPEG`, no `WITH_MEDIACODEC`); what it links of FFmpeg is
libswscale, for the graphics pipeline's scaling (a surface mapped onto a
scaled output), without which it offers Windows nothing past EGFX 10.4. A decoder per surface, at most four of them on VAAPI,
and a change of the setting applies at once: the session advertises the
graphics pipeline's capabilities again, which Windows answers with new
surfaces, whose decoders are made in the new mode (one picture may be
skipped while it does). A device that fails mid-stream does the same, in
software for the rest of the session or until the setting changes; one
update is lost. Against Windows 10 at 1920x1200 with a full-screen
animation (debug build, 18 s), the process used 2.3-2.7 s of user CPU in
software and 1.8-2.3 s on VAAPI, against 2.7-2.8 and 2.4-2.5 through
FreeRDP's own FFmpeg path before; the overlay's line is the decoder's
(`H.264 vaapi map 7.2 ms`, `H.264 sw 3.9 ms`). The phone's FreeRDP decodes
on MediaCodec through the same Rust decoder as RustDesk's and TigerVNC's,
and says "(hardware)" or "(software)" as they do. The other backends decode
as their libraries do and do not use the setting.

The submodules must be initialised (`git submodule update --init --recursive`;
RealVNC's four host-only submodules need `--recursive`) and patched
(`./patch.sh -n` at the root). Four are this crate's alone: FFmpeg,
dav1d, libva's headers, above, and winit, at the tag of the release `Cargo.lock` names, which
`Cargo.toml`'s `[patch.crates-io]` takes in place of the crates.io one so that
our fixes to it live in `patches/winit/` like every other submodule's.
Unpatched, winit builds and is exactly crates.io's, with the defects the
patches fix.

Licences: the client is GPL-3.0-or-later like the rest of the repository, and
what it takes from crates.io keeps each crate's own licence, as `Cargo.lock`
lists them. One is not code and is built into the binary as it is: the
toolbar's icons are Google's Material Symbols Outlined font, version 2.975
(Apache License 2.0), in `assets/material-symbols/` with its licence and its
codepoints file. It comes from google/material-design-icons at commit
49d4db35df87 (2026-10-09), which ships Material Symbols only as a variable
font; egui sets none of a variable font's axes, so what is here is a static
instance of it made with fontTools (unfilled, weight 400, grade 0, optical
size 24; `src/gui/icons.rs` has the command), 1.4 MB of the binary. FFmpeg is
linked statically into the binary and the TigerVNC and FreeRDP libraries, as
configured LGPL 2.1 or later (no `--enable-gpl`, no `--enable-nonfree`;
`third_party/ffmpeg/COPYING.LGPLv2.1`), unmodified; dav1d, inside it, is
BSD-2-Clause (`third_party/dav1d/COPYING`). FFmpeg is compiled against
libva's headers, which are MIT (`third_party/libva/COPYING`), and the libva
shim is ours;
libva is not linked. libjpeg-turbo is linked statically into the libvnc and
TigerVNC libraries (the IJG License, with the Modified BSD and zlib licences
for parts of it: `third_party/libjpeg-turbo/LICENSE.md`; its TurboJPEG API,
which is BSD-3-Clause alone, is not built), and pixman into TigerVNC's (MIT:
`third_party/pixman/COPYING`). OpenSSL is linked statically into the
FreeRDP library, configured without its tools, engines, loadable modules,
legacy algorithms and the protocols an RDP client does not use (Apache-2.0:
`third_party/openssl/LICENSE.txt`). GnuTLS is linked statically into the libvnc
and TigerVNC libraries, as released and configured without its tools and
optional libraries (LGPL 2.1 or later: `third_party/gnutls/COPYING.LESSERv2`,
the copies of libtasn1 and libunistring inside it included), and nettle
with it, which TigerVNC also calls itself for its RSA-AES security types
(LGPL 3 or later, or GPL 2 or later: `third_party/nettle/COPYING.LESSERv3`
and `COPYINGv2`), with nettle's mini-gmp, from GMP and under the same two
licences, in place of GMP. RealVNC's links GCC's libatomic where the
compiler has a static one, under the GCC Runtime Library Exception, as
every program GCC builds links libgcc. libpipewire-0.3 (MIT) is not linked either, nor
compiled against: the sound output declares the functions and structures it
uses by hand and loads the system's library at run time. Nothing of this crate's graph reaches the
Android app or its licence page.

```sh
cd linux

# The connection dialog: pick a backend, fill in the address, user, password,
# domain and that backend's options, and connect. Nothing is saved.
cargo run

# The same dialog, filled in from the command line — a backend on its own, or
# with an address, a user, a password and options. Nothing connects until
# "Connect" is pressed: a machine on the command line is not a machine to
# connect to, which a typo would otherwise be.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass
cargo run -- rdp

# The client's own settings, which are every connection's rather than this
# one's: the clipboard in each direction (both on unless told otherwise), the
# keyboard grab that sends Alt+Tab and the rest on (on unless told), and the
# far end's cursor drawn into the picture rather than made the window's own
# (native unless told), and video decoded on the GPU (off unless told; then
# as --video says, auto by default: VAAPI, else software, stream by stream).
# The dialog's "Settings" section has the same five. Options take their value
# as --opt value, --opt=value, -o value or -ovalue, and short options stack
# (-cu NAME).
cargo run -- tigervnc 127.0.0.1:5901 --no-send-clipboard --no-recv-clipboard
cargo run -- tigervnc 127.0.0.1:5901 --no-grab-keyboard
cargo run -- tigervnc 127.0.0.1:5901 --cursor software
cargo run -- rustdesk 127.0.0.1 -o ConnectBy=address --hw-decode
cargo run -- rustdesk 127.0.0.1 -o ConnectBy=address --hw-decode --video vaapi
cargo run -- tigervnc 127.0.0.1::5903 -o H264=true -o Encoding=h264 --hw-decode

# Session options of this run: the main window's first scale (auto by
# default; fit, fill and the rest as the Scale button names them; none is
# 1:1, none-resize 1:1 with the window made the desktop's size once it is
# known), and view only forced, its toolbar button and panel
# row disabled. --trust accepts every trust, security and message prompt
# without showing it, reading the known hosts and writing nothing.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass -c --scale none-resize
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass -c --view-only --trust

# No sound output at all (the session is handed none), or one by name.
cargo run -- spice 127.0.0.1:5930 --password spicepass -c --audio off

# Straight into a session, with --connect. Without --password the credentials
# prompt asks. Disconnecting it quits; if the far end ends it instead, the
# dialog comes up with the reason.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass --connect
cargo run -- rdp 127.0.0.1:3389 --user proto --password protopass --connect
cargo run -- rustdesk 127.0.0.1 -o ConnectBy=address --password rustdeskpass --connect
cargo run -- spice 127.0.0.1:5930 --password spicepass --connect

# On a given display server rather than winit's choice (Wayland where
# WAYLAND_DISPLAY is set, X11 otherwise).
cargo run -- --platform x11 tigervnc 127.0.0.1:5901 --password vncpass --connect
cargo run -- --platform wayland

# The version, the options, then the backends and their options
# ("backends:"); each backend's library is opened for its version.
cargo run -- --help

# "vento v6": the crate's version, which is 0.0.N for the app's release N
# (`versionCode` in `app/build.gradle`, checked by the release workflow),
# shown as vN.
cargo run -- --version

# Each backend and the file its code runs from, one line each: the library a
# C backend loaded, or this binary for a Rust one; where a library cannot be
# loaded, "<name> <path it looked for> (error: <why>)".
cargo run -- --list

# Forget a server: its pins, its RealVNC identity and RealVNC's own record of
# it, for every backend; connects nowhere, prints what it removed.
cargo run -- --forget 127.0.0.1:5901

# A session, headless: connect, print every event, answer the prompts from the
# flags, run the steps of the spec in order (waits, input, size requests,
# the framebuffer and its thumbnail written), print the facts, and leave.
# --probe SPEC (-P SPEC) selects this instead of the window; a first step of
# wait-connect=SECONDS sets how long it waits for the connection (30 s
# otherwise). A certificate the known hosts match is accepted without
# --trust; --trust accepts any other for this run and pins nothing.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass -P \
    'wait-connect=10,wait=1,click=500:300,key=0x61,clipboard="hello, \"you\"",wait=4,out=/tmp/shot.ppm,thumb=/tmp/shot-thumb.ppm'
cargo run -- freerdp 127.0.0.1:3389 --user proto --password protopass \
    --trust --probe wait=2,resize=1280x720,wait=3,out=/tmp/rdp.ppm
```

`build.rs` runs the Meson build at the repository root, which makes each
backend a `libvento_<name>.so` carrying its vendored static dependencies and
the stderr logger. The binary links none of them: each backend's crate
opens its library by name the first time it is needed (see *Backend
libraries* under *Installing*). It finds them through the binary's rpath,
**`$ORIGIN/lib`**: the `lib/`
directory beside the binary, wherever that is. `build.rs` lays the target
directory out that way, the libraries hard-linked into
`target/<profile>/lib/` beside `target/<profile>/vento`, so `cargo run` works
without anything on `LD_LIBRARY_PATH`; a second entry, `$ORIGIN/../lib`, is
what the test harness in `target/<profile>/deps/` finds them by. Nothing
absolute is in the rpath. The C probes are not built
here: they are a plain `meson setup … -Dprobes=true` of their own
(backends/UPGRADING.md, Verification); the option is off by default, so this
build compiles none of them.

### Installing

An installed client has the target directory's layout: the `vento` binary
and, beside it, a `lib/` directory with the `libvento_*.so` files (RealVNC's
only where the build had clang). The rpath is relative to the binary, so the
two can go anywhere together: `/opt/vento`, a home directory.

**Release builds come from `linux/Dockerfile`**, which runs the whole build
in a container and outputs only that tree:

```sh
# from the repository root, with the submodules initialised
docker build -f linux/Dockerfile --output type=local,dest=build/dist/vento .
```

The binary needs the glibc symbol versions it was linked against, so it runs
on the build's glibc and later ones. The Dockerfile builds on Ubuntu 18.04
(glibc 2.27), with clang 18, Meson and NASM fetched at pinned
versions, and Rust at `rust-toolchain.toml`'s channel; the glibc baseline is
the Dockerfile's alone. It copies the checkout (`linux/Dockerfile.dockerignore`
limits what is sent), applies `patches/` to submodules still at their pinned
commits, runs `cargo build --release --locked` (which downloads the GnuTLS
and nettle tarballs and checks their SHA-256; the context leaves out any
unpacked in the checkout), strips the libraries' debug information, and
checks the tree: the relative rpath (`readelf`), no library missing (`ldd` on
the binary and each library), and `vento --list`, which opens each library,
naming the tree's own `lib/` for all four C backends and no error.
It writes nothing to the checkout. A build from scratch takes about nine
and a half minutes on eight cores (the toolchain about a minute and a half
of it, cached after the first build; GnuTLS and nettle about a minute and a
half more, and OpenSSL half a minute).

A build on the machine itself gives the same layout, tied to that machine's
glibc and libraries:

```sh
cargo build --release
mkdir -p /opt/vento/lib
cp target/release/vento /opt/vento/
cp target/release/lib/libvento_*.so /opt/vento/lib/
```

The tree does not carry the system's libraries. A machine running it needs
glibc (2.27 or later for the Dockerfile's tree), zlib, and per backend:
**libvnc** nothing more; **tigervnc** libstdc++; **freerdp** nothing more
(its UTF-16 conversion is WinPR's own, not ICU's); **realvnc** libstdc++
(and libatomic, where the build's compiler had no static one).
libjpeg-turbo, pixman, GnuTLS, nettle and OpenSSL are inside the
libraries. Loaded at run time where they are there, and not needed
otherwise: libpipewire-0.3 and libasound (sound), libva and libva-drm
(hardware decoding), and the display stack (Vulkan or GL, Wayland or X11).

**Backend libraries.** The binary does not link the backends' libraries: each
backend's crate opens its own (`api::c::library`) the first time it is
needed, with every symbol bound then (`RTLD_NOW`). A library that cannot be
loaded (the file missing, a library it needs missing or too old, a file of
another build) makes that one backend not available, and the binary starts
and runs the others. The backend is still listed: `--help` opens every
library to show its version, or `not available: <reason>` with the loader's
reason, `--list` the file it was loaded from (read from `/proc/self/maps`)
or the one it looked for and `(error: <reason>)`, and a session on it, from the command line or the connection dialog,
fails to start with "<backend> is not available: <reason>". A window opens
only the library of the backend it connects with, when it connects. A library
stays open once opened. The three open backends' libraries are opened with
their symbols kept to themselves (`RTLD_LOCAL`) and are linked with
`-Bsymbolic`, so that each calls its own `vento_log`; RealVNC's is opened
into the global scope, which its hosting layer needs.

The probe answers the first credentials prompt with `--password` and a second
with a cancel, since a second prompt is the far end refusing the first answer
(SPICE and RustDesk ask again for every refused password); it then says the
authentication failed and leaves with 1. The clipboard settings hold in the
probe too: under `--no-recv-clipboard` what the far end copies is printed as
its size alone. So does `--hw-decode`: the facts then say the
codec with `(hardware)` or `(software)`, and a `decoder` line after them is
the overlay's `dec` line, with the reason where hardware was asked for and
the picture is software.

`--probe`'s spec (`cli::steps`, with tests) is a comma-separated list of
`name=value` steps, run in order once connected, with nothing between them,
except `wait-connect=SECONDS`, which is taken only as the first step and is
how long the probe waits for the connection before any of the others run
(`cli::CONNECT_TIMEOUT`, 30 s, without it; anywhere else it is refused).
`wait=SECONDS` (a fraction is fine) prints events for that long, so a click
that should land on a drawn desktop comes after a `wait=1`. `click=X:Y`,
`key=KEYSYM` (read as probe.c's `--key` reads it), `clipboard=TEXT`,
`resize=WxH`, `layout=WxH;WxH;…`, `sound-cycle=SECONDS`, `out=FILE` and
`thumb=FILE[:N]` (1/N, 8 unless given) each print the line they always have.
A value runs to the next comma; a part of it in single or double quotes holds
commas and the other quote, with `\\`, `\'`, `\"`, `\n` and `\t` as escapes
there (and nowhere else), so `clipboard="a, \"b\""` sends `a, "b"`. An empty
spec connects, prints the facts and leaves. Steps stop at the first one that
finds the session gone.

The probe's exit status is 0 only if it connected within `wait-connect=`,
stayed connected through the steps, wrote every `out=` and `thumb=` picture,
and closed within 10 s of disconnecting; it is 1 otherwise (a failed write
does not stop the steps after it), and 2 for a command line or spec it
refused. `click=`, `key=` and `clipboard=` are sent, not confirmed: no
protocol here says input arrived, so 0 says they went out on a live session
and nothing more; under `-o ViewOnly=true` every backend drops them. A
`clipboard=` step fails the run (1) where the backend can tell, at the end,
that the text went nowhere, which today is SPICE without a guest agent that
takes a clipboard.

`resize=WxH` asks for a desktop of that size, as the window does when it is
resized, and `layout=WxH;WxH;…` asks for that many screens side by side,
handing the far end's own screens back by position (their ids kept) and
numbering any past them afresh. Like input they are sent, not confirmed: a
`desktop_size` line, and the facts printed at the end, which list each
monitor as `id=N WxH+X+Y`, say what the far end made of them, and
`scripts/testvnc/heads.sh` says the same from the server's side.

The probe prints the session's sound as it changes: `sound switch: Enable
audio on` (or `off`, or `Mute audio off`, or `none`) whenever the switch the
window would show changes, and `audio playing` and `audio stopped` as a
stream starts and the last one closes. A `sound-cycle=N` step turns an Enable
switch off, and on again, every N seconds from then on once the backend has
offered one (`sound-cycle=0` stops it), printing each turn. The far end's side
of it is what the socket carries, which `ss -tin` on the client shows.

`--verbose` lets the backends' debug lines through (and not the GUI stack's,
which would be tens of thousands a minute). What is typed is never among
them: the lines that name a key — `key down 0x61 as 3` for every key sent,
and a backend's word that it has no way to send a keysym — are trace lines,
which only `RUST_LOG=trace` in the environment lets through (with every
debug line). That is for a test of the keys; a log with them in it holds any
password typed into the far end.

**Log lines** go to stderr as `HH:MM:SS.mmm L tag: text`, e.g.
`14:03:27.512 I TigerVnc: CConnection: Using RFB protocol version 3.8`: the
local time to the millisecond (short, and what the far end's own logs and
the user's clock are read in; the date is left out), then the level letter
(`W` for warnings and errors, `I`, `D`, `T`) and the tag. One writer makes
every line, `vento_log_line` in `backends/platform/linux/cpp/vento_log.c`,
in a single `writev`: the C backends' `vento_log` (their libraries each carry
a copy of that file), the Rust logger (`src/log.rs`, whose tag is the crate's
name, through `platform_linux::stderr`), FFmpeg's log under the tag `ffmpeg`
(installed by `log::init`; libva's errors come through it too), and on a
desktop FreeRDP's WLog under `FreeRdp`, with the logger's name and function
in front (`routeWLog`, unless `WLOG_APPENDER` is set; its level is still
`WLOG_LEVEL`'s), which otherwise writes its info lines to stdout among the
probe's. The probe's own lines on stdout keep their `[seconds]` since the
start: they are its output, not the log. Not covered: the user-facing
`vento: …` errors, and what a library writes to stderr itself (the RealVNC
loader's `linker:` lines and its core's `Failed to parse log param element`,
libpipewire's and ALSA's own messages, Mesa's).

## The window

One window, which is either the connection dialog or a session. A session
that ends goes back to the dialog, filled in with what it was connected to and,
unless the disconnect was asked for, told why it ended — and to a window, if
the session had gone fullscreen. The exception is the session `--connect`
opened: that one is the whole program, so disconnecting it (the toolbar's
Disconnect, or Cancel while it connects) quits with status 0, the backend
released as when the window is closed. Only an asked-for disconnect does: if
the far end hangs up or the connection fails, the dialog comes up with the
reason as for any other session, and a session started from there afterwards
goes back to the dialog when it is disconnected. The window's title is the desktop's name once
the protocol has said it (the address until then), and rings the bell by putting one in the title for a
moment — unless the backend's Bell option (`AcceptBell`, on the VNC backends)
is off, in which case a bell is dropped before the window or the probe sees
it. The name is the far end's to choose, so it is cleaned once, as the
client reads it, for every place it shows — each window's title, the
panel's heading and row, the name a mixer lists the session's sound under:
control characters are dropped (a NUL, which neither X11's title property
nor Wayland's can carry, used to take the client down), tabs and newlines
become spaces, the ends are trimmed, and past 128 characters it is cut with
an ellipsis; a name of nothing but control characters is none, and the
address stands in. A typed or pasted address is cleaned the same way
before it is a title.

The connection dialog is a panel in the connection panel's frame (`panel::frame`:
an egui window's, no shadow), 520 points wide or the window's width less 8 points
(`panel::MARGIN`) at each side, down to the connection panel's 360. It is centred
across the window and 40 points below the margin at the top, or centred in what
room there is in a window too short for that, so switching backends does not move
it unless the window is crowded; it scrolls where it is taller than the window
allows. Why the last session ended is shown under its heading. Behind it, and in a
session's window around and before its desktop (the "Connecting to …" screen),
the backdrop is #111111 (`gui::BACKDROP`); the panels, the prompts and the toolbar
keep egui's fills. Every window, pop-outs included, has a minimum inner size
(`gui::min_size`, logical points, set with `with_min_inner_size`): its width is the
largest of the connection panel's width plus a margin at both sides (376), a
prompt's width with its frame (394) and the toolbar with all nine of its buttons
(334), so 394 with egui's default style; its height is 360, which keeps the
dialog's heading, a notice and its connection fields in view (the panels scroll,
so their content sets no height).

Nothing in the interface fades or folds: egui's own animations are off
(`animation_time` and the programmatic scroll's, both zero), so a dropdown, a
tooltip, a prompt or the panel is there at full opacity the frame it is
opened, and gone the frame it is closed. The one movement left is the
toolbar's slide in and out at the top edge, below, which is the client's and
not egui's.

Disconnecting, or closing the window, ends a session at once, including one
that is still connecting: every backend's dial and handshake give way to it,
where several used to sit out their 20-second connect timeout first. A session
that has still not closed ten seconds after a disconnect is given up on, and
a backend is always released off the window's thread, so neither the dialog
coming back nor the window closing waits for one that will not let go; once
the window has gone the process gives it two seconds, then exits anyway.

A session is told when its window cannot be seen — minimised, on X11 wholly
covered, on Wayland suspended — and the backends pause their updates then, as
the phone's do when its session screen stops: the three VNC backends stop
reading, FreeRDP and IronRDP send a Suppress Output PDU, SPICE stops
acknowledging, and RustDesk drops to a frame a second. They resume when it is
shown again. A window that is on screen without the keyboard focus stays
live. X11 says both through winit: `_NET_WM_STATE_HIDDEN`, checked every
frame and on a focus change, and the visibility events, which under a
compositing manager never say covered. The check is no round trip to the X
server: winit read the property on every call, and `patches/winit/0005` has
it keep the property until the window manager's `PropertyNotify` says it
changed (counted with the RECORD extension on the rig: two `GetProperty` a
second while a session drew, before; one at each minimise and restore and
none otherwise, after).
Neither says anything of a window whose frame the window manager unmapped
to put its workspace away — i3's workspaces not on any output and its
scratchpad, a monitor i3 lost, Openbox's other desktops: X sends no
visibility event for a window that stops being viewable, and i3 sets
`_NET_WM_STATE_HIDDEN` only on a tab that is not the active one. So the
server is also asked whether the window is viewable at all
(`GetWindowAttributes`, on winit's connection), once a second while frames
run — one round trip a second, and a put-away window pauses within that;
the visibility event its frame's remap brings makes it live at once. The X
server's own screen saver and a lock screen cover the window with one of
their own, which the visibility events report as covered, and the session
pauses behind them as behind any window and resumes when they go; a
monitor blanked by DPMS alone changes nothing, and the session stays live.
Wayland has no minimised state, but xdg-shell v6 has `suspended`, which a
compositor may send while the window cannot be seen at all (minimised, on a
workspace not shown); `patches/winit/0002` makes winit report it as
`Occluded`, as winit 0.30 did not. Where the compositor does not send it —
sway 1.11 and niri do not — a session never pauses there; a hidden window's
frames stop with its frame callbacks while the backend keeps receiving.
Checked on `scripts/testx11` for tigervnc, libvnc, rfb and freerdp: live with
another window focused, paused minimised or covered, live again restored;
on Openbox, paused on another desktop and live again back on this one; on
an Xvfb split into two monitors under i3 with tigervnc (the socket's receive
queue filling to 15 KB within a second against an animated far end, 0 live):
live on the other monitor's workspace with the focus elsewhere and with a
fullscreen window on the focused monitor, paused with its workspace switched
away, in the scratchpad, as an inactive tab, under the server's screen saver
window and with its monitor gone, live again the moment each was undone; and
on Wayland with tigervnc in a headless tinywl (wlroots 0.20) made to suspend
its windows on a signal: paused (the socket's receive queue filled to 10 KB,
no commits) while suspended, live again when it was not.

**Sound** plays as a PipeWire stream of the session's own where a PipeWire
daemon answers (`backends/platform/linux/src/pipewire.rs`), and on ALSA's
`default` device where none does (`alsa.rs`: PulseAudio's plugin on a
PulseAudio desktop, PipeWire's from a sandbox that is granted only the pulse
socket, a card on a desktop with no sound server). libpipewire-0.3 and
libasound are both **loaded at run time**, not linked: nothing of either is
needed to build the client or to start it, a machine without libpipewire
plays through ALSA, and one with neither runs without sound — the log says
so once (`audio: no sound output: no PipeWire (…), and ALSA cannot be used
either (…); the far end's sound is dropped`) and everything else works. Every session's streams are nodes of one
connection to the daemon (the process's one client, `Vento`), each named for
its session (`media.name`: the desktop's name, the address until the far end
says one), `media.role` Movie; each is pulled by the graph a cycle at a time
(512 frames asked for, `node.latency`), so nothing is buffered between the
queue and the graph, and the device's share of the delay — the cycle and
what the sink adds — is read off the stream's clock and is what the queue
and RDP's wave confirms count. libpipewire honours `PIPEWIRE_REMOTE`,
`PIPEWIRE_NODE` (the sink, as `target.object`), `PIPEWIRE_LATENCY` and
`PIPEWIRE_PROPS` as it does for any client. A daemon that restarts is
connected to again within a second or so and the streams made again on it;
one that is not back after three tries a second apart leaves its streams to
ALSA until it is, which they look for every five seconds. A stream that
fails on its own — refused as it is made, or failed by the daemon later — is
made again a second later, and goes to ALSA at its third failure in a row.
`--audio`
`pipewire`, `alsa` or `auto` (the default; also `VENTO_AUDIO_OUTPUT`) picks
one for tests: `pipewire` plays nothing without a daemon rather than going
to ALSA. `off` hands the session no output at all, so the far end's sound is
dropped and neither library is loaded. Which one plays is in the panel's Sound row (`Opus 48 kHz stereo
(PipeWire)`) and the overlay (`+ PipeWire 11 ms`). Against ALSA's `default`
on a PipeWire desktop — PipeWire's ALSA plugin, with a 50 ms ring in front of
the graph — the client's share of RDP's sound is 23-28 ms rather than 84-89
(77 rather than 105 when the queue kept a fixed total of 100 ms) and a mute
is heard in 5 ms rather than 55-65.

Either way it is behind one queue for every backend (`runtime::media::audio`),
which keeps in front of the device only as much as keeps the device from
finding it empty: a period of the device's, what the far end's own rhythm
has needed over the last eight seconds — its largest write, or the most that
lands at once, and how much later than its sound a write has come against
the writes before it (writes that land together as late as the first of
them: the second of Windows' pairs is a wave further on, no lateness) — and
a margin: 30 ms as a stream starts, 5 ms less
after every 10 s of its sound that did not run dry, and 10 ms from 40 s on;
with the device's buffer behind that (ALSA's is 50 ms; PipeWire's a cycle,
5-11 ms on a null sink), and never more than 250 ms together, past which the
oldest is dropped, so that a burst costs a skip rather than a delay that
stays. On the rigs that is, from 40 s on, 32-50 ms for QEMU's, SPICE's and
RustDesk's 10-20 ms of sound at a time, 55-65 ms for RDP's 23 ms waves, and 395 ms for Windows' PCM, which comes in
186 ms waves two at a time (and twice that before anything is dropped). A
stream starts once it has that, and from a far end that sends its writes in
groups only once the group has landed (up to 25 ms after its last write, the
queue then as short as it would have been had it started on it): a start
between the two writes of a pair kept the second on top, a wave further
behind than needed until it was dropped eight seconds in, in one skip (one in
three of `pattern.sh`'s Windows PCM runs on PipeWire; none in seven since). Where
the queue runs dry anyway and the far end's sound comes within half a second
— later than anything it had shown — the session remembers how late, a
step of 5 ms more (to 100 ms; a write later than that was a stall, a gap
whatever is kept, and is not remembered at all, and what the far end sends
to catch up after it is skipped in one jump rather than played behind),
the queue starts again once what it has lasts until the far end's next
write as late as that one, with the period and the margin, rather than once
it has its whole target (`pattern.sh`'s Windows PCM with a pair 60 ms late:
a 55-60 ms gap and nothing dropped, where, taken for a stall, it had been a
gap until the next pair, 255 ms or more of the tone, and 326 ms dropped), and the margin covers that from then on wherever the
rhythm of late does not, forgetting a step after every half minute of sound
that did not run dry (a gap, then none: under 64 busy loops a SPICE session
ran dry twice in its first 40 s and not in the 50 s after, and
`scripts/testaudio/pattern.sh`'s QEMU-like writes, held back 40 ms every 5 or
20 s, ran dry at the first stall only); and a queue that has kept more than
that for eight seconds drops the excess, faded over 2.5 ms, so that the
sound is never further behind than it has to be. On an idle machine,
minute-long sessions to Windows and SPICE ran dry with no margin in three of
four and with 5 ms in one of four; with 10 ms, one of seventeen sessions of
half a minute to five minutes across the six backends with sound ran dry,
once. The start's 30 ms is for the seconds before the far end's rhythm has
shown much, and for a loaded machine, which a margin learnt from a gap is
late for: with 64 busy loops started as a SPICE session began to play, a
10 ms margin from the start ran dry in each of three sessions, 11 to 45 s
in (26-32 ms late; 7 gaps in the first 30 s of two of them), and the start's
margin in none of three for 76 s (one at 77 s, 36 ms late), the first 30 s
84-95 ms end to end rather than 67-87 and the last 66-75 rather than 79-139
(a margin learnt from a gap stays for minutes). Idle, it costs 20-25 ms in
the first 30 s — QEMU's sound 74 ms end to end on PipeWire rather than
48-51, RDP's client share 41 rather than 28 — and nothing after (48 and
26 ms). Loaded from before the connection, four of seven sessions ran dry
once as the stream began, 111-411 ms late, with either margin: no margin is
for that, and it is not remembered — `pattern.sh`'s writes held back
250 ms half a second in, beside 64 busy loops, play 22-30 ms behind a
minute later on PipeWire and 84 on ALSA, where remembering it as 100 ms
kept them 104-107 and 168. Every stream starts at it, not at what the session's earlier
streams came down to, since its rhythm starts from nothing; the lateness
the session remembers is a floor under it rather than added to it. A
fixed total of 100 ms, which this replaced, held QEMU's sound about
70 ms longer and RDP's about 55 ms. TigerVNC's QEMU audio plays there, offered to
any server wherever the session has an output and asked for while the
session's sound switch is on — the `Audio` option is where it starts, and
the switch is there from the server's first update that says it has audio:
off, QEMU's disable message takes the client off the guest's card, the
stream closes at once and nothing more is sent (on `scripts/testqemu`'s
audio guest the socket went from 195 kB/s of samples to under 100 B/s
within the half second, and back on its enable), so a session that began
without it can still have it. The stream opens when the
server begins sending, closes two seconds after it stops and its tail has
been heard, and closes while the session is paused, since nothing is read
then. What the server sent meanwhile arrives at once on restore, and the
queue keeps only the newest of it. On `scripts/testqemu`'s audio guest
(`AUDIO=1`, timed by `scripts/testaudio/bursts.py`) the guest's card to the
null sink takes 47-49 ms on PipeWire, 106-111 ms on ALSA, depending on the
connection, and stayed within 2 ms of that over five minutes (with a
fixed 100 ms between the queue and the device, 116-118 and 124 ms; with the
queue's 100 ms in front of the device rather than around it, 121-150 ms).
TigerVNC's own ALSA sink,
which this replaced, took 83-88 ms until the first pause and 1013 ms after
it, for as long as the stream ran: its second's buffer filled with the
backlog, and a device that drains no faster than the server sends never
caught up. FreeRDP's sound
(Sound = "This computer") plays there too, through the module's rdpsnd
device (`rdpsnd_vento.c`) in place of FreeRDP's PulseAudio one, so the client
no longer links libpulse and the fifteen libraries behind it; a session
handed no output does not ask the server for sound. The device plays PCM
itself, and AAC and Opus through the platform's decoders
(`runtime::media::audio::decode`: the client's FFmpeg here, MediaCodec on the
phone) rather than FreeRDP's, which is built without FFmpeg for its sound;
Windows picks AAC when it is offered (Windows 10 offers AAC, PCM, ADPCM, A-law
and GSM, and sends raw AAC LC frames at 44.1 kHz stereo, one 23 ms frame a
wave). The log says which (`sound: the server sends AAC_MS 44100 Hz 2 ch
16-bit, which the platform decodes`, `audio: decoding AAC ... with aac
(libavcodec)`). Nothing in RDP's sound asks the server to stop sending
during a session (the client's formats are an answer, sent once), so the
session's sound switch is a mute, there from the server's list of formats
when it lists one this end plays — which Windows sends when something first
plays, a tenth of a second or so before the first wave, and not at
connect — until the session ends. It tells the server how far behind it
plays in every wave's confirm, applies the server's volume as the stream's
gain, and closes its stream two seconds after the last wave has been heard,
since FreeRDP passes the server's close on to no device. The sound keeps
playing while the window is hidden: hiding pauses only the picture, the
server keeps sending sound as it plays it, and nothing arrives late on the
restore. Against Windows 10 (`scripts/testaudio/winbursts.py`, a tone played
in the RDP session) the tone reaches the null sink 252-282 ms after Windows
was handed it on PipeWire and 322-368 ms on ALSA, depending on the
connection, and stays within 0.1 ms of that for five minutes; most of it
is Windows' own and the network's before a wave arrives — which varies by up
to 100 ms from one connection to the next — and 23-28 ms is the client's on
PipeWire, 84-89 ms on ALSA (the queue, up to its 55-65 ms as the waves come
and go, and the device). With a fixed 100 ms between the queue and the
device the client's share was 76-89 ms on PipeWire and 101-111 ms on ALSA;
with the queue's 100 ms in front of the device the tone took 331-451 ms, and
through PulseAudio's device, on another day, 224-309 ms. SPICE's sound is the playback channel, opened only where the server lists
one and the session has an output, and only while the session's sound switch
is on (the `Audio` option, on unless turned off, is where it starts): the
switch is there from the server's channel list, before the picture, and
turning it off closes the channel, which is the whole of asking the server
to stop — a channel that is not there is sent nothing — and turning it on
opens it again, the server starting it where the guest is (on the audio
guest, bursts every two seconds: heard at 132-135 ms while on, none of the
five in the ten seconds off). The channel
offers Opus where the platform decodes it (the client's FFmpeg
here, MediaCodec on the phone), which QEMU then sends, and takes PCM
otherwise. The stream follows the server's start and stop, closing two
seconds after a stop once what was queued has been heard; the guest's own
volume and mute arrive as messages and are the stream's gain. A playback
channel that does not open, or ends, costs the sound and not the session
(the switch goes off, and turning it on tries the channel again),
and hiding the window holds back only the picture's acknowledgements, so the
sound plays on. On the audio guest heard over SPICE (`scripts/testqemu`,
`AUDIO=spice`) the guest's card to the null sink took 68 ms as Opus on
PipeWire and 125 ms on ALSA (121 and 145 ms with a fixed 100 ms between the
queue and the device; 133-140 ms as Opus and 132-134 ms as PCM, flat over two
minutes, before PipeWire). IronRDP's sound (Sound =
"This computer", FreeRDP's row less the remote machine, which IronRDP has no
way to ask for) is IronRDP's own rdpsnd channel with a handler over the same
output (`backends/modules/ironrdp/src/sound.rs`): it offers
the part of the server's format list it plays — PCM, and AAC or Opus where
the platform decodes them — so Windows picks AAC here as it does from
FreeRDP. Device redirection goes with it, announcing no devices, since
Windows sends no sound to a client without it, and Windows sends its format
list only once something first plays, which is when the session's switch
comes, a mute as FreeRDP's is. The stream closes two seconds after
the last wave has been heard, and plays on while the window is hidden. To
Windows 10 (`winbursts.py`) the tone reached the null sink 243 ms after
Windows was handed it, as AAC on PipeWire, the client's share 31 ms (74 ms
with a fixed 100 ms between the queue and the device), and 604 ms as PCM (AAC withheld in a scratch
build: Windows' long PCM waves). RustDesk's sound is the peer's Opus, asked for only
where the platform decodes Opus and played through its decoder, and only
while the session's sound switch is on: the `Audio` option, on as in their
own client, is what the login asks, and the switch, there from the login
unless the peer withholds its sound (their permission, which it can also
give or take back during the session), asks again with their option message
alone, which subscribes the session to the peer's sound or takes it off
(bursts every two seconds: heard at 108-111 ms while on, none of the ten in
the twenty seconds off, the socket going from 14 kB/s to under 1 kB/s);
what arrives after it was turned off is dropped and the stream closed. The peer
sends silence for eight seconds after its desktop falls quiet and then
nothing, so the stream closes ten seconds after the last sound. On
`scripts/testrustdesk`'s desktop with a sound (`AUDIO=1`, timed by
`bursts.py`) the desktop's sink to the null sink took 40 ms on PipeWire and
92 ms on ALSA (101 and 108 ms with a fixed 100 ms between the queue and the
device; 113-118 ms, flat over two minutes, before PipeWire). The
probe prints `audio playing` and `audio stopped` as a session's
sound starts and stops, and the session's sound switch as it changes. The
switch is the backend's to offer, as soon as its protocol says the far end
may send sound, and is one of two kinds (`runtime::media::audio::Switch`):
**Enable audio**, where the far end can be asked to stop sending its sound and
to send it again during the session — TigerVNC's QEMU audio, SPICE and RustDesk,
above — and off, nothing arrives; and **Mute audio**, where it cannot —
FreeRDP and IronRDP — so the sound keeps arriving and plays silently here.
libvnc, rfb and RealVNC have no sound and no switch. The window shows the
switch as a **Sound** button on its toolbar and a row of the connection
panel (below), and says what is playing in the panel and the debug overlay.

**The toolbar** sits flush with the top edge, square-cornered, edged on its
sides and bottom but not along the top, which is the window's edge, and stays
out of the way: it slides off the top of the window when the pointer is elsewhere —
in a window all but the lower 4 points of its background, the margin under its
buttons, left showing along the edge as a mark of where it is, and in
fullscreen wholly, except that the pointer on the screen's top row anywhere
across it slides out the same 4-point peek, no buttons showing, until the
pointer leaves that row — and slides back when the pointer reaches the top edge within the toolbar's own
span — the rest of the edge is the desktop's, for whatever the far end has at
the top of its screen — and for the first three seconds of a session, so that
it is seen once. In a window that strip reaches 8 points down, since a pointer
coming off the title bar seldom stops on the window's very first row; in
fullscreen, where the screen's edge stops the pointer, it is the top row of
pixels. It can be dragged along the edge, by any part of it, buttons
included — a drag never presses the button it started on — and stops at the
window's sides; there is no grip, and the pointer over its margins is a hand
to show it can be taken hold of there; where it is left is kept as a fraction of the room either side
of it, so that a resize keeps it centred, or against the edge it was put
against, and the next session's toolbar starts there, for as long as the
process runs (it is not saved). The strip that brings it out follows it.
It has no shadow, and its buttons are flat: an icon (Material Symbols
Outlined, unfilled) and no frame at rest, egui's frame while the pointer is
on one or pressing it. A button with a state shows it in its icon, and its
tooltip names the button, its state and, for Scale and Monitor, what a
click goes to ("Scale: auto (next: fit)"). View only and Info while on,
and Sound while off or muted, keep a frame at rest, a dim fill of the selection's
blue with the icon in the selection's colour. There are no words on it: which monitor or display
is showing is in the tooltip too. On it:

- **Scale** cycles auto → fit → fill → fit width → fit height → fill
  width → fill height → 1:1, its icon `desktop_portrait`, `aspect_ratio`,
  `open_in_full`, `fit_page_width`, `fit_page_height`, `width` (↔),
  `height` (↕) and `view_real_size` (1:1) in turn. Auto is the default
  (`--scale` sets another). The aspect ratio is always kept.
  *Fit* shows the whole desktop, shrunk or enlarged, with bars where the
  aspect ratios differ; *fill* covers the whole window, shrunk or enlarged,
  the overflow panned; *auto* is fill's scale but never past a device pixel
  per desktop pixel, so a desktop smaller than the window is shown 1:1.
  *Fit width* and *fit height* never enlarge the desktop past a device pixel
  per desktop pixel; *fill width* and *fill height* do; 1:1 is a desktop
  pixel per device pixel. A desktop that is smaller than the window in an
  axis is centred in it; one that is bigger starts at its top left (with two
  monitors side by side under auto or fill, the first) and is panned: move
  the pointer within 48 points of a window edge, over the desktop, and it
  moves that way, faster the nearer the edge.
- **Resize** (`resize`, beside Scale at 1:1 when not fullscreen) asks for
  the window to be the size of what it shows, the desktop or its monitor,
  in device pixels, so that nothing is panned or left over; a maximised
  window is unmaximised first. It is hidden while the window's inner size
  is that size already, and stays shown where the window manager keeps
  another size.
- **Monitor** (when the far end reports more than one) shows the whole desktop
  (`tv_displays`, several screens) or one monitor's rectangle (`monitor`, one
  screen), with the scale modes applied to that. Where a
  backend offers *displays* instead (RustDesk sends one screen at a time) the
  button (`monitor`) asks the far end for the next one.
- **Pop out** (`open_in_new`, beside Monitor, when the far end reports more
  than one monitor) puts what this window shows in windows of its own: its
  monitor, or every monitor while it shows the whole desktop. See
  [pop-out windows](#pop-out-windows).
- **View only** stops the mouse, the keyboard and the clipboard going to the
  far end, and tells the backend so: an eye while on, a mouse while off.
- **Sound** (wherever the far end may send sound) is the session's sound
  switch, and does what the backend says it can. Where the far end can be
  asked (an Enable switch: TigerVNC's QEMU audio, SPICE, RustDesk) it asks
  it to stop sending its sound and to send it again: `volume_up` while
  asked for it, `volume_off` while not, when nothing arrives at all (the
  extension's disable, the playback channel closed, their option message;
  each backend's own paragraph above has what its socket did). Where it
  cannot (a Mute switch: FreeRDP, IronRDP) it mutes the sound here and lets
  it be heard again: `volume_up` while heard, `volume_mute` while muted.
  It comes when the protocol first says the far end may send sound — at
  connect for TigerVNC and SPICE, at the login for RustDesk, with the
  server's list of formats for RDP, which Windows sends when something first
  plays — and stays, whether or not anything is playing, until the session
  ends or the far end will send no more (a SPICE playback channel that
  ended turns it off; a RustDesk peer that withholds its sound takes it
  away). There is none for a session whose far end sends no sound — RDP
  with its Sound option off or at the remote machine, a session with no
  sound output, a protocol without sound. The connection's Audio option is
  where an Enable switch starts, so a session begun without its sound can
  still ask for it. A mute is at once and needs no reconnect: it is applied
  where the device takes the samples (`runtime::media::audio::Controls`), so
  what is queued keeps draining under it and unmuting is the sound as it is now;
  from the click to the null sink going silent took 65-101 ms (TigerVNC,
  FreeRDP and SPICE alike, the device's 50 ms buffer, a frame and the
  capture), and back 70-100 ms. Either is the session's: a stream the far
  end opens later (FreeRDP opens one per format, TigerVNC one per
  AudioBegin) starts muted, every window's button is the same one, and it
  is not saved — the next session starts as its connection's option says.
  There is no shortcut for it, as there is none for the other buttons.
- **Info** opens the connection panel, below.
- **Fullscreen** fills the screen (winit's borderless fullscreen on the
  current monitor); the toolbar still slides in at the top edge, and the same
  button, or disconnecting, leaves it.
- **Disconnect**.

### Pop-out windows

A far end with several monitors can be shown in several windows, one monitor
in each — the session is still one connection, one desktop texture drawn in
every window, one set of keys and one pointer at the far end. Pick a monitor
with **Pop out**:

- Showing one monitor (picked with **Monitor**), the main window's Pop out
  opens a window for it, the shape of the monitor, and the main window moves
  on to the next monitor that has no window (the whole desktop once every
  monitor has one).
- Showing the whole desktop, it gives every monitor without a window one:
  the main window keeps the first, in the far end's order, and each of the
  others gets a window of its own.

A pop-out is an ordinary new window: it goes wherever the window manager or
the compositor puts new windows, and it is moved, resized and made
fullscreen like the main window — its own **Fullscreen** fills the local
output it is on. On X11 that output is the RandR 1.5 monitor the window is
on, those of `xrandr --listmonitors`, which are what the window manager goes
by (through Xinerama): one per output on multi-head hardware, but a screen
split with `xrandr --setmonitor` — an ultrawide used as two, or the one
framebuffer of Xvfb or of many virtual machines' X servers — is a monitor for
each part. winit listed RandR's CRTCs instead, one for such a screen, and its
borderless fullscreen moves the window to the corner of the one it picks
before asking for `_NET_WM_STATE_FULLSCREEN`, so a window on the right half of
a split screen went fullscreen on the left half; `patches/winit/0006` lists
the monitors (the CRTCs still where the server is older than RandR 1.5), and
`patches/winit/0007` lists them again when a monitor is added or taken away
while the client runs, which comes with no RandR event of its own.

A pop-out's toolbar is a window's own: **Scale** and **Monitor** (any
monitor, or the whole desktop) are its own, **Fullscreen** is its own
window's, **View only** and **Sound** are the session's, and **Return** (`pip_exit`) closes
it and gives its monitor back to the main window, which shows it if it was
showing a single monitor. Closing it from the window manager is the same as
Return. Each window's title names the monitor it shows. The panel, the
prompts and the disconnect stay in the main window; a pop-out with a prompt
waiting says *Answer in the main window*, and the main window asks the
window manager for attention when one comes up while another window has the
focus. Closing the main window, or the session ending, closes every pop-out
first (each letting go of the pointer capture and anything it held), and
then the main window goes back to the dialog or quits as before.

What follows the focus between the windows: the keys (typed into whichever
window has the focus — egui's, not the far end's, while a widget of *that*
window has egui's focus, so a field in the main window's panel takes nothing
typed into a pop-out — and not let go of at the far end when the focus moves
from one of ours to another — only when it leaves the process), X11's
keyboard grab (moved to the focused window), Wayland's shortcuts inhibitor
(the focused window's only), relative mode's capture (taken only in the
focused window, and handed to the next one in the same step when the focus
moves, any buttons held through it still held), and the clipboard (read
when the focus comes to one of our windows from elsewhere, never on a move
between them). A drag belongs to the window it was pressed in until every
button is up — the display server keeps sending that window the pointer
outside it — and the other windows send nothing meanwhile; past the window's
edge the pointer carries on at that window's scale onto the rest of the far
end's desktop, as far as the desktop's own edges, so dragging a window of the
far end's from one pop-out into the window showing the monitor beside it
moves it there. The session pauses (see below) only while every window is
minimised, put away or covered. The texture is sampled nearest only while every
window draws the desktop one to one, and linear otherwise (at one to one, on
whole device pixels, linear draws the same picture), so that two windows at
different scales do not change the sampler back and forth every frame.

A far end's monitors change under the windows — another client resizes, a
screen is added or taken away, the follower below resizes it: each window
finds its monitor again by the far end's id for it (RFB's screen id,
SPICE's head; RDP has none, and there the id is the monitor's place in the
list, which is the only identity RDP gives one), then by its rectangle, then
by its place in the list; a pop-out whose monitor is gone is closed, and the
main window says so (*Monitor 2 of the far end is gone, and so is its
window*); the main window goes back to the whole desktop. A local output
going away is the display server's to deal with: it moves the windows that
were on it, and the window reads which output it is on again when it moves
or changes scale. **Follow window size** follows every window, a monitor
each, where the far end takes a layout (see the panel, below).

Proven on `scripts/testx11` against a private `scripts/testvnc` given two
1280×800 heads with `heads.sh 2`: two windows each showing its own monitor,
typing into the pop-out reaching the far end, a far end's window dragged from
the pop-out into the main window and landing on the other monitor under the
pointer, Return and the window manager's close each giving the monitor back,
`heads.sh 1` closing the pop-out of the monitor that went with the notice in
the main window, one window minimised leaving the session live and both
minimised pausing it, and the disconnect and the main window's close each
closing the pop-out first; and FreeRDP against `scripts/testrdp` with
`-o Monitors=2`. On a headless sway, a drag from the pop-out into the main
window carried the far end's window across, typing reached the far end
through the pop-out, Pop out gave a plain tiled window (not fullscreen), and
Return and the compositor's close (`swaymsg kill`) each gave the monitor
back. On `scripts/testx11` at 2560×800 split into two monitors of 1280×800
with `xrandr --setmonitor` (Openbox), against the same two-head far end: Pop
out from the whole desktop opened one plain window for the far end's second
monitor, without `_NET_WM_STATE_FULLSCREEN`; moved onto the right monitor,
its Fullscreen made it exactly that monitor, 1280×800 at 1280,0, where winit
without the two patches made it the left one, 1280×800 at 0,0, over the main
window; leaving fullscreen put it back where it was; and with the screen
whole again Fullscreen still filled the 2560×800. The capture's hand-over
between windows is unit-tested only: no rig here has a far end of several
monitors in relative mode.

Following, with **Follow window size** on and every monitor in a window of
its own: on `scripts/testx11` against the two-head `scripts/testvnc` with
TigerVNC, the two windows set to 1000×700 and 900×600 with `xdotool
windowsize` made the far end `1900x700, id=1 1000x700+0+0, id=2
900x600+1000+0` (`heads.sh`), one layout for the two, and the pop-out alone
set to 1100×650 then changed only screen 2; on a headless sway with two
outputs, `swaymsg resize set` on the two floating windows did the same, the
far end following the windows' 996×671 and 896×571 to the pixel with ids 1
and 2 kept; and FreeRDP against `scripts/testrdp` with `-o Monitors=2`
went from two monitors of 1280×800 to 1000×700 and 900×600, a desktop of
1900×700, as xrdp's own reset of the graphics pipeline stated them.

### Why RealVNC and RustDesk have no monitors to pick

Neither backend reports monitors, so neither has **Monitor** or **Pop out**,
and neither can be asked for a layout:

- **RealVNC**'s core has no layout in it at all. It never asks for the
  `ExtendedDesktopSize` pseudo-encoding (-308), which is how an RFB server
  says where its screens are — tapped between the core and Xvnc, its one
  SetEncodings lists `DesktopSize` (-223) and not -308, so the server never
  sends it a screen list — and its callbacks into this backend carry the
  desktop's size and nothing else (`setDesktopSize(width, height)` in
  `HostedCore.h`). Nor can its resize be reached through the binding this
  backend has, so it does not follow a window's size either.
- **RustDesk** sends displays, not monitors: each of the peer's displays is a
  picture of its own, its own video stream at its own size (a `VideoFrame`
  names the display it is of), rather than a rectangle of one desktop. The
  backend asks for one at a time (`CaptureDisplays`) and the panel's
  **Display** row and the **Monitor** button switch between them
  (`request_display`). Showing two at once would be a second stream with its
  own decoder and its own texture per window, which is not what a pane on
  the shared desktop is, so there is nothing for a pop-out to show.

**The connection panel** is the phone's `ConnectionPanel`: controls above
facts. It opens in the top right corner and is dragged by anything on it
that is not a control; its text is not selectable, so a drag from a label or
a value moves it. Where it was put is kept in memory for the session (not
saved) as an anchor per axis, a fraction of the window's size: the panel's
edge nearest that edge of the window, or its centre where the panel sits in
the middle 30% of the room it has to move in on that axis (the room rather
than the window, so a panel most of the window's height dragged against the
bottom stays against it). A resize places it from that anchor, kept 8 points
(`panel::MARGIN`) from every edge of the window (`panel::Anchor`); the drag is
constrained to the same rect, which also caps the panel's size. Its frame has
no shadow. egui decides what a press clicks or drags by where the pointer is
at the end of the frame, so a press and a quick move off the panel read in
one frame dragged the desktop and pressed at the far end: `gui::PointerSplit`
makes a press the last pointer event of its frame and a release the first,
holding the rest for the next frame. A window too small for the panel shrinks it,
and a larger one gives it back its full size: its width and the height its
content is offered are minimums as well as defaults, since egui keeps the
size a window was squeezed to and only grows it to fit its content. It is an egui window in `Order::Middle` and the
toolbar an area in `Order::Foreground`, so the toolbar is drawn over it even
after the panel is clicked, which raises a window only among its own order.
It is the main window's alone, so a pop-out has none. It has no title bar: its first
row is the desktop's name (the address until the protocol has said one) over
the session's state line, with the close button, a cross, at that row's right
end; the toolbar's Info closes it as well. The controls are the backend's *live* options from the registry — the
ones `set_option` acts on mid-session, such as the VNC encodings' quality and
compression, SPICE's image compression, RustDesk's codec and frame rate — as
a control per kind (a dropdown for a choice, a box for a switch, a field for a
number or text), showing what the session connected with and then whatever has
been set since, with the dialog's rules for a gated option and a choice that
requires a switch. Such a choice is offered only while its switch is on, and
turning the switch off puts the option back to its default, in the dialog and
the panel alike — unticking TigerVNC's "Offer H.264 encoding" takes Encoding
from H.264 back to Tight — rather than leaving it selected and sent; the
command line refuses one given without its switch (`-o Encoding=h264` without
`-o H264=true`). Then the two that are not options, because whether they
can be offered is a fact about the live connection: **Desktop size**, where the
far end takes one (`can_resize`), offering this window's size in desktop
pixels first, the size the desktop has, the far end's own screens and the
usual sizes, and asking `request_desktop_size` for the one picked — what
arrives is what the far end granted, and the dropdown shows that rather than
what was asked; and **Display**, where the far end sends one of several
screens at a time (RustDesk), asking `request_display`. Then the toolbar's
**Sound** button as a switch, where there is one, worded by what it does:
**Enable audio**, on while the far end is asked for its sound, or **Mute
audio**, on while muted. And **Follow window
size**, below, the five **settings**, below, which a running session
takes a change to at once, and **Debug overlay**, the overlay below, which is
the session's and not a setting.

Under them, the desktop's name, the facts the backend reports in its own words
in the backend's order, the diagnostic ones among the rest, the bytes transferred, the desktop size and
its monitors; and, while the far end's sound plays, **Sound** before the
diagnostic ones — what arrives, `AAC 44.1 kHz stereo` or `PCM 48 kHz
stereo`, with the output it plays on (`(PipeWire)` or `(ALSA)`), and `,
muted` while it is (the row is the client's, worded in
`api::backend::ConnectionFact::sound` for the phone's panel too). Everything is refreshed once a second while the panel is open,
without disturbing a control being used: an open dropdown stays open, and a
field being typed in keeps its text until it is left, which is when its value
is sent. Nothing on the panel has a tooltip.

**Follow window size** (off by default, per session) asks the far end for a
desktop the size of the windows in pixels whenever one changes shape — the
phone's `WindowFollower`, in `follow.rs`: asked on a tick (the panel's
one-second poll, brought forward to 600 ms after any window's resize, one
debounce for them all) once every window has settled for two ticks, never
twice for the same question, and only once the far end says it takes what is
asked, which a VNC server says some time after connecting. Turning it on asks
at once. With fullscreen it makes the desktop the screen's size. Where the far
end takes a layout (`can_layout`: TigerVNC, libvnc and the Rust RFB client
over an Xvnc that sends its screens, FreeRDP and IronRDP over a display-control
channel, SPICE with the guest agent) each window showing one monitor asks for
that monitor the window's size, in one layout of all the monitors left to
right in the far end's order, the far end's ids handed back so that it moves
its screens rather than making new ones, and a monitor no window shows
keeping its own size; a main window showing the whole desktop asks for
nothing over several monitors. Where it does not, or over a far end of one
screen, the main window asks for the desktop its size, as before — and only
over one screen, since a single size over several would merge them into
one. Granted against the test rigs by TigerVNC's Xvnc (through all three VNC
backends), xrdp (FreeRDP; and IronRDP with `-o Compression=false`, since its
re-activation after the resize does not yet take the compressed FontMap PDU
xrdp sends under RDP 6.1 compression, and the session ends, and with
`-o Graphics=bitmap`, since xrdp's session process dies of SIGSEGV on any
layout sent to it with RemoteFX on) and QEMU with the SPICE agent. Layouts of
two screens were granted by the same Xvnc to all three VNC backends, by xrdp
to both RDP backends (the monitors reported as xrdp stated them), and by a
QEMU guest with two QXL heads (`HEADS=2`; the shared `testspice` has one, and
a second screen asked of it is left out).

**Settings** are the client's own, one answer for every connection, as the
phone's `AppSettings` has them: **Send clipboard** and **Receive clipboard**
(both on by default; `--no-send-clipboard`, `--no-recv-clipboard`), **Grab
keyboard** (on by default; `--no-grab-keyboard`), **Software cursor** (off
by default; `--cursor software`), below, and **Hardware decoding** (off by
default; `--hw-decode`, above, decoding as `--video` says, `auto` by default;
with `--video off` the box is disabled). Grab keyboard is whether the window
holds the window manager's or the compositor's shortcuts off at all — the X11
keyboard grab, or Wayland's shortcuts inhibitor, below — so that Alt+Tab,
Super and the rest reach the far end; off, neither is ever taken, and a
running session lets go of whichever it holds the moment it is unticked. They are a section of
the connection dialog, apart from the backend's options because they are not
the connection's, and rows of the connection panel, where a session shows
what it is running with and takes a change at once. Nothing is saved: a
session hands them back to the dialog when it ends, and the process forgets
them when it does.

**Keys** go to the far end as X11 keysyms, from winit's *logical* key — what
the layout says the key means — as Latin-1 keysyms where there are such, the
Unicode keysym (`0x01000000 | codepoint`) otherwise, and a table for the named
keys; the number pad sends `KP_*` with Num Lock on or off (`KP_7`, or
`KP_Home`; winit names the latter Home and says where it was), and modifiers
are told apart by side. A level winit has no name for is sent as the key's
own unmodified level, so Alt pressed after Shift (`Meta_L` on most layouts)
is still Alt — the far end has the Shift and its own layout to make of them
what it does — and a key with no name at any level as its keysym, such as
the pad's 5 without Num Lock (`KP_Begin`). Each
physical key gets a stable id so a release finds its press whatever the
modifiers did meanwhile; every key still down is released when the window
loses focus. The client keeps no shortcuts of its own: the one chord it
reads, Ctrl+Alt in relative mode (below), still goes to the far end, and its
keys stay down there until they are released here, so a far end with a use
for Ctrl+Alt, or for a key pressed with them, still gets it. A key held
down repeats at the far end as its own autorepeat would, but a repeat is
never the escape, and a key the far end no longer has down (one let go of
when input went elsewhere) is sent again only once it is pressed again.
Every key is the far end's while a session has focus and takes input (not in view
only, nor under a prompt) — including the compositor's or window manager's own
chords (unless **Grab keyboard** is off), see below — except while the pointer
is on the toolbar: the toolbar is this end's, so while it is under the pointer
the keyboard is too, the grab or the inhibitor is let go (Alt+Tab is the window
manager's again), the keys the far end has down are released there, and what
is typed goes nowhere. Moving off the toolbar takes the keyboard back.

**The mouse** sends absolute desktop coordinates, mapped through the view and
clamped to it — the desktop pixel drawn under the pointer, by the centre of
the device pixel it is in, so that a scaled desktop's last row and column can
be pointed at — with an RFB button mask (left 1, middle 2, right 4, and a mouse's side
buttons, back 128 and forward 256 — buttons 8 and 9) and the
wheel as momentary presses of 8/16/32/64 (up, down, left, right); smooth
scrolling is turned into notches of fifty points. The side buttons go as each
protocol has them: RFB's ExtendedMouseButtons (`rfb`, `tigervnc`; without the
server's say-so, and always in `libvnc`, whose library writes the one-byte
mask, back goes and forward is dropped), RDP's X1 and X2 (`freerdp`, `rdp`,
where the server takes extended mouse events), SPICE's side and extra, and
RustDesk's back and forward; RealVNC's core is handed the mask as it is and
sends back but not forward (to TigerVNC's Xvnc, which takes both from
`tigervnc` and `rfb`). RFB's horizontal wheel has no place in SPICE and is
dropped there. The far
end's cursor shape,
where the protocol sends one, is the window's own pointer, below, unless
**Software cursor** is on.

**The native cursor** (the default) makes that shape the window's own
pointer — winit's custom cursor, created through the event loop
from the shape's pixels and hotspot and set on the window — so the
compositor moves it, and it is as sharp as the compositor's own. It is used
only where this end owns the pointer: in relative mode the far end draws its
cursor into the picture and the local one is locked and hidden. A shape is
uploaded once (cached by its pixels and hotspot) and set only when what the
cursor should be changes: a new shape, the pointer arriving on the desktop
or leaving it for the toolbar, the default when the far end hides its cursor
or has sent none. Where the desktop is not drawn one to one the shape is
resampled to the view's scale, nearest neighbour, and its hotspot with it;
the log says the size and hotspot of each one set. `cursor.rs` says how this
shares the window's cursor with egui-winit, which sets it from egui's icon.
**Software cursor** (a setting, off by default; `--cursor software`) draws
the shape into the picture under the local pointer instead, with the OS
cursor hidden over the desktop, as the Android app does; in relative mode it
changes nothing, as there the far end draws its own.

**Relative pointer mode** — the far end owning the cursor (QEMU's VNC, SPICE in
server mode) — locks (Wayland) or hides and centres (X11) the local pointer
while the window has focus, and sends raw motion as deltas; the far end draws
its own cursor into the picture. **Hold Ctrl and Alt together to let go** of
the pointer (the two keys still reach the far end); a click on the desktop
takes it back. The session's first capture puts up *Press Ctrl+Alt to release
pointer.* in a small box at the top centre, below where the toolbar comes
down to, for five seconds or until the capture ends, and the title of the
window holding the capture ends in *(Press Ctrl+Alt to release pointer)*
for as long as it lasts. Losing focus or
disconnecting also lets go.


**The clipboard**: what the far end copies becomes the local clipboard
(**Receive clipboard**); what was copied here while the window was away is
offered to the far end when the window regains focus, once per change (**Send
clipboard**) — or, if it is read while input has nowhere to go (a prompt up,
view only, the session still connecting), when it has, unless something newer
was copied at either end meanwhile. The rule is the phone's `SessionClipboard`: the last text seen
crossing either way is remembered and is never sent back where it came from,
and what is read here is remembered whether or not it may be sent, so that
turning sending on offers the next copy and not an old one. Proven against
`scripts/testvnc` with `wl-copy`/`wl-paste` and the far end's `xclip`, both
directions, both ways round, and on X11 with `xclip` at both ends.

The local clipboard is the window's own (`src/gui/clipboard/`), not
egui-winit's, and is read and served on a thread of its own, so the window
never waits on another program. A read is answered when it is in, and the
focus-in offer is made then; it is given up after **2 seconds** in all, and on
Wayland after **1 second** in which the source writes nothing, so a hung or
stopped owner costs a log line and no frozen window. At most 16 MiB is read.
The owner is asked what it has first and the best text type is taken
(`UTF8_STRING`, then `text/plain;charset=utf-8`, then Latin-1 `STRING`, then
`TEXT` and plain `text/plain`); what arrives is decoded by the type it really
came as, and text under a UTF-8 name that is not UTF-8 is read as Latin-1 —
not with U+FFFD in it, which TigerVNC's servers refuse whole. What the far end
copies is served as `UTF8_STRING`, `TEXT`, `text/plain;charset=utf-8` (and
`UTF-8`), `text/plain` (UTF-8) and `STRING` (Latin-1, `?` for the rest). A
Ctrl+C in one of the window's text fields sets the local clipboard, and Ctrl+V
there pastes what the local clipboard holds at that moment: the key starts a
read like the focus-in one, and the text is pasted when it is in, a few
milliseconds later — or not at all, from an owner that does not answer within
the deadline, with the window responsive meanwhile.

**The debug overlay** (the connection panel's **Debug overlay**) is the
phone's `SessionHud`: a small monospace box in the bottom left of the
desktop, with its lines in its order where they apply here, and sampled
rates over a one-second window. First what is the session's, the same in
every window: the state, the desktop's size, damage rectangles in all and a
second, the pixel path (bytes a second through `read_region`, the last
rectangle), and the clipboard's length as the far end would be given it;
the cursor's desktop position (or *theirs* in relative mode) and the buttons
held; whether the mouse is captured (here, or in another window) or
hovering, the buttons, how many keys are held at the far end, and VIEW ONLY
when it is; the display server, the follower's state (off, waiting
for the far end, or the size it asked for), the cursor shape (native, or
software when that setting is on, its size and hotspot) and the two clipboard
switches; three delays (below); and, where the backend decodes video through
this end's decoders (RustDesk), `dec` and the decoder's line: the codec, the
method (`sw`, `vaapi map`, `vaapi dmabuf` for a picture left on the GPU,
`vaapi copy` where the driver will not map it), the smoothed time of the decoder's call, and why it is
software where hardware was asked for — made on the protocol thread twice a
second and copied, not built per frame — and, with hardware decoding on, how
the picture reaches the screen: `zero-copy` and what the window's pass
costs it a picture, `zero-copy idle` before a picture has come that way, or
`zero-copy off` and why; wherever the session has a sound switch, `snd`:
what plays, what is queued in front of the device, the queue's target and
the margin in it, and the device's own buffer, named for the output (`queue
26 ms, target 46 (margin 10) + PipeWire 10 ms`, `queue 30 ms, target 50
(margin 10) + ALSA 50 ms`; the queue and the device together how far behind
what arrived it is heard), the stream's underruns and dropped frames, or
`nothing playing`; and DISABLED while the far end is asked not to send it, MUTED
while it is muted here. Then the
window's own: main or pop-out, the
monitor it shows, the local output it is on, whether it has the focus, its
frames a second, its texture updates this frame and its pixels per point;
its scale mode and scale in device pixels per desktop pixel, the desktop's
origin and size in it, and the shortcuts inhibitor's state as this window
has it (X11's grab being the client's, a window it is not on says *on another
window*). The three delays, each as the last, then the mean and the worst of
the last 32: **render**, from a backend's `frame_end` (stamped when the
mailbox queues the frame; a frame that took in later ones while unread keeps
the oldest) to that frame's pixels being submitted and presented, which
includes waiting for the surface to take a frame; **pointer**, from winit
handing over a mouse event that egui reads (the mouse when it is not
captured) to the pointer event it made being given to the backend in the
next frame; and **keys**, from winit handing over a key to the backend's
`key_down` returning, which is the same call, so it measures this end's
handling and nothing more. None of them is the round trip — input in,
the far end's pixels back — which no backend here can be asked for: the
events a backend gives this end have no echo of the pointer, and a far end
that moves its own cursor (RDP's pointer position, RustDesk's) does not say
it is answering a move of ours. The counters
are kept whether or not it is on — for the delays a clock read per backend
frame, per presented frame, per frame with mouse input and per key, and a
store into a ring — and the box is a few labels a frame while
it is.

**Prompts** are modal dialogs over the session, which is blocked until one is
answered — so Escape, or clicking outside, is a no, and Enter is the first
button (Continue, Connect, OK, or the far end's own confirm label), except on
a certificate or identity that has *changed*, where it does nothing and the
answer is a click (or Tab to a button, and Enter): a key on its way to the far
end must not accept a changed server for you. Enter while held, its repeats,
answers nothing, and Enter in a field submits the form. The prompts: credentials (the user name
field only where the scheme has one; for RDP a name typed without a domain
keeps the session's, `.\user` is a local account, and `MicrosoftAccount\address`,
or an address at a common email domain, is a Microsoft account; the split is
`api::credentials`', and a refused login's message adds advice for its domain,
`api::credentials::logon_failed`), a certificate the
[known hosts](#known-hosts) do not match, an unverifiable server (asked every
time, and never remembered), TigerVNC's question about an RSA-AES server key
(likewise), RealVNC's security prompt with its identity and encryption states in
words and its *remember server* box, and messages (OK, or confirm/cancel when
the far end asks a question).

### Known hosts

A server's certificate is one it made for itself, so — as on the phone, and as
over SSH — its identity is the key: the first time a certificate is accepted
with **Remember server** ticked, its fingerprint is pinned against the address,
and from then on a connection whose certificate matches asks nothing, and one
whose certificate does not asks loudly. The prompt says which it is: *you have
not connected to this server before*, *this server's identity is the same as
`<address>`, which you have connected to before* (the same certificate pinned
under another address — one machine by name and by number), or *this server's
identity has changed since you last connected*, where the box starts unticked.
Cancel, or Continue with the box unticked, pins nothing. A certificate that a
certificate authority would vouch for is asked about like any other: the
question is whether this is the machine it was last time.

The pins are in **`known_hosts`** in the state directory (`--state-dir`, by
default `$XDG_STATE_HOME/vento`, `~/.local/state/vento`), one line per
address: `<address> <fingerprint>`. (Before, they were in
`$XDG_CONFIG_HOME/vento`; nothing moves them, so pins made then are asked
about again.)
The address is the one typed, folded for case and nothing else, so
`host` and `host:3389` are two entries; the fingerprint is SHA-256, as the
backend printed it, and compared as its hex digits whatever their case or
separators. RustDesk pins the peer's key under `<id>@<rendezvous server>` and
takes a first one without asking, since the rendezvous server has signed it;
only a changed one is a question. The file is replaced whole (written beside
itself and renamed over) with mode 0600; a line that does not parse is ignored
and kept, a missing file is an empty one, and one that cannot be read is
treated as empty and left alone. To forget a server, `vento --forget
<address>` (below).

Only the window writes it. The probe reads it — a pinned host connects
headless without `--trust` — and never writes it. `--trust`, in the window or
the probe, says yes for that run and nothing else: the window then reads the
known hosts and shows no trust, security or message prompt, accepting each,
and pins nothing and keeps no RealVNC identity for it.

No backend keeps a store of certificates of its own: FreeRDP is told to read
and write none, TigerVNC hands its certificates over rather than keep its
viewer's `x509_known_hosts` (on the phone as well), and libvnc and the four
Rust clients never had one. What the backends do still keep is RealVNC's, under the state
directory too:

| Backend | What | Where |
|---------|------|-------|
| RealVNC | the core's private key, its own record of identities accepted with *remember server*, its config and logs | `<state>/realvnc/net.pgaskin.remotedesktop/.vnc/` (`rsaKeys`, `identities`, `config.d/`), `<state>/realvnc/vnclog/` |
| RealVNC | the identity the core asks to have remembered, handed back as `Identity` next time — kept by this client, as `RealVncBackend` keeps it | `<state>/realvnc_identities` |
| FreeRDP | nothing; the directory exists because the library will not connect without a path for a store it is never allowed to use | `<state>/freerdp/` |

RealVNC's identity is the core's opaque blob rather than a fingerprint, and
only the core can compare one, so it is not a pin and the phone does not make
it one: the core asks its own security question, and the phone and this client
both leave the core's store where it is while they use it.

**Forgetting a server.** `vento --forget <address> [--state-dir DIR]`
forgets a server everywhere it is remembered, connects nowhere, and prints
each thing it removed: its pins in `known_hosts`, its line in
`realvnc_identities`, and what a backend's library keeps of its own — each
backend's `Kind::forget`, the Linux side of the phone's
`BackendProvider.forgetHosts`, which only RealVNC's has: its core keeps its
own record of the servers it accepted, `<state>/realvnc/net.pgaskin.remotedesktop/.vnc/identities`,
written by the core itself (`<address>=<identity>` for a server with a key,
`<address>/extra=<flags>` for what was accepted of it, such as an unencrypted
connection) whether or not *Remember server* was ticked, and a server in it is
not asked about whatever this client hands the core. The core's lines for the
address go and the rest are kept (the phone's *Clear known hosts* deletes the
file, since it forgets every server at once).

The address matches as the stores key entries, which say nothing of a
backend: every backend's entries for the server are forgotten, so the command
takes no backend. Case is folded; `host:1`, `host:5901` and `host::5901` are
one server (a number under 100 is a VNC display, `::` a port), and so are a
RustDesk ID's pins under every rendezvous server (`<id>@<server>`; name one to
forget that one alone). A bare host is only itself: its port was the
backend's default, and the key does not say which backend's, so `host` and
`host:5900` are forgotten by their own names — the command lists the entries
it kept for the same host. Forget a server while no session to it is open: a
session that ends afterwards keeps what it was told to remember.

TigerVNC's library used to accept a
certificate that verified against the system's certificate authorities (or
TigerVNC's own `~/.config/tigervnc/x509_ca.pem`) and matched the name without
handing it over; a patch in `patches/tigervnc` hands it over too, so it is
asked about and pinned like any other, here and on the phone.

### Display servers

winit picks the display server: Wayland where `WAYLAND_DISPLAY` (or
`WAYLAND_SOCKET`) is set, X11 where only `DISPLAY` is, with no falling back
from one to the other. `--platform x11` and `--platform wayland` override
that, and the log's
`display server:` line says which the window got. What differs between the two
is chosen at run time rather than by a cfg: the keyboard shortcuts (Wayland's
inhibitor, one per window, or X11's keyboard grab, one for the client, moved to
whichever of its windows has the focus), the pointer capture, and the native
cursor's alpha.

Every window (the dialog's, a session's, a pop-out) is opened with the window
icon and the app ID `vento`, which is the Wayland app ID and both parts of
X11's `WM_CLASS`, for window manager rules to match on. On X11 winit sets it as `_NET_WM_ICON`, one 128×128 image (64 KiB of
property data a window; the 512×512 original would be 1 MiB), which the
window manager and task switchers scale. On Wayland winit 0.30 sets no
icon (it does not speak the staging `xdg_toplevel_icon_v1`), and the
compositor looks one up by the window's app ID in a desktop file. The client
installs no desktop file, so a Wayland compositor shows its generic icon.

The dependency choice: winit, egui-winit and egui-wgpu are driven directly
rather than through eframe, because eframe wraps away the things this window
needs — the raw display and window handles for the inhibitor and the X11
keyboard grab, device events for raw motion, keyboard events as winit delivers
them with the layout's logical key, and the event loop's wake-up (the backend's
mailbox wakes it through an event-loop proxy). It costs a page of surface
setup. The zero-copy path reaches under wgpu to wgpu-hal's Vulkan device
(`as_hal`, `texture_from_dmabuf_fd`, `create_texture_from_hal`) and asks
Vulkan one question itself through `ash` (whether the device samples a
layout modifier): the three calls are the part of a wgpu upgrade to read
the release notes for, and `gui/zerocopy.rs`'s tests are what proves them.


#### Wayland

What the window uses beyond `xdg_wm_base`:

- `zwp_keyboard_shortcuts_inhibit_manager_v1` — while the window has focus
  and a session takes input (connected, not view only, no prompt up) and the
  pointer is not on the toolbar, the compositor's own shortcuts are inhibited
  so that Alt+Tab and the like reach the far end. winit does not speak this
  protocol, so `src/gui/wayland.rs` borrows winit's connection and surface
  through the window's raw handles, binds the manager and a seat from the
  registry on an event queue of its own, and creates the inhibitor; it is
  released on focus loss, on disconnect, in view only, under a prompt and
  while the pointer is on the toolbar, and never created with **Grab
  keyboard** off. The log says when it is created, granted (`active`) and
  released. The compositor may show its own indicator
  while it is in force (niri does), and keeps one chord of its own to take
  focus away.
- `zwp_pointer_constraints_v1` and `zwp_relative_pointer_manager_v1` — the
  pointer lock and raw motion of relative mode, through winit's
  `CursorGrabMode::Locked` and `DeviceEvent::MouseMotion`.
- The clipboard is `wl_data_device` on winit's own connection — the
  selection is told only to, and set only by, the client with the focus —
  bound by `src/gui/clipboard/wayland.rs` on an event queue of its own and
  read from its own thread, with a keyboard and pointer of its own for the
  serials. A read is a `wl_display.sync` (the selection arrives just before
  the focus) and then the offer into a non-blocking pipe. Tried in a headless
  sway: a `wl-copy` stopped with `SIGSTOP` was given up after 1 s, and a copy
  at the far end arriving meanwhile was taken at once (smithay-clipboard,
  before, waited until the source was continued). Nothing keeps what the
  window owns once it exits, unless the compositor or a tool does. The
  thread is stopped while the event loop is still there; one that has not
  stopped 2.5 s later is still on the connection, so the windows (which hold
  it) are then never dropped and the connection lasts until the process is
  gone. Before, winit closed it under the thread: with the thread's quit held
  back by a scratch hook, the client ended in a segmentation fault; now it
  exits 0.
- The native cursor goes through winit's `CustomCursor`, which on Wayland is
  a `wl_shm` buffer in the pointer surface. winit takes straight alpha and
  premultiplies it (the shapes arrive straight, which is what the phone's
  `Bitmap.setPixels` takes too, and what TigerVNC's library makes of
  `CursorWithAlpha`'s premultiplied pixels), refuses anything over 2048
  pixels a side, and copies the pixels into a fresh buffer on a
  `set_cursor` — on every one, even of the same cursor, until
  `patches/winit/0004` had it keep the buffer while that cursor stays the
  window's (in a headless sway under `WAYLAND_DEBUG`, setting one cursor
  three times made one `wl_buffer`, not three, and a different one a new
  one). Any other cursor in between, egui's icons included, still drops
  it, which is why the window's cursor is set on a change and never once a
  frame. The buffer is in device pixels:
  with `wp_viewporter` winit sets its destination to the buffer's size over
  the window's scale factor, and without it sets the buffer scale to the
  output's integer scale, so a shape resampled to device pixels per desktop
  pixel is the size of the desktop pixels under it on any scale (tried on
  niri at scale 1; the fractional case rests on the viewporter path). winit
  re-applies the cursor on every pointer enter, so one set lasts. `grim`
  leaves the cursor out of a screenshot unless asked (`-c`).

#### X11

Proven on `scripts/testx11` — Xvfb and Openbox, a plain X server and a real
window manager with global keybindings of its own, and the client on the
host as an ordinary X client of it — against `scripts/testvnc` and
`scripts/testqemu`. Xvfb has no DRI3, so wgpu takes llvmpipe there; on an X
server with a GPU it takes the hardware's Vulkan or GL through the Xlib
handles.

- **The keyboard is grabbed** in place of the inhibitor: an active
  `GrabKeyboard` on the window while it has focus and a session takes input,
  released on focus loss, on disconnect, in view only, while a prompt is
  up and while the pointer is on the toolbar (`src/gui/x11.rs`): there the
  keys go nowhere at the far end, so the window manager has them — in view
  only, and with the pointer on the toolbar, Super+Return reached Openbox's
  witness and Alt+Tab moved the focus. It goes over
  winit's own connection — the Xlib display from the raw handle, as XCB through
  x11rb — because X reports a grabbed key only to the client that grabbed it;
  winit keeps receiving every key. On the rig, Super+Return, Ctrl+Alt+T and
  Alt+Tab all reached the far end (whose own Openbox switched windows) while
  the window manager's witness log stayed empty and the active window did not
  change; with an xterm focused, Super+Return reached Openbox. Every chord
  goes, Alt+F4 included, so **the way out is the mouse**: a click on another
  window has the window manager move the focus, which ends the grab. Not every
  mouse binding survives the grab: Openbox's Alt+drag did not move the window
  while it was held (it did with another window focused). The log says `X11
  keyboard grabbed` and `released`, and the overlay's inhibitor line says
  `grabbed`. With **Grab keyboard** off it is never taken.
- **A refused grab is retried** every 250 ms while the window has focus and
  a session takes input, with the loop woken for it; the overlay says why
  meanwhile (`refused (already grabbed)`). Tried with Openbox's Alt+Tab
  switcher holding the keyboard, and with another client's grab: the grab
  was taken 9 ms after the other one ended.
- **The grab's own focus events are filtered.** A grab and an ungrab make
  the server send focus-out and focus-in events of their own, which winit
  reports as the focus moving; with the focus PointerRoot (no window
  manager) that is an endless ungrab/grab loop. A focus-out while the grab
  is held is checked against the server's focus (the window, or PointerRoot
  with the pointer inside it) and dropped if it is still ours, and a repeated
  focus event is dropped too. On a bare Xvfb with PointerRoot focus and
  another client's passive grab firing, one focus-out was filtered and keys
  kept arriving. Without a window manager the grab then holds until
  disconnect even when the pointer leaves the window, since the server sends
  a grabbing client no focus-out for that — not a supported setup.
- **Relative mode captures without a pointer grab.** winit's X11 `Locked` is
  unsupported, and `Confined` (an active pointer grab) has the server deliver
  every XI2 raw motion twice, once for the grab and once for the root
  window's selection, which would be double speed. So the cursor is hidden,
  raw motion is sent as before, and the pointer is warped back to the
  window's centre whenever it moves; a warp makes no raw motion, and the
  window swallows the motion events it causes. Until the warp, though, the
  pointer is wherever the move took it, and unfenced a fast move and a
  click went to the window beside (`xdotool mousemove_relative -- 700 0
  click 1` activated the witness xterm and ended the capture five times in
  five). So while captured there are XFixes pointer barriers on the
  window's four edges (`x11::Fence`), moved with the window: they stop the
  pointer on its edge, change nothing of what is delivered, and pass the
  warps. With them that move and click, and moves of up to 3000 pixels
  each way and diagonally, stayed in the window every time, also after the
  window was moved; Super+Return and Alt+Tab still went to the far end.
  They need XFixes 5 (2011); without it the capture is as before, unfenced,
  with a warning in the log. Ctrl+Alt, a focus loss and the disconnect take
  them down with the capture, and letting go puts the pointer back where it
  was before the capture. The overlay's mouse line shows the raw motion
  sent (`Δ x,y`): ten moves of 20 read 200,0 over VNC (`rfb`) and over
  SPICE, and the guest's cursor moved. A single report of 900 pixels, more
  than half the window, read 900,0 against the barrier and was warped
  straight back. Raw values
  from an absolute device (a VM's tablet, xdotool's absolute `mousemove`)
  are positions rather than deltas, which relative mode cannot use — as on
  Wayland. XI2 raw events stay selected whatever the focus, since winit
  deselects them on every focus-out, the filtered ones included.
- **A wheel notch is one notch.** A wheel without smooth-scroll valuators
  comes as a press and a release of buttons 4–7 per notch, and winit 0.30
  made a wheel event of both, so each notch went to the far end twice; it
  makes one of the press only with `patches/winit/0001`, upstream's
  03dad26c from the 0.31 X11 backend cherry-picked onto 0.30. A real wheel under libinput was not affected (its
  buttons are flagged emulated and dropped, the valuators read instead);
  XTEST's were, and those of an X server that is itself a remote session
  (Xvnc, xrdp). On the rig, `xdotool click 5` three times and `click 4`
  once over a TigerVNC session gave the far end 6 and 2 presses before,
  3 and 1 after.
- **A window destroyed from outside closes cleanly.** Any client may
  destroy any window (`xdotool windowclose`, a window manager's forced
  close), and the window's focus-out arrives before the `DestroyNotify`;
  winit 0.30 panicked when asked the size of a window that was gone in
  between, and `patches/winit/0003` has it answer with the last size it
  reported. On the `Destroyed` that follows the window drops its surface
  and closes as if asked: on the rig, `xdotool windowclose` on a TigerVNC
  session exited 0 with the backend torn down (it panicked before).
- **The native cursor is an Xcursor, premultiplied.** winit copies the
  pixels it is given into a Render cursor as they are, which wants them
  premultiplied, so `cursor.rs` premultiplies them for X11 (Wayland's side
  does that itself). Read back through XFixes (`scripts/testx11/cursor.py`),
  each shape had the size and hotspot the log said, and no pixel had a
  colour above its alpha. There is no buffer scale: the cursor is in device
  pixels, so the resampling is right at any scale factor (×2 at
  `WINIT_X11_SCALE_FACTOR=2` in a fill mode, ×1 at 1:1).
- **The clipboard is the window's own** (`src/gui/clipboard/x11.rs`), which
  reads and serves `CLIPBOARD` from a thread of its own over a connection of
  its own. Proven both ways with `xclip`, every advertised target converting.
  A read asks for `TARGETS` first and receives `INCR`; an owner that never
  answers is given up after 2 s with the window answering `_NET_WM_PING` in
  milliseconds throughout (arboard, before, tried six formats for four
  seconds each, 24 s frozen). Text up to 1 MiB is served in one property and
  longer text by `INCR`, in 1 MiB steps. On exit it hands the text to a
  clipboard manager if one runs (`SAVE_TARGETS`, waiting up to 2 s); without
  one, what the far end copied is gone when the window closes, as with any X
  client (on the rig, `xclip -o` then found no owner).
- **Fullscreen** is `_NET_WM_STATE_FULLSCREEN`, which needs an EWMH window
  manager; under Openbox the window became the screen's 1600×1000, and
  with follow window size on the far end followed it there and back. It is
  on the RandR 1.5 monitor the window is on (see
  [pop-out windows](#pop-out-windows)): with the screen split in two by
  `xrandr --setmonitor`, a window on the right half went fullscreen on the
  right half, where with winit's list of CRTCs it was moved to the CRTC's
  corner first and went fullscreen on the left.
- **The scale factor** is `WINIT_X11_SCALE_FACTOR` if set (a number, or
  `randr`), otherwise `Xft.dpi` over 96 from XSETTINGS or the resources,
  otherwise RandR's idea of the monitor's DPI (1 on Xvfb, which reports a
  0 mm screen).

**Xwayland** — `--platform x11` on a Wayland session — is for testing, and the
Wayland backend is the one to use there; the log says so when the X server is
Xwayland. Tried on niri through xwayland-satellite 0.8.2 (Xwayland 24.1,
rootless): the window opens on the hardware's Vulkan, connects, draws the far
end, and takes its keyboard grab, and in relative mode the capture engages
and Xwayland puts its pointer at the window's centre. What is *unverified*,
because it needs hands on the real keyboard and mouse and no input was
injected into the live session: whether that grab holds off niri's own
shortcuts (that is the compositor's decision; niri does not offer ordinary
clients the Xwayland keyboard grab protocol, and its shortcuts sit behind
satellite), whether the real pointer stays in the window while captured
(Xwayland honours a warp through pointer-constraint emulation that depends
on its compositor), and absolute pointing and the native cursor there.

## RealVNC

RealVNC's viewer core is proprietary and nothing of it is in this repository.
Its adapter is the `remotedesktop-realvnc` crate in `backends/modules/realvnc`,
and its library, `libvento_realvnc.so` (the session and the hosting layer that
runs the viewer's Android core in this process), is built only when `clang` is
on `PATH`. The crate opens it as the other backends' are opened (*Backend
libraries* under *Installing*), through the rpath (`lib/` beside the
binary), the first time a session, `--help` or `--list` asks. Without clang
the build says so in a note, the backend is still listed, `--help` shows it
as `not available: libvento_realvnc.so not available (was clang installed
when building?)` and `--list` with that as its `(error: …)`, and a
session on it fails to start with that sentence. Where the library was built
but cannot be loaded, the reason is the loader's, as for the other backends. The next `cargo build` after `PATH` changes
(clang installed, say) looks again, and when the answer differs the C tree is
configured afresh with the other compilers, so nothing has to be cleaned by
hand. To use it, take
`libvncviewer.so` and `libc++_shared.so` for your host's ABI out of the viewer
APK of the build `realvnc/.../Library.java` pins (the session verifies them by
SHA-256 and refuses any other), and put them in `realvnc/` under
`--extlib-dir`, the directory of the extra libraries backends need (each
backend's in a directory named for it):

```sh
mkdir -p ~/.local/share/vento/realvnc
unzip -j com.realvnc.viewer.android_4.9.3.60175.apk \
    'lib/x86_64/libvncviewer.so' 'lib/x86_64/libc++_shared.so' \
    -d ~/.local/share/vento/realvnc
cargo run -- realvnc 127.0.0.1::5901 --password vncpass
```

`--extlib-dir` defaults to `$XDG_DATA_HOME/vento` where
`realvnc/libvncviewer.so` is under it, and otherwise to `lib/` beside the
binary, so a release tree with `lib/realvnc/` works wherever it is unpacked;
`--help` prints the default it resolved. `--state-dir` (where the core keeps
its key, identities and logs — as plain files where the phone has encrypted
ones; see [Known hosts](#known-hosts) — and the client its known hosts)
defaults to `$XDG_STATE_HOME/vento`.

The core's own log comes out on stderr as `HH:MM:SS.mmm I RealVnc: <category>: …` (`W`
for its warnings and errors) whatever `--verbose` says: everything the
phone's core would send to logcat, less three categories' VERBOSE lines,
which the core is told not to write, here or on the phone — `CKeyboard`'s
and `CConn`'s for every key going down and up, which name the key, and
`LineSpeedEstimator`'s for every estimate. What those three say at any other
level still comes through.
