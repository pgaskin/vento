# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root project, configured by
# the NDK toolchain, and builds `libremotedesktop_freerdp.so`: the session, the
# module's Rust crate linked in as the JNI layer over it, and the OpenSSL port.
# Under `backends/CMakeLists.txt` on a Linux host it is a subdirectory and
# builds `vento_freerdp`, the session alone, with the OpenSSL the host root
# builds from the same submodule as the port, libswscale for the graphics
# pipeline's scaling, and a probe.
# H.264 goes to vento's decoders on both (MediaCodec on Android, FFmpeg on
# Linux) through the subsystem in h264_vento.c. Sound goes to vento's output and
# its AAC and Opus decoders on both (AAudio and MediaCodec on Android; a
# PipeWire stream or ALSA's default device, and FFmpeg, on Linux) through the
# rdpsnd device in rdpsnd_vento.c. The FreeRDP configuration in the middle is
# shared, and the platform-specific parts around it are `if(ANDROID)` and
# `else()`, so the Android build keeps the sequence of statements it had.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_freerdp C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/freerdp/CMakeLists.txt")
    message(FATAL_ERROR "third_party/freerdp is empty — run `git submodule update --init`")
endif()

# The Android library is built reproducibly so a build elsewhere matches; a
# host build keeps its real paths for gdb.
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
if(ANDROID)
    include("${VENTO_BACKENDS}/platform/android/cmake/vento_android.cmake")
else()
    include("${VENTO_BACKENDS}/platform/linux/cmake/vento_linux.cmake")
endif()

# One shared library per protocol, as in every other backend module, so a
# build without this protocol drops all of it. Everything below is linked into
# it statically, and into the host's `vento_freerdp` the same way.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# FreeRDP is used for its static libraries only; no-op install/export macros
# stop it warning about targets that are never installed.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
    string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")
endif()

# Answers find_package(OpenSSL) with the port in
# backends/platform/android/ports/openssl on Android, and on the host with the
# host root's build of the same submodule (platform/linux/cmake/openssl.cmake),
# instead of CMake's own module, which would either fail against a sysroot
# without OpenSSL or find the host's. This overrides the command rather than
# adding a module to CMAKE_MODULE_PATH because winpr *replaces* that path with
# FreeRDP's own, and winpr, in the scope where our module would be invisible,
# looks for OpenSSL a second time and fails the configure if it is missing.
if(ANDROID)
    set(SHIM_CMAKE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
else()
    set(SHIM_CMAKE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake/host")
endif()
macro(find_package name)
    if("${name}" STREQUAL "OpenSSL")
        include("${SHIM_CMAKE_DIR}/FindOpenSSL.cmake")
    else()
        _find_package(${ARGV})
    endif()
endmacro()

# This list comes from upstream's Android build
# (client/Android/cmake/ExternalFreeRDP.cmake), minus what a phone doesn't use.
# Two of them are required rather than preferences: a current OpenSSL keeps
# RC4 and MD4 in the legacy provider, which neither this build's OpenSSL
# (configured `no-legacy`) nor a distribution's library loads, and RDP needs
# both on the wire. MD5 is in the default provider, so WITH_INTERNAL_MD5 is
# kept only because upstream's Android build sets it. The last is ours: JSON
# is used for Azure sign-in and for reading keyboard layouts and timezones
# from files, none of which this build has, and setting it stops their
# configure from looking for a JSON parser.
foreach(off WITHOUT_FREERDP_3x_DEPRECATED WITHOUT_WINPR_3x_DEPRECATED WITH_INTERNAL_RC4
        WITH_INTERNAL_MD4 WITH_INTERNAL_MD5 WITH_SMARTCARD_EMULATE WITH_JSON_DISABLED)
    set(${off} ON CACHE BOOL "" FORCE)
endforeach()
# Implied by WITHOUT_FREERDP_3x_DEPRECATED: their include/CMakeLists.txt caches
# it when it sees that option, but only after their root has written every
# visible WITH_* variable into the library's build-configuration string
# (`freerdp_get_build_config`), so the string, and with it the library's bytes,
# changed on the second configure. Caching it here puts it in the string from
# the first configure.
set(WITH_OPAQUE_SETTINGS ON CACHE INTERNAL "")
# Opus and uriparser each fall back to pkg-config when the package search finds
# nothing, which can find the host's copy; turning them off explicitly is the
# only setting independent of what the build machine has installed.
foreach(off WITH_CLIENT_SDL WITH_CLIENT WITH_SERVER WITH_SAMPLE WITH_SHADOW WITH_PROXY
        WITH_MANPAGES WITH_AAD WITH_CUPS WITH_PCSC WITH_OSS WITH_KRB5 WITH_WINPR_TOOLS WITH_X11
        WITH_WAYLAND WITH_FUSE WITH_OPUS WITH_URIPARSER)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()
set(WITH_CLIENT_COMMON ON CACHE BOOL "" FORCE)
set(WITH_CLIENT_CHANNELS ON CACHE BOOL "" FORCE)

# H.264 is decoded by the platform's decoders, which the session is given
# (vento/video.h), through this module's subsystem (h264_vento.c), in the slot
# patches/freerdp adds for a subsystem linked in by the application. None of
# the library's own subsystems is built, so there is a single H.264 decoder,
# the one RustDesk and TigerVNC use, with the same hardware decoding and
# fallback. The library's code above the decoder (AVC444's combining of two
# pictures, the conversion into the surface) is upstream's, unchanged.
set(WITH_H264_EXTERNAL ON CACHE BOOL "" FORCE)

# Sound is played on the platform's output, which the session is given
# (vento/audio.h), through this module's device, which the channel finds by
# name (rdpsnd_vento.h). TigerVNC's sound uses the same queue and policy. Of
# the library's own devices only the one that discards every sample is built
# (it is built regardless of these options), and a session with sound off or
# played on the remote machine uses it. None of the sound libraries the others
# would link (PulseAudio and the fifteen libraries behind it, ALSA, OSS, sndio,
# OpenSL ES, AAudio) is looked for, except OpenSL ES on Android, which is the
# microphone's: the library has no capture device of ours, and the channel
# only loads (and the device only opens) when the session asks for the
# microphone. Its playback device is built with it but never named, so sound
# still goes through ours.
#
# AAC and Opus from the server are decoded by the platform's decoders in the
# same table, so the library's DSP only handles PCM and is built without
# FFmpeg (`WITH_DSP_FFMPEG`) on both platforms: the device claims those formats
# itself and receives the packets unchanged.
foreach(off WITH_PULSE WITH_ALSA WITH_OSS WITH_SNDIO WITH_AAUDIO WITH_DSP_FFMPEG)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()

set(WITH_OPENSLES ${ANDROID} CACHE BOOL "" FORCE)

if(ANDROID)
    # No FFmpeg on Android, so the codec libraries are turned off explicitly.
    # H.264 uses MediaCodec through vento's decoders, so FreeRDP's own
    # MediaCodec subsystem is off too.
    foreach(off WITH_OPENH264 WITH_FFMPEG WITH_SWSCALE WITH_MEDIACODEC)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
else()
    # Only libswscale from FFmpeg. The library has nothing left to decode
    # (H.264 and sound go to vento's decoders), but its graphics pipeline uses
    # it to scale a surface the server maps onto a scaled output
    # (`freerdp_image_scale`), and only offers the capability versions that
    # allow this (10.5 and 10.6, which Windows 10 confirms) when it has a
    # scaler (rdpgfx_main.c). Without it a session is EGFX 10.4, as on
    # Android. libavcodec is not linked (`WITH_FFMPEG` off; their swscale
    # branch would also set it for the camera channel's MJPEG if it finds
    # libavcodec).
    set(WITH_SWSCALE ON CACHE BOOL "" FORCE)
    foreach(off WITH_FFMPEG WITH_VIDEO_FFMPEG WITH_FFMPEG_HWACCEL WITH_OPENH264)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
    # Under the host root the vendored trees share one cache, and libvncserver
    # has its own WITH_OPENSSL, WITH_JPEG and WITH_PNG with the opposite
    # values. FreeRDP's values are forced here, immediately before its tree
    # reads them, as the libvnc module forces its own.
    set(WITH_OPENSSL ON CACHE BOOL "" FORCE)
    set(WITH_JPEG OFF CACHE BOOL "" FORCE)
    set(WITH_PNG OFF CACHE BOOL "" FORCE)
    # The rest of what their Linux configure looks for and this build doesn't
    # use, off explicitly for the same reason as Opus and uriparser above:
    # journal logging, a PKCS#11 module, USB redirection, a clang-format
    # target, and the web view.
    foreach(off WITH_LIBSYSTEMD WITH_PKCS11 CHANNEL_URBDRC WITH_CLANG_FORMAT WITH_WEBVIEW)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
    # WinPR's own UTF-8/UTF-16 conversion (crt/unicode_builtin.c) instead of
    # ICU's, which is the only thing their Linux build uses ICU for (ICU for
    # timezone mapping is off by default and stays off). The conversions cover
    # clipboard text and protocol strings (names, the domain, the
    # credentials) between UTF-8 and RDP's UTF-16, which needs no locale data.
    # ICU was four libraries (38 MB, 32 of them data) required at the major
    # version of the build machine's distribution, and the first library
    # missing on other machines after libasound. The Android build is
    # unchanged: it converts through Java's String (crt/unicode_android.c),
    # which links nothing.
    set(WITH_UNICODE_BUILTIN ON CACHE BOOL "" FORCE)
    # Their FindFFmpeg looks for eight libraries and skips the search when
    # FFMPEG_LIBRARIES is cached. When the Linux client's build provides a
    # static FFmpeg (VENTO_FFMPEG_PREFIX, vento_linux.cmake), the two libraries
    # the scaler needs are cached here as the results that module would have
    # found, and the other six as not found, so none of the host's FFmpeg is
    # used (including libavcodec, libavformat, libavfilter and libavdevice,
    # which it would otherwise find and link).
    if(TARGET vento_ffmpeg::swscale)
        set(_ffmpeg "")
        foreach(lib SWSCALE AVUTIL)
            string(TOLOWER ${lib} name)
            set(${lib}_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE PATH "" FORCE)
            set(${lib}_LIBRARIES vento_ffmpeg::${name} CACHE STRING "" FORCE)
            list(APPEND _ffmpeg vento_ffmpeg::${name})
        endforeach()
        set(FFMPEG_LIBRARIES "${_ffmpeg}" CACHE STRING "" FORCE)
        set(FFMPEG_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE STRING "" FORCE)
        foreach(lib AVCODEC SWRESAMPLE AVFORMAT AVFILTER AVDEVICE POSTPROCESS)
            unset(${lib}_LIBRARIES CACHE)
            unset(${lib}_INCLUDE_DIRS CACHE)
        endforeach()
        unset(AVCODEC_VERSION CACHE)
    endif()
endif()

# Takes the version from the hard-coded string in their
# cmake/GetProjectVersion.cmake instead of `git describe`, which also adds a
# short commit hash whose length depends on the clone. The string is the
# release's, so the reported version is the same either way; only the revision
# becomes "n/a".
set(USE_VERSION_FROM_GIT_TAG OFF CACHE BOOL "" FORCE)

# Their verbose asserts print the failure and then abort. Debug builds keep
# them, since an abort with a message is useful there; release builds get
# neither, as with the other vendored libraries.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(WITH_VERBOSE_WINPR_ASSERT ON CACHE BOOL "" FORCE)
else()
    set(WITH_VERBOSE_WINPR_ASSERT OFF CACHE BOOL "" FORCE)
endif()

# Their configure looks for these libraries, though each is behind an option
# already turned off above: Opus and uriparser warn when they are missing, and
# AAudio, looked for on Android only, sets WITH_AAUDIO's default. Disabling the
# search rather than the warning gives the same result on a machine that has
# them installed, which the NDK toolchain's find-root rules only mostly
# guarantee. The JSON libraries need no such line, since WITH_JSON_DISABLED
# skips their detection.
foreach(pkg Opus uriparser AAudio)
    set(CMAKE_DISABLE_FIND_PACKAGE_${pkg} ON)
endforeach()

# No link-time optimisation, so the library is the same however many times its
# tree has been configured. Their cmake/CommonConfigOptions.cmake makes
# CMAKE_INTERPROCEDURAL_OPTIMIZATION an option() defaulting to whether the
# compiler supports it, and an option() is a cache entry: on a tree's first
# configure it doesn't exist yet when targets defined before theirs read it
# (the OpenSSL port here, libvncserver's and TigerVNC's under the host root),
# and on later configures it does, so which targets were built with
# -flto=thin, and with that the library's bytes, depended on the tree's
# history. A forced cache entry sets it before their option() is reached,
# which then does nothing. The value is off, which is also what a target
# defined before this line sees on a first configure, so every target is built
# the same way regardless of the cache, including those of every tree under
# the host root, which share this cache.
set(CMAKE_INTERPROCEDURAL_OPTIMIZATION OFF CACHE BOOL "" FORCE)

add_subdirectory("${THIRD_PARTY}/freerdp" freerdp EXCLUDE_FROM_ALL)
remotedesktop_vendored_warnings("${THIRD_PARTY}/freerdp")

# Their targets have no usable include directories under add_subdirectory
# (their own build adds them globally), so the four are listed here: the
# source tree's two, each followed by the one their configure generates.
set(FREERDP_INCLUDES
        "${THIRD_PARTY}/freerdp/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/include"
        "${THIRD_PARTY}/freerdp/winpr/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/winpr/include")
# The H.264 subsystem uses the library's declaration of the interface, which
# is kept with its subsystems rather than in its public headers, so only that
# file gets the include directory.
set_source_files_properties(h264_vento.c PROPERTIES INCLUDE_DIRECTORIES
        "${THIRD_PARTY}/freerdp/libfreerdp/codec")

if(ANDROID)
    # The session (the platform-independent shim over FreeRDP's client) and
    # the Rust crate at the module's root (the backend over the session, and
    # the shared JNI layer). OpenSSL's headers come from the port for the TLS
    # version constants, as SYSTEM like the rest, because linking the port
    # would add them as non-system include directories.
    add_library(remotedesktop_freerdp SHARED freerdp_session.c h264_vento.c rdpsnd_vento.c)
    vento_android_library(remotedesktop_freerdp remotedesktop_freerdp)
    remotedesktop_warnings(remotedesktop_freerdp)
    target_include_directories(remotedesktop_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
            "$<TARGET_PROPERTY:openssl,INTERFACE_INCLUDE_DIRECTORIES>")
    target_link_libraries(remotedesktop_freerdp PRIVATE freerdp-client freerdp winpr)

    # A library can have only one `JNI_OnLoad`, and WinPR has its own, which
    # keeps the `JavaVM` it uses to read the time zone. WinPR's is compiled
    # under another name, and the crate's (which registers the natives) calls
    # it first (`natives!`'s `on_load`, in `src/lib.rs`).
    # This is a definition on that one source file rather than a patch, so
    # jni.h's declaration is renamed with it and the two still match.
    set_property(SOURCE "${THIRD_PARTY}/freerdp/winpr/libwinpr/utils/android.c"
            TARGET_DIRECTORY winpr APPEND PROPERTY COMPILE_DEFINITIONS JNI_OnLoad=winpr_JNI_OnLoad)
else()
    # The session alone, for the desktop process that links it: the
    # platform-independent include directory is its interface, and `vento_log`
    # comes from the platform (the probe's `vento_linux`, or the library's own
    # copy under VENTO_SHARED, vento_linux.cmake). OpenSSL's headers come from
    # the host root's build, for the same constants.
    vento_backend_library(vento_freerdp freerdp_session.c h264_vento.c rdpsnd_vento.c)
    target_include_directories(vento_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
            "$<TARGET_PROPERTY:openssl,INTERFACE_INCLUDE_DIRECTORIES>")
    # WLog's lines through `vento_log` rather than its console appender's
    # layout on stdout and stderr (`routeWLog`).
    target_compile_definitions(vento_freerdp PRIVATE VENTO_FREERDP_WLOG)
    target_link_libraries(vento_freerdp ${VENTO_BACKEND_LINK} freerdp-client freerdp winpr)

    vento_backend_probe(vento_freerdp freerdp_probe.c)

    # Tests for the session's pure parts (freerdp_test.c), which include the
    # three sources instead of linking the library, and wrap the two input
    # calls used for keys.
    if(VENTO_PROBES)
        add_executable(vento_freerdp_test freerdp_test.c)
        remotedesktop_warnings(vento_freerdp_test)
        target_include_directories(vento_freerdp_test PRIVATE "${_VENTO_BACKENDS}/api/include"
                "${CMAKE_CURRENT_SOURCE_DIR}/include")
        target_include_directories(vento_freerdp_test SYSTEM PRIVATE ${FREERDP_INCLUDES}
                "$<TARGET_PROPERTY:openssl,INTERFACE_INCLUDE_DIRECTORIES>"
                "${THIRD_PARTY}/freerdp/libfreerdp/codec")
        target_link_libraries(vento_freerdp_test PRIVATE freerdp-client freerdp winpr vento_linux)
        target_link_options(vento_freerdp_test PRIVATE
                "LINKER:--wrap=freerdp_input_send_keyboard_event"
                "LINKER:--wrap=freerdp_input_send_unicode_keyboard_event")
        add_test(NAME vento_freerdp_test COMMAND vento_freerdp_test)
    endif()
endif()
