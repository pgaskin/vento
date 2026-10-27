# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone and for the Linux client.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `runtime/` and `api/` hold only what two or more
backends, or both clients, share — `runtime/` what a protocol client is
written against and a platform is written to, its picture and sound
(`runtime::media`) included, and `api/` what a client holds a session by;
and what one platform needs of all of them lives under that platform,
`platform/<os>/` — its crate, its C, its Meson half and, for the phone, its
Java, the generator of the Java each module compiles, and its ports. Inside
a module, each language has one place: the Rust crate at its root, the C in
`cpp/`, the Java in `java/`, the options in `catalog.rs`. What is left at
the top is what builds them all: the Gradle parent, its tasks, the Meson
file that takes the ports and modules in, and the ports.

    backends/
      claude-notes.md, UPGRADING.md
      build.gradle      the :backends Gradle module, parent of the modules
      buildSrc/         the Gradle tasks its helpers use
      meson.build       the C backends and their libraries, in order
                        (the project is the repository root's meson.build)
      ports/            our build files for the vendored trees both
                        platforms compile
      runtime/          what a protocol and a platform are written to,
                        src/media/ too
      api/              what a client holds a session by
      modules/<name>/   one per backend (eight)
      platform/
        android/        crate, codegen/, java/, javatest/, cpp/, meson.build, ports/
        linux/          crate, cpp/, meson.build, ports/, probe/, tests/
      build/            the :backends module's Gradle output, untracked

## The backends: `modules/<name>/`

One directory per backend, each a Gradle module (`:backends:<name>`, remapped
in `settings.gradle`): `rfb`, `libvnc`, `tigervnc`, `freerdp`, `ironrdp`,
`realvnc`, `spice`, `rustdesk`. In each:

- `catalog.rs` — the backend's entry in the catalog: its `About` (name,
  description, place in a picker, how its identities are pinned, whether its
  user name carries a domain) and its options table, with their tests.
  Compiled by `api` by path (`api/src/catalog.rs`), so that
  `vento-codegen` builds from `api` alone; Gradle's `codegenSources`
  lists every one.
- `build.gradle` — the Android module, its source sets pointed at the
  directories below rather than at AGP's `src/main/`, and for a C backend its
  native library (`nativeLibrary` in `:backends`).
- `Cargo.toml`, `src/`, `build.rs`, `examples/` — the backend's crate, at
  the module's root, which holds its adapter to `api`'s `Backend`: the
  protocol itself for the four in Rust (and its `probe` example, behind the
  `probe` feature), or the small adapter over the C library for the other
  four, whose `build.rs` reads the backend's header into Rust with bindgen.
  Every backend's adapter is in its module; no client holds one of its own.
  On Linux the four C backends' crates open their libraries,
  `libvento_<name>.so`, at run time rather than linking them
  (`api::c::library`, over the table of the header's functions bindgen
  generates there), so a library that cannot be loaded is one backend that
  is listed and says it is not available while the client starts with the
  rest; on the phone the C is linked into the module's library. RealVNC's
  crate is the Linux client's alone (the phone's RealVNC backend is its
  Java) and empty on any other target; its library is built only under
  clang.
- `cpp/` (the four C backends) — the C/C++ session over the upstream
  library, its `meson.build` for both platforms, the host probe's `main`
  (`-Dprobes=true`), and `include/vento/<name>.h`, the backend's own C
  API: its config, its constructor and its live calls. RealVNC's also holds
  `host/`, the hosting layer that runs their Android viewer core in a Linux
  process (built only on the host, only with clang).
- `java/` — the backend's provider and whatever else of it the phone needs
  in Java: the `.java` files themselves, with no package directories over
  them (javac goes by each file's `package` line). `resources/` — its
  `META-INF/services` registration.
- `build/` — Gradle's outputs, and a C backend's Meson trees, untracked.

## What they share

- `runtime/` (`remotedesktop-runtime`) — what a protocol client is written
  against and a platform is written to: the `Client` it implements, the
  `Handler` its thread calls and the `Callbacks` a platform answers with, the
  `Runner` that is the session's thread and the `Bridge` between them, the
  address parser, the dial a hang-up interrupts, the line speed the protocols
  that state none measure (`rate`), and the rustls pinning and shared stream
  of the Rust backends that speak TLS (`tls`); and `media`, what a protocol
  hands its picture and sound to: video (the decoders a platform supplies, and
  what is done with their planes) and audio (the outputs a platform supplies,
  their decoders, and the queue a device drains). `media` uses nothing of the
  rest of the crate, nor the rest of it `media`; a platform crate depends on
  `runtime/` for the traits it implements.
- `api/` (`remotedesktop-api`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules both clients keep alike
  (credentials, trust), held to their cases (`tests/vectors.rs`). The
  options' cases are a file beside it, `tests/options.json`, since the one
  rule with a Java copy (`settle`) is held to them by the Java tests in
  `platform/android/javatest/` too; the rest are written in the test. With
  the `c` feature, the one adapter over the C vtable and the shared headers'
  declarations (`api::c::ffi`), which each C backend's own declarations
  are written over. The neutral C API it adapts is in `api/include/vento/`
  (`backend.h`, `video.h`, `audio.h`, `log.h`, `text.h`; each C backend's
  own header is in its module): with the crate that reads them into Rust,
  as each backend's header is with its own.

## The platforms: `platform/<os>/`

- `platform/android/` (`remotedesktop-platform-android`) — the phone's
  platform: the one JNI layer, MediaCodec and AAudio (the crate), and, in
  `java/`, the `:backends` module's Java — `Backend`, `BackendProvider` and
  the types around them, which every backend's provider is written against,
  the files with no package directories over them — with its tests in
  `javatest/`; `codegen/` (`vento-codegen`, a crate of its
  own), the host tool that writes each module's generated Java — its catalog
  and its natives class — at build time, out of `api` and this crate's JNI
  table alone, so that it needs none of the backends' crates, bindgen or
  libclang; `cpp/`, the logcat logger and the export map a C
  backend's library is linked with; `meson.build`, the NDK's flags and how a
  C backend becomes its module's library; and `ports/`, the
  generated Android builds of openssl, nettle and gnutls (`gen.sh` in each
  regenerates its `gen/`).
- `platform/linux/` (`remotedesktop-platform-linux`) — the Linux client's
  platform: FFmpeg, built statically from `third_party/` by `build.rs`, and
  the decoders and the PipeWire/ALSA outputs over it (the crate); `cpp/`, the
  C those decoders call and the stderr logger a C backend links (whose
  `vento_log_line` the client's Rust logger and FFmpeg's log write through
  too, `src/stderr.rs`);
  `meson.build`, how a C backend becomes `libvento_<name>.so` and the FFmpeg
  it links (`ffmpeg_prefix`); `ports/`, nettle and GnuTLS for Linux, built at
  build time by their own autotools build from the tarballs the Android ports
  use (downloaded at configure time when they are not in `third_party/`),
  OpenSSL, built the same way by its own Configure from the submodule the
  Android port uses (`build-upstream.py` runs both), and the RealVNC hosting
  layer's four libraries; `probe/`, the development tools — the C probes'
  shared body and the two sound rigs (`--features probe`); and `tests/`, the
  decoders' fixture tests.

## Building them

- `build.gradle` — the `:backends` Gradle module, whose sources it points
  at `platform/android/java/` and `javatest/`, and the helpers every backend
  module calls (`generateCatalog`, `generateNatives`, `nativeLibrary`,
  `codegenSources`), and the Meson pin (`mesonVersion`, `fetchMeson`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, Meson, codegen,
  tarballs, patch check), pulled in by the root `buildSrc`.
- `crypto-sources.txt` — the version, SHA-256 and mirrors of the nettle and
  GnuTLS release tarballs, read by Gradle's `fetchCryptoSources` and by
  `fetch-crypto-sources.py`, which the Linux Meson build runs; both unpack
  them into `third_party/`.
- `meson.build` and `ports/` — the native build of the C backends, from the
  Meson project at the repository root (`meson.build`, `meson.options`):
  Gradle's `MesonBuildTask` builds each C module's library for Android, and
  `linux/build.rs` every backend's for the Linux client; with
  `-Dprobes=true` it builds the C probes and the tests `meson test` runs.
  UPGRADING.md, "The native build", says what is where.
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace; `linux/` is a
workspace of its own that reaches them by path.
