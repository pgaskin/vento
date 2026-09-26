---
name: upgrade-backends
description: Upgrade the third-party upstreams behind the backends (submodules under third_party/, the nettle and gnutls tarballs, and the generated ports), one at a time, shared libraries first, each as its own reviewed commit. Use when asked to upgrade, bump, or update the backends, a submodule, or a port.
---

# Upgrading the backends

Every backend is a shim of ours over somebody else's client, pinned as a
submodule under `third_party/` (or, for nettle and gnutls, as a release
tarball named in `backends/build.gradle`). Upgrading one is moving that pin,
carrying our patches across, regenerating whatever is generated, adapting the
shim, and proving the build. This file is the procedure that came out of the
first full round (September 2026), including the things that went wrong.

Read `patch.sh` and `.gitmodules` first: the gitlink names the *pristine*
upstream commit, our changes live in `patches/<name>/`, and the `base` tag in
each submodule remembers which is which. `git add -A` at the root records the
patched commit as the pin and doubles the patches on the next clone; never do
it.

## Scope

- In scope: the submodules, the two tarballs, the ports' `gen/` output, our
  shims, and any new option an upstream change makes worth offering.
- Out of scope unless asked: the toolchain pins (NDK, cmake, rust channel,
  cargo-ndk, Gradle/AGP, JDK) and the app's own Java/Gradle dependencies.
  Rust crate bumps beyond what a moved submodule's manifests force are also
  out; `cargo update` is its own job.

## Order

Shared libraries first, each separately, then the backends, one at a time.
Nothing runs in parallel: every step needs a clean tree to commit from.

1. libjpeg-turbo (libvnc, tigervnc)
2. pixman (tigervnc)
3. openssl (freerdp), via `backends/ports/openssl/gen.sh`
4. nettle, then gnutls (libvnc, tigervnc), via the tarball pins and
   `backends/ports/{nettle,gnutls}/gen.sh`
5. libvncserver, tigervnc, freerdp, ironrdp, hbb_common, ryll

A shared library can be blocked by a consumer: nettle 4 could not move until
TigerVNC had upstream's nettle-4 compatibility. When an evaluation says "stay"
because of a consumer, note it and come back after that consumer moves.

## Who does what

For each upstream, one orchestrating agent (the strongest model available)
that spawns two subagents in turn and then reviews their work itself:

1. **Evaluate** (strong model). Reads the real diff between `base` and the
   candidate, not the changelog: every API item, setting, callback, CMake
   option and target our shim uses, checked against the target; patches
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

Without patches: `checkout --detach`, `tag -f base`, `update-index`,
`./patch.sh check`. A submodule with no `base` tag yet gets one at the
target.

A tarball: bump version, sha256 and both URLs in `backends/build.gradle`,
verify the hash against upstream's signature or announcement (not against
the download), `rm -rf third_party/<name>`, `./gradlew
:backends:fetchCryptoSources`, then the port's `gen.sh`. gnutls' gen builds
nettle itself, so a nettle bump reruns gnutls' gen too and may change its
`gen/`.

A port's `gen.sh` rerun at the same pin must produce an empty diff. If it
does not, fix the script first as its own commit, regenerated at the old
pin, so the upgrade's diff is only the upgrade. The three things that have
broken this: a build date baked in (pin it to the commit date via
`SOURCE_DATE_EPOCH`), a path recorded relative to a `mktemp` directory (build
in a fixed directory under the port; `build` is gitignored), and `sort`
under the host locale (`export LC_ALL=C`).

## Adapting the shim

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
- Warnings in a vendored tree are silenced at the target
  (`backends/cmake/warnings.cmake`), never on ours and never globally.
- The README's list of blob directories under `third_party/`, and the
  licence file paths `app/build.gradle` reads, are both things a moved pin
  can invalidate.
- New host tools an upstream starts looking for (JSON parsers, Opus,
  uriparser) are switched off by name, so the configure result does not
  depend on what the build machine has installed.

## Options

A backend advertises its settings as `BackendOption`s in its
`*Provider.java`. When an upstream change adds something a person would
switch (a new encoding or codec, a security type, a behaviour toggle), add a
row in the shape, wording and scope of its neighbours, plumb it through Java,
JNI and the client, and default it to upstream's answer. Do not add rows for
what the app already decides, what nobody would set, or what cannot be
exercised here (an H.264 path with no decoder behind it).

The user-facing strings in this app are hand-written by its owner. A new
row's label and summary are a draft; name them in the report so they get
rewritten.

## Verification

Native modules:

    ./gradlew :backends:<module>:assembleRelease
    ./gradlew :backends:<module>:assembleRelease -PstrictWarnings

Rust modules:

    cargo check --locked --workspace
    cargo test --locked -p <crate>
    cargo clippy --locked --workspace
    ./gradlew :backends:<module>:assembleRelease

Always: `./patch.sh check`, and `scripts/check-jni-symbols.sh` after any
change to a native surface. At the end of the round:
`./gradlew :app:assembleDebug :plugins:realvnc:assembleDebug
:plugins:rustdesk:assembleDebug`.

Nothing here runs on a device or against a real server. Say so in the
report rather than implying otherwise; the test harnesses under `scripts/`
need VMs.

Build in the repository's own `target/` and `.cxx`. `/tmp` is a RAM-backed
tmpfs: no cargo target directories, scratch workspaces, unpacks or clones
there, including in the agent scratchpad. A scratch build goes under
`build/scratch/` (gitignored) and is deleted afterwards.

## Committing

One commit per upstream, after review, by the lead:

- Subject `Update <name> to <version>` for a release, or `Update <name> to
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

- **libjpeg-turbo** 3.2.0. One patch (add_subdirectory from downstream).
  `main` is the 3.2 line; `3.1.x` is ESR. arm64 SIMD is Neon intrinsics;
  x86 SIMD stays off (needs NASM).
- **pixman** 0.46.4. No patches. Hand-written source list in
  `backends/tigervnc/src/main/cpp/cmake/pixman.cmake`; only the region API
  is reached. The checkout's origin is freedesktop's gitlab while
  `.gitmodules` names the X11Libre mirror; they match.
- **openssl** 3.5.8, LTS to 2030. No patches. Moving to 4.x needs `no-ech`,
  and the engine references in `gen.sh` and the port's CMakeLists go.
- **nettle** 4.0, **gnutls** 3.8.13 (the release that added nettle-4
  support; newest in the 3.8 line). nettle 4.1 warns of further API changes.
  `MINI_GMP_ENABLE_FLOAT=0` is an answer configure puts in CFLAGS, so the
  port sets it itself.
- **libvncserver** at upstream master; five patches. No releases since
  0.9.15.
- **tigervnc** master dd416cbf, reporting 1.16.80; five patches (the PAM one
  dropped). The certificate check and known-hosts store are the shim's now,
  ported from `vncviewer/CConn.cxx`; audio is an AAudio sink behind the
  `Audio` row. Builds `common/` only, with `config.h` written by our CMake
  and an empty `pthread` target for the link.
- **freerdp** 3.32.0. No patches. `LegacyTls` row for TLS 1.0 at level 0;
  `remotedesktop_vendored_warnings` silences their tree.
- **ironrdp** master 9b151c4c. No patches. A submodule rather than crates.io
  because the bitmap stride fix (80bb81b3) is still unreleased; revisit at
  the next ironrdp-session release. EGFX is deliberately off; the provider's
  javadoc says why.
- **hbb_common** 3d6fb2c, and terminal there: the next upstream commit moved
  `message.proto` into rustdesk's own repository
  (`libs/base/protos/message.proto`). Moving further means a second source
  for that file (a rustdesk submodule, or vendoring the one file with its
  commit recorded) and `crypto.rs` importing `IdPk` from the rendezvous
  module. The kx-v2 key exchange that followed is not needed: this client
  never answers the exchange, like theirs without a token.
- **ryll** develop 3ba0f24; four patches, none merged upstream. No tag since
  v0.1.7. Only the protocol and compression crates are consumed; `client` is
  deliberately not (bundled root store).
