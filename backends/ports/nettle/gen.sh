#!/bin/sh
# One-time host generation for the nettle port (rerun after bumping the pin in
# backends/build.gradle; must produce an empty git diff otherwise).
#
# Nettle's build is autotools plus a generator: configure writes config.h and
# version.h, and make builds `eccdata` for the host and runs it to write the
# tables for each curve. None of that may happen while the app is building, so
# it happens here and the answers are committed.
#
# Both ABIs are generated and compared rather than one being generated and
# trusted: they are both LP64 and little-endian, so the files come out
# identical, and a future ABI that breaks that assumption stops this script
# instead of silently getting arm64's answers.
#
# Host prereqs: cc (for eccdata), make, and the NDK below.
set -eu

cd "$(dirname "$0")"
port=$PWD
# The path built rather than cd'd into, so that the check below is what reports
# a missing tree — a cd under set -e aborts before it can be reached.
src=$(cd ../../.. && pwd)/third_party/nettle
. ../ndk.sh

[ -f "$src/configure" ] || { echo "error: no nettle sources; ./gradlew :backends:fetchNettleSources" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Flag notes.
#   mini-gmp       Nettle's own bignums. GMP is a second autotools upstream and
#                  Android has none of it; mini-gmp is a file in the tarball.
#   no assembler   Nettle's per-architecture assembly is m4 input, and m4 is a
#                  host tool this build refuses to need. What that costs is the
#                  C fallbacks for AES, GHASH and the hashes.
#   no fat         Runtime CPU dispatch selects between those assembly variants,
#                  so with the assembler off it has nothing to select.
build() {
    mkdir -p "$tmp/$1" && cd "$tmp/$1"
    ndk_setup "$1"
    "$src/configure" --host="$triple" --prefix="$tmp/$1/root" \
        --enable-mini-gmp --disable-shared --disable-documentation \
        --disable-openssl --disable-assembler --disable-fat \
        > configure.log 2>&1 || { tail -20 configure.log; exit 1; }
    make -j"$(nproc)" > make.log 2>&1 || { tail -40 make.log; exit 1; }
    make install > install.log 2>&1 || { tail -20 install.log; exit 1; }
}

echo "> arm64-v8a"
build arm64-v8a
echo "> x86_64"
build x86_64

for f in config.h version.h ecc-*.h; do
    cmp "$tmp/arm64-v8a/$f" "$tmp/x86_64/$f" ||
        { echo "error: $f differs between ABIs; gen/ can no longer be one copy" >&2; exit 1; }
done

rm -rf "$port/gen" && mkdir -p "$port/gen"
cd "$tmp/arm64-v8a"
cp config.h version.h ecc-*.h "$port/gen/"

# The source lists are read back out of the archives rather than out of the
# Makefile: what an archive holds is what the library is, conditionals and all.
list() {
    ar t "$tmp/arm64-v8a/$1.a" | sed 's/\.o$/.c/' | sort | sed 's/^/        /'
}
{
    echo "# Written by gen.sh — the members of libnettle.a and libhogweed.a as"
    echo "# nettle's own build produced them, and the headers it installed."
    echo
    echo "set(NETTLE_SOURCES"
    list libnettle
    echo ")"
    echo
    echo "set(HOGWEED_SOURCES"
    list libhogweed
    echo ")"
    echo
    echo "set(NETTLE_HEADERS"
    (cd "$tmp/arm64-v8a/root/include/nettle" && ls *.h) | sort | sed 's/^/        /'
    echo ")"
} > "$port/gen/sources.cmake"

echo "> done; check git status for unexpected diffs"
