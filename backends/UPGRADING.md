---
name: upgrade-backends
description: Upgrade the third-party upstreams behind the backends (submodules under third_party/, the nettle and gnutls tarballs, and the generated ports), one at a time, shared libraries first, each as its own reviewed commit. Use when asked to upgrade, bump, or update the backends, a submodule, or a port.
---

# Upgrading the backends

Every backend is a shim of ours over somebody else's client, pinned as a
submodule under `third_party/` (or, for nettle and gnutls, as a release
tarball named in `backends/crypto-sources.txt`). Upgrading one is moving
that pin, carrying our patches across, regenerating whatever is generated,
adapting the shim, and proving the build. This file is the procedure that
came out of the first full round (September 2026), including the things
that went wrong.

Read `patch.sh` and `.gitmodules` first: the gitlink names the *pristine*
upstream commit, our changes live in `patches/<name>/`, and the `base` tag in
each submodule remembers which is which. `git add -A` at the root records the
patched commit as the pin and doubles the patches on the next clone; never do
it.

## Scope

- In scope: the submodules, the two tarballs, the ports' `gen/` output, our
  shims, and any new option an upstream change makes worth offering.
- Out of scope unless asked: the toolchain pins (NDK, Meson, rust channel,
  cargo-ndk, Gradle/AGP, JDK) and the app's own Java/Gradle dependencies.
  Rust crate bumps beyond what a moved submodule's manifests force are also
  out; `cargo update` is its own job.

## Order

Shared libraries first, each separately, then the backends, one at a time.
Nothing runs in parallel: every step needs a clean tree to commit from.

1. libjpeg-turbo (libvnc, tigervnc)
2. pixman (tigervnc)
3. openssl (freerdp), via `backends/platform/android/ports/openssl/gen.sh`
   (the Linux build's `backends/platform/linux/ports/openssl` has nothing to
   regenerate)
4. nettle, then gnutls (libvnc, tigervnc), via the tarball pins and
   `backends/platform/android/ports/{nettle,gnutls}/gen.sh`
5. libvncserver, tigervnc, freerdp, ironrdp, rustdesk with hbb_common, ryll

`third_party/winit` is not a backend's and not in this round: it is the
Linux client's winit, pinned at the tag of the release `linux/Cargo.lock`
names and taken in through `linux/Cargo.toml`'s `[patch.crates-io]`. It moves
when that crate's winit does (a `cargo update` there, its own job): the pin to
the new release's tag, `patches/winit/` rebased onto it, and any patch
upstream has taken dropped.

`third_party/ffmpeg` and `third_party/dav1d` are the Linux client's too, and
not in this round either: the static FFmpeg `backends/platform/linux/build.rs`
builds for every desktop decoder (the Rust ones, which TigerVNC's and
FreeRDP's H.264 and FreeRDP's AAC and Opus sound go through too, and
FreeRDP's scaling, which links libswscale alone), each pinned at a release
tag. FFmpeg moves
to a newer release's tag, unpatched (its configure names libatomic wherever it
is installed, and that build script leaves it out of the link); the decoders
it is configured with are `DECODERS` and `HWACCELS` in that build script, so a
consumer that starts using another codec adds it there. Proven by the desktop
crate's fixture tests (`tests/decode.rs` for the video, `tests/sound.rs` for
AAC and Opus), the RustDesk probe with each codec, the TigerVNC H.264 and
FreeRDP AVC probes, and `scripts/testaudio/winbursts.py` for FreeRDP's AAC.
Android builds neither.

`third_party/libva` goes with FFmpeg: headers only, for its VAAPI decoders
(`HWACCELS` in the same build script). Nothing of it is compiled or linked;
libva is reached through `backends/platform/linux/cpp/va_shim.c`, which
defines each libva function FFmpeg calls and loads the real one on first
use. A newer FFmpeg may call a libva function the shim does not have yet,
which is a link error naming it (`undefined symbol: va...`): add it to
`VA_FUNCTIONS` there. `vento_video.c` calls two of them itself for a held
picture (`vaSyncSurface`, `vaExportSurfaceHandle`) and reads FFmpeg's
`AVVAAPIDeviceContext` for the display, which a new FFmpeg could rename.
libva moves to a newer release's tag when FFmpeg wants newer headers (its
configure says which `VA_CHECK_VERSION` a feature needs). Proven by the
Linux platform crate's `hardware` and `hardware_auto` tests with
`VENTO_TEST_HW=require` on a machine with a VAAPI device, and
`hardware_off`.

A shared library can be blocked by a consumer: nettle 4 could not move until
TigerVNC had upstream's nettle-4 compatibility. When an evaluation says "stay"
because of a consumer, note it and come back after that consumer moves.

## Who does what

For each upstream, one orchestrating agent (the strongest model available)
that spawns two subagents in turn and then reviews their work itself:

1. **Evaluate** (strong model). Reads the real diff between `base` and the
   candidate, not the changelog: every API item, setting and callback our
   shim uses, and every build file our port mirrors (see "The native
   build"), checked against the target; patches
   dry-run against a scratch worktree (`git apply --check`); release vs
   development-tip judged for a phone app; candidate options proposed with
   key, label, summary, type, choices, default, scope and live. Concrete
   findings with file:line references.
2. **Upgrade** (the implementation model). Moves the pin, rebases the
   patches, adapts the shim, adds the agreed options end to end, runs the
   verification list. Does not commit.
3. **Review** (the orchestrator). Independently verifies pins, reads every
   regenerated patch hunk by hunk against the old one, reviews the whole
   diff, reruns the builds. Fixes small things itself; sends larger ones
   back. Does not commit.

The lead reads the report and the diff, then commits. Give every agent a
written conventions file (this one plus the session's specifics: target
candidates already fetched, what is out of scope, where scratch files go)
rather than a prompt from memory; agents get cut off and resumed, and a
file survives that where a transcript does not. Have them save their reports
to the scratchpad as they go, for the same reason.

## Before starting

    for p in third_party/*; do git -C "$p" fetch origin; done
    git -C third_party/<name> ls-remote --tags origin   # releases live here

The submodules are shallow. `git log base..origin/master` is grafted and
lies about counts; `git diff --stat`, `git ls-remote` and
`merge-base --is-ancestor` tell the truth, and `git fetch --unshallow` is
fine when a real log is needed. Tags on release branches (openssl's
`openssl-3.5.*`) are not fetched by default.

Which pin to pick:

- A release tag on the maintained line, if one exists that the consumers can
  use. Check which line is maintained (libjpeg-turbo's `main` became the 3.2
  line while `3.1.x` went to ESR; openssl 3.5 is LTS while 3.6 was a year
  from EOL and 4.0 removes APIs the port still names).
- The development tip only for a concrete reason, and then a cut that
  builds: TigerVNC had no release in six months and master carried the
  nettle-4 compatibility, the leak fix and a features-off configuration
  upstream now builds itself, but only cuts after the gettext work stopped
  needing libintl.
- Stay, honestly, when there is nothing to gain: pixman's master touched
  only SIMD compositing that the region code never reaches; gnutls and
  libvncserver were already at the newest thing that exists.

## Moving a pin

With patches:

    cd third_party/<name>
    git rebase --onto <TARGET> base HEAD
    # re-derive each conflict against the new code; drop a patch upstream
    # made redundant and say which commit did; keep subjects and bodies
    git tag -f base <TARGET>
    cd -
    git update-index --cacheinfo 160000,<TARGET>,third_party/<name>
    ./patch.sh <name> && ./patch.sh check

When upstream has taken every patch, the rebase leaves the submodule at
`<TARGET>`, and `./patch.sh` would apply the old files again (it saves only
when the submodule is ahead of `base`), so delete `patches/<name>/` by hand
before running it.

Without patches: `checkout --detach`, `tag -f base`, `update-index`,
`./patch.sh check`. A submodule with no `base` tag yet gets one at the
target.

A tarball: bump `<name>.version`, `<name>.sha256` and both `<name>.url`
lines in `backends/crypto-sources.txt`, the one place they live (Gradle's
`fetchCryptoSources` and the Linux Meson build both read it), verify the
hash against upstream's signature or announcement (not against the
download), `./gradlew :backends:fetchCryptoSources` (it replaces a tree
whose `.pin` is another hash), then the port's `gen.sh`. gnutls' gen builds
nettle itself, so a nettle bump reruns gnutls' gen too and may change its
`gen/`. The Linux build has no `gen/`: its configure
(`fetch-crypto-sources.py`) replaces a tree whose `.pin` is not the pinned
hash, the same way, and then runs both tarballs' own configure and make at
build time, again for a new `.pin`, so a bump has nothing to regenerate there;
a configure option the new release renames or drops shows up in its
configure log (`backends/platform/linux/ports/<name>/build/configure.log` in
the build tree).

A port's `gen.sh` rerun at the same pin must produce an empty diff. If it
does not, fix the script first as its own commit, regenerated at the old
pin, so the upgrade's diff is only the upgrade. The three things that have
broken this: a build date baked in (pin it to the commit date via
`SOURCE_DATE_EPOCH`), a path recorded relative to a `mktemp` directory (build
in a fixed directory under the port; `build` is gitignored), and `sort`
under the host locale (`export LC_ALL=C`).

## The native build

Every C and C++ library a backend links is built by the Meson project at the
repository root (`meson.build`, `meson.options`) from build files of ours; no
upstream build file is read. `backends/meson.build` takes, for the backends
selected (`-Dbackends=`), the platform's half
(`backends/platform/{android,linux}/meson.build`), the ports, and each
module's `cpp/meson.build`, which makes the module's library. A port says
what upstream's own build would compile for our configuration: the sources,
the definitions and flags, and the answers its configure would write into
its config headers, which are generated from upstream's own templates.

- `backends/ports/<name>`: libjpeg-turbo, pixman, libvncserver, tigervnc
  and freerdp (whose `generate.py` writes the two files upstream's CMake
  writes by running code, `settings_keys.h` and the channel table
  `tables.c`).
- `backends/platform/android/ports/<name>`: OpenSSL, GnuTLS and nettle for
  Android, with configure's output in `gen/` (`gen.sh`).
- `backends/platform/linux/ports/<name>`: OpenSSL, nettle and GnuTLS for
  Linux, built by their own build systems at build time
  (`build-upstream.py`), and the RealVNC hosting layer's four libraries.

Gradle builds each C module's library (`MesonBuildTask`, a tree per variant
and ABI under the module's `build/meson/`) with the Meson `backends/build.gradle`
pins (`mesonVersion`, unpacked into the Gradle user home by `fetchMeson`);
`linux/build.rs` builds every backend for Linux in one tree, with the `meson`
on the path or `MESON`.

Moving a port's pin: a port is what upstream's build files say, so read their
diff between `base` and the target (`git diff base <target> --
'*CMakeLists.txt' '*.cmake' meson.build '*.in'`) and carry each change over: a
source added to or removed from a library we build, a new definition or
flag, a new template variable or `#cmakedefine` (Meson leaves a
`#cmakedefine` it has no answer for undefined, as CMake does an unset
variable, and warns about an unknown `@VAR@`), and the lines the ports read
versions from (by string splitting, which fails the configure when a line
moves). A removed source fails the configure; an added one that something
calls fails the link, and one that nothing calls shows only in the diff.

## Adapting the shim

- A C backend's shim is one file in its module's `cpp/`, `<name>_session.c`
  (`.cxx` for TigerVNC), which drives the library against the neutral C API
  in `backends/api/include/vento/` and names no platform; what it offers
  is declared in the backend's own header beside it,
  `include/vento/<name>.h`. Above it is the crate at the module's root, which
  reads that header into Rust (its `build.rs`, with bindgen), fills the
  backend's `vento_<name>_config` from the options and words its rows; it is what the Linux client holds,
  and on the phone it is linked into the module's library as a static
  library (`nativeLibrary` in `backends/build.gradle`,
  `backends/platform/android/meson.build`) with the one JNI layer in
  `backends/platform/android`. On Linux the crate opens
  `libvento_<name>.so` at run time (`api::c::library`) through the table
  its `build.rs` has bindgen generate there, and calls the header's
  functions with `api::call!`, which is the extern declaration on the phone;
  a function added to `vento/<name>.h` is one more the table requires, so a
  library without it is "not available". RealVNC's crate is the Linux
  client's backend (the phone's RealVNC backend is its Java over the viewer's
  JNI; on the phone the crate is only the session's network, linked into the
  module's native library), and its library is built only under clang. An upstream
  change reaches the session file;
  the crate changes only when the config does, and so does `<name>_probe.c`,
  the host probe's `main` beside them. The module's
  `meson.build` makes its library on both platforms; the vendored tree's
  configuration is its port's (`backends/ports/<name>`), so a new option
  goes there.
- A Rust backend's shim is its protocol crate, and its adapter is that
  crate's `src/backend.rs`: the `Kind`, the config read from the options, and
  the rows. An upstream change (IronRDP, RustDesk, ryll) reaches the
  protocol code; `backend.rs` changes only when an option or a row does.
- Next to nothing about a protocol is in Java or JNI any more. The phone
  reaches every backend but RealVNC through one JNI layer,
  `backends/platform/android`, over the same `api::Backend` the Linux
  client holds; what a module's Java keeps is its provider and, for RDP and
  the two borrowed VNC clients, a `NativeBackend.Hooks`. An upgrade that
  keeps the adapter's `Kind` building has nothing to change on that side.
- TigerVNC's H.264 decoder context is the shim's own on both platforms
  (`H264VentoDecoderContext`, made by the module's own definition of their
  factory, `H264DecoderContext::createContext`, linked into the module's
  library in place of their `H264DecoderContext.cxx`, which the port leaves
  out; their libav one is not built, `HAVE_LIBAV` unset), and
  hands each rectangle to vento's video decoders through `vento/video.h`'s
  table. Its QEMU audio is
  played on vento's sound output through `vento/audio.h`'s table
  (`VentoAudio.h`; AAudio on the phone, ALSA on the host, both behind
  `runtime::media::audio`'s queue), not their viewer's `AudioOutput`, which is
  not compiled. An upstream change to `H264DecoderContext`
  or `H264Decoder` (the reset flags, the 64-context limit) or to the QEMU
  audio calls in `CConnection` reaches both platforms — in particular the
  session's sound switch leans on `CConnection::framebufferUpdateEnd`
  asking for audio (format, then enable) in the update that acknowledged
  it, which the shim's own `framebufferUpdateEnd` follows with a disable
  for a session started without sound, and on `writeQEMUAudioEnable`
  staying public in `CMsgWriter` and `CConnection::writer()` staying
  reachable from the subclass (protected since 93d8d71e).
- Every comment of ours that names an upstream version or behaviour ("in
  3.19.1", "the assembly is GAS", "returns Some") is a claim to re-verify
  against the target and correct or remove. Grep for the old version string
  across `backends/` and `README.md`, excluding `gen/`.
- Defaults that upstream flips reach us silently. FreeRDP turned ExtSecurity
  on and raised its TLS floor; a client that "prefers NLA" or "never NLA" has
  to drive every switch upstream added.
- Reproducibility stamps: a new upstream may add a git-describe version, a
  build timestamp or a host path (`USE_VERSION_FROM_GIT_TAG`,
  `BUILD_TIMESTAMP`). Pin them; `strings` the built `.so` for a date, a
  hostname or a path.
- Warnings in a vendored tree are silenced at the target (`vendored_args` in
  the root `meson.build`), never on ours and never globally.
- The README's list of blob directories under `third_party/`, and the
  licence file paths `app/build.gradle` reads, are both things a moved pin
  can invalidate.
- New libraries an upstream's build starts looking for (JSON parsers, Opus,
  uriparser) are not looked for by the ports, which name every library they
  link, so the result does not depend on what the build machine has
  installed; check that the port's configuration answers say the same.

## Options

A backend's settings are its table, `OPTIONS` in
`backends/modules/<name>/catalog.rs` (compiled by the `api` crate, which
names the file by path), one `Spec` per row, and the one copy: the Linux client builds its dialog and
`-o key=value` from it, and on the phone `vento-codegen` writes it into the
module's generated `<Name>Catalog` at build time, which the provider answers
from. When an upstream change adds something a person would switch (a new
encoding or codec, a security type, a behaviour toggle), add a row in the
shape, wording and scope of its neighbours, read it in the adapter (`o.get`,
`o.bool`, `o.int`) into the config — for a C backend a new field in
`vento_<name>_config`, which the backend crate's `build.rs` reads into
Rust with bindgen — and default it to upstream's answer, and add its key to the
test `the_keys_are_the_ones_records_hold` in the same file, which is there
because a stored key renamed is a setting forgotten. The value reaches a session
in the options it is created with, every key and value as strings, so
nothing in Java or JNI changes for a row. Where the phone's words are wrong
on a desktop ("this phone"), give the row a `desktop_summary` or the choice
a `desktop_label`. Do not add rows for what the app already decides, what nobody would set, or
what cannot be exercised here (an H.264 path with no decoder behind it).

The user-facing strings in this app are hand-written by its owner. A new
row's label and summary are a draft; name them in the report so they get
rewritten.

## Adding a backend

Not part of an upgrade round, but the same layers, so here once. A backend
is a module under `backends/modules/<name>/`, and each layer of it is one
line or one file somewhere that already has seven:

- **The protocol.** A Rust crate at the module's root (`Cargo.toml`, `src/`,
  `examples/`; `crate-type = ["lib", "cdylib"]`) written against
  `remotedesktop-runtime` (its `media` too, where it has video or sound to
  hand on), or a C shim in `cpp/` over
  `vento/backend.h` with its config in its own header,
  `cpp/include/vento/<name>.h` (on the include path of the module's targets
  through its `meson.build`), a host probe
  `<name>_probe.c` (built with `-Dprobes=true`), and a small crate at the
  module's root (`remotedesktop-api` with the `c` feature;
  `api::c::start` and a `Behaviour` do the rest) whose `build.rs` reads
  the header with bindgen, its own items only, over `api::c::ffi` —
  libvnc's, copied.
- **The catalog.** `modules/<name>/catalog.rs`: the backend's `About` (its
  pinning and whether its user name carries a domain are data there, which
  the shared rules read) and its `OPTIONS` table, with their tests; one
  `#[path]` module, a `pub use` and a place in `ALL` in
  `backends/api/src/catalog.rs`. The file is in `codegenSources`
  (`backends/build.gradle`) by being there.
- **The adapter.** In the backend's own crate a `Kind` (`api::kind`):
  the `About`, the version, and `create`, which turns a `Connect` into an
  `Arc<dyn Backend>`. Credentials prompts, the trust decision
  (`api::trust::decide` over each client's `Store`), the bell and the
  events are the `api` crate's and are not written again.
- **The workspace.** The crate's directory in the root `Cargo.toml`'s
  `members` and `default-members`, which name each one.
- **The Linux client.** The crate in `linux/Cargo.toml` and its `KIND` in
  `linux/src/backends/mod.rs`'s `all()`; a C module also goes into the
  `backends` option (`meson.options`) and `backends/meson.build`, which
  `linux/build.rs` builds, and into the lists of directories and libraries
  there that it watches and puts in `lib/`.
- **The phone's library.** One `platform_android::natives!` line in the
  crate's `lib.rs` under `cfg(target_os = "android")`, naming the module's
  natives class and the `Kind` (`decoders =` for a backend whose picture is
  video, `on_load =` for a linked library that wanted a `JNI_OnLoad` of its
  own), with `remotedesktop-platform-android` as an Android-only dependency.
  A Rust module's `build.gradle` has cargo-ndk build the cdylib into its
  `jniLibs` (rfb's, copied); a C module's calls `nativeLibrary` from
  `:backends` with its backend's and crate's names, which builds the crate as
  a static library and the module's library with Meson, and its `meson.build`
  links the crate (`crate_library_dir`) as libvnc's does.
- **The phone's Java.** The module's `build.gradle` registers
  `generateCatalog` and `generateNatives` from `:backends`; its
  `<Name>Provider` answers from `<Name>Catalog` and creates a
  `NativeBackend` on `new <Name>Natives()`, and is declared in
  `resources/META-INF/services/`, as a `BackendProvider`; the module's
  `sourceSets` point AGP at `java/` (the files, no package directories)
  and `resources/` (rfb's, copied).
  What the protocol needs of the phone that the layer does not do goes in
  a `NativeBackend.Hooks` (`RdpHooks` remembers RDP's desktop sizes and words
  the prompt's hint, `VncHooks` words an anonymous TLS question); most
  backends need none. `settings.gradle` includes the module, and the app
  takes it from `backendModules` in `app/build.gradle` — or an add-on under
  `plugins/` does, as RustDesk's.
- **The checks.** A line in `scripts/check-jni-symbols.sh`'s `MODULES`, and
  the probe (below) against a test desktop.

## Verification

Native modules:

    ./gradlew :backends:<module>:assembleRelease
    ./gradlew :backends:<module>:assembleRelease -PstrictWarnings

The C backends again on the host, which is the one build that then runs — a
probe per backend, against the test desktops under `scripts/` (README,
Development). The three open ones build against the system's zlib, and
the phone's other libraries (GnuTLS and nettle from the same tarballs and
OpenSSL from the same submodule, by their own builds rather than the
ports'); the
RealVNC one builds under clang only and runs their pinned library out of
the viewer APK, so a change to `realvnc/.../Library.java`'s pins, to
`realvnc_traffic.h`'s walk or to the four host-only submodules is proven
here and nowhere else:

    CC=clang CXX=clang++ meson setup build/scratch/host -Dstrict_warnings=true -Dprobes=true
    meson compile -C build/scratch/host
    meson test -C build/scratch/host

Rust modules:

    cargo check --locked --workspace
    cargo test --locked -p <crate>
    cargo clippy --locked --workspace
    ./gradlew :backends:<module>:assembleRelease

Every backend through `api::Backend`, on the host: the Linux client (`linux/`,
its own workspace; see its claude-notes.md) builds all eight (RealVNC's library
under clang only; without it the backend is listed as not available), and its headless probe connects one, writes the picture out and
exits — the run that goes through the same adapters the phone does, RealVNC's
aside:

    (cd linux && cargo build && cargo test && cargo clippy --all-targets)
    linux/target/debug/vento <backend> <address> --password ... --trust \
        --probe wait=3,out=build/scratch/<backend>.ppm

`linux/build.rs` does not watch the submodules, so after a pin moves its
`cargo build` keeps the C libraries it already built: `touch meson.build`
first, and check that the library under `linux/target/debug/lib/` names the
new version.

Always `./patch.sh check`. After an options change, `cargo test --locked -p
remotedesktop-api` and `./gradlew :backends:testDebugUnitTest`: the
first checks every table's defaults against its own rows and the keys
records hold, and both hold `settle`, and its Java copy, to
`backends/api/tests/options.json`. After
any change to a native surface — `platform_android`'s `surface.rs`,
`NativeBackend`, or a module's natives class or library —
`scripts/check-jni-symbols.sh`. At the end of the round: `./gradlew :app:assembleDebug
:plugins:realvnc:assembleDebug :plugins:rustdesk:assembleDebug`.

Nothing in the Gradle or cargo steps runs on a device or against a real
server. The probes are the exception: they run against the Docker desktops
under `scripts/` (`testvnc`, `testrdp`, `testspice`, `testrustdesk`;
`testh264` for TigerVNC's H.264, which needs a Wayland session and a GPU to
nest in). Say in the report which were run and which were not, rather than
implying otherwise; the other harnesses under `scripts/` need VMs.

Build in the repository's own `target/` and the modules' `build/`. `/tmp` is a RAM-backed
tmpfs: no cargo target directories, scratch workspaces, unpacks or clones
there, including in the agent scratchpad. A scratch build goes under
`build/scratch/` (gitignored) and is deleted afterwards.

## Committing

One commit per upstream, after review, by the lead:

- Subject `<name>: update to <version>` for a release, or `<name>: update to
  owner/repo@<full sha>` for a commit; a body that says what upstream changed
  that reached us, what the shim did about it, and what happened to each
  patch.
- Upstream commits as `owner/repo@<full sha>` wherever a GitHub repository
  backs the submodule, in subjects and bodies alike, substituted in place
  without rewrapping the text around them.
- Signed (`git commit -S`). Author is the model that led the work, committer
  is the repository owner, with the implementing model as `Co-Authored-By`.
  Preserve author and commit dates if rewriting.
- `git add` the named paths, never `-A`; the gitlink is already staged by
  `update-index`.

## What each upstream is, as of the September 2026 round

- **libjpeg-turbo** 3.2.0. No patches.
  `main` is the 3.2 line; `3.1.x` is ESR. arm64 SIMD is Neon intrinsics;
  x86 SIMD is upstream's NASM assembly on Linux where NASM is installed, and
  off on Android, so that an APK does not depend on the build machine.
- **pixman** 0.46.4. No patches. Hand-written source list in
  `backends/ports/pixman/meson.build`; only the region API is reached. The checkout's origin is freedesktop's gitlab while
  `.gitmodules` names the X11Libre mirror; they match.
- **openssl** 3.5.9, LTS to 2030. No patches. Both builds configure
  `no-quic` and `no-dtls`: FreeRDP is a TLS client and makes no QUIC or
  DTLS connection. Without QUIC `quic.h` declares no
  `OSSL_QUIC_client_method`, so a consumer that starts using QUIC fails to
  compile; `ssl/quic/quic_tls*.c` stay in the source list (the API for an
  external QUIC stack). QUIC's qlog was the one caller of `OpenSSL_version`
  (FreeRDP calls none), so the linked libraries no longer hold "OpenSSL 3.5.9 ..." or the build date; the
  version they carry is the default provider's bare `3.5.9` (`strings
  <lib> | grep -x`). Moving to 4.x needs `no-ech`,
  and the engine references in `gen.sh` and the port's `meson.build` go. The
  Linux build (`platform/linux/ports/openssl`) runs Configure with the
  port's options without `no-legacy`, the assembly on as there, plus
  `no-autoload-config` and OPENSSLDIR `/etc/ssl`, in a directory named after
  the version, so a bump is a fresh build there; an option the new release
  renames or drops shows up in its configure log (`configure.log` in that
  directory of the build tree).
- **nettle** 4.0, **gnutls** 3.8.13 (the release that added nettle-4
  support; newest in the 3.8 line). nettle 4.1 warns of further API changes.
  `MINI_GMP_ENABLE_FLOAT=0` is an answer configure puts in CFLAGS, so the
  port sets it itself. The Linux build (`platform/linux/ports/{nettle,gnutls}`)
  configures the same tarballs with the port's options but for the
  assembly, which it leaves on, a CA bundle path and a system priority
  file path.
- **libvncserver** at upstream master; nine patches. No releases since
  0.9.15. VeNCrypt's top-level Plain (credentials without TLS), which the
  library accepts, is refused by the shim's `onGetCredential` before the
  credentials are sent. `0005` (as TigerVNC's `0005`)
  hands a certificate that verified against the system's authorities, and is
  for the name dialled, to
  `GetX509CertFingerprintMismatchDecision` too when it is installed, and
  `0006` one that verified for another name, which upstream refuses outright
  (TigerVNC's viewer asks about it, and so does that backend): so the shim's
  `trust_needed` and the known hosts see every certificate. Should upstream
  ever take them or change `verify_certificate_callback`, check that a
  CA-signed certificate, for the name or not, still reaches the hook.
  `0007` calls `GotCursorShape` with a 0x0 shape for a cursor update of no
  pixels, the server hiding the pointer, which upstream returns from
  before the callback. `0008` waits for the socket between the TLS
  handshake's retries, at most 15 s each, where it retried at once and
  logged each time: `0002` took gnutls' own deadline away, and with it the
  only wait on the non-blocking socket. `0009` waits for sockets with
  `poll()` (a new `sock_wait()` in `common/sockets.c`) where `select()`
  overflowed its `fd_set` for a socket at descriptor 1024 or above, and makes
  `WriteToTLS` wait for room where it retried at once; Windows keeps
  `select()`. Upstream's own attempt (993df689) was reverted in 7dd27504
  because its connect wait used `POLLIN`, which broke a server's reverse
  connection to a repeater; `0009` waits for `POLLOUT`. Should upstream take
  a `poll()` change of its own, check the connect wait's direction.
- **tigervnc** master ed951a41, reporting 1.16.80; six patches (the PAM one
  dropped, the libav context's pixel format fix with that context, and the
  H.264 context's factory replaced from the module). The
  client API is upstream's after their October 2026 cleanup: `writer()` is
  protected, so pointer events and layouts go through `sendPointerEvent` and
  `requestScreenLayout`, and `getUserPasswd` has no `secure`. Their `setPF`
  ignores every change for a server older than RFB 3.8, and before the
  handshake the version reads as 0.0, so the shim asks for a format only
  once the connection is up, and against an older server reports the
  server's own. The certificate check is the shim's now: what cannot be excused is
  refused and the rest goes to `trust_needed`, their viewer's known-hosts store
  not used at all; the RSA-AES key question is ported from
  `vncviewer/CConn.cxx`; audio is offered wherever there is an output, the
  `Audio` row is where the session's Enable switch starts, and it plays on
  vento's sound output through `vento/audio.h`'s table (`VentoAudio.h`).
  `0006` checks the socket with `poll()` in `FdInStream`/`FdOutStream` where
  `select()` overflowed its `fd_set` at descriptor 1024 or above (Windows
  keeps `select()`); the shim's loop also polls for `POLLOUT` while the
  socket's stream holds unsent output.
  Builds `common/` only, with `config.h` written by its port.
- **freerdp** 3.32.2; two patches. `0001`, codec/h264: `WITH_H264_EXTERNAL`,
  a slot for an application's subsystem, `g_Subsystem_external`, tried
  first, and `WITH_GFX_H264` following it. `0002`, core/connection: with a
  `TCPConnect` other than the library's own (the module's, which dials
  through the session's network), a server redirection's target names are
  not looked up with the local resolver before the reconnect; the first in
  the preferred order is taken as it is, and the module's connect falls
  back to the target's net addresses when the network cannot resolve it.
  Check that `rdp_client_redirect` still chooses the target in
  `connection.c` and nothing else in a client session resolves a name
  outside `TCPConnect` (the RPC gateway's lookup is unreachable while the
  gateway is never enabled). An earlier patch, upstream's
  FreeRDP/FreeRDP@190167212b9af27d375b09e75a8e3e41aa7d0284 cherry-picked
  for 3.32.0's channel reassembly (a static channel's message of more than
  one chunk whose length is not a multiple of 128, such as a far-end
  clipboard past 1600 bytes, was a "read error" that ended the session),
  was dropped at 3.32.2; 3.32.1 is the first release with it. A far-end
  clipboard past 1600 bytes is still the check: `xclip` in the testrdp
  container sets one.
  The module's `h264_vento.c` is built against `libfreerdp/codec/h264.h`,
  which is internal: check that
  `H264_CONTEXT_SUBSYSTEM` and the context's `pYUVData`, `iStride`,
  `YUVWidth`, `YUVHeight` and `pSystemData` still mean the same, that
  contexts are still made, reset and fed inside the GDI's SurfaceCommand
  (the session sets whose they are around it), and that AVC444 still feeds
  both pictures through the surface's one context. Since 3.32.1 a
  subsystem's 0 from Decompress means a frame without a picture, which the
  library skips; ours returns 1 or below zero. Also since 3.32.1,
  `h264_context_set_option` with `H264_CONTEXT_OPTION_HW_ACCEL` re-creates
  the subsystem (Uninit, then Init), and since 3.32.2 only when the value
  changes; the GDI sets it inside the surface command to whether
  `SoftwareGdi` is off, and the session keeps `SoftwareGdi` on, so the
  value stays the context's default and nothing is re-created. No
  `WITH_VIDEO_FFMPEG`,
  `WITH_FFMPEG_HWACCEL` or `WITH_MEDIACODEC`. Its sound plays on vento's
  output through the module's rdpsnd device (`rdpsnd_vento.c`), found with
  no patch through the public `freerdp_register_addin_provider` (registered
  after every `freerdp_client_context_new`, which puts the library's own
  back) and named "sys:vento" in both rdpsnd channels' arguments: check that
  the provider is still consulted for an rdpsnd device by subsystem name,
  that `rdpsndDevicePlugin` (FormatSupported, Open, PlayEx returning the
  latency, SetVolume, ServerFormatAnnounce) still means the same, that the
  channel still offers the server what its DSP decodes *or* the device
  plays, and hands a wave of a format the device claims to PlayEx as it
  came (the device claims AAC, `WAVE_FORMAT_AAC_MS`, and Opus where vento's
  decoders take them, and decodes them through `vento/audio.h`), and that
  the Close PDU still does not reach the device (it closes its own stream
  when idle). No `WITH_PULSE`, `WITH_ALSA`, `WITH_OSS`, `WITH_SNDIO`,
  `WITH_OPENSLES`, `WITH_AAUDIO` (3.32.1's, on by default on Android when
  the NDK has libaaudio) or `WITH_DSP_FFMPEG`; their "fake" device is still built
  and is what Sound = off gets. On Linux `WITH_SWSCALE` is the one FFmpeg
  switch on, and `WITH_FFMPEG` is off so that their swscale branch does not
  link libavcodec for the camera channel's MJPEG; check that scaling still
  needs only libswscale and libavutil, and that rdpgfx still offers 10.5
  and 10.6 only with a scaler (the session is EGFX 10.4 without). Also on
  Linux, `WITH_UNICODE_BUILTIN`: WinPR's own UTF-8/UTF-16 conversion
  (`winpr/libwinpr/crt/unicode_builtin.c`) in place of ICU's, so the
  library links no ICU; check that the option still exists under that name
  and is still the only thing in their Linux build that looks for ICU
  (`WITH_TIMEZONE_ICU` stays off by default), and that `readelf -d
  libvento_freerdp.so` names no `libicu*`. Android keeps
  `unicode_android.c`, Java's String.
  `LegacyTls` row for TLS 1.0 at level 0; `vendored_args` silences their
  tree.
- **ironrdp** e899a826 (October 2026), the commit tagged
  ironrdp-session-v0.12.0, ironrdp-connector-v0.11.0,
  ironrdp-blocking-v0.11.0 and the other crates' releases. One patch,
  `patches/ironrdp/0001`, egfx: a per-surface H.264 decoder interface
  (`decode_for_surface`, `decode_yuv420_for_surface`, `delete_surface` on
  `H264Decoder`, defaulting to the surface-agnostic methods), because each
  EGFX surface is its own AVC stream and a decoder that keeps per-surface
  state was fed every surface's frames. Check it still applies if the egfx
  crate's decode paths move.
  IronRDP versions each crate separately and tags a set of releases together
  on one `chore(release)` commit; that commit is the pin to take. Every
  crate's minor version moves in the same release, so expect API changes;
  the protocol crate's compile errors list them. The bitmap stride fix
  (80bb81b3), unreleased at the previous pin, is in ironrdp-session 0.12.0.
  A server redirection, at connection or mid-session, ends the session with
  a message rather than being followed. EGFX is the Graphics option's third
  choice, `egfx` (off by default, experimental): the graphics pipeline over
  a dynamic channel, with its H.264 (AVC420/AVC444) decoded by the app's own
  video decoders (`modules/ironrdp/src/pipeline.rs`, one platform decoder per
  surface, the same `vento/video.h` path FreeRDP's H.264 uses). Check
  `ironrdp-egfx`'s `GraphicsPipelineClient`, `GraphicsPipelineHandler` and
  `H264Decoder`/`Yuv420Frame` still mean the same. The pipeline acknowledges
  frames eagerly, so a focused idle session to Windows is sent frames
  continuously; it is fine on a GPU but backs up a software renderer.
  sspi, its NTLM and CredSSP, comes from crates.io at the release
  ironrdp-connector requires (0.23.0 for 0.11), and moves in both lock files
  with it. It needs to be 0.22.0 or later: from that release a down-level
  logon name's account name may contain `@`
  (Devolutions/sspi-rs@4878c50503b204bba95def66ca7d45e6d396f898), so
  `MicrosoftAccount\user@example.com` reaches NTLM as that user and domain.
- **rustdesk** 1.5.0 (fada664), and **hbb_common** 229b904 with it. No
  patches. Only two `.proto` files are consumed
  (`modules/rustdesk/build.rs`), none of either repository's Rust:
  `message.proto` from rustdesk's `libs/base/protos/` (it moved there from
  hbb_common in rustdesk b50fde6) and `rendezvous.proto`, with `IdPk` in it,
  from hbb_common. hbb_common is pinned at the commit the rustdesk pin names
  as its own `libs/hbb_common` (`git -C third_party/rustdesk ls-tree HEAD
  libs/hbb_common`), so the two files are the pair that release was built
  with; both are `package hbb`, and a message defined in both fails the
  build as a duplicate. rustdesk moves to a release tag; its own submodule
  is not initialised. Key exchange version 1, a key per direction, is
  1.5.0's: the peer advertises `IdPk.kx_version` in its signed id, the
  controller picks `min(advertised, 1)` in `PublicKey.kx_version`, and
  `crypto::session_keys` derives the keys as hbb_common's
  `Encrypt::new_split` does, checked against that commit's
  `test_wire_vectors`. A peer that advertises nothing (every release before
  1.5.0) runs version 0, one key for both directions, as upstream's
  controller does with it. A new version in `tcp.rs`'s `KX_VERSION_LATEST`
  or a change to `new_split` or its vectors is the thing to re-check on a
  move; the probe's log names the version each session ran. The rendezvous
  server's half of the exchange (`KeyExchange.version`, `signed_params`)
  is not answered, like upstream's client without a token or a WebRTC
  offer. The licence entries in `plugins/rustdesk/build.gradle` read
  rustdesk's `LICENCE`; hbb_common has none of its own. The client reports
  `VERSION` (`modules/rustdesk/src/client.rs`), which the peer compares
  with the versions that introduced its features (`lr.version` in
  `src/server/connection.rs`, the `is_support_*` functions in
  `src/common.rs`): raise it with the pin once every gate up to the new
  version is checked. Cursor ids are a hash of the shape from 1.5.0 and the
  platform's handle before it; the client files shapes by content either way.
- **ryll** v0.3.0 (f12d9b2); two patches, `0001` (the rustls provider and
  the public root store are the embedder's) and `0002` (openh264 behind a
  feature). The two LZ4 patches were dropped as upstream's df2f06b, which
  reads the bitmap format as SpiceBitmapFmt and decodes the blocks in order,
  each with the output before it as its dictionary. That decoder takes
  exactly the image's `BinaryData` bytes and refuses anything after them, so
  `images.rs` bounds them by the size prefix. develop's `h264-decode` feature
  (6c719a70) does what `0002` does, and `0002` goes at the next release.
  Only the protocol and compression crates are consumed; `client` is
  deliberately not (bundled root store).
- **logger**, **libc-shim**, **mcpelauncher-linker** (branch `ng`, with its
  nested `bionic` and `core`) and **libjnivm** (October 2026): the host-only
  stack under `backends/modules/realvnc/cpp/host`, pinned at the commits
  that stack was proven with and in no APK. No patches. Moving any of them
  is proven by the RealVNC probe alone; the libc-shim gaps the hosting
  layer closes from glibc (`Loader.cxx`) are the things to re-check.
