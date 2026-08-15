# pixman, which TigerVNC's core::Region is a wrapper over and which is the only
# thing in this build that has no CMake of its own — upstream is meson, and a
# meson build under the NDK would be a second toolchain in a build that has
# three already.
#
# The generic C is all of it: the SIMD sources are separate translation units
# with their own compile flags, and every one of them is a fast path for
# compositing, which is not what a region is. What is left needs two generated
# headers and nothing else.

set(PIXMAN_DIR "${THIRD_PARTY}/pixman/pixman")
set(PIXMAN_GEN "${CMAKE_CURRENT_BINARY_DIR}/pixman-gen")

# The version is meson's, so that a moved pin cannot leave a stale number here.
file(READ "${THIRD_PARTY}/pixman/meson.build" _meson)
if(NOT _meson MATCHES "version[ \t]*:[ \t]*'([0-9]+)\\.([0-9]+)\\.([0-9]+)'")
    message(FATAL_ERROR "cannot find the version in third_party/pixman/meson.build")
endif()
set(PIXMAN_VERSION_MAJOR ${CMAKE_MATCH_1})
set(PIXMAN_VERSION_MINOR ${CMAKE_MATCH_2})
set(PIXMAN_VERSION_MICRO ${CMAKE_MATCH_3})
configure_file("${PIXMAN_DIR}/pixman-version.h.in" "${PIXMAN_GEN}/pixman-version.h" @ONLY)

# meson writes this one from its feature tests. Ours says the two things the
# sources actually stop for: a package name, and how the compiler spells thread
# local storage — without which pixman-compiler.h fails the build rather than
# quietly building something that is not thread-safe.
file(WRITE "${PIXMAN_GEN}/pixman-config.h"
        "#define PACKAGE \"pixman\"\n"
        "#define TLS __thread\n")

add_library(pixman STATIC
        "${PIXMAN_DIR}/pixman.c"
        "${PIXMAN_DIR}/pixman-access.c"
        "${PIXMAN_DIR}/pixman-access-accessors.c"
        "${PIXMAN_DIR}/pixman-arm.c"
        "${PIXMAN_DIR}/pixman-bits-image.c"
        "${PIXMAN_DIR}/pixman-combine32.c"
        "${PIXMAN_DIR}/pixman-combine-float.c"
        "${PIXMAN_DIR}/pixman-conical-gradient.c"
        "${PIXMAN_DIR}/pixman-edge.c"
        "${PIXMAN_DIR}/pixman-edge-accessors.c"
        "${PIXMAN_DIR}/pixman-fast-path.c"
        "${PIXMAN_DIR}/pixman-filter.c"
        "${PIXMAN_DIR}/pixman-glyph.c"
        "${PIXMAN_DIR}/pixman-general.c"
        "${PIXMAN_DIR}/pixman-gradient-walker.c"
        "${PIXMAN_DIR}/pixman-image.c"
        "${PIXMAN_DIR}/pixman-implementation.c"
        "${PIXMAN_DIR}/pixman-linear-gradient.c"
        "${PIXMAN_DIR}/pixman-matrix.c"
        "${PIXMAN_DIR}/pixman-mips.c"
        "${PIXMAN_DIR}/pixman-noop.c"
        "${PIXMAN_DIR}/pixman-ppc.c"
        "${PIXMAN_DIR}/pixman-radial-gradient.c"
        "${PIXMAN_DIR}/pixman-region16.c"
        "${PIXMAN_DIR}/pixman-region32.c"
        "${PIXMAN_DIR}/pixman-region64f.c"
        "${PIXMAN_DIR}/pixman-riscv.c"
        "${PIXMAN_DIR}/pixman-solid-fill.c"
        "${PIXMAN_DIR}/pixman-timer.c"
        "${PIXMAN_DIR}/pixman-trap.c"
        "${PIXMAN_DIR}/pixman-utils.c"
        "${PIXMAN_DIR}/pixman-x86.c")

target_compile_definitions(pixman PRIVATE HAVE_CONFIG_H)
target_include_directories(pixman PUBLIC "${PIXMAN_GEN}" "${PIXMAN_DIR}")
