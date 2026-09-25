cmake_minimum_required(VERSION 3.30)
project(remotedesktop_tigervnc CXX C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../third_party" ABSOLUTE)

foreach(sub tigervnc pixman libjpeg-turbo)
    if(NOT EXISTS "${THIRD_PARTY}/${sub}/README.rst" AND NOT EXISTS "${THIRD_PARTY}/${sub}/meson.build"
            AND NOT EXISTS "${THIRD_PARTY}/${sub}/CMakeLists.txt")
        message(FATAL_ERROR "third_party/${sub} is empty — run `git submodule update --init`")
    endif()
endforeach()

include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/reproducible.cmake")
include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/warnings.cmake")

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import these projects for their static libraries only; neutering
# install/export stops them warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

# AGP configures the debug variant at -O0, which for a JPEG and zlib decoder is
# several times the CPU of a release one — and the hot-path measurements are
# taken on debug builds. A trailing -O2 wins over the toolchain's earlier -O0.
# -DNDEBUG goes with it deliberately: upstream's asserts are invariant checks
# that abort the app, and a measurement build is not where they should fire.
string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")
string(APPEND CMAKE_CXX_FLAGS_DEBUG " -O2 -DNDEBUG")

include(cmake/pixman.cmake)

# libjpeg-turbo, for Tight and for the JPEG rectangles TigerVNC's own encoder
# sends. The SIMD path on x86 needs NASM, which is a host tool this build does
# not require; on arm64, the one ABI a phone uses, it is Neon intrinsics in C.
set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
if(CMAKE_ANDROID_ARCH_ABI MATCHES "^x86")
    set(WITH_SIMD OFF CACHE BOOL "" FORCE)
endif()
add_subdirectory("${THIRD_PARTY}/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)

# TigerVNC's own root CMakeLists is not usable here — it requires X11 on
# anything that is UNIX and not APPLE, which Android is, and finds everything
# else it wants with find_package — so this build takes `common/` on its own,
# answers the variables its sub-projects read, and writes the config.h their
# sources include. That is the whole adaptation: their build files need no
# patch, and the one source line of theirs that does is the branch that picks
# an H.264 decoder context.
set(VERSION "unknown")
file(READ "${THIRD_PARTY}/tigervnc/CMakeLists.txt" _tigervnc_cmake)
if(_tigervnc_cmake MATCHES "set\\(VERSION ([0-9][0-9.]*)\\)")
    set(VERSION ${CMAKE_MATCH_1})
endif()
# Theirs defaults to the time of the build, which nothing in `common/` prints
# but config.h still carries into every translation unit; the same fixed value
# as libjpeg-turbo's, for the same reason.
set(BUILD_TIMESTAMP "0")

# Nettle, which is what their RSA-AES security types are written against and
# the one thing no other client here can do. Their own build finds it with
# find_package, which sets the four variables below, and then HAVE_NETTLE for
# config.h; this build has it as a subdirectory, so it answers all five
# directly and skips the search.
add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../../../../ports/nettle" nettle)
set(NETTLE_FOUND TRUE)
set(NETTLE_LIBRARIES nettle)
set(HOGWEED_LIBRARIES hogweed)
set(NETTLE_INCLUDE_DIRS "")
set(HAVE_NETTLE 1)

# GnuTLS, which is the only thing their VeNCrypt is written against, answered
# the same way. Their find module also sets GNUTLS_INCLUDE_DIR, which the
# target carries for us.
add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../../../../ports/gnutls" gnutls)
set(GNUTLS_FOUND TRUE)
set(GNUTLS_LIBRARIES gnutls)
set(GNUTLS_INCLUDE_DIR "")
set(HAVE_GNUTLS 1)

set(ZLIB_LIBRARIES z)
set(JPEG_LIBRARIES jpeg-static)
set(JPEG_INCLUDE_DIR "${THIRD_PARTY}/libjpeg-turbo/src" "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")
set(PIXMAN_LIBRARIES pixman)
set(PIXMAN_INCLUDE_DIRS "${THIRD_PARTY}/pixman/pixman" "${CMAKE_CURRENT_BINARY_DIR}/pixman-gen")

# H.264, which their build has two implementations of — FFmpeg's libavcodec and
# Windows' Media Foundation — and this one has a third. Their
# `common/rfb/CMakeLists.txt` compiles the decoder itself whenever ENABLE_H264
# is on and adds either of theirs only where it found FFmpeg or is building for
# Windows, neither of which is true here; the context class is in this
# directory, H264_ANDROID is what selects it, and the six lines that do are the
# one patch it takes. HAVE_LIBAV stays unset, since that is what would ask for
# FFmpeg's.
#
# So the whole of H.264 here is the platform's own decoder through the NDK's C
# API: nothing vendored, nothing added to the APK, and it runs on the hardware
# every other video on the phone runs on.
set(ENABLE_H264 ON)
set(HAVE_H264 1)
add_definitions(-DH264_ANDROID)
set(H264_LIBRARIES mediandk)

# Neither of the other things config.h can say is true of this build: gettext
# is not in bionic, so every message stays in English and their no-NLS branch
# makes `_()` the identity; and audio is their viewer's own PulseAudio or
# waveOut sink, which is not compiled here — the protocol half of it is in
# `common/` whatever this says, and the sink is AndroidAudioOutput.
configure_file("${THIRD_PARTY}/tigervnc/config.h.in" "${CMAKE_CURRENT_BINARY_DIR}/config.h")
add_definitions(-DHAVE_CONFIG_H)

# Their client library links `pthread` by name on anything UNIX, which is right
# for glibc before 2.34 and names a library the NDK does not have: bionic's
# threads are in libc. An empty target by that name is what the link resolves
# to instead, the same way install and export are neutered above.
add_library(pthread INTERFACE)

# Their sub-projects put their own tree on the include path as
# `${CMAKE_SOURCE_DIR}/common`, which under this build is a directory that does
# not exist — a stale -I and nothing worse, as long as the real one is here.
# libtool control files are for the autotools half of their build, of which
# there is none here.
include_directories("${CMAKE_CURRENT_BINARY_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}")
include_directories(SYSTEM "${THIRD_PARTY}/tigervnc/common")
macro(libtool_create_control_file)
endmacro()
add_subdirectory("${THIRD_PARTY}/tigervnc/common" tigervnc-common EXCLUDE_FROM_ALL)

add_library(remotedesktop_tigervnc SHARED tigervnc_jni.cxx H264AndroidDecoderContext.cxx
        AndroidAudioOutput.cxx)
remotedesktop_warnings(remotedesktop_tigervnc)
# gnutls by name as well as through theirs: the certificate check is the
# shim's now, and it calls the known-hosts functions itself.
target_link_libraries(remotedesktop_tigervnc PRIVATE rfbclient rfb network rdr core gnutls
        jnigraphics log mediandk aaudio)
