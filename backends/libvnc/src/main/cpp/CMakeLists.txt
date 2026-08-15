cmake_minimum_required(VERSION 3.30)
project(remotedesktop_libvnc C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../third_party" ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/libvncserver/CMakeLists.txt")
    message(FATAL_ERROR "third_party/libvncserver is empty — run `git submodule update --init`")
endif()

include("${CMAKE_CURRENT_SOURCE_DIR}/../../../../cmake/reproducible.cmake")

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

# Shims find_package(JPEG) onto the in-tree libjpeg-turbo below; prepended so it
# wins over the copy CMake ships, which would probe the host.
list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")

# libjpeg-turbo, for Tight — the encoding this backend exists to have. The SIMD
# path on x86 needs NASM, which is a host tool this build does not require; on
# arm64, the one ABI a phone uses, the assembly is GAS and builds.
set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
set(WITH_JAVA OFF CACHE BOOL "" FORCE)
set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
if(CMAKE_ANDROID_ARCH_ABI MATCHES "^x86")
    set(WITH_SIMD OFF CACHE BOOL "" FORCE)
endif()
add_subdirectory("${THIRD_PARTY}/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)

# jconfig.h and jconfigint.h are generated, so the headers are in two places;
# set before add_subdirectory so libvncserver's sources inherit both.
include_directories(
        "${THIRD_PARTY}/libjpeg-turbo/src"
        "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")

# GnuTLS, which is one of the two libraries libvncclient reaches TLS through
# and the one TigerVNC's client requires — so the port is built once and used
# by both. `find_package(GnuTLS)` above finds the shim beside FindJPEG's.
add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../../../../ports/gnutls" gnutls)

# LibVNCServer, of which we build the client half only.
set(WITH_LIBVNCSERVER OFF CACHE BOOL "" FORCE)
set(WITH_LIBVNCCLIENT ON CACHE BOOL "" FORCE)
set(WITH_EXAMPLES OFF CACHE BOOL "" FORCE)
set(WITH_TESTS OFF CACHE BOOL "" FORCE)
set(LIBVNCSERVER_INSTALL OFF CACHE BOOL "" FORCE)
set(WITH_OPENSSL OFF CACHE BOOL "" FORCE)
set(WITH_GNUTLS ON CACHE BOOL "" FORCE)
set(WITH_GCRYPT OFF CACHE BOOL "" FORCE)
set(WITH_SASL OFF CACHE BOOL "" FORCE)
set(WITH_PNG OFF CACHE BOOL "" FORCE)
set(WITH_LZO OFF CACHE BOOL "" FORCE)
set(WITH_SDL OFF CACHE BOOL "" FORCE)
set(WITH_GTK OFF CACHE BOOL "" FORCE)
set(WITH_QT OFF CACHE BOOL "" FORCE)
set(WITH_XCB OFF CACHE BOOL "" FORCE)
set(WITH_FFMPEG OFF CACHE BOOL "" FORCE)
set(WITH_SYSTEMD OFF CACHE BOOL "" FORCE)
set(WITH_LIBSSHTUNNEL OFF CACHE BOOL "" FORCE)
set(WITH_WEBSOCKETS OFF CACHE BOOL "" FORCE)
set(WITH_TIGHTVNC_FILETRANSFER OFF CACHE BOOL "" FORCE)
set(CMAKE_WARN_DEPRECATED OFF CACHE BOOL "" FORCE)
add_subdirectory("${THIRD_PARTY}/libvncserver" libvncserver EXCLUDE_FROM_ALL)

add_library(remotedesktop_libvnc SHARED libvnc_jni.c)
target_include_directories(remotedesktop_libvnc PRIVATE
        "${THIRD_PARTY}/libvncserver/include"
        "${CMAKE_CURRENT_BINARY_DIR}/libvncserver/include")
target_link_libraries(remotedesktop_libvnc PRIVATE vncclient jnigraphics log)
