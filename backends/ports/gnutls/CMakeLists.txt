# GnuTLS as a set of static libraries, built from the release tarball in
# third_party/gnutls plus the configure output in gen/ (see gen.sh). One target
# per Makefile upstream has, because the include path is what differs between
# them and that is what a Makefile is here.
#
# `gnutls` is the one a consumer links: an interface over the lot of them, with
# the public headers — one of which is generated — on its include path.

set(GNUTLS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../third_party/gnutls")
if(NOT EXISTS "${GNUTLS_DIR}/lib/gnutls.h.in" AND NOT EXISTS "${GNUTLS_DIR}/lib/global.c")
    message(FATAL_ERROR "third_party/gnutls is empty — run `./gradlew :backends:fetchGnutlsSources`")
endif()

set(GEN "${CMAKE_CURRENT_SOURCE_DIR}/gen")
# Four answers depend on the ABI — the width of a wchar_t, the layout of a long
# double — so those files are generated per ABI and this directory comes first.
set(GEN_ABI "${GEN}/${CMAKE_ANDROID_ARCH_ABI}")
if(NOT EXISTS "${GEN_ABI}/config.h")
    message(FATAL_ERROR "no gnutls config.h for ${CMAKE_ANDROID_ARCH_ABI}; add the ABI to gen.sh")
endif()

include("${GEN}/sources.cmake")

# gen.sh names every source the way upstream's build does, relative to the tree
# it unpacked from.
foreach(part gl lib lib_accelerated lib_algorithms lib_auth lib_ext lib_extras
        lib_hpke lib_minitasn1 lib_nettle lib_unistring lib_x509)
    list(TRANSFORM GNUTLS_SOURCES_${part} PREPEND "${GNUTLS_DIR}/")
endforeach()

if(NOT TARGET nettle)
    add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../nettle" nettle)
endif()

# The gnulib half. It is compiled with its own directory and the build root on
# the path and nothing else, which is what keeps its replacement headers — a
# stdint.h, a string.h — from reaching anything that did not ask for them.
add_library(gnutls_gl STATIC ${GNUTLS_SOURCES_gl})
target_include_directories(gnutls_gl PRIVATE
        "${GEN_ABI}" "${GEN}" "${GEN_ABI}/gl" "${GEN}/gl" "${GNUTLS_DIR}/gl")

# Their bundled unistring, likewise compiled against itself alone.
add_library(gnutls_unistring STATIC ${GNUTLS_SOURCES_lib_unistring})
target_include_directories(gnutls_unistring PRIVATE
        "${GEN_ABI}" "${GEN}" "${GEN_ABI}/lib/unistring" "${GEN}/lib/unistring"
        "${GNUTLS_DIR}/lib/unistring")

# Their bundled libtasn1, which is what parses a certificate.
add_library(gnutls_minitasn1 STATIC ${GNUTLS_SOURCES_lib_minitasn1})
target_include_directories(gnutls_minitasn1 PRIVATE
        "${GEN_ABI}" "${GEN}" "${GEN}/gl" "${GNUTLS_DIR}/gl"
        "${GNUTLS_DIR}/lib" "${GNUTLS_DIR}/lib/minitasn1")
target_compile_definitions(gnutls_minitasn1 PRIVATE ASN1_BUILDING=1)

# Everything else in lib/ shares one include set: gnulib, the public headers,
# the private ones at the root of lib/, and libtasn1.
set(GNUTLS_LIB_INCLUDES
        "${GEN_ABI}" "${GEN}" "${GEN}/gl" "${GNUTLS_DIR}/gl"
        "${GEN}/lib/includes" "${GNUTLS_DIR}/lib/includes"
        "${GNUTLS_DIR}/lib" "${GNUTLS_DIR}/lib/x509" "${GNUTLS_DIR}/lib/minitasn1"
        "${GEN}/lib/unistring" "${GNUTLS_DIR}/lib/unistring")

foreach(part lib lib_accelerated lib_algorithms lib_auth lib_ext lib_extras
        lib_hpke lib_nettle lib_x509)
    add_library(gnutls_${part} STATIC ${GNUTLS_SOURCES_${part}})
    # Its own directory too, as automake gives every Makefile.
    string(REPLACE "_" "/" dir "${part}")
    target_include_directories(gnutls_${part} PRIVATE
            "${GNUTLS_DIR}/${dir}" ${GNUTLS_LIB_INCLUDES})
    target_compile_definitions(gnutls_${part} PRIVATE GNUTLS_BUILDING_LIB=1)
    target_link_libraries(gnutls_${part} PRIVATE nettle hogweed)
endforeach()

# The nettle backend has two directories of its own beside the shared set: the
# integer helpers, and the backports of what a newer nettle would have.
target_include_directories(gnutls_lib_nettle PRIVATE
        "${GNUTLS_DIR}/lib/nettle/int" "${GNUTLS_DIR}/lib/nettle/backport")

# lib/ itself is the only part that reads either of these. The priority file is
# the one an administrator writes to change a system's TLS defaults; naming a
# path that cannot exist on Android is how this build says there is none.
target_compile_definitions(gnutls_lib PRIVATE
        SYSTEM_PRIORITY_FILE="/etc/gnutls/config"
        LOCALEDIR="/usr/share/locale")

foreach(target gnutls_gl gnutls_unistring gnutls_minitasn1 gnutls_lib
        gnutls_lib_accelerated gnutls_lib_algorithms gnutls_lib_auth
        gnutls_lib_ext gnutls_lib_extras gnutls_lib_hpke gnutls_lib_nettle
        gnutls_lib_x509)
    target_compile_definitions(${target} PRIVATE HAVE_CONFIG_H)
    # Upstream's build turns these off; they are somebody else's warnings.
    target_compile_options(${target} PRIVATE
            -Wno-unused-parameter -Wno-sign-compare -Wno-unused-but-set-variable)
endforeach()

add_library(gnutls INTERFACE)
target_include_directories(gnutls INTERFACE
        "${GEN}/lib/includes" "${GNUTLS_DIR}/lib/includes")
target_link_libraries(gnutls INTERFACE
        gnutls_lib gnutls_lib_auth gnutls_lib_ext gnutls_lib_algorithms
        gnutls_lib_x509 gnutls_lib_nettle gnutls_lib_accelerated
        gnutls_lib_extras gnutls_lib_hpke gnutls_minitasn1 gnutls_unistring
        gnutls_gl hogweed nettle)
