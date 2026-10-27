#!/bin/bash
# Is every warning this build can emit either absent or a decision on the record?
#
#   ./check-warnings.sh          # javac, cargo (host and Android), lint, our C
#
# Six tools emit warnings here and they have six ideas of what one is. The policy
# has three parts:
#
#   1. **Third-party code is silenced and ours is not.** Fixing a warning in a
#      vendored tree takes a diff that will not go upstream and a patch this
#      repository would carry for ever. Silence it at the *target*, never
#      globally and never on a target of ours: `vendored_args` in the root
#      `meson.build` for the C, `--cap-lints` for anything cargo fetched, and
#      `lint` reads only this project.
#   2. **A warning of ours is fixed or answered at the line it is about.** Not in
#      a baseline file and not by switching the check off: `@SuppressWarnings`,
#      `@SuppressLint`, `tools:ignore` and `#[allow]` each say why, there, where
#      it is read when the code changes. One whole check is switched off in
#      `app/build.gradle`, a tool disagreeing with a decision rather than a
#      finding, and the root `build.gradle` makes three upgrade notices
#      informational.
#   3. **Fatal here and nowhere else.** `-Werror`, `-Dwarnings` and lint's
#      `warningsAsErrors` are switched on by this script and by nothing in the
#      path of building the app. A warning is a message from a *toolchain*, and
#      toolchains move: a new JDK adds a lint, a new NDK adds a diagnostic, and a
#      build that fails for one is a build a third party cannot make at all,
#      which includes the rebuild check-reproducible.sh exists to verify.
#
# So this is the third part. It is not in `check` because most of it cannot be: a
# strict native build is a different Meson option and a strict Java one is a
# different compiler argument, which is a second Gradle invocation either way.
#
# One caveat worth knowing before it fires: `RUSTFLAGS=-Dwarnings` reaches the
# vendored trees under `third_party/` that are path dependencies rather than
# registry crates, so a pin bump there could fail this for third-party code.
# The fix when that happens is `[lints] workspace = true` in our crates and
# no RUSTFLAGS here, not a suppression in theirs.
set -euo pipefail
cd "$(dirname "$0")/.."

# A strict native build recompiles only our own C sources, whose flags are the
# ones that change; so does the next ordinary build after it.
echo "== javac -Xlint:all -Werror, cargo -Dwarnings, lint warningsAsErrors, clang -Werror"

# The Android target compiles `platform_android`'s JNI layer and each crate's
# `natives!` line, which the host one does not: cfg(target_os = "android") is
# where the whole JNI half lives.
ndk=$(sed -n "s/^ *nativeNdkVersion *= *'\\(.*\\)'/\\1/p" backends/build.gradle)
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
if [ -z "$sdk" ] || [ ! -d "$sdk/ndk/$ndk" ]; then
    echo "check-warnings: no NDK $ndk under \$ANDROID_HOME/ndk" >&2
    exit 1
fi

# `--all-features` for the probes: the examples are behind each crate's `probe`
# feature, and a warning in one is still a warning.
RUSTFLAGS=-Dwarnings cargo check --workspace --all-targets --all-features
ANDROID_NDK_HOME="$sdk/ndk/$ndk" RUSTFLAGS=-Dwarnings \
    cargo ndk -t arm64-v8a -t x86_64 --platform 34 check --workspace --all-targets --all-features

# `check` for the lint and the unit tests, `assemble` for the native halves and
# the dexer, and both under the property the build files read.
./gradlew -PstrictWarnings check assembleDebug

echo "check-warnings: clean"
