#!/bin/sh
# One-time host generation for the gnutls port (rerun after bumping the pin in
# backends/build.gradle; must produce an empty git diff otherwise).
#
# GnuTLS is autotools with gnulib underneath it: configure writes config.h and
# forty-odd replacement headers, and decides several hundred feature tests that
# describe the platform rather than the machine that ran them. None of that may
# happen while the app is building, so it happens here, out of tree — which is
# what makes "everything in the build directory is generated" true, and the
# copying below a `find` rather than a list somebody maintains.
#
# The source list is read out of the objects the build produced, grouped by the
# Makefile that compiled them, because that grouping is what the include paths
# in CMakeLists.txt are per.
#
# Host prereqs: cc (for nettle's eccdata), make, and the NDK below.
set -eu

cd "$(dirname "$0")"
port=$PWD
root=$(cd ../../.. && pwd)
src=$root/third_party/gnutls
nettle=$root/third_party/nettle
. ../ndk.sh

for d in "$src/configure" "$nettle/configure"; do
    [ -f "$d" ] || { echo "error: no sources at ${d%/configure}; ./gradlew :backends:fetchCryptoSources" >&2; exit 1; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Flag notes, for the ones that are a decision rather than a subtraction.
#   nettle-mini      Match the nettle port, which has no GMP to link against.
#   included-*       libtasn1 and unistring as gnutls' own copies: two upstreams
#                    fewer, and both are inside the tarball we already pinned.
#   no p11-kit       PKCS#11 is a shared module loaded by path, which is not a
#                    thing an app's sandbox has.
#   no trust store   Only configure's, which would otherwise be whichever path
#                    the *host* that ran this script keeps its CAs in. It does
#                    not turn the trust store off: gnutls compiles in Android's
#                    own store on Android, which is the right one and is why a
#                    server with a certificate somebody vouches for connects
#                    silently. Everything else — which is every VNC server that
#                    signed its own — falls through to the app's known hosts.
#   prefix           Never installed; it is here because configure bakes it into
#                    a locale path, and a scratch directory in it would make
#                    this script's output depend on where it ran.
#   no hw accel      The x86 and aarch64 paths are perlasm output, which is a
#                    per-ABI source list and a second thing to generate. What it
#                    costs is AES in C, which is worth measuring before it is
#                    worth building.
build() {
    abi=$1
    mkdir -p "$tmp/$abi/nettle" && cd "$tmp/$abi/nettle"
    ndk_setup "$abi"
    "$nettle/configure" --host="$triple" --prefix="$tmp/$abi/root" \
        --enable-mini-gmp --disable-shared --disable-documentation \
        --disable-openssl --disable-assembler --disable-fat \
        > configure.log 2>&1 || { tail -20 configure.log; exit 1; }
    make -j"$(nproc)" > make.log 2>&1 || { tail -40 make.log; exit 1; }
    make install > install.log 2>&1 || { tail -20 install.log; exit 1; }

    mkdir -p "$tmp/$abi/gnutls" && cd "$tmp/$abi/gnutls"
    NETTLE_CFLAGS="-I$tmp/$abi/root/include" NETTLE_LIBS="-L$tmp/$abi/root/lib -lnettle" \
    HOGWEED_CFLAGS="-I$tmp/$abi/root/include" HOGWEED_LIBS="-L$tmp/$abi/root/lib -lhogweed -lnettle" \
    PKG_CONFIG_LIBDIR="$tmp/$abi/root/lib/pkgconfig" \
    "$src/configure" --host="$triple" --prefix=/nonexistent \
        --with-nettle-mini --with-included-libtasn1 --with-included-unistring \
        --without-p11-kit --without-idn --without-tpm --without-tpm2 \
        --without-zlib --without-brotli --without-zstd \
        --without-default-trust-store-file --without-default-trust-store-dir \
        --disable-doc --disable-tools --disable-tests --disable-cxx --disable-nls \
        --disable-libdane --disable-guile --disable-shared --enable-static \
        --disable-hardware-acceleration --disable-full-test-suite \
        > configure.log 2>&1 || { tail -40 configure.log; exit 1; }
    make -j"$(nproc)" > make.log 2>&1 || { grep -n "[Ee]rror" make.log | head -20; exit 1; }
}

echo "> arm64-v8a"
build arm64-v8a
echo "> x86_64"
build x86_64

# An out-of-tree build directory holds nothing but generated files, so this is
# the whole of what gen/ has to carry.
cd "$tmp/arm64-v8a/gnutls"
generated=$(find config.h gl lib -name '*.h' | sort)

rm -rf "$port/gen" && mkdir -p "$port/gen"
for f in $generated; do
    if ! cmp -s "$tmp/arm64-v8a/gnutls/$f" "$tmp/x86_64/gnutls/$f"; then
        echo "  $f differs between ABIs"
        mkdir -p "$port/gen/arm64-v8a/$(dirname "$f")" "$port/gen/x86_64/$(dirname "$f")"
        cp "$tmp/arm64-v8a/gnutls/$f" "$port/gen/arm64-v8a/$f"
        cp "$tmp/x86_64/gnutls/$f" "$port/gen/x86_64/$f"
        continue
    fi
    mkdir -p "$port/gen/$(dirname "$f")"
    cp "$tmp/arm64-v8a/gnutls/$f" "$port/gen/$f"
done

# The objects, mapped back to their sources: automake prefixes an object with
# the target it is compiled for when that target has its own flags, and puts a
# subdirectory's objects under the Makefile that lists them — which is the
# grouping the include paths are per, so it is the grouping here.
cd "$tmp/arm64-v8a/gnutls"
# Not `find … | while read`: that loop is a subshell of a pipeline, so its
# `exit 1` would end only the subshell, the pipeline would take sort's status,
# and set -e would see a success — leaving a sources.cmake with whatever could
# not be mapped silently missing from it.
find lib gl -name '*.o' -not -path '*/.libs/*' > "$tmp/objects"
: > "$tmp/mapped"
while read -r obj; do
    source=$(echo "$obj" | sed -e 's/\.o$/.c/' -e 's|/lib[a-z0-9_]*_la-|/|')
    [ -f "$src/$source" ] || { echo "error: no source for $obj" >&2; exit 1; }
    group=$(dirname "$obj")
    while [ ! -f "$group/Makefile" ]; do
        [ "$group" = . ] && { echo "error: no Makefile above $obj" >&2; exit 1; }
        group=$(dirname "$group")
    done
    echo "$group $source" >> "$tmp/mapped"
done < "$tmp/objects"
sort "$tmp/mapped" > "$tmp/grouped"
cut -d' ' -f1 "$tmp/grouped" | uniq > "$tmp/groups"

{
    echo "# Written by gen.sh — every object gnutls' own build produced, grouped"
    echo "# by the Makefile that compiled it."
    echo
    while read -r group; do
        echo "set(GNUTLS_SOURCES_$(echo "$group" | tr '/' '_')"
        awk -v g="$group" '$1 == g { print "        " $2 }' "$tmp/grouped"
        echo ")"
        echo
    done < "$tmp/groups"
} > "$port/gen/sources.cmake"

echo "> done; check git status for unexpected diffs"
