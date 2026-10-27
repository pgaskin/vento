// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// LibVNCServer's libvncclient as a `vento_session`. The config is filled in
// from the backend's options by `backends/modules/libvnc`, the adapter both
// clients use.

#ifndef VENTO_LIBVNC_H
#define VENTO_LIBVNC_H

#include <stdbool.h>

#include <vento/backend.h>
#include <vento/net.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_libvnc_config {
    /* `host`, `host:port`, `host:display`, `host::port`, `[literal]` or
       `[literal]:port`; a bare IPv6 literal is refused with an error. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    bool shared;
    /* Whether the VeNCrypt sub-types without a certificate may be used. They
       are always accepted in negotiation; if this is false, the connection
       ends with an explanation when the server picks one, before the tunnel
       is built. */
    bool anonymous_tls;
    const char *encoding;    /* a libvncclient encodings string, best first */
    int compress_level;      /* 0–9, or -1 for the server's own default */
    int quality_level;       /* 0–9 for Tight's JPEG, or -1 for lossless */
    int color_level;         /* 0 for 32-bit, 1 for 64 colours, 2 for 8 */
    int connect_timeout_ms;  /* 0 or less for the library's default */
    /* What the connection is made through: the session's network
       (vento/net.h), copied, and owned by the session from here on, released
       when it ends, even if this call fails. NULL to have the library dial
       for itself, which a disconnect does not interrupt (the probe). */
    const vento_net *net;
} vento_libvnc_config;

/* "LibVNCServer x.y.z". Calling it also checks that the library loads and
   links. */
const char *vento_libvnc_version(void);

/* Starts connecting and returns at once; everything after arrives through
   `callbacks`, which is copied. NULL out of memory, with nothing started. */
vento_session *vento_libvnc_create(const vento_libvnc_config *config,
                                   const vento_callbacks *callbacks);

/* Live: `SetEncodings` may be sent at any point in a session. */
void vento_libvnc_set_encodings(vento_session *s, const char *encoding, int compress_level,
                                int quality_level);

#ifdef __cplusplus
}
#endif

#endif
