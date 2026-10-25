// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The resolver's error codes between the core and glibc. libc-shim's
// `getaddrinfo` and `getnameinfo` return glibc's EAI_* codes, which are
// negative, and its `gai_strerror` is glibc's; bionic's codes are positive
// and numbered differently (EAI_NONAME is -2 in glibc and 8 in bionic). The
// core is built against bionic's and puts the number in its error text, so
// these wrappers give it bionic's numbers, and `gai_strerror` takes them back
// to glibc's for the text.

#ifndef VENTO_REALVNC_GAI_H
#define VENTO_REALVNC_GAI_H

#include <netdb.h>
#include <sys/socket.h>

#include <string>
#include <unordered_map>

namespace vento_realvnc {

/* Bionic's values, from the NDK's <netdb.h>. */
enum BionicEai {
    BIONIC_EAI_ADDRFAMILY = 1,
    BIONIC_EAI_AGAIN = 2,
    BIONIC_EAI_BADFLAGS = 3,
    BIONIC_EAI_FAIL = 4,
    BIONIC_EAI_FAMILY = 5,
    BIONIC_EAI_MEMORY = 6,
    BIONIC_EAI_NODATA = 7,
    BIONIC_EAI_NONAME = 8,
    BIONIC_EAI_SERVICE = 9,
    BIONIC_EAI_SOCKTYPE = 10,
    BIONIC_EAI_SYSTEM = 11,
    BIONIC_EAI_OVERFLOW = 14,
};

/* glibc's code as bionic's. A positive code is already bionic's: libc-shim's
   `getnameinfo` returns bionic's EAI_FAMILY for an address it can't convert.
   glibc's codes with no bionic counterpart (the asynchronous lookup's, and
   IDN's) are a non-recoverable failure. */
inline int toBionicEai(int code) {
    switch (code) {
        case 0: return 0;
        case EAI_ADDRFAMILY: return BIONIC_EAI_ADDRFAMILY;
        case EAI_AGAIN: return BIONIC_EAI_AGAIN;
        case EAI_BADFLAGS: return BIONIC_EAI_BADFLAGS;
        case EAI_FAIL: return BIONIC_EAI_FAIL;
        case EAI_FAMILY: return BIONIC_EAI_FAMILY;
        case EAI_MEMORY: return BIONIC_EAI_MEMORY;
        case EAI_NODATA: return BIONIC_EAI_NODATA;
        case EAI_NONAME: return BIONIC_EAI_NONAME;
        case EAI_SERVICE: return BIONIC_EAI_SERVICE;
        case EAI_SOCKTYPE: return BIONIC_EAI_SOCKTYPE;
        case EAI_SYSTEM: return BIONIC_EAI_SYSTEM;
        case EAI_OVERFLOW: return BIONIC_EAI_OVERFLOW;
        default: return code > 0 ? code : BIONIC_EAI_FAIL;
    }
}

/* Bionic's code as glibc's, for `gai_strerror`. Bionic's EAI_BADHINTS and
   EAI_PROTOCOL have no glibc code and are never produced here; they, and
   anything unknown, are passed through for glibc's "Unknown error". */
inline int toHostEai(int code) {
    switch (code) {
        case BIONIC_EAI_ADDRFAMILY: return EAI_ADDRFAMILY;
        case BIONIC_EAI_AGAIN: return EAI_AGAIN;
        case BIONIC_EAI_BADFLAGS: return EAI_BADFLAGS;
        case BIONIC_EAI_FAIL: return EAI_FAIL;
        case BIONIC_EAI_FAMILY: return EAI_FAMILY;
        case BIONIC_EAI_MEMORY: return EAI_MEMORY;
        case BIONIC_EAI_NODATA: return EAI_NODATA;
        case BIONIC_EAI_NONAME: return EAI_NONAME;
        case BIONIC_EAI_SERVICE: return EAI_SERVICE;
        case BIONIC_EAI_SOCKTYPE: return EAI_SOCKTYPE;
        case BIONIC_EAI_SYSTEM: return EAI_SYSTEM;
        case BIONIC_EAI_OVERFLOW: return EAI_OVERFLOW;
        default: return code;
    }
}

/* libc-shim's two resolver calls, as the core calls them: its `addrinfo` is
   bionic's layout, which only the shim converts, so the arguments pass
   through untouched. */
using ShimGetaddrinfo = int (*)(const char *, const char *, const void *, void **);
using ShimGetnameinfo = int (*)(const void *, socklen_t, char *, socklen_t, char *, socklen_t,
                                int);

inline ShimGetaddrinfo shimGetaddrinfo;
inline ShimGetnameinfo shimGetnameinfo;

inline int bionicGetaddrinfo(const char *node, const char *service, const void *hints,
                             void **res) {
    return toBionicEai(shimGetaddrinfo(node, service, hints, res));
}

inline int bionicGetnameinfo(const void *addr, socklen_t addrLen, char *host, socklen_t hostLen,
                             char *serv, socklen_t servLen, int flags) {
    return toBionicEai(shimGetnameinfo(addr, addrLen, host, hostLen, serv, servLen, flags));
}

inline const char *bionicGaiStrerror(int code) {
    return ::gai_strerror(toHostEai(code));
}

/* Puts the three in place of libc-shim's in `libc`, a symbol table built
   from `shim::get_shimmed_symbols`. */
inline void wrapResolver(std::unordered_map<std::string, void *> &libc) {
    shimGetaddrinfo = (ShimGetaddrinfo) libc.at("getaddrinfo");
    shimGetnameinfo = (ShimGetnameinfo) libc.at("getnameinfo");
    libc["getaddrinfo"] = (void *) &bionicGetaddrinfo;
    libc["getnameinfo"] = (void *) &bionicGetnameinfo;
    libc["gai_strerror"] = (void *) &bionicGaiStrerror;
}

} // namespace vento_realvnc

#endif
