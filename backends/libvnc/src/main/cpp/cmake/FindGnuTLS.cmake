# find_package(GnuTLS), answered with the port added as a subdirectory beside
# it rather than with whatever is on the host. LibVNCServer asks for a minimum
# version, which in module mode is checked against this variable.

set(GNUTLS_FOUND TRUE)
# Read rather than repeated: the pin moves in backends/build.gradle, and a
# hardcoded number here would go on satisfying a minimum the port no longer
# meets. (The same reason cmake/pixman.cmake reads meson's.)
file(READ "${CMAKE_CURRENT_LIST_DIR}/../../../../../ports/gnutls/gen/arm64-v8a/config.h" _gnutls_config)
if(NOT _gnutls_config MATCHES "#define VERSION \"([0-9]+\\.[0-9]+\\.[0-9]+)\"")
    message(FATAL_ERROR "no VERSION in the generated gnutls config.h")
endif()
set(GnuTLS_VERSION "${CMAKE_MATCH_1}")
set(GNUTLS_VERSION "${GnuTLS_VERSION}")
set(GNUTLS_INCLUDE_DIR "")
set(GNUTLS_LIBRARIES gnutls)
set(GNUTLS_DEFINITIONS "")
