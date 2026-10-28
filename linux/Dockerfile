# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Linux client's release build. It runs the whole build in a container
# and outputs only the installed tree: the `vento` binary and, beside it,
# `lib/` with the backends' `libvento_*.so` (the binary's rpath is
# `$ORIGIN/lib`). The tree can be moved anywhere as a whole: /opt/vento, a
# home directory, a USB stick. From the repository root, with the submodules
# initialised (`git submodule update --init --recursive`):
#
#   docker build -f linux/Dockerfile --output type=local,dest=build/dist/vento .
#
# The build copies the checkout and patches the copy: it applies `patches/`
# to submodules at their pinned commits and accepts submodules `./patch.sh`
# has already patched. It writes nothing to the checkout.
# `linux/Dockerfile.dockerignore` limits the context to the sources the build
# reads. GnuTLS and nettle are not in it: the Meson build downloads their
# release tarballs and checks them against the SHA-256 hashes in
# `backends/crypto-sources.txt`, as on a machine without them, so the result
# does not depend on whatever is unpacked in the checkout.
#
# The binary needs the glibc symbol versions it was linked against, so it runs
# on the build's glibc and later ones. The build therefore runs on Ubuntu
# 18.04 (glibc 2.27), with a newer clang, Meson and NASM fetched at pinned
# versions and checked. This baseline lives here and not in the build
# scripts, which build against the machine's own libraries. The C++ runtime is
# GCC 8's libstdc++, the one 18.04 ships.
#
# At run time the tree needs:
# - Linux on x86_64 with glibc 2.27 or later, libstdc++ (GLIBCXX_3.4.22) and
#   zlib (libz.so.1);
# - for the window, a Vulkan or GL driver and a Wayland or X11 session;
# - for RealVNC, the two libraries from RealVNC's viewer, in the tree's
#   lib/realvnc/ or in $XDG_DATA_HOME/vento/realvnc/ (linux/claude-notes.md
#   says how to extract them from its APK).
# Sound (PipeWire, ALSA) and hardware decoding (libva) are loaded at run time
# when installed. libjpeg-turbo, pixman, GnuTLS, nettle, OpenSSL, libatomic,
# FFmpeg and dav1d are linked in.

FROM ubuntu:18.04 AS base
SHELL ["/bin/bash", "-eo", "pipefail", "-c"]
ENV DEBIAN_FRONTEND=noninteractive

# From Ubuntu: GCC 8 for its libstdc++ headers (clang uses the newest GCC
# installed), make, m4 for nettle's assembly, perl for OpenSSL's Configure
# (the base image has only perl-base, without the modules it needs),
# pkg-config, Python 3.8 for Meson, Ninja 1.8.2, git for `git apply`, and
# zlib's development files.
RUN apt-get update && apt-get install -y --no-install-recommends \
 ca-certificates gnupg xz-utils binutils make m4 perl g++-8 pkg-config ninja-build python3.8 git zlib1g-dev \
 && rm -rf /var/lib/apt/lists/*

# Newer build tools than 18.04 ships:
# - clang and LLVM's binutils from apt.llvm.org, the key checked by its
#   fingerprint;
# - NASM 2.16 (dav1d needs 2.14; 18.04 has 2.13), built from source;
# - Meson 1.11.2 (18.04 has 0.45), the version `mesonVersion` in
#   backends/build.gradle pins, run from its release tarball with Python 3.8;
# - rustup, which installs the channel in rust-toolchain.toml.
ARG LLVM_VERSION=18
ARG NASM_VERSION=2.16.03
ARG MESON_VERSION=1.11.2
ARG RUSTUP_VERSION=1.28.2

ARG LLVM_PGP_FINGERPRINT=6084F3CF814B57C1CF12EFD515CF4D18AF4F7421
ARG NASM_CHECKSUM=sha256:1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148
ARG MESON_CHECKSUM=sha256:698feae069cef3ecd4d7aaf281d7df359bdfcf555a9a1564383d3b913fa8a736
ARG RUSTUP_CHECKSUM=sha256:20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c

ARG LLVM_PGP_URL=https://apt.llvm.org/llvm-snapshot.gpg.key
ARG NASM_URL=https://www.nasm.us/pub/nasm/releasebuilds/${NASM_VERSION}/nasm-${NASM_VERSION}.tar.xz
ARG MESON_URL=https://github.com/mesonbuild/meson/releases/download/${MESON_VERSION}/meson-${MESON_VERSION}.tar.gz
ARG RUSTUP_URL=https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/x86_64-unknown-linux-gnu/rustup-init

FROM base AS nasm
ADD --checksum=${NASM_CHECKSUM} ${NASM_URL} ./nasm.tar.xz
RUN mkdir /nasm && tar -C /nasm --strip-components=1 -xf nasm.tar.xz
RUN cd /nasm && CC=gcc-8 ./configure --prefix=/opt/nasm > /dev/null && make -j"$(nproc)" > /dev/null && make install > /dev/null

FROM base AS meson
ADD --checksum=${MESON_CHECKSUM} ${MESON_URL} ./meson.tar.gz
RUN mkdir /opt/meson && tar -C /opt/meson --strip-components=1 -xf meson.tar.gz
RUN mkdir /opt/meson/bin && printf '#!/bin/sh\nexec python3.8 /opt/meson/meson.py "$@"\n' > /opt/meson/bin/meson && chmod +x /opt/meson/bin/meson

# The minimal profile. rust-toolchain.toml also lists Android targets and
# lint components for the phone's build; RUSTUP_TOOLCHAIN at the build step
# overrides the file so rustup does not fetch them.
FROM base AS rustup
ADD --checksum=${RUSTUP_CHECKSUM} --chmod=755 ${RUSTUP_URL} ./rustup-init
COPY --link rust-toolchain.toml /
RUN RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo ./rustup-init -y --no-modify-path --profile minimal \
 --default-toolchain "$(sed -n 's/^channel *= *"\(.*\)"/\1/p' /rust-toolchain.toml)"

# RealVNC's hosting layer builds only with clang, so clang is the C and C++
# compiler of the whole build and Rust's linker (there is no `cc`; GCC 8
# provides headers and libraries only). bindgen needs libclang.
FROM base AS sdk
ADD ${LLVM_PGP_URL} ./llvm.asc
RUN gpg --batch --with-colons --with-fingerprint llvm.asc 2> /dev/null | grep -q "^fpr:::::::::${LLVM_PGP_FINGERPRINT}:" \
 && gpg --batch --dearmor < llvm.asc > /usr/share/keyrings/llvm.gpg \
 && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bionic/ llvm-toolchain-bionic-${LLVM_VERSION} main" > /etc/apt/sources.list.d/llvm.list \
 && apt-get update && apt-get install -y --no-install-recommends clang-${LLVM_VERSION} llvm-${LLVM_VERSION} libclang-${LLVM_VERSION}-dev \
 && rm -rf /var/lib/apt/lists/*
COPY --link --from=nasm /opt /opt
COPY --link --from=meson /opt /opt
COPY --link --from=rustup /opt /opt
ENV PATH="/usr/lib/llvm-${LLVM_VERSION}/bin:/opt/nasm/bin:/opt/meson/bin:/opt/cargo/bin:$PATH" \
    RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo \
    CC=clang CXX=clang++ \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang \
    LIBCLANG_PATH=/usr/lib/llvm-${LLVM_VERSION}/lib
RUN clang --version | head -1 && nasm -v && meson --version && rustc --version

FROM sdk AS build
WORKDIR /src
COPY --link . /src/

# Apply each submodule's patch series in order if its first patch applies
# (the submodule is at its pinned commit). If its last patch reverses
# cleanly, `./patch.sh` already applied the series in the checkout. Anything
# else is an error.
RUN for dir in patches/*/; do \
     name=$(basename "$dir"); tree=third_party/$name; \
     patches=("$dir"*.patch); \
     [ -e "${patches[0]}" ] || continue; \
     [ -d "$tree" ] || { echo "error: $tree is not in the build context" >&2; exit 1; }; \
     if (cd "$tree" && git apply --check "../../${patches[0]}" 2> /dev/null); then \
         for patch in "${patches[@]}"; do (cd "$tree" && git apply "../../$patch"); done; \
         echo "$name: ${#patches[@]} patches applied"; \
     elif (cd "$tree" && git apply -R --check "../../${patches[-1]}" 2> /dev/null); then \
         echo "$name: patched already"; \
     else \
         echo "error: $tree is neither at its pinned commit nor patched" >&2; exit 1; \
     fi; \
 done

WORKDIR /src/linux
RUN RUSTUP_TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' /src/rust-toolchain.toml) cargo build --release --locked

# Copy the tree out of target/release/ (linux/build.rs lays it out there) and
# strip the libraries' debug information (the C backends are built with
# it). The tools are LLVM's, since 18.04's binutils cannot read
# clang's DWARF 5.
RUN mkdir -p /out/lib \
 && cp target/release/vento /out/ \
 && cp target/release/lib/libvento_*.so /out/lib/ \
 && llvm-strip --strip-debug /out/lib/libvento_*.so

# Check the copy: the relative rpath, none in the backends' libraries (which
# find their dependencies on the system), no library missing with
# LD_LIBRARY_PATH unset, and `vento --list`, which opens each backend's
# library (the binary does not link them), naming the tree's own lib/ for
# all four C backends and no error but RealVNC's for its viewer libraries,
# which the tree does not carry. Print the newest glibc and libstdc++
# symbol versions each file needs.
RUN llvm-readelf -d /out/vento | grep -q 'RUNPATH.*\[\$ORIGIN/lib' \
 && ! llvm-readelf -d /out/lib/libvento_*.so | grep -E '\((RPATH|RUNPATH)\)' \
 && for file in /out/vento /out/lib/*.so; do \
     ! ldd "$file" | grep 'not found' || { echo "error: $file" >&2; exit 1; }; \
 done \
 && /out/vento --list | tee /tmp/backends \
 && test "$(grep -c '^[a-z]* /out/lib/libvento_[a-z]*\.so\( (error: .*)\)\?$' /tmp/backends)" = 4 \
 && ! grep -F '(error:' /tmp/backends | grep -vF 'are not in /out/lib/realvnc.' \
 && for file in /out/vento /out/lib/*.so; do \
     echo "$file: $(llvm-objdump -T "$file" | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1)" \
      "$(llvm-objdump -T "$file" | grep -oE 'GLIBCXX_[0-9.]+' | sort -uV | tail -1)"; \
 done \
 && ls -l /out /out/lib

FROM scratch
COPY --link --from=build /out/ /
