# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The host build of the C backends: the session files under `modules/*/cpp`,
# against the libraries a Linux desktop has instead of the ports and vendored
# trees Android needs (except libjpeg-turbo, pixman, GnuTLS, nettle and
# OpenSSL, which are built from the same sources as on Android), and, with
# `-DVENTO_PROBES=ON`, a probe in front of each that connects, prints every
# callback and writes the framebuffer out.
# Three of them are open clients; the fourth, RealVNC, is RealVNC's
# proprietary Android core, run in this process by the hosting layer in its
# module (`modules/realvnc/cpp/host`) from two libraries the user extracts from
# the viewer's APK (linux/claude-notes.md, section "RealVNC", says how). The
# build needs none of RealVNC's files; the probe needs the two libraries.
# Gradle never reads this file (each module's CMakeLists is its own root there,
# under the NDK toolchain). It is the Linux client's build of the C backends
# (`linux/build.rs` runs it, with VENTO_SHARED on) and the development build:
# a session file that builds and connects here works through the C API alone.
#
#   cmake -S backends -B build/scratch/host -G Ninja -DVENTO_PROBES=ON
#   cmake --build build/scratch/host
#   build/scratch/host/libvnc/vento_libvnc_probe HOST:1 --password ... --out shot.ppm
#
# Options: -DVENTO_PROBES=ON builds the four probes (and FreeRDP's and
# RealVNC's tests, which `ctest` runs), off by default so that the Linux client's build of this
# tree compiles none of them; -DSTRICT_WARNINGS=ON makes a warning in our own
# sources an error, as `scripts/check-warnings.sh` does for the Android build;
# CMAKE_BUILD_TYPE defaults to RelWithDebInfo, since the decoders are slow at
# -O0 and a probe without line numbers is hard to debug; -DVENTO_REALVNC=OFF
# leaves the RealVNC module out, as does a compiler other than clang (libc-shim,
# under the hosting layer, does not build with GCC), with a message rather
# than an error; -DVENTO_SHARED=ON makes each `vento_<name>` a shared library
# under `lib/` carrying its dependencies and the logger, the form the Rust
# client under `linux/` loads at run time (its build.rs runs this file with
# that option on); -DVENTO_FFMPEG_PREFIX=DIR links FreeRDP's scaler against the
# static FFmpeg under DIR instead of the host's, which is how the Linux
# client's build passes it the FFmpeg `backends/platform/linux/build.rs` built
# (see platform/linux/cmake/vento_linux.cmake).
#
# Needs, as development packages: zlib, FFmpeg's libswscale and libavutil
# (unless VENTO_FFMPEG_PREFIX is given; FreeRDP also links the host's
# libavcodec where there is one, for its camera channel's MJPEG, which is
# unused), plus pkg-config, make and m4 (GnuTLS and nettle are built by their
# own autotools build; nettle's assembly needs m4, and is left out without it),
# perl with its core modules (OpenSSL's Configure), nasm (libjpeg-turbo's x86
# SIMD; it builds as plain C without it), and a C and C++ compiler CMake can
# find: clang for RealVNC (`CC=clang CXX=clang++ cmake ...`), whose four
# host-only submodules are initialised with `--recursive`. GnuTLS and nettle
# are the release tarballs the Android ports build, pinned in
# `crypto-sources.txt`: the first configure downloads them, checks their
# SHA-256 and unpacks them into `third_party/gnutls` and `third_party/nettle`
# (as `./gradlew :backends:fetchCryptoSources` does), so it needs the network
# unless they are there already.

cmake_minimum_required(VERSION 3.30)
project(vento_backends C CXX)

if(ANDROID)
    message(FATAL_ERROR "this is the host build; Gradle configures each module's own CMakeLists")
endif()

if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "" FORCE)
endif()

# Besides its link-time optimisation, which its module turns off, one FreeRDP
# setting reaches past its own tree: its C standard, cached by its
# cmake/ProjectCStandard.cmake. Everything configured before it
# (vento_linux.cmake's logger, libvncserver, TigerVNC) sees no cache entry on
# a first configure and reads it on every later one, so their C standard was
# the compiler's default or gnu11 depending on the tree's history. Cached here,
# ahead of all of them, it is gnu11 from the first configure, as a
# reconfigured tree already had.
set(CMAKE_C_STANDARD 11 CACHE STRING "")
set(CMAKE_C_STANDARD_REQUIRED ON CACHE BOOL "")

# The sessions' C++ is C++17 (TigerVNC's uses std::optional and
# std::shared_mutex). Clang 16 and later, GCC 11 and later, and the NDK's
# clang default to it, but an older compiler (clang 14, Ubuntu 22.04's)
# defaults to C++14 and fails the build. It is set here only, so the Android
# libraries are unchanged; a tree that sets its own (libjnivm per target,
# mcpelauncher-linker in its directory) keeps it.
set(CMAKE_CXX_STANDARD 17 CACHE STRING "")
set(CMAKE_CXX_STANDARD_REQUIRED ON CACHE BOOL "")

include(cmake/warnings.cmake)
include(platform/linux/cmake/vento_linux.cmake)

# libjpeg-turbo, from `third_party/` as on Android, rather than the host's:
# libvncclient's Tight and TigerVNC's both decode with it, and it is linked
# statically into each of their libraries (`vento_linux.cmake` keeps its
# symbols out of their exports). The host's would tie a built tree to its
# distribution (libjpeg-turbo's soname is libjpeg.so.62 on Fedora and Debian
# and libjpeg.so.8 on Ubuntu, where .so.62 is IJG's 6b), so it is not looked
# for. Built once here, where Android builds a copy per module root, since the
# two modules share this tree and a target name can occur once in it; its
# jconfig.h is generated at configure time, which a host build allows.
# Its options are its own and go into the shared cache, so they are forced,
# and they are set in a block so that position-independent code, which a
# static archive linked into a shared library needs, applies to it alone.
# NASM, which the video decoders need already, gives it its x86 SIMD; without
# NASM it builds in C. The build string is the fixed one Android's
# `reproducible.cmake` gives it, not the build date. Its warnings are not
# silenced as the other vendored trees' are, since that `-w` would also reach
# its NASM sources, where it means something else.
if(NOT EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo/CMakeLists.txt")
    message(FATAL_ERROR "third_party/libjpeg-turbo is empty — run `git submodule update --init`")
endif()
block(SCOPE_FOR VARIABLES)
    macro(install)
    endmacro()
    macro(export)
    endmacro()
    set(CMAKE_POSITION_INDEPENDENT_CODE ON)
    set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
    set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
    set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
    set(WITH_TESTS OFF CACHE BOOL "" FORCE)
    set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
    set(BUILD "0" CACHE STRING "" FORCE)
    # It rejects an empty documentation or manual directory, which is what
    # CMake's GNUInstallDirs (included by FreeRDP and libvncserver) cached for
    # both in a tree configured before this block existed. This sets a value
    # for its check where the cache has an empty one. (Where the cache has
    # none, as on a first configure, it creates its own, and a variable set
    # here would make it set a property on a cache entry that does not exist.)
    # Nothing is installed from this tree, so any value works.
    foreach(dir DOCDIR MANDIR)
        if(DEFINED CACHE{CMAKE_INSTALL_${dir}} AND "$CACHE{CMAKE_INSTALL_${dir}}" STREQUAL "")
            set(CMAKE_INSTALL_${dir} ${dir})
        endif()
    endforeach()
    add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)
endblock()
# Its headers are in two places, since jconfig.h is generated: the modules
# put both on their path with the target, `jpeg-static`.
set(VENTO_JPEG_INCLUDE_DIRS
        "${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo/src"
        "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")

# GnuTLS and nettle, from the tarballs the phone's ports build (downloaded
# here when they are not in `third_party/`), configured and built by their
# own autotools build at build time (the file says why).
# libvncclient and TigerVNC link them in, as they do libjpeg-turbo.
include(platform/linux/cmake/gnutls.cmake)

# OpenSSL, from the submodule the phone's port builds, configured and built
# by its own build at build time in the same way. FreeRDP links it in.
include(platform/linux/cmake/openssl.cmake)

# Four roots under Gradle, four subdirectories here; each branches on
# `ANDROID` and takes its host half. The vendored trees under them share
# this one cache, which is why every option a tree reads is forced in its
# module rather than left to a default.
# With the probes, `ctest` in the build tree runs the tests a module has
# beside its probe (so far FreeRDP's, freerdp_test.c, and RealVNC's,
# realvnc_test.cxx).
if(VENTO_PROBES)
    enable_testing()
endif()

add_subdirectory(modules/libvnc/cpp libvnc)
add_subdirectory(modules/tigervnc/cpp tigervnc)
add_subdirectory(modules/freerdp/cpp freerdp)

option(VENTO_REALVNC "Build the RealVNC backend's host half (needs clang)" ON)
if(NOT VENTO_REALVNC)
    message(STATUS "RealVNC: skipped (VENTO_REALVNC is off)")
elseif(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang" OR NOT CMAKE_C_COMPILER_ID MATCHES "Clang")
    message(STATUS "RealVNC: skipped, since its hosting layer needs clang and this is "
            "${CMAKE_CXX_COMPILER_ID} (CC=clang CXX=clang++ cmake ... to build it)")
else()
    add_subdirectory(modules/realvnc/cpp realvnc)
endif()
