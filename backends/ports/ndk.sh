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

ndk=${ANDROID_NDK:-$HOME/sdk/android/ndk/$ndk_version}
[ -d "$ndk" ] || { echo "error: no NDK at $ndk (set ANDROID_NDK)" >&2; exit 1; }
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
