// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Tests of the hosting layer's resolver wrappers (host/Gai.h), built with the
// probes (`-DVENTO_PROBES=ON`) and run by `ctest` in that build tree or as
// `realvnc/vento_realvnc_test`. The expected codes are bionic's, from the
// NDK's <netdb.h>, so they do not come from the code under test. Nothing here
// loads the core or resolves a name over the network.

#include <stdio.h>
#include <string.h>

#include <libc_shim.h>

#include "Gai.h"

using namespace vento_realvnc;

static int failures;

#define CHECK(cond)                                                                   \
    do {                                                                              \
        if (!(cond)) {                                                                \
            fprintf(stderr, "%s:%d: %s: CHECK failed: %s\n", __FILE__, __LINE__,      \
                    __func__, #cond);                                                 \
            failures++;                                                               \
        }                                                                             \
    } while (0)

/* Each glibc code, the bionic code the NDK's <netdb.h> gives it, and back. */
static void testCodes() {
    const struct {
        int glibc, bionic;
    } codes[] = {
            {0, 0},
            {EAI_ADDRFAMILY, 1},
            {EAI_AGAIN, 2},
            {EAI_BADFLAGS, 3},
            {EAI_FAIL, 4},
            {EAI_FAMILY, 5},
            {EAI_MEMORY, 6},
            {EAI_NODATA, 7},
            {EAI_NONAME, 8},
            {EAI_SERVICE, 9},
            {EAI_SOCKTYPE, 10},
            {EAI_SYSTEM, 11},
            {EAI_OVERFLOW, 14},
    };
    for (const auto &c : codes) {
        if (toBionicEai(c.glibc) != c.bionic || (c.glibc && toHostEai(c.bionic) != c.glibc)) {
            fprintf(stderr, "glibc %d: bionic %d and back %d, wanted %d\n", c.glibc,
                    toBionicEai(c.glibc), toHostEai(toBionicEai(c.glibc)), c.bionic);
            failures++;
        }
    }
    // glibc's own, with no bionic counterpart.
    CHECK(toBionicEai(EAI_INPROGRESS) == 4);
    // The shim's getnameinfo already returns bionic's EAI_FAMILY.
    CHECK(toBionicEai(5) == 5);
}

/* bionic's `struct addrinfo`, as the core passes hints. */
struct BionicAddrinfo {
    int ai_flags, ai_family, ai_socktype, ai_protocol;
    uint32_t ai_addrlen;
    char *ai_canonname;
    void *ai_addr;
    BionicAddrinfo *ai_next;
};

/* Through the symbol table, as the core calls them: a name that is not a
   numeric address, with AI_NUMERICHOST (4 in both), fails without a lookup,
   with EAI_NONAME. */
static void testTable() {
    std::unordered_map<std::string, void *> libc;
    for (const auto &s : shim::get_shimmed_symbols()) {
        libc[s.name] = s.value;
    }
    wrapResolver(libc);
    auto gai = (ShimGetaddrinfo) libc.at("getaddrinfo");
    auto strerr = (const char *(*) (int)) libc.at("gai_strerror");
    BionicAddrinfo hints = {};
    hints.ai_flags = 4;
    void *res = nullptr;
    const int r = gai("not-a-number", "5900", &hints, &res);
    CHECK(r == 8);
    CHECK(strcmp(strerr(r), ::gai_strerror(EAI_NONAME)) == 0);

    hints.ai_flags = 4;
    hints.ai_socktype = 1; // SOCK_STREAM
    CHECK(gai("127.0.0.1", "5900", &hints, &res) == 0 && res != nullptr);
    if (res) {
        ((void (*)(void *)) libc.at("freeaddrinfo"))(res);
    }
}

int main() {
    testCodes();
    testTable();
    if (failures) {
        fprintf(stderr, "%d checks failed\n", failures);
        return 1;
    }
    printf("vento_realvnc_test: all passed\n");
    return 0;
}
