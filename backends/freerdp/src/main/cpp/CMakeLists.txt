cmake_minimum_required(VERSION 3.30)
project(remotedesktop_freerdp C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../third_party" ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/freerdp/CMakeLists.txt")
    message(FATAL_ERROR "third_party/freerdp is empty — run `git submodule update --init`")
endif()

include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/reproducible.cmake")
include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/warnings.cmake")

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import this project for its static libraries only; neutering install/export
# stops it warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")

# Answers find_package(OpenSSL) with the port in backends/ports/openssl instead
# of the copy CMake ships, which would either fail against a sysroot that has no
# OpenSSL or find the host's. An override of the command rather than a module on
# CMAKE_MODULE_PATH, because winpr *replaces* that path with FreeRDP's own — and
# it is winpr, in the scope where a module of ours is invisible, that looks for
# OpenSSL a second time and fails the configure when it is not there.
set(SHIM_CMAKE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
macro(find_package name)
    if("${name}" STREQUAL "OpenSSL")
        include("${SHIM_CMAKE_DIR}/FindOpenSSL.cmake")
    else()
        _find_package(${ARGV})
    endif()
endmacro()

# Upstream's own scripts/android-build-freerdp.sh is where this list comes from,
# less everything a phone has no use for. Three of them are load-bearing rather
# than taste: a current OpenSSL keeps RC4, MD4 and MD5 in the legacy provider,
# which this build does not compile, and RDP needs all three on the wire.
foreach(off WITHOUT_FREERDP_3x_DEPRECATED WITH_INTERNAL_RC4 WITH_INTERNAL_MD4
        WITH_INTERNAL_MD5 WITH_SMARTCARD_EMULATE)
    set(${off} ON CACHE BOOL "" FORCE)
endforeach()
foreach(off WITH_CLIENT_SDL WITH_CLIENT WITH_SERVER WITH_SAMPLE WITH_SHADOW WITH_PROXY
        WITH_MANPAGES WITH_OPENH264 WITH_FFMPEG WITH_AAD WITH_SWSCALE WITH_CUPS
        WITH_PCSC WITH_ALSA WITH_PULSE WITH_OSS WITH_WEBVIEW WITH_KRB5 WITH_LIBUSB
        WITH_WINPR_TOOLS WITH_X11 WITH_WAYLAND WITH_DSP_FFMPEG WITH_FUSE)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()
set(WITH_CLIENT_COMMON ON CACHE BOOL "" FORCE)
set(WITH_CLIENT_CHANNELS ON CACHE BOOL "" FORCE)

# H.264 on the phone's own decoder, through the NDK's MediaCodec — which is a
# build switch here and was eighteen hand-written NDK declarations the last time
# this repository wanted the same thing. Nothing is vendored and nothing is
# added to the APK for it.
set(WITH_MEDIACODEC ON CACHE BOOL "" FORCE)

# Their asserts print what failed and then abort. Keep that in a build somebody
# is debugging, where an abort with a message is the finding; a release build
# gets neither, which is the same trade every other borrowed library here makes.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(WITH_VERBOSE_WINPR_ASSERT ON CACHE BOOL "" FORCE)
else()
    set(WITH_VERBOSE_WINPR_ASSERT OFF CACHE BOOL "" FORCE)
endif()

# Six libraries their configure looks for and warns about not finding, each one
# behind an option this build has already answered or a feature a viewer has no
# use for. The search is what is switched off rather than the warning: the answer
# is then the same on a machine that happens to have them installed, which is
# what the NDK toolchain's find-root rules only mostly guarantee.
foreach(pkg PulseAudio Opus uriparser cJSON jansson json-c)
    set(CMAKE_DISABLE_FIND_PACKAGE_${pkg} ON)
endforeach()

add_subdirectory("${THIRD_PARTY}/freerdp" freerdp EXCLUDE_FROM_ALL)

add_library(remotedesktop_freerdp SHARED freerdp_jni.c)
remotedesktop_warnings(remotedesktop_freerdp)
# Their targets carry no usable include directories under add_subdirectory —
# their own build puts them on the path globally — so the four are named here:
# the tree's and, beside each, the one their configure wrote.
target_include_directories(remotedesktop_freerdp SYSTEM PRIVATE
        "${THIRD_PARTY}/freerdp/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/include"
        "${THIRD_PARTY}/freerdp/winpr/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/winpr/include")
target_link_libraries(remotedesktop_freerdp PRIVATE
        freerdp-client freerdp winpr jnigraphics log)
