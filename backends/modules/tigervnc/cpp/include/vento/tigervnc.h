// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// TigerVNC's own client as a `vento_session`. The config is filled in from
// the backend's options by `backends/modules/tigervnc`, the adapter both
// clients use, and each field is explained below. A certificate goes to
// `trust_needed`, as libvnc's does, and nothing is kept of it here; `question`
// fires only for an RSA-AES server key, which is asked about in TigerVNC's
// viewer's words every time.

#ifndef VENTO_TIGERVNC_H
#define VENTO_TIGERVNC_H

#include <stdbool.h>

#include <vento/audio.h>
#include <vento/backend.h>
#include <vento/net.h>
#include <vento/video.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_tigervnc_config {
    /* As libvnc's: `host`, `host:port`, `host:display`, `host::port`,
       `[literal]` or `[literal]:port`. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    bool shared;
    bool anonymous_tls;      /* as libvnc's */
    int encoding;            /* an RFB encoding number, the one to ask for first */
    int compress_level;      /* 0–9, or -1 for the server's own default */
    int quality_level;       /* 0–9, -1 for lossless, or -2 to follow the line speed */
    int color_level;         /* 0 full, 1 for 64 colours, 2 for 8, or -2 to follow */
    bool h264;               /* whether to offer H.264 at all */
    /* What decodes it: the platform's decoders (vento/video.h), copied, and
       owned by the session from here on, released when it ends, even if this
       call fails. NULL for none, and H.264 is then never offered whatever
       `h264` says, since nothing could draw it. */
    const vento_video_decoders *video;
    bool audio;              /* whether to ask for the server's sound, for the session */
    /* Where it is played: the platform's sound output (vento/audio.h),
       copied, and owned by the session from here on, released when it ends,
       even if this call fails. NULL for none, and the server's sound is then
       never asked for whatever `audio` says. */
    const vento_audio *audio_output;
    int connect_timeout_ms;  /* 0 or less for the network's default */
    /* What the connection is made through: the session's network
       (vento/net.h), copied, and owned by the session from here on, released
       when it ends, even if this call fails. NULL to have the library dial
       for itself, with no deadline and not ended by a disconnect (the
       probe). */
    const vento_net *net;
} vento_tigervnc_config;

/* "TigerVNC x.y.z": the one call that proves the whole path. */
const char *vento_tigervnc_version(void);

/* Starts connecting and returns at once; everything after arrives through
   `callbacks`, which is copied. NULL where the session could not be set up. */
vento_session *vento_tigervnc_create(const vento_tigervnc_config *config,
                                     const vento_callbacks *callbacks);

/* Live, all of it: `SetEncodings` may be sent at any point in a session, and
   the client can change the pixel format safely. */
void vento_tigervnc_set_picture(vento_session *s, int encoding, int compress_level,
                                int quality_level, int color_level, bool h264);

#ifdef __cplusplus
}
#endif

#endif
