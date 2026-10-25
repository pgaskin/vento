# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and builds `libremotedesktop_realvnc.so`: the only native code in
# the phone's side of this module, the estimator read in `realvnc_traffic.c`.
# It links against nothing: the viewer core is already in the process, and
# the read goes through a pointer obtained from the core's own Java rather
# than through a symbol of the core's. Under `backends/CMakeLists.txt` it is a
# subdirectory on a Linux host and builds `vento_realvnc`: the session in
# `realvnc_session.cxx` over the hosting layer in `host/`, which runs the
# core's Android build in a glibc process, with a probe in front of it.
#
# The host side needs clang: libc-shim does not compile under GCC (its
# `vector::insert` overloads trip -Wchanges-meaning, as of GCC 16), and the
# root skips this module under any other compiler. At run time it needs the
# two libraries from the viewer's APK (linux/claude-notes.md, section
# "RealVNC", says how to extract them), and nothing of RealVNC's at build
# time.
#
# The four submodules below are used only by the host side and are in no APK
# or licence page; each is pinned at the commit the hosting layer was tested
# with:
#
#   logger               minecraft-linux/logger          The Unlicense (public domain)
#   libc-shim            minecraft-linux/libc-shim       no licence file; GitHub lists none.
#                                                        Two bionic-derived headers in it carry
#                                                        AOSP's Apache-2.0 and BSD notices.
#   mcpelauncher-linker  minecraft-linux/mcpelauncher-linker  MIT (its own files); its nested
#                                                        `bionic` and `core` are AOSP, Apache-2.0
#                                                        and BSD
#   libjnivm             ChristopherHX/libjnivm          MIT

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_realvnc C CXX)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)
get_filename_component(REPO "${CMAKE_CURRENT_SOURCE_DIR}/../../../.." ABSOLUTE)

if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")

if(ANDROID)
    add_library(remotedesktop_realvnc SHARED realvnc_traffic.c)
    remotedesktop_warnings(remotedesktop_realvnc)
else()
    include("${VENTO_BACKENDS}/platform/linux/cmake/vento_linux.cmake")

    # Off by default. The jnivm trace prints every JNI call the core makes,
    # which shows a class or method missing from the stand-ins in
    # `host/Classes.cxx` (the line `Call Unknown ... Class=... Method=...
    # Signature=...` before the core's own assertion says exactly what to
    # add), but it is several thousand lines per session.
    option(VENTO_REALVNC_TRACE "Trace every JNI call the RealVNC core makes" OFF)

    # Position-independent, as the other three modules set for their trees:
    # the four submodules below are static archives, and under VENTO_SHARED
    # they are linked into a shared library.
    set(CMAKE_POSITION_INDEPENDENT_CODE ON)

    foreach(sub logger libc-shim mcpelauncher-linker libjnivm)
        if(NOT EXISTS "${THIRD_PARTY}/${sub}/CMakeLists.txt")
            message(FATAL_ERROR "third_party/${sub} is empty — run `git submodule update --init --recursive`")
        endif()
    endforeach()
    if(NOT EXISTS "${THIRD_PARTY}/mcpelauncher-linker/bionic/linker/linker.cpp")
        message(FATAL_ERROR "third_party/mcpelauncher-linker's own submodules are empty — run `git submodule update --init --recursive`")
    endif()

    # The submodules, unmodified: the logger first, since libc-shim and jnivm
    # link it by name. jnivm's garbage collection and debug codegen stay at
    # their defaults, as the hosting layer was tested with.
    set(JNIVM_ENABLE_TRACE ${VENTO_REALVNC_TRACE} CACHE BOOL "" FORCE)
    add_subdirectory("${THIRD_PARTY}/logger" logger EXCLUDE_FROM_ALL)
    # libc-shim links `atomic` by name where its 24-byte atomics need it,
    # which is libatomic.so.1 at run time: a GCC library, for a few lock-based
    # functions, that a desktop may not have installed (Debian and Ubuntu
    # images do not). Where the compiler has a static libatomic.a, it
    # satisfies the name instead; it is position-independent on x86_64, so it
    # links into the shared library, and `--exclude-libs` keeps it out of the
    # exports. Where there is none (a distribution that packages it
    # separately, as Fedora does in libatomic-static), the linker resolves the
    # name to the shared library.
    execute_process(COMMAND ${CMAKE_C_COMPILER} -print-file-name=libatomic.a
            OUTPUT_VARIABLE _atomic OUTPUT_STRIP_TRAILING_WHITESPACE)
    if(IS_ABSOLUTE "${_atomic}" AND EXISTS "${_atomic}" AND NOT TARGET atomic)
        add_library(atomic STATIC IMPORTED)
        set_target_properties(atomic PROPERTIES IMPORTED_LOCATION "${_atomic}")
    endif()
    add_subdirectory("${THIRD_PARTY}/libc-shim" libc-shim EXCLUDE_FROM_ALL)
    add_subdirectory("${THIRD_PARTY}/mcpelauncher-linker" mcpelauncher-linker EXCLUDE_FROM_ALL)
    add_subdirectory("${THIRD_PARTY}/libjnivm" libjnivm EXCLUDE_FROM_ALL)
    foreach(sub logger libc-shim mcpelauncher-linker libjnivm)
        remotedesktop_vendored_warnings("${THIRD_PARTY}/${sub}")
    endforeach()

    # The pins, read from the only place they are stated: `Library.java` holds
    # the hashes of the build the declarations were read from, and the session
    # refuses any other. Generated at configure time so that a changed pin
    # cannot leave a stale copy here.
    set(LIBRARY_JAVA "${REPO}/realvnc/src/main/java/com/realvnc/vncviewer/jni/Library.java")
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${LIBRARY_JAVA}")
    file(READ "${LIBRARY_JAVA}" _library_java)
    if(NOT _library_java MATCHES "VERSION = \"([0-9.]+)\"")
        message(FATAL_ERROR "no VERSION in ${LIBRARY_JAVA}")
    endif()
    set(_pin_version "${CMAKE_MATCH_1}")
    set(_pins "// Generated by this module's CMakeLists from Library.java; do not edit.\n")
    string(APPEND _pins "#define VENTO_REALVNC_VERSION \"${_pin_version}\"\n")
    foreach(abi arm64-v8a x86_64)
        foreach(lib libvncviewer libc++_shared)
            # Finds the map key, then the first quoted hex string after it, to
            # avoid a regex over a name containing `++`.
            string(FIND "${_library_java}" "\"${abi}/${lib}.so\"," _at)
            if(_at LESS 0)
                message(FATAL_ERROR "no SHA-256 for ${abi}/${lib}.so in ${LIBRARY_JAVA}")
            endif()
            string(SUBSTRING "${_library_java}" ${_at} 200 _tail)
            string(REGEX MATCH "\"([0-9a-f]+)\"[ \n\r]*[,)]" _hash "${_tail}")
            set(_hash "${CMAKE_MATCH_1}")
            string(LENGTH "${_hash}" _hash_length)
            if(NOT _hash_length EQUAL 64)
                message(FATAL_ERROR "no SHA-256 for ${abi}/${lib}.so in ${LIBRARY_JAVA}")
            endif()
            string(TOUPPER "${abi}_${lib}" _name)
            string(REPLACE "-" "_" _name "${_name}")
            string(REPLACE "++" "XX" _name "${_name}")
            string(APPEND _pins "#define VENTO_REALVNC_SHA256_${_name} \"${_hash}\"\n")
        endforeach()
    endforeach()
    file(CONFIGURE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/gen/realvnc_pins.h" CONTENT "${_pins}")

    # The session and the hosting layer, for any desktop process linking
    # them: the neutral include directory is the interface, and `vento_log`
    # comes from the platform (the probe's `vento_linux`, or the library's own
    # copy of it under VENTO_SHARED; see vento_linux.cmake). The hosting
    # layer's headers are private and not on the interface.
    vento_backend_library(vento_realvnc GLOBAL realvnc_session.cxx
            host/DesktopCore.cxx host/Loader.cxx host/Looper.cxx host/Classes.cxx host/Files.cxx)
    target_compile_features(vento_realvnc PRIVATE cxx_std_17)
    target_include_directories(vento_realvnc PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/host" "${CMAKE_CURRENT_BINARY_DIR}/gen")
    target_include_directories(vento_realvnc SYSTEM PRIVATE
            "${THIRD_PARTY}/libjnivm/include" "${THIRD_PARTY}/libc-shim/include"
            "${THIRD_PARTY}/mcpelauncher-linker/public_include")
    target_link_libraries(vento_realvnc ${VENTO_BACKEND_LINK} linker libc-shim logger jnivm
            ${CMAKE_DL_LIBS} pthread)

    vento_backend_probe(vento_realvnc realvnc_probe.cxx)

    # Tests for the hosting layer's resolver wrappers (realvnc_test.cxx), over
    # libc-shim's own resolver.
    if(VENTO_PROBES)
        add_executable(vento_realvnc_test realvnc_test.cxx)
        remotedesktop_warnings(vento_realvnc_test)
        target_compile_features(vento_realvnc_test PRIVATE cxx_std_17)
        target_include_directories(vento_realvnc_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/host")
        target_include_directories(vento_realvnc_test SYSTEM PRIVATE
                "${THIRD_PARTY}/libc-shim/include")
        target_link_libraries(vento_realvnc_test PRIVATE libc-shim)
        add_test(NAME vento_realvnc_test COMMAND vento_realvnc_test)
    endif()
endif()
