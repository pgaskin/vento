// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// RealVNC's own viewer core as a `vento_session`. The core is RealVNC's
// proprietary `libvncviewer.so`, built for Android, and nothing of it is in
// this repository: a session is created over two libraries the caller
// supplies in `library_dir`, extracted from the viewer's APK, and
// `vento_realvnc_create` first refuses any copy whose SHA-256 is not the one
// `realvnc/.../Library.java` pins. The declarations the session is written
// against were read out of that build, and the hash of a third-party binary
// is the only thing that makes loading it safe.
//
// The parameters are `RealVncBackend`'s: the address and credentials
// `createSession` takes, and the option rows `RealVncProvider` offers, each
// under the key the core registers it by. Left out is anything that would
// make the core behave differently from on a phone (the platform always
// draws the cursor, as on Android), and the whole `_Hosted*`/`_Udp*`
// transport surface, which the provider does not offer either.
//
// There is one core per process and one core thread, as in `RealVncCore`:
// the first `create` loads the libraries and brings the core up on a thread
// that then owns every call into it and delivers every callback. That thread
// outlives every session, because the core never releases a session
// (`RealVncBackend.reap`). `destroy` retires a session from it and returns
// once nothing of that session can run any more.

#ifndef VENTO_REALVNC_H
#define VENTO_REALVNC_H

#include <stdbool.h>

#include <vento/backend.h>
#include <vento/net.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_realvnc_config {
    /* As the core takes it: `host`, `host:display`, `host::port`, or an
       address of RealVNC's own. */
    const char *address;
    const char *user_name;   /* NULL for none, which is the UserName option left unset */
    const char *password;    /* NULL or empty for none, which prompts (see RealVncBackend) */

    /* The rows, under the keys the core reads. `vento_realvnc_config_init`
       fills in RealVncProvider's defaults, which are not all zero. */
    const char *quality;            /* Quality: Auto, High, Medium, Low or Custom */
    const char *preferred_encoding; /* PreferredEncoding, read only at Custom */
    /* ColorLevel: full, rgb222 or rgb111; NULL follows the quality as the
       Android backend does when no depth has been chosen (rgb111 at Low,
       full otherwise; RealVncProvider.colorLevelFor). */
    const char *color_level;
    bool view_only;          /* ViewOnly */
    bool shared;             /* Shared */
    bool auto_reconnect;     /* AutoReconnect */
    bool enable_udp;         /* EnableUdpRfb: RealVNC's SCTP/RTP transport, with a TCP fallback */
    const char *encryption;  /* Encryption: AlwaysOn, PreferOn, PreferOff or Server */
    bool warn_unencrypted;   /* WarnUnencrypted */
    bool dot_when_no_cursor; /* DotWhenNoCursor */
    bool accept_bell;        /* AcceptBell */
    int keep_alive_interval_s;         /* KeepAliveInterval */
    int keep_alive_response_timeout_s; /* KeepAliveResponseTimeout */

    /* The identity this server was last accepted with (the opaque blob a
       previous session's `vento_realvnc_identity_saved` returned), or NULL.
       With it, the core's own security prompt is silent for a server it
       recognises. */
    const char *identity;

    /* Where `libc++_shared.so` and `libvncviewer.so` are. Read by the first
       `create` in the process and ignored by the rest: the core is loaded
       once. */
    const char *library_dir;
    /* Where the core keeps its persistent files (its RSA key, its
       identities, its logs and its caches), in a directory of its own, as it
       uses the app's files directory on the phone. The same once-per-process
       rule applies. The Android core keeps these in `EncryptedFile`s; the
       hosting layer under this backend keeps them as plain files
       (`host/Files.cxx` in this module's C++). */
    const char *state_dir;

    /* 0 or less for the Android backend's twenty seconds. Needed, as
       RealVncBackend says: a connection that fails reports nothing at all. */
    int connect_timeout_ms;

    /* The session's network where its route is not direct: the core's
       connections are then dialled through it (`realvnc_net.h` in this
       module's C says how), each within `connect_timeout_ms`, RealVNC's UDP
       transport included where the route carries datagrams (where it does
       not, the core falls back to TCP at once). The session owns it from
       `create` on, as the other
       C backends' configs say of theirs, and releases it on every path.
       NULL for a direct route, where the core dials the server itself. */
    const vento_net *net;
    /* What carries `net` (`Network::tunnel`), which tells two routes apart:
       a session to a server another session is connected to on a route
       with another `tunnel` (or none: direct) is refused, since the core's
       connections to one server cannot be told apart. NULL with `net`
       NULL. */
    const char *tunnel;

    /* For tests (the Linux client's debug builds set it from an environment
       variable): act as if the core's socket calls could not be hooked, for
       this session and every later one in the process, so that a session
       with `net` refuses to start instead of dialling directly. */
    bool test_unhooked;
} vento_realvnc_config;

/* Fills in RealVncProvider's defaults, which `createSession` sees on the
   phone when no option has been set. Call before filling in the rest. */
void vento_realvnc_config_init(vento_realvnc_config *config);

/* "RealVNC Viewer 4.9.3.60175", the build the declarations were read from and
   the only one `create` will load. Known without loading anything. */
const char *vento_realvnc_version(void);

/* Verifies and, the first time, loads and initialises the core, then starts
   connecting and returns; everything after arrives through `callbacks`,
   which is copied. NULL where the core could not be brought up (the
   libraries are missing or are not the pinned build, `state_dir` cannot be
   made, or the host cannot run them), with the reason in the log.

   This backend's `info` rows are, in order: desktop name, protocol version,
   connection type, security description, last used encoding, line speed,
   server pixel format, viewer pixel format: the eight strings the core's
   `ConnectionInfoBindings` formats itself, two of which change over time.
   `traffic` is received bytes only, read out of the core's own line-speed
   estimator as `RealVncTraffic` reads it, and is false until that
   number has been checked against a published one; the sent count is always
   zero, because the core has none, and the module's crate says it is not
   counted (`Behaviour::sent_counted`), so that it is shown as unknown rather
   than as nothing sent.

   `answer_trust` answers whichever of `security_needed` and `message` has
   been waiting longest, with no remember; `vento_realvnc_answer_security`
   below is the same answer with one. `answer_credentials` ignores the
   domain, and an empty user name is sent as the one the prompt proposed. */
vento_session *vento_realvnc_create(const vento_realvnc_config *config,
                                    const vento_callbacks *callbacks);

/* The answer to `security_needed`, with the one thing `answer_trust` cannot
   carry: whether to remember the identity. Remembering it makes the core's
   next check of this server a `VENTO_IDENTITY_OK` with no prompt, and makes
   the core hand the identity to `vento_realvnc_identity_saved`. The
   encryption decision is never remembered, as on the phone. No-op where no
   security prompt is outstanding. */
void vento_realvnc_answer_security(vento_session *s, bool accept, bool remember);

/* The identity the core asked to have remembered, if any (an opaque string
   of its own, to be given back as `identity` next time), or NULL. The
   caller frees it. The core keeps a copy of its own under `state_dir` as
   well, and a server it recognises there is one it does not ask about. */
char *vento_realvnc_identity_saved(vento_session *s);

/* A live option change, as `RealVncBackend.setOption`: the value joins the
   session's options and is applied at once, and a key in the quality group
   (Quality, ColorLevel, PreferredEncoding) carries the other two with it
   and re-reads the encoder, as the core requires. A ColorLevel set
   this way is the connection's own from then on. */
void vento_realvnc_set_option(vento_session *s, const char *key, const char *value);

#ifdef __cplusplus
}
#endif

#endif
