# Nettle and Hogweed as two static libraries, built from the release tarball in
# third_party/nettle plus the configure and generator output in gen/ (see
# gen.sh). Upstream's build is autotools and would want a configure run and an
# m4 for every compile of the app; this is the same compilation with the
# answers already written down.
#
# Both libraries are built whole. What a protocol actually calls is a few
# dozen of the 388 objects — the rest cost link time and nothing in the APK,
# since a static archive contributes only the members something references.

set(NETTLE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../third_party/nettle")
if(NOT EXISTS "${NETTLE_DIR}/nettle-types.h")
    message(FATAL_ERROR "third_party/nettle is empty — run `./gradlew :backends:fetchNettleSources`")
endif()

include("${CMAKE_CURRENT_SOURCE_DIR}/gen/sources.cmake")

# The public headers are what `#include <nettle/…>` has to find, and upstream
# makes that directory by installing. Copying is the same job without a prefix;
# gen/ wins over the source tree, because a header that is in both is one
# configure wrote.
foreach(header ${NETTLE_HEADERS})
    set(from "${NETTLE_DIR}/${header}")
    if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/gen/${header}")
        set(from "${CMAKE_CURRENT_SOURCE_DIR}/gen/${header}")
    endif()
    configure_file("${from}" "${CMAKE_CURRENT_BINARY_DIR}/include/nettle/${header}" COPYONLY)
endforeach()

list(TRANSFORM NETTLE_SOURCES PREPEND "${NETTLE_DIR}/")
list(TRANSFORM HOGWEED_SOURCES PREPEND "${NETTLE_DIR}/")

add_library(nettle STATIC ${NETTLE_SOURCES})
add_library(hogweed STATIC ${HOGWEED_SOURCES})

foreach(target nettle hogweed)
    target_include_directories(${target}
            PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/gen" "${NETTLE_DIR}"
            INTERFACE "${CMAKE_CURRENT_BINARY_DIR}/include")
    target_compile_definitions(${target} PRIVATE HAVE_CONFIG_H)
    # Upstream builds with these off; they are somebody else's warnings.
    target_compile_options(${target} PRIVATE -Wno-unused-parameter -Wno-sign-compare)
endforeach()

# Hogweed is the public-key half and is no use without the symmetric one.
target_link_libraries(hogweed PUBLIC nettle)
