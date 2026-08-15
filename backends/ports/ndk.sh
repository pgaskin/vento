# Sourced by the ports' gen.sh scripts: the cross toolchain they configure
# somebody else's autotools build with, for one ABI named as $1.
#
# Both pins are read out of backends/build.gradle rather than repeated here. A
# generated config.h describes the platform it was probed on, so a gen/ made
# against a different NDK or API level is a lie the compiler cannot catch — and
# a copy that has to be kept in step by hand is how that lie gets written.
gradle=$(cd "$(dirname "${BASH_SOURCE:-$0}")/.." && pwd)/build.gradle
ndk_version=$(sed -n "s/.*nativeNdkVersion *= *'\([^']*\)'.*/\1/p" "$gradle" | head -1)
api=$(sed -n "s/^ *minSdk *\([0-9]*\).*/\1/p" "$gradle" | head -1)
[ -n "$ndk_version" ] && [ -n "$api" ] || {
    echo "error: no NDK or minSdk pin in $gradle" >&2; exit 1; }

# Every place an NDK is normally found, most specific first. ANDROID_NDK_ROOT
# and ANDROID_HOME are the current names; ANDROID_NDK_HOME and ANDROID_SDK_ROOT
# are what older tooling sets and are tried after them.
#
# The `..` and `*` forms are the ones that earn their keep: a machine whose
# environment points at some other NDK still finds the pinned one beside it,
# and an SDK with several installed is searched rather than guessed at. Every
# candidate is checked against the pin, so the first one found is not the
# answer — the first one that *is* this NDK is.
ndk_candidates() {
    printf '%s\n' \
        "${ANDROID_NDK_ROOT:-}" \
        "${ANDROID_NDK_HOME:-}" \
        "${ANDROID_NDK_ROOT:+$ANDROID_NDK_ROOT/../$ndk_version}" \
        "${ANDROID_NDK_HOME:+$ANDROID_NDK_HOME/../$ndk_version}" \
        "${ANDROID_HOME:+$ANDROID_HOME/ndk/$ndk_version}" \
        "${ANDROID_SDK_ROOT:+$ANDROID_SDK_ROOT/ndk/$ndk_version}"
    for dir in ${ANDROID_HOME:+"$ANDROID_HOME"/ndk/*} ${ANDROID_SDK_ROOT:+"$ANDROID_SDK_ROOT"/ndk/*}; do
        [ -d "$dir" ] && printf '%s\n' "$dir"
    done
    ndk_build=$(command -v ndk-build 2>/dev/null) && dirname "$ndk_build"
    :
}

# A directory is this NDK only if it says so itself: Pkg.Desc separates an NDK
# from any other SDK package with a source.properties in it, which is what a
# glob over ndk/* can otherwise hand back.
ndk_why() {
    [ -f "$1/source.properties" ] || { echo "no source.properties"; return; }
    desc=$(sed -n 's/^ *Pkg\.Desc *= *//p' "$1/source.properties" | head -1)
    rev=$(sed -n 's/^ *Pkg\.Revision *= *//p' "$1/source.properties" | head -1)
    if [ "$desc" != "Android NDK" ]; then echo "a ${desc:-?}, not an NDK"
    elif [ "$rev" != "$ndk_version" ]; then echo "NDK ${rev:-?}, wanted $ndk_version"
    fi
}

ndk=
tried=
oldifs=$IFS
IFS='
'
for candidate in $(ndk_candidates); do
    why=$(ndk_why "$candidate")
    # Resolved rather than kept as written: the `..` form would otherwise put a
    # different NDK's version number inside every path this exports.
    if [ -z "$why" ]; then ndk=$(cd "$candidate" && pwd); break; fi
    tried="$tried  $candidate: $why
"
done
IFS=$oldifs
[ -n "$ndk" ] || {
    echo "error: no NDK $ndk_version (set ANDROID_NDK_ROOT, or ANDROID_HOME to an SDK with it installed)" >&2
    [ -n "$tried" ] && printf 'tried:\n%s' "$tried" >&2
    exit 1
}

bin=$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin

ndk_setup() {
    case $1 in
        arm64-v8a) triple=aarch64-linux-android ;;
        x86_64)    triple=x86_64-linux-android ;;
        *)         echo "error: unknown ABI $1" >&2; exit 1 ;;
    esac
    export CC=$bin/$triple$api-clang
    export CXX=$bin/$triple$api-clang++
    export AR=$bin/llvm-ar RANLIB=$bin/llvm-ranlib NM=$bin/llvm-nm STRIP=$bin/llvm-strip
    export CFLAGS="-O2 -fPIC" CXXFLAGS="-O2 -fPIC"
    [ -x "$CC" ] || { echo "error: no clang at $CC" >&2; exit 1; }
}
