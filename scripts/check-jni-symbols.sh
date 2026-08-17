#!/bin/bash
# Does every `native` method we declare exist in the library we ship?
#
# `realvnc/check-jni-abi.sh` asks this of somebody else's binary; this
# asks it of ours, and the failure mode is the same one: a native binds by
# mangled name, so a renamed method, a moved class or a Rust entry point whose
# name drifted does not fail to compile. It fails the first time that method is
# called, as an UnsatisfiedLinkError — which for something like
# `nativeReadThumbnail` might be the first time somebody looks at the home
# screen after a session.
#
# One library per protocol module, which is what the loop below is: each is
# asked about its own declarations, and a module deleted is a line deleted. What
# built the library does not matter here — a cargo one and a CMake one both end
# up as an ELF with dynamic symbols in it — so the only per-module difference is
# the task that makes it and where it lands.
#
#   backends/check-jni-symbols.sh            # builds if needed
#   NOBUILD=1 backends/check-jni-symbols.sh  # against whatever is already there
set -euo pipefail
cd "$(dirname "$0")/.."   # the build is at the repository root

# module : the class declaring the natives : the library it loads : how it is built
MODULES=(
    "rfb:net/pgaskin/remotedesktop/backend/rfb/RfbNative:remotedesktop_rfb:cargo"
    "libvnc:net/pgaskin/remotedesktop/backend/libvnc/LibVncNative:remotedesktop_libvnc:cmake"
    "tigervnc:net/pgaskin/remotedesktop/backend/tigervnc/TigerVncNative:remotedesktop_tigervnc:cmake"
    "ironrdp:net/pgaskin/remotedesktop/backend/ironrdp/IronRdpNative:remotedesktop_ironrdp:cargo"
    "freerdp:net/pgaskin/remotedesktop/backend/freerdp/FreeRdpNative:remotedesktop_freerdp:cmake"
    "realvnc:net/pgaskin/remotedesktop/backend/realvnc/RealVncTraffic:remotedesktop_realvnc:cmake"
    "rustdesk:net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNative:remotedesktop_rustdesk:cargo"
    "spice:net/pgaskin/remotedesktop/backend/spice/SpiceNative:remotedesktop_spice:cargo"
)

if [ "${NOBUILD:-0}" != 1 ]; then
    tasks=()
    for entry in "${MODULES[@]}"; do
        module="${entry%%:*}"
        case "${entry##*:}" in
            cargo) tasks+=(":backends:${module}:cargoBuildDebugNative") ;;
            cmake) tasks+=(":backends:${module}:assembleDebug") ;;
        esac
    done
    ./gradlew --quiet "${tasks[@]}"
fi

fail=0
for entry in "${MODULES[@]}"; do
    IFS=: read -r module class lib built <<<"$entry"

    case "$built" in
        cargo) root="backends/$module/build/generated/rustNativeLibs" ;;
        cmake) root="backends/$module/build/intermediates/cxx" ;;
    esac
    # The newest, not the first: a CMake module's build tree keeps a directory
    # per configuration, so `find | head -1` can hand back a library from an
    # older one — which is a check that answers about a binary nobody is
    # shipping, in both directions. Seen: a MISSING for a method that had been
    # there for a stage.
    so="$(find "$root" -name "lib${lib}.so" -printf '%T@ %p\n' 2>/dev/null \
            | sort -rn | head -1 | cut -d' ' -f2-)"
    if [ -z "$so" ]; then
        echo "no lib${lib}.so — run ./gradlew :backends:${module}:assembleDebug" >&2
        exit 1
    fi
    echo "== ${module}: ${so#*/build/}"

    # The host's readelf, as check-page-alignment.sh uses: these are ordinary
    # ELF dynamic symbols and no cross toolchain is needed to read them.
    symbols="$(readelf -W --dyn-syms "$so" | awk '$4=="FUNC"{print $NF}')"

    java="backends/$module/src/main/java/$class.java"
    prefix="Java_$(dirname "$class" | tr / _)_$(basename "$class")"
    while read -r method; do
        [ -n "$method" ] || continue
        if grep -qx "${prefix}_${method}" <<<"$symbols"; then
            printf 'ok       %s.%s\n' "$(basename "$class")" "$method"
        else
            printf 'MISSING  %s.%s (%s)\n' "$(basename "$class")" "$method" "${prefix}_${method}"
            fail=1
        fi
    done < <(grep -E 'static +native' "$java" | grep -oE '\bnative[A-Z][A-Za-z0-9]*')
done

if [ "$fail" = 0 ]; then
    echo "PASS: every declared native has a symbol in its own library"
else
    echo "FAIL: see above" >&2
    exit 1
fi
