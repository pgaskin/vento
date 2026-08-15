#!/bin/sh
# One-time host generation for the openssl port (rerun after moving the
# submodule pin; must produce an empty git diff otherwise).
#
# OpenSSL's build is a perl Configure, which writes a Makefile, a configuration
# header, nine DER templates and — the part that makes this a port rather than a
# file list — every assembly file, out of perlasm, per ABI. None of that may
# happen while the app is building, so it happens here.
#
# The source list is read out of the Makefile's own rules rather than out of a
# directory: which C file is compiled depends on what the assembly replaced, so
# the two ABIs do not build the same set and neither does a configuration with
# different switches.
#
# Host prereqs: perl with its core modules (fedora: perl-core), make, and the
# NDK below.
set -eu

cd "$(dirname "$0")"
port=$PWD
root=$(cd ../../.. && pwd)
src=$root/third_party/openssl
. ../ndk.sh

[ -f "$src/Configure" ] || {
    echo "error: no sources at $src; run: git submodule update --init" >&2; exit 1; }

for m in FindBin IPC::Cmd Time::Piece Pod::Usage; do
    perl -e "use $m;" 2>/dev/null || {
        echo "error: OpenSSL's Configure needs perl's $m (fedora: perl-core)" >&2; exit 1; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

abis="arm64-v8a x86_64"

# Flag notes, for the ones that are a decision rather than a subtraction. The
# list is upstream's own scripts/android-build-openssl.sh minus what an RDP
# client cannot reach.
#   no-legacy    RC4, MD4 and MD5 are in the legacy provider, and RDP needs all
#                three on the wire — FreeRDP's WITH_INTERNAL_* switches compile
#                its own instead, which is why the provider can go.
#   no-dso       Loading a provider by path is not a thing an app's sandbox has,
#                and it takes engines and the module directory with it.
#   no-apps      Everything here is the two libraries; nothing installs a binary.
#   asm          Kept, unlike the gnutls port's: this is the TLS under a session
#                that carries video, and AES in C is where that shows. It is
#                what makes gen/ per-ABI rather than one directory.
configure_args="no-shared no-tests no-apps no-docs no-legacy no-engine no-dso
    no-comp no-dtls no-srp no-psk no-ssl3 no-weak-ssl-ciphers"

for abi in $abis; do
    echo "> $abi"
    case $abi in
        arm64-v8a) target=android-arm64 ;;
        x86_64)    target=android-x86_64 ;;
    esac
    mkdir -p "$tmp/$abi" && cd "$tmp/$abi"
    ndk_setup "$abi"
    # Configure wants the NDK's clang by bare name on the path, and takes the
    # ABI from its own target rather than from a triple of ours.
    ANDROID_NDK_ROOT=$ndk PATH=$bin:$PATH CC=clang \
        perl "$src/Configure" "$target" $configure_args \
        > configure.log 2>&1 || { tail -20 configure.log; exit 1; }
    ANDROID_NDK_ROOT=$ndk PATH=$bin:$PATH \
        make -j"$(nproc)" build_libs > make.log 2>&1 || {
            grep -n "[Ee]rror" make.log | head -20; exit 1; }
done

# Everything the build produced that is compiled or included, which is the whole
# of what gen/ has to carry: an out-of-tree build directory holds nothing else.
rm -rf "$port/gen"
cd "$tmp/arm64-v8a"
generated=$(find . -type f \( -name '*.h' -o -name '*.c' -o -name '*.S' -o -name '*.s' \
    -o -name '*.inc' \) | sed 's|^\./||' | sort)
for abi in $abis; do
    (cd "$tmp/$abi" && find . -type f \( -name '*.h' -o -name '*.c' -o -name '*.S' \
        -o -name '*.s' -o -name '*.inc' \) | sed 's|^\./||' | sort) > "$tmp/files.$abi"
done

# A file only one ABI has is per-ABI by definition; so is one both have and whose
# contents differ. Everything else is written once.
for abi in $abis; do
    while read -r f; do
        common=1
        for other in $abis; do
            cmp -s "$tmp/$abi/$f" "$tmp/$other/$f" || common=
        done
        if [ -n "$common" ]; then
            [ "$abi" = arm64-v8a ] || continue
            dest=$port/gen/$f
        else
            dest=$port/gen/$abi/$f
        fi
        mkdir -p "$(dirname "$dest")"
        cp "$tmp/$abi/$f" "$dest"
    done < "$tmp/files.$abi"
done

# The sources, per ABI because the assembly decides which C files survive, and
# grouped by the archive they are compiled for, because that is what the include
# paths and the defines are per. Read out of the Makefile's own rules: the first
# prerequisite of an object is what compiles into it, which is the only place
# that says whether a name came from the tree or from a generator.
#
# The -D flags come out of the same rule, and are per group rather than a union
# over the build: `aes_platform.h` and three provider files read these macros,
# and upstream compiles the providers without most of them, so a union would
# have a provider calling into assembly that was never selected for it.
for abi in $abis; do
    cd "$tmp/$abi"
    # The archive an object goes into is the first two fields of its name, not
    # everything before the last dash: half the assembly is called aes-gcm-armv8.
    awk -v src="$src/" '
        /^[^ \t].*\.o:/ { obj = $1; sub(/:$/, "", obj); dep = $2 }
        /^\t\$\(CC\)/ && obj != "" {
            n = split(obj, path, "/"); split(path[n], name, "-")
            group = name[1] "-" name[2]
            at = index(dep, src)
            if (at) print group, "SOURCES", substr(dep, at + length(src))
            else print group, "GENERATED", dep
            for (i = 1; i <= NF; i++) if ($i ~ /^-D/) print group, "DEFINES", substr($i, 3)
            obj = ""
        }' Makefile | sort -u > "$tmp/grouped"
    {
        echo "# Written by gen.sh — every object openssl's own build produced for"
        echo "# $abi, grouped by the archive it goes into."
        echo
        # libtemplate is a provider skeleton upstream compiles and archives into
        # nothing; it is in neither library, so it is not here either.
        for group in libcrypto-lib libcommon-lib libdefault-lib libssl-lib; do
            for kind in DEFINES SOURCES GENERATED; do
                echo "set(OPENSSL_${kind}_$(echo "$group" | tr '-' '_')"
                awk -v g="$group" -v k="$kind" '$1 == g && $2 == k { print "        " $3 }' \
                    "$tmp/grouped"
                echo ")"
                echo
            done
        done
    } > "$port/gen/$abi/sources.cmake"
done

echo "> done; check git status for unexpected diffs"
