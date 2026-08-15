# OpenSSL as two static libraries, built from the submodule in
# third_party/openssl plus the Configure output in gen/ (see gen.sh). One target
# per archive upstream builds, because that is what the defines are per: the
# providers are compiled without most of the assembly's macros, and a file that
# reads one it was not compiled for calls into a kernel that was never selected.
#
# `openssl` is the one a consumer links: an interface over the lot of them, with
# the public headers — a quarter of which are generated — on its include path.

set(OPENSSL_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../third_party/openssl")
if(NOT EXISTS "${OPENSSL_DIR}/Configure")
    message(FATAL_ERROR "third_party/openssl is empty — run `git submodule update --init`")
endif()

set(GEN "${CMAKE_CURRENT_SOURCE_DIR}/gen")
# Configure decides the word sizes, the assembly and therefore which C files
# survive, so nearly everything here is per ABI and this directory comes first.
set(GEN_ABI "${GEN}/${CMAKE_ANDROID_ARCH_ABI}")
if(NOT EXISTS "${GEN_ABI}/sources.cmake")
    message(FATAL_ERROR "no openssl configuration for ${CMAKE_ANDROID_ARCH_ABI}; add the ABI to gen.sh")
endif()

enable_language(ASM)
include("${GEN_ABI}/sources.cmake")

# Upstream's own -I list, both halves of it: a build directory's copy of a
# header shadows the tree's, which is how a configured header wins over the
# `.h.in` beside it.
set(OPENSSL_INCLUDES
        "${GEN_ABI}" "${GEN}" "${GEN_ABI}/include" "${GEN}/include"
        "${GEN_ABI}/crypto" "${GEN}/crypto"
        "${GEN_ABI}/providers/common/include" "${GEN}/providers/common/include"
        "${OPENSSL_DIR}" "${OPENSSL_DIR}/include" "${OPENSSL_DIR}/crypto"
        "${OPENSSL_DIR}/crypto/modes"
        "${OPENSSL_DIR}/providers/common/include"
        "${OPENSSL_DIR}/providers/implementations/include"
        "${OPENSSL_DIR}/providers/fips/include")

# gen/ holds a generated file once when both ABIs produced the same bytes and
# under the ABI when they did not, so where a name resolves is a question rather
# than a rule.
function(_openssl_add name group)
    set(sources ${OPENSSL_SOURCES_${group}})
    list(TRANSFORM sources PREPEND "${OPENSSL_DIR}/")
    foreach(f ${OPENSSL_GENERATED_${group}})
        if(EXISTS "${GEN_ABI}/${f}")
            list(APPEND sources "${GEN_ABI}/${f}")
        else()
            list(APPEND sources "${GEN}/${f}")
        endif()
    endforeach()

    add_library(${name} STATIC ${sources})
    target_include_directories(${name} PRIVATE ${OPENSSL_INCLUDES})
    target_compile_definitions(${name} PRIVATE ${OPENSSL_DEFINES_${group}}
            OPENSSL_BUILDING_OPENSSL OPENSSL_PIC OPENSSL_USE_NODELETE
            # Where a provider is loaded from and where the trust store lives.
            # Both are paths that cannot exist on Android, which is this build
            # saying there is neither: no module is loadable in an app's
            # sandbox, and what verifies a certificate here is the app.
            OPENSSLDIR="/etc/ssl" ENGINESDIR="/nonexistent" MODULESDIR="/nonexistent")
    # Somebody else's warnings, and one of ours: the NDK's clang finds
    # unreachable code and sign conversions all through a library that has been
    # compiled by everything for thirty years.
    target_compile_options(${name} PRIVATE -w)
endfunction()

_openssl_add(openssl_crypto libcrypto_lib)
_openssl_add(openssl_providers_common libcommon_lib)
_openssl_add(openssl_providers_default libdefault_lib)
_openssl_add(openssl_ssl libssl_lib)

add_library(openssl INTERFACE)
target_include_directories(openssl INTERFACE
        "${GEN_ABI}/include" "${GEN}/include" "${OPENSSL_DIR}/include")
# The providers are a static archive each, and the default one is reached only
# through a table libcrypto holds, so the linker has to be told they belong
# together rather than left to drop what nothing names.
target_link_libraries(openssl INTERFACE
        openssl_ssl openssl_crypto openssl_providers_default openssl_providers_common
        openssl_crypto)
