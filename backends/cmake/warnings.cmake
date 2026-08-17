# SPDX-License-Identifier: GPL-3.0-or-later
#
# What a warning is worth in a native build here, in one place because four
# CMake projects across three modules answer it the same way.
#
# **Somebody else's code is silenced and ours is not.** A warning in a vendored
# tree is not a finding: it is a diff nobody here will send upstream and a patch
# this repository would then carry for ever. A warning in one of our four
# translation units is a finding, because they are JNI shims full of
# hand-managed lifetimes and there is nobody else to report it to.
#
# Two rules follow, and the second is the one that is easy to get wrong:
#
# 1. Silence at the *target*, never globally, and never on a target of ours.
#    `-w` on a directory reaches ours too; the vendored trees are separate
#    targets and the flags below never touch them.
# 2. Their headers are ours to include and theirs to fix. `-Wall` on our target
#    diagnoses everything it compiles, which includes several thousand lines of
#    somebody else's templates — so a vendored include directory goes on the
#    path as SYSTEM, and the warning is the file's rather than the target's.
#
# `-Werror` is deliberately *not* here. A warning is a message from a toolchain,
# and a build that fails for one is a build a third party cannot make at all —
# including the reproducible-build check, whose whole point is that they can.
# It is switched on by `scripts/check-warnings.sh`, which is the one caller that sets
# STRICT_WARNINGS, and by nothing in the path of building the app.

option(STRICT_WARNINGS "Fail the build on a warning in our own sources" OFF)

# Every native target of ours, and no others.
function(remotedesktop_warnings target)
    target_compile_options(${target} PRIVATE -Wall -Wextra)
    if(STRICT_WARNINGS)
        target_compile_options(${target} PRIVATE -Werror)
    endif()
endfunction()
