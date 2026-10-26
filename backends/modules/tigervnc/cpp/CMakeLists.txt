# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and makes `libremotedesktop_tigervnc.so`: the session, with the
# module's Rust crate linked in as the JNI layer over it (and with it
# MediaCodec and AAudio, which decode its H.264 and play its sound), and the
# vendored libjpeg-turbo and pixman and the nettle and GnuTLS ports linked in,
# since a phone has none of them. Under `backends/CMakeLists.txt` it is a
# subdirectory on a Linux host and makes `vento_tigervnc`, the session, with
# the same libjpeg-turbo and pixman and the host root's nettle and GnuTLS
# (built from the ports' tarballs), and a probe. The middle (how TigerVNC's
# `common/` itself is configured) is shared, and the parts around it are
# `if(ANDROID)` and `else()`.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_tigervnc CXX C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)

if(ANDROID)
    set(VENDORED tigervnc pixman libjpeg-turbo)
else()
    set(VENDORED tigervnc pixman)
endif()
foreach(sub ${VENDORED})
    if(NOT EXISTS "${THIRD_PARTY}/${sub}/README.rst" AND NOT EXISTS "${THIRD_PARTY}/${sub}/meson.build"
            AND NOT EXISTS "${THIRD_PARTY}/${sub}/CMakeLists.txt")
        message(FATAL_ERROR "third_party/${sub} is empty — run `git submodule update --init`")
    endif()
endforeach()

# What is baked into the phone's library is pinned so that a build elsewhere
# matches; a host build is for running, and keeps its real paths for gdb.
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
if(ANDROID)
    include("${VENTO_BACKENDS}/platform/android/cmake/vento_android.cmake")
else()
    include("${VENTO_BACKENDS}/platform/linux/cmake/vento_linux.cmake")
endif()

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically, and into the host's library the same way.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# These projects are imported for their static libraries only; neutering
# install/export stops them warning about targets that are never installed.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
    # AGP configures the debug variant at -O0, which for a JPEG and zlib decoder is
    # several times the CPU of a release one, and the hot-path measurements are
    # taken on debug builds. A trailing -O2 wins over the toolchain's earlier -O0.
    # -DNDEBUG goes with it deliberately: upstream's asserts are invariant checks
    # that abort the app, and a measurement build is not where they should fire.
    string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")
    string(APPEND CMAKE_CXX_FLAGS_DEBUG " -O2 -DNDEBUG")

    include(cmake/pixman.cmake)

    # libjpeg-turbo, for Tight and for the JPEG rectangles TigerVNC's own encoder
    # sends. The SIMD path on x86 needs NASM, which is a host tool this build does
    # not require; on arm64, the ABI phones use, it is Neon intrinsics in C.
    set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
    set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
    set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
    set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
    if(CMAKE_ANDROID_ARCH_ABI MATCHES "^x86")
        set(WITH_SIMD OFF CACHE BOOL "" FORCE)
    endif()
    add_subdirectory("${THIRD_PARTY}/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)
endif()

# TigerVNC's own root CMakeLists is not usable here: it requires X11 on
# anything that is UNIX and not APPLE, which Android is and a probe has no use
# for, and it builds a viewer and a server this build does not need. So this
# build takes `common/` on its own, sets the variables its sub-projects read,
# and writes the config.h their sources include. Their build files need no
# patch, and neither do their sources for the build itself: the one file of
# theirs that would need a change, the H.264 decoder context's factory, is
# replaced by ours (below).
set(VERSION "unknown")
file(READ "${THIRD_PARTY}/tigervnc/CMakeLists.txt" _tigervnc_cmake)
if(_tigervnc_cmake MATCHES "set\\(VERSION ([0-9][0-9.]*)\\)")
    set(VERSION ${CMAKE_MATCH_1})
endif()
# TigerVNC's defaults to the time of the build, which nothing in `common/` prints
# but config.h still carries into every translation unit; the same fixed value
# as libjpeg-turbo's, for the same reason.
set(BUILD_TIMESTAMP "0")

if(ANDROID)
    # Nettle, which TigerVNC's RSA-AES security types (which no other client
    # built here implements) are written against. Their own build finds it with
    # find_package, which sets the four variables below, and then HAVE_NETTLE for
    # config.h; this build has it as a subdirectory, so it sets all five
    # directly and skips the search.
    add_subdirectory("${VENTO_BACKENDS}/platform/android/ports/nettle" nettle)
    set(NETTLE_FOUND TRUE)
    set(NETTLE_LIBRARIES nettle)
    set(HOGWEED_LIBRARIES hogweed)
    set(NETTLE_INCLUDE_DIRS "")
    set(HAVE_NETTLE 1)

    # GnuTLS, which their VeNCrypt is written against, set the same way. Their
    # find module also sets GNUTLS_INCLUDE_DIR, which the target carries
    # instead.
    add_subdirectory("${VENTO_BACKENDS}/platform/android/ports/gnutls" gnutls)
    set(GNUTLS_FOUND TRUE)
    set(GNUTLS_LIBRARIES gnutls)
    set(GNUTLS_INCLUDE_DIR "")
    set(HAVE_GNUTLS 1)

    set(ZLIB_LIBRARIES z)
    set(JPEG_LIBRARIES jpeg-static)
    set(JPEG_INCLUDE_DIR "${THIRD_PARTY}/libjpeg-turbo/src" "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")
    set(PIXMAN_LIBRARIES pixman)
    set(PIXMAN_INCLUDE_DIRS "${THIRD_PARTY}/pixman/pixman" "${CMAKE_CURRENT_BINARY_DIR}/pixman-gen")
else()
    # zlib from the host, found by CMake's module as their own root finds it.
    # The rest are the Android build's libraries, so that a built tree carries
    # them rather than needing a distribution's (whose libjpeg and nettle
    # sonames vary between distributions): nettle and GnuTLS as the host root
    # builds them from the ports' tarballs, under the ports' target names, and
    # so set as in the Android branch; the host root's libjpeg-turbo; and
    # pixman built by the same file as on Android.
    set(NETTLE_FOUND TRUE)
    set(NETTLE_LIBRARIES nettle)
    set(HOGWEED_LIBRARIES hogweed)
    set(NETTLE_INCLUDE_DIRS "")
    set(HAVE_NETTLE 1)
    set(GNUTLS_FOUND TRUE)
    set(GNUTLS_LIBRARIES gnutls)
    set(GNUTLS_INCLUDE_DIR "")
    set(HAVE_GNUTLS 1)
    find_package(ZLIB REQUIRED)
    include(cmake/pixman.cmake)
    set(JPEG_LIBRARIES jpeg-static)
    set(JPEG_INCLUDE_DIR ${VENTO_JPEG_INCLUDE_DIRS})
    set(PIXMAN_LIBRARIES pixman)
    set(PIXMAN_INCLUDE_DIRS "${THIRD_PARTY}/pixman/pixman" "${CMAKE_CURRENT_BINARY_DIR}/pixman-gen")
endif()

# H.264, which their build has two implementations of (FFmpeg's libavcodec and
# Windows' Media Foundation), and this one has a third, on both platforms: the
# context beside this file, which hands each rectangle to vento's own video
# decoders (the platform's, through `vento/video.h`: MediaCodec on Android,
# the Linux client's FFmpeg with its opt-in hardware decoding on a desktop).
# Their `common/rfb/CMakeLists.txt` compiles the decoder itself whenever
# HAVE_H264 is set and adds either of theirs only where it found FFmpeg or is
# building for Windows, neither of which applies here; their factory, which
# picks one of the two, is replaced by ours below. HAVE_LIBAV stays unset,
# since it would compile FFmpeg's context (and with it a second FFmpeg into
# this library), and so do the three results their build compiles the libav
# context on, whatever a cache left behind.
set(HAVE_H264 1)
foreach(lib AVCODEC AVUTIL SWSCALE)
    set(${lib}_FOUND FALSE)
endforeach()

# Neither of the other things config.h can say is true of this build. gettext
# is not in bionic, so every message stays in English and their no-NLS branch
# makes `_()` the identity; the host build does the same, so that the two
# builds do not differ. And audio is their viewer's own PulseAudio or waveOut
# sink, which is not compiled here: the protocol half of it is in `common/`
# regardless, and the sound is played on the platform's output, passed to the
# session as `vento/audio.h`'s table (`VentoAudio.h`).
configure_file("${THIRD_PARTY}/tigervnc/config.h.in" "${CMAKE_CURRENT_BINARY_DIR}/config.h")
add_definitions(-DHAVE_CONFIG_H)

if(ANDROID)
    # Their client library links `pthread` by name on anything UNIX, which is right
    # for glibc before 2.34 and names a library the NDK does not have: bionic's
    # threads are in libc. An empty target by that name is what the link resolves
    # to instead, the same way install and export are neutered above.
    add_library(pthread INTERFACE)
endif()

# Their sub-projects put their own tree on the include path as
# `${CMAKE_SOURCE_DIR}/common`, which under both the Gradle build and the host
# root is a directory that does not exist: a stale -I and nothing worse, as
# long as the real one is here. This directory goes on the path too, and the
# neutral headers for the tables the session and the H.264 context call
# through. libtool control files are for the autotools half of their build,
# which is not used here.
include_directories("${CMAKE_CURRENT_BINARY_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}"
        "${VENTO_BACKENDS}/api/include")
include_directories(SYSTEM "${THIRD_PARTY}/tigervnc/common")
macro(libtool_create_control_file)
endmacro()
add_subdirectory("${THIRD_PARTY}/tigervnc/common" tigervnc-common EXCLUDE_FROM_ALL)
remotedesktop_vendored_warnings("${THIRD_PARTY}/tigervnc/common")

# The H.264 context goes into their library, with the factory their
# decoder calls (`H264DecoderContext::createContext`) and the base class's
# destructor, which are all of their `H264DecoderContext.cxx` and are defined
# beside our context in its place. Inside `rfbclient`, the reference resolves
# however the library ends up linked (the Android shared library, the Linux
# client's, or the static one a probe is built on, where an object of ours
# that only their archive refers to would be dropped before it was looked
# for). Compiled as a target of ours, with our warnings, and only its objects
# handed over. Should their file be renamed or gain anything else, the
# configure stops here or the link fails on a missing or duplicate symbol.
get_target_property(_rfbclient_sources rfbclient SOURCES)
if(NOT "H264DecoderContext.cxx" IN_LIST _rfbclient_sources)
    message(FATAL_ERROR "rfbclient no longer compiles H264DecoderContext.cxx; check what replaces it")
endif()
list(REMOVE_ITEM _rfbclient_sources "H264DecoderContext.cxx")
set_target_properties(rfbclient PROPERTIES SOURCES "${_rfbclient_sources}")
add_library(vento_tigervnc_h264 OBJECT H264VentoDecoderContext.cxx)
remotedesktop_warnings(vento_tigervnc_h264)
target_sources(rfbclient PRIVATE $<TARGET_OBJECTS:vento_tigervnc_h264>)

if(ANDROID)
    # The session, which is the shim over TigerVNC's client and names no
    # platform; the Rust crate at the module's root, which is the backend over
    # the session and the JNI layer every backend shares; and MediaCodec's
    # decoder and AAudio's output, which decode the session's H.264 and play
    # its sound, through the tables the crate passes it (vento_android_library
    # links both).
    add_library(remotedesktop_tigervnc SHARED tigervnc_session.cxx)
    vento_android_library(remotedesktop_tigervnc remotedesktop_tigervnc)
    remotedesktop_warnings(remotedesktop_tigervnc)
    # gnutls by name as well as through theirs: the shim does the certificate
    # check, and calls gnutls itself to describe a verification status and to
    # fingerprint the certificate.
    target_link_libraries(remotedesktop_tigervnc PRIVATE rfbclient rfb network rdr core gnutls)
else()
    # The session, for whatever desktop process links it: the neutral include
    # directory is the interface, and `vento_log` is left to the platform (the
    # probe's `vento_linux`, or the library's own copy of it under
    # VENTO_SHARED; see vento_linux.cmake). The H.264 context is inside
    # `rfbclient`, and decodes with whatever decoders the process passes the
    # session (the Linux client's FFmpeg), and the sound plays on whatever
    # output it passes (the Linux client's PipeWire or ALSA). The probe passes
    # neither, so it never offers H.264 and never asks for sound.
    vento_backend_library(vento_tigervnc tigervnc_session.cxx)
    target_link_libraries(vento_tigervnc ${VENTO_BACKEND_LINK} rfbclient rfb network rdr core
            ${GNUTLS_LIBRARIES})

    vento_backend_probe(vento_tigervnc tigervnc_probe.cxx)
endif()
