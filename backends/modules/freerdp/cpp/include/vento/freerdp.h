// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's client as a `vento_session`. The config is filled in from the
// backend's options by the crate in `backends/modules/freerdp`, which both
// clients use. Its credentials include a domain, the only place where the
// credentials sent back differ between the protocols.

#ifndef VENTO_FREERDP_H
#define VENTO_FREERDP_H

#include <stdbool.h>

#include <vento/audio.h>
#include <vento/backend.h>
#include <vento/video.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_freerdp_config {
    /* `host`, `host:port`, `[literal]` or `[literal]:port`; no display
       numbers, since RDP has none. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *domain;      /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    const char *nla;         /* "prefer", "require" or "off" */
    bool legacy_tls;         /* TLS down to 1.0 at OpenSSL's security level 0 */
    bool compression;
    const char *graphics;    /* "gfx", "gfx-novideo", "rfx" or "bitmap" */
    const char *experience;  /* "full", "balanced" or "plain" */
    const char *sound;       /* "off", "local" or "remote"; see `audio` */
    /* Send the near end's microphone (the audin channel, through the library's
       OpenSL ES device, so Android only). The app checks RECORD_AUDIO first,
       since the capture device cannot be opened without it. */
    bool microphone;
    int scale;               /* 100, 140 or 180; anything else is taken as 100 */
    int width, height;       /* the desktop asked for, per monitor */
    int monitors;
    int keyboard_layout;
    const char *client_name;
    /* The near end's platform, for the general capability set's major type:
       "android" or "linux"; NULL or anything else sends unspecified. */
    const char *platform;
    /* A writable directory: the library sets up its certificate store there
       and fails to connect without one, though the store is never read or
       written (every certificate goes to `trust_needed`). Also becomes the
       process's HOME if none is set, as in an Android app's process: WinPR
       reads it and cannot start a session without it. An existing HOME is
       left unchanged. */
    const char *config_path;
    int connect_timeout_ms;  /* 0 or less for the library's default */
    /* The decoders for the graphics pipeline's H.264: the platform's
       (vento/video.h), owned by the session after the call and released when
       it ends, or immediately if it fails to start. They apply the user's
       hardware decoding setting. NULL for none, and the session then offers
       no H.264. */
    const vento_video_decoders *video;
    /* The output for "local" sound: the platform's (vento/audio.h), owned by
       the session after the call and released when it ends, or immediately
       if it fails to start. It applies the session's gain and mute. NULL for
       none, and the session then doesn't ask the server for sound regardless
       of `sound`. */
    const vento_audio *audio;
} vento_freerdp_config;

/* "FreeRDP x.y.z". Also a check that the library loads and links. */
const char *vento_freerdp_version(void);

/* Starts connecting and returns immediately; everything after that arrives
   through `callbacks`, which is copied. NULL if the session could not be set
   up. */
vento_session *vento_freerdp_create(const vento_freerdp_config *config,
                                    const vento_callbacks *callbacks);

/* Sets the monitor count for the *next* layout request, so changing the
   setting does not rearrange the desktop in use. */
void vento_freerdp_set_monitor_count(vento_session *s, int count);

#ifdef __cplusplus
}
#endif

#endif
