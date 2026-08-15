# SPDX-License-Identifier: GPL-3.0-or-later
#
# What a native build bakes in that the sources do not decide — where the build
# ran, and when. Include this before any `add_subdirectory`: all three are
# inherited by a subdirectory added afterwards and by none added before.

get_filename_component(_REPRO_REPO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)

# `assert()` and `__FILE__` put the absolute path of each source file in
# .rodata, and those strings are not debug info: release stripping keeps them,
# so a library built here would differ from one built in somebody's buildserver.
# Every checkout maps its own root to the same `.`, which is what makes the
# output independent of where it lives.
#
# A directory property rather than CMAKE_C_FLAGS, which is what one would reach
# for first: FreeRDP compiles its own CMAKE_C_FLAGS into a build-configuration
# string, and a flag naming the repository root would then put back exactly the
# path it was added to remove.
add_compile_options("-ffile-prefix-map=${_REPRO_REPO_ROOT}=.")

# lld hashes the linked image *before* strip, and that image still carries NDK
# sysroot paths in .debug_* and .comment, which no prefix map of ours covers.
# Those sections are stripped from what is packaged; the build id note is the
# one path-dependent thing that survives, so drop it.
string(APPEND CMAKE_SHARED_LINKER_FLAGS " -Wl,--build-id=none")
string(APPEND CMAKE_EXE_LINKER_FLAGS " -Wl,--build-id=none")

# libjpeg-turbo defaults its build string to today's date and puts it in a
# version banner, which makes its object files a function of the calendar. Any
# fixed value will do; this one is meant to be unmistakably not a date.
set(BUILD "0" CACHE STRING "" FORCE)
