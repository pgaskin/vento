#!/bin/bash
# Does this build depend on where it was run?
#
# Builds the app twice from the same sources, in two copies of this tree at two
# different absolute paths, and compares the APKs with check-reproducible.go —
# which is the apksigcopier comparison rather than a hash of the file. A
# reproducible build must not depend on the checkout's location, which is what
# the two copies are for; everything else about the two builds is the same
# machine, the same toolchain and the same caches.
#
#   ./check-reproducible.sh             # release, both ABIs
#   ./check-reproducible.sh debug
#   ./check-reproducible.sh -k release  # keep the copies, to diff by hand
#
# Both artefacts are compared: the app and the add-on, which are released
# together and neither of which contains anything of anybody else's.
#
# Neither build is this working tree's, which is deliberate: a tree somebody
# develops in has a warm CMake cache, and a cache that has outlived the option
# it holds produces a library the current sources would not. Both copies carry
# everything tracked plus the two unpacked crypto trees and less every build
# directory, so each compiles the native halves and the crates from scratch.
#
# To compare an APK you are about to ship against a rebuild, run the Go program
# directly: it takes any two APKs, or a URL.
set -euo pipefail
cd "$(dirname "$0")/.."

keep=
while getopts ":k" arg; do
    case "$arg" in
        k) keep=1 ;;
        *) echo "usage: $0 [-k] [buildType]" >&2; exit 2 ;;
    esac
done
shift $((OPTIND - 1))

buildType=${1-release}
[ $# -le 1 ] || { echo "usage: $0 [-k] [buildType]" >&2; exit 2; }

command -v go >/dev/null || { echo "error: go is not on the PATH" >&2; exit 1; }

capitalized=$(echo "${buildType:0:1}" | tr '[:lower:]' '[:upper:]')${buildType:1}
outputs=app/build/outputs/apk/$buildType
pluginOutputs=plugins/realvnc/build/outputs/apk/$buildType

here=$PWD
# Two copies and two builds of them is around 40 GB, so they go beside the
# repository rather than under $TMPDIR, which on a good many machines is a
# tmpfs. $REPRO_BASE overrides where.
#
# Different lengths as well as different prefixes: a path baked into an object
# usually shows up as a size difference first, and two paths of one length
# would hide it.
base=${REPRO_BASE:-$(dirname "$here")}
a=$base/rvnc-a
b=$base/rvnc-input-reproducible-build-check-b
trap '[ -n "$keep" ] || rm -rf "$a" "$b"' EXIT

for dir in "$a" "$b"; do
    echo "==> copying to $dir"
    rm -rf "$dir"
    mkdir -p "$dir"
    rsync -a \
        --exclude='/target/' \
        --exclude='build/' \
        --exclude='.cxx/' \
        --exclude='.gradle/' \
        --exclude='/notes/sessions/' \
        "$here/" "$dir/"
done

for dir in "$a" "$b"; do
    echo "==> building $buildType in $dir"
    (cd "$dir" && ./gradlew ":app:assemble$capitalized" ":plugins:realvnc:assemble$capitalized")
done

status=0
shopt -s nullglob
for out in "$outputs" "$pluginOutputs"; do
    for apk in "$a/$out"/*.apk; do
        name=$(basename "$apk")
        echo
        echo "==> $name"
        go run ./check-reproducible.go "$apk" "$b/$out/$name" || status=1
    done
done

exit $status
