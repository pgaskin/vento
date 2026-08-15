# The port in backends/ports/openssl, answered as a find_package: FreeRDP asks
# for OpenSSL through its own find_feature, and CMake's own FindOpenSSL would
# either fail against a sysroot that has none or — worse — find the host's.
#
# It sets what FreeRDP reads and nothing more; the imported targets are here
# because a find module is expected to define them, not because anything in this
# build uses them.
if(NOT TARGET openssl)
    add_subdirectory("${CMAKE_CURRENT_LIST_DIR}/../../../../../ports/openssl" openssl)
endif()

set(OPENSSL_FOUND TRUE)
# Both spellings: this is include()d rather than found, so nothing maps the
# module's upper-case answer onto the name FreeRDP checks.
set(OpenSSL_FOUND TRUE)
set(OPENSSL_INCLUDE_DIR "")
set(OPENSSL_SSL_LIBRARY openssl_ssl)
set(OPENSSL_CRYPTO_LIBRARIES openssl_crypto)
set(OPENSSL_LIBRARIES openssl)
# Read out of the submodule rather than repeated: FreeRDP prints it, and one
# place where a pin is written down is the whole of why it is a submodule.
file(STRINGS "${CMAKE_CURRENT_LIST_DIR}/../../../../../../third_party/openssl/VERSION.dat"
        _openssl_version_dat)
foreach(line ${_openssl_version_dat})
    if(line MATCHES "^(MAJOR|MINOR|PATCH)=(.*)$")
        list(APPEND _openssl_version_parts "${CMAKE_MATCH_2}")
    endif()
endforeach()
list(JOIN _openssl_version_parts "." OPENSSL_VERSION)

if(NOT TARGET OpenSSL::Crypto)
    add_library(OpenSSL::Crypto ALIAS openssl_crypto)
    add_library(OpenSSL::SSL ALIAS openssl_ssl)
endif()
