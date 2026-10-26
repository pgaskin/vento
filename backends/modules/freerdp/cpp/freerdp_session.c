// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's client as a `vento_session`, with the same interface as the
// IronRDP backend, so a difference between them is a difference between the
// two clients rather than between two bindings of one. That is the purpose of
// a second RDP client: every RFB question in this project could be checked
// against another VNC client, while every RDP one had a single answer. Nothing
// in here is platform-specific apart from where WinPR's log goes (`routeWLog`):
// on Android, the Rust adapter in the module's `src/` is linked into the same
// library, behind the shared JNI layer in `backends/platform/android`.
//
// Three behaviours come from the library rather than from this file. Each was
// found by driving the library on the phone without a backend, before this
// file existed:
//
//   1. **The library installs command-line prompts over the callbacks you
//      set.** Creating the context installs them unconditionally, and
//      `freerdp_client_start` does it again when `UseCommonStdioCallbacks` is
//      on, so ours are set after the context exists and the flag is set off
//      explicitly. On a phone there is no terminal, so those prompts refuse,
//      and the refusal is reported as a TLS failure even though the handshake
//      succeeded.
//   2. **`update->DesktopResize` is required.** `gdi_ResetGraphics` asserts on
//      it, so without the callback the client aborts.
//   3. **The graphics pipeline paints nothing unless it is attached to the
//      GDI** when its channel connects. It negotiates correctly either way, so
//      the result is a black desktop on a working connection.
//
// (A fourth is Android-specific and handled in this module's CMakeLists.txt:
// WinPR defines its own `JNI_OnLoad`, so it is compiled under another name and
// the one the library exports, the Rust crate's, calls it.)
//
// Two more are ours, both required by the `vento_session` interface:
//
//   * One thread owns the connection, and every call into the session is a
//     queued command it drains, woken by our event in the library's handle
//     set, because setting a flag doesn't wake a thread inside
//     `WaitForMultipleObjects`.
//   * There are two framebuffers. The GDI decodes into its own between
//     `BeginPaint` and `EndPaint`, and the interface allows pixels to be read
//     from the drawing thread at any time, so the damage is copied under a
//     writer lock when the batch ends and reads are served from the copy. The
//     alternative is holding that lock across a decode, which would make the
//     drawing thread wait for an H.264 frame.

#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <freerdp/freerdp.h>
#include <freerdp/client.h>
#include <freerdp/client/channels.h>
#include <freerdp/client/cliprdr.h>
#include <freerdp/client/cmdline.h>
#include <freerdp/crypto/certificate.h>
#include <freerdp/client/disp.h>
#include <freerdp/channels/rdpgfx.h>
#include <freerdp/channels/rdpsnd.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/gdi/gfx.h>
#include <freerdp/graphics.h>
#include <freerdp/autodetect.h>
#include <freerdp/settings.h>
#include <winpr/string.h>
#include <winpr/synch.h>
#include <winpr/wlog.h>
#include <openssl/tls1.h>

#include <vento/backend.h>
#include <vento/freerdp.h>
#include <vento/log.h>
#include <vento/text.h>

#include "h264_vento.h"
#include "rdpsnd_vento.h"

#define TAG "FreeRdp"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

#define CMD_RING 512
#define MAX_HELD_KEYS 64

enum {
    CMD_POINTER = 1,
    CMD_KEY_DOWN,
    CMD_KEY_UP,
    CMD_RELEASE_KEYS,
    CMD_CLIPBOARD,
    CMD_FOCUS,
    CMD_RESIZE,
    CMD_LAYOUT,
};

typedef struct {
    int type;
    int a, b;
    INT64 c;
    WCHAR *text; /* owned by the queue */
    size_t textLen;
} Cmd;

typedef struct Session Session;

/* The context FreeRDP allocates: the library's context and a pointer to our
   session. The client layer keeps an `rdpClientContext` in the allocation, so
   `ContextSize` must name a struct that starts with one. The library
   allocates exactly the size given; with a smaller one, each client layer
   call fails with an "application bug" in the log. */
typedef struct {
    rdpClientContext client;
    Session *session;
} ShimContext;

struct Session {
    vento_session base; /* first, so a `Session *` is also a `vento_session *` */
    vento_callbacks cb;

    ShimContext *ctx; /* protocol thread; published under both locks */
    freerdp *instance;

    pthread_t thread;
    HANDLE wake; /* in the library's handle set, wakes the protocol thread */

    pthread_mutex_t lock;
    pthread_cond_t cond;

    Cmd ring[CMD_RING];
    int head, tail;
    int dropped; /* since the ring was last empty, under `lock` */

    int quit;
    int started;
    int connected;
    int closedReported;
    int sawPointerPosition; /* protocol thread only: only the first is logged */
    _Atomic int viewOnly;
    _Atomic int canResize;
    /* The server's display-control limits: monitors in a layout (at most
       VENTO_MAX_MONITORS, 0 until the server sends them) and total pixels. */
    _Atomic UINT32 dispMonitors;
    _Atomic uint64_t dispArea;

    /* the credential prompt, answered through the session */
    int credAnswered;
    char *credUser, *credDomain, *credPass;

    /* the certificate prompt, answered the same way */
    int trustAnswered, trustAccepted;

    /* The connecting screen's status (see statusThread), under `lock`: the
       text due at `statusDue` if the step it names is still under way, the
       text last shown, and the thread that shows it, started at the first
       step worth naming. The texts are string literals, compared by
       address. */
    const char *statusPending;
    const char *statusShown;
    struct timespec statusDue;
    pthread_cond_t statusCond; /* on CLOCK_MONOTONIC */
    pthread_t statusThread;
    int statusStarted, statusStop;

    /* the copy op_read_region reads from */
    pthread_rwlock_t fbLock;
    uint8_t *fb;
    int fbW, fbH;

    /* The channels' interfaces, kept because each is passed only once, in an
       event when the channel connects. */
    CliprdrClientContext *cliprdr;
    DispClientContext *disp;
    _Atomic int gfxOpen; /* the graphics pipeline's channel is up */
    /* The capability set the server confirmed from the pipeline's offer: its
       version, and whether that version with its flags enables H.264. */
    _Atomic UINT32 gfxVersion;
    _Atomic int gfxH264;

    /* The codec of the last surface the server sent, recorded before calling
       the library's handler. The settings only say what was *requested*: the
       client never clears RemoteFxCodec when the server declines it, and a
       server may decline because of other settings (xrdp offers the codec
       only to a client that reports a LAN connection). */
    _Atomic UINT32 lastCodec;
    pSurfaceBits gdiSurfaceBits;
    /* The GDI's handler for the pipeline's reset, called from ours: on the
       pipeline, the server sends its monitors in the reset. */
    pcRdpgfxResetGraphics gdiResetGraphics;
    /* The GDI's handler for every surface command, called from ours so the
       H.264 contexts the library creates inside it belong to this session
       (h264_vento.h). */
    pcRdpgfxSurfaceCommand gdiSurfaceCommand;
    /* The capability set the server confirmed, advertised again when a
       decoder loses track of the stream, and whether that has happened since
       the server last confirmed one. */
    RDPGFX_CAPSET gfxCaps;
    _Atomic int gfxRestarting;
    /* The platform's decoders, used for every H.264 surface command, and
       their report on the last picture. */
    VentoH264 h264;
    /* The platform's sound output, used by the channels' rdpsnd devices
       (rdpsnd_vento.h). */
    VentoSound audio;

    /* The desktop's monitors (x, y, width, height each), under `lock`: as
       the server last sent them (`serverCount`, -1 before it has, 0 if the
       last layout it sent was not a valid desktop), and as this end last
       requested them (`askedCount`, at connect and on every layout sent). The
       server's layout is used when it covers the current desktop, the
       requested one when the desktop has that size; otherwise the desktop is
       not divided into monitors.
       `wantCount` is the latest layout op_request_layout queued for the
       protocol thread to send. */
    int32_t serverLayout[4 * VENTO_MAX_MONITORS];
    int serverCount;
    int32_t askedLayout[4 * VENTO_MAX_MONITORS];
    int askedCount;
    int32_t wantLayout[4 * VENTO_MAX_MONITORS];
    int wantCount;
    /* A size requested before the server sent its display-control limits, 0
       for none (holdUntilCaps). */
    int heldWidth, heldHeight;

    /* The near end's clipboard text in wire format (UTF-16, CRLF,
       NUL-terminated), held until the far end pastes. RDP's clipboard uses
       delayed rendering: the format list announces text and the data is sent
       only on request. */
    WCHAR *clipOut;
    size_t clipOutLen;

    struct {
        INT64 id;
        UINT16 code;  /* RDP scancode, with 0xE000 for an extended one */
        int shift;    /* the US layout needs Shift for it, and this end pressed one */
        WCHAR unicode; /* instead of `code`, for a character with no scancode */
    } keys[MAX_HELD_KEYS];
    int nkeys;
    int shiftHeld; /* by the user, not synthesised by this end */

    UINT16 buttons; /* the last RFB button mask, since RDP sends button changes */

    char *host;
    int port;
    /* Why the address is invalid, empty if it is valid: a session is created
       and started in one call, so the protocol thread is the first place it
       can be reported. */
    char addressError[160];
    char *user, *domain, *password;
    char *nla;
    int legacyTls;
    char *experience;
    char *graphics;
    char *sound;
    int microphone;
    char *clientName;
    UINT32 osMajorType;
    char *configPath;
    int width, height, monitors, keyboardLayout;
    int scale; /* percent; the scale the far end is asked to draw its interface at */
    int compression;
    int connectTimeoutMs;
};

static const vento_session_ops OPS;

/* ---- the address -------------------------------------------------------- */

/* Whether FreeRDP's connect (`freerdp_tcp_default_connect`) would take
   `host` as something other than a name or an address: a path is a Unix
   socket to connect to, `|` uses the port number as a file descriptor that
   is already connected, and `vsock://` dials a virtual machine socket. */
static int specialHost(const char *host) {
    return host[0] == '/' || host[0] == '|' || strncmp(host, "vsock://", 8) == 0;
}

/* An address is `host`, `host:port`, `[literal]` or `[literal]:port`. An
   unbracketed IPv6 literal is refused with an error rather than guessed at,
   since it can't be told apart from a host followed by a port. RDP has no
   display numbers, unlike VNC, so a second colon has no meaning. A host that
   FreeRDP would not dial as one (see `specialHost`) is refused too. The host
   is left at the start of `address`, which the caller still frees; `why` is
   empty unless the address is unusable. */
static void splitAddress(char *address, int *port, char *why, size_t whyLen) {
    size_t len = strlen(address);
    while (len > 0 && (address[len - 1] == ' ' || address[len - 1] == '\t')) {
        address[--len] = 0;
    }
    const size_t lead = strspn(address, " \t");
    if (lead > 0) {
        memmove(address, address + lead, len - lead + 1);
        len -= lead;
    }
    /* Unless another colon follows: "::1" is the unbracketed IPv6 loopback,
       and gets the bracketing error below rather than "no host" (as in
       runtime::address). */
    if (len == 0 || (address[0] == ':' && strchr(address + 1, ':') == NULL)) {
        snprintf(why, whyLen, "No host in address");
        return;
    }

    const char *host = address;
    size_t hostLen = len;
    const char *number = NULL;

    if (specialHost(address)) {
        snprintf(why, whyLen, "%s is not a host name or IP address", address);
        return;
    }
    if (address[0] == '[') {
        const char *close = strchr(address, ']');
        if (!close) {
            snprintf(why, whyLen, "%s missing closing bracket", address);
            return;
        }
        if (specialHost(address + 1)) {
            snprintf(why, whyLen, "%s is not a host name or IP address", address);
            return;
        }
        host = address + 1;
        hostLen = (size_t) (close - host);
        if (close[1] == ':') {
            number = close + 2;
        } else if (close[1]) {
            snprintf(why, whyLen, "%s has junk after closing bracket", address);
            return;
        }
    } else if (strchr(address, ':') != strrchr(address, ':')) {
        snprintf(why, whyLen, "IPv6 addresses must be bracketed, like [::1]:3389, not %s", address);
        return;
    } else {
        const char *colon = strchr(address, ':');
        if (colon) {
            hostLen = (size_t) (colon - address);
            number = colon + 1;
        }
    }

    if (hostLen == 0) {
        snprintf(why, whyLen, "No host in address");
        return;
    }
    if (number) {
        char *end = NULL;
        const long p = strtol(number, &end, 10);
        if (end == number || *end || p <= 0 || p > 65535) {
            snprintf(why, whyLen, "%s does not end in a port number", address);
            return;
        }
        *port = (int) p;
    }
    memmove(address, host, hostLen);
    address[hostLen] = 0;
}

/* ---- the command queue -------------------------------------------------- */

/* A full ring means the protocol thread has been inside one call for as long
   as CMD_RING - 1 inputs took, which is more likely a stalled connection than
   a busy one. One command is dropped: the oldest whose loss the far end can't
   tell from slow input. That is a key press (its release then finds nothing
   held and sends nothing), or a command restated by a later one of its type:
   a pointer (its mask holds all the buttons), the focus, a size, a layout, a
   clipboard. A release is never chosen, since losing one leaves the key held
   at the far end until it is pressed again. If nothing in the ring can go,
   the new command is dropped instead, unless it is a release, in which case
   the oldest command is dropped. Returns whether `c` has room; under
   `lock`. */
static bool releases(int type) {
    return type == CMD_KEY_UP || type == CMD_RELEASE_KEYS;
}

static bool makeRoom(Session *s, const Cmd *c) {
    int victim = -1;
    unsigned later = 1u << c->type;
    for (int i = (s->head + CMD_RING - 1) % CMD_RING;; i = (i + CMD_RING - 1) % CMD_RING) {
        const int type = s->ring[i].type;
        if (type == CMD_KEY_DOWN || (!releases(type) && (later & (1u << type)))) {
            victim = i;
        }
        later |= 1u << type;
        if (i == s->tail) {
            break;
        }
    }
    if (victim < 0 && !releases(c->type)) {
        return false;
    }
    victim = victim < 0 ? s->tail : victim;
    free(s->ring[victim].text);
    for (int i = victim; (i + 1) % CMD_RING != s->head; i = (i + 1) % CMD_RING) {
        s->ring[i] = s->ring[(i + 1) % CMD_RING];
    }
    s->head = (s->head + CMD_RING - 1) % CMD_RING;
    return true;
}

static void post(Session *s, const Cmd *c) {
    pthread_mutex_lock(&s->lock);
    if (s->quit) {
        pthread_mutex_unlock(&s->lock);
        free(c->text);
        return;
    }
    /* Motion is merged into the newest queued command when that is a pointer
       command with the same buttons: the protocol thread can be inside a
       decode, and by the time it drains the queue, every position but the
       last is out of date. A button change is a separate event and is never
       merged. */
    if (c->type == CMD_POINTER && s->head != s->tail) {
        const int last = (s->head + CMD_RING - 1) % CMD_RING;
        if (s->ring[last].type == CMD_POINTER && s->ring[last].c == c->c) {
            s->ring[last] = *c;
            pthread_mutex_unlock(&s->lock);
            SetEvent(s->wake);
            return;
        }
    }
    bool kept = true;
    int dropped = 0;
    if ((s->head + 1) % CMD_RING == s->tail) {
        kept = makeRoom(s, c);
        dropped = ++s->dropped;
    }
    if (kept) {
        s->ring[s->head] = *c;
        s->head = (s->head + 1) % CMD_RING;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    if (!kept) {
        free(c->text);
    }
    if (dropped == 1) {
        LOGW("input: the protocol thread is %d commands behind, so some are dropped (never a "
             "release)", CMD_RING - 1);
    }
    SetEvent(s->wake);
}

/* ---- callbacks out ------------------------------------------------------ */

static void stopStatus(Session *s);

static void fireClosed(Session *s, const char *detail) {
    pthread_mutex_lock(&s->lock);
    const int already = s->closedReported;
    s->closedReported = 1;
    pthread_mutex_unlock(&s->lock);
    if (already) {
        return;
    }
    /* Nothing may follow `closed`. */
    stopStatus(s);
    /* The library's message, which can quote the server's; the callback
       takes UTF-8. */
    char *utf8 = vento_utf8_from_bytes(detail);
    s->cb.closed(s->cb.user, utf8 ? utf8 : "");
    free(utf8);
}

/* The far end's clipboard text: UTF-16 with CRLF on the wire, UTF-8 with LF
   in the callback. It is limited to VENTO_MAX_CLIPBOARD like every backend's,
   counted in bytes of the UTF-8 text as the IronRDP backend does. */
static void fireClipboard(Session *s, const WCHAR *text, size_t units) {
    char *utf8 = ConvertWCharNToUtf8Alloc(text, units, NULL);
    if (!utf8) {
        return;
    }
    vento_text_to_lf(utf8);
    const size_t bytes = strlen(utf8);
    if (bytes > VENTO_MAX_CLIPBOARD) {
        LOGW("the far end's clipboard is over the %d bytes this end takes; skipped",
             VENTO_MAX_CLIPBOARD);
    } else if (bytes > 0) {
        s->cb.clipboard(s->cb.user, utf8);
    }
    free(utf8);
}

/* ---- the framebuffer ---------------------------------------------------- */

VentoSound *vento_sound_of(rdpContext *context) {
    Session *s = context ? ((ShimContext *) context)->session : NULL;
    return s ? &s->audio : NULL;
}

static Session *sessionOf(rdpContext *context) {
    return ((ShimContext *) context)->session;
}

/* One allocation per desktop size, filled with opaque black so a region read
   before anything has been decoded is black rather than transparent. */
static BOOL allocFramebuffer(Session *s, int w, int h) {
    if (w <= 0 || h <= 0) {
        return FALSE;
    }
    const size_t bytes = (size_t) w * (size_t) h * 4u;
    uint8_t *shadow = malloc(bytes);
    if (!shadow) {
        return FALSE;
    }
    memset(shadow, 0, bytes);
    for (size_t i = 3; i < bytes; i += 4) {
        shadow[i] = 0xFF;
    }
    pthread_rwlock_wrlock(&s->fbLock);
    free(s->fb);
    s->fb = shadow;
    s->fbW = w;
    s->fbH = h;
    pthread_rwlock_unlock(&s->fbLock);
    return TRUE;
}

static BOOL onBeginPaint(rdpContext *context) {
    rdpGdi *gdi = context->gdi;
    gdi->primary->hdc->hwnd->invalid->null = TRUE;
    gdi->primary->hdc->hwnd->ninvalid = 0;
    return TRUE;
}

/* The only cost of the second framebuffer: one copy per damaged rectangle per
   paint, under the writer lock. The GDI's buffer is BGRX and the session's
   framebuffer is RGBX, so red and blue are swapped here and the alpha the
   codecs leave undefined is set, in one pass over the damage rather than the
   library's converting copy followed by ours. */
static BOOL onEndPaint(rdpContext *context) {
    Session *s = sessionOf(context);
    rdpGdi *gdi = context->gdi;
    HGDI_WND hwnd = gdi->primary->hdc->hwnd;
    if (hwnd->invalid->null || hwnd->ninvalid <= 0) {
        return TRUE;
    }

    pthread_rwlock_wrlock(&s->fbLock);
    /* Both sizes are read here, under the lock that guards the copy's size,
       and the damage is clipped to the smaller of the two buffers. The
       desktop is resized on two threads (the protocol thread for a
       reactivation, the graphics pipeline's channel thread for its reset),
       and a paint can end on either, so a size read before taking the lock
       could be the old size of a copy that has since shrunk, and rows were
       then written at the old stride past the end of the new allocation. The
       two buffers are also resized one after the other (onDesktopResize),
       with a call to this from inside gdi_resize in between. */
    const int stride = s->fbW;
    const int fw = s->fb ? (stride < gdi->width ? stride : gdi->width) : 0;
    const int fh = s->fb ? (s->fbH < gdi->height ? s->fbH : gdi->height) : 0;
    for (INT32 i = 0; i < hwnd->ninvalid; i++) {
        const HGDI_RGN r = &hwnd->cinvalid[i];
        int x = r->x, y = r->y, w = r->w, h = r->h;
        if (x < 0) { w += x; x = 0; }
        if (y < 0) { h += y; y = 0; }
        if (x + w > fw) { w = fw - x; }
        if (y + h > fh) { h = fh - y; }
        if (w <= 0 || h <= 0) {
            r->w = 0;
            continue;
        }
        r->x = x; r->y = y; r->w = w; r->h = h;
        for (int row = 0; row < h; row++) {
            const uint32_t *src = (const uint32_t *) (gdi->primary_buffer
                    + (size_t) (y + row) * gdi->stride) + x;
            uint32_t *dst = (uint32_t *) (s->fb
                    + (size_t) (y + row) * (size_t) stride * 4u) + x;
            for (int col = 0; col < w; col++) {
                const uint32_t p = src[col];
                dst[col] = 0xFF000000u | ((p & 0x00FF0000u) >> 16)
                        | (p & 0x0000FF00u) | ((p & 0x000000FFu) << 16);
            }
        }
    }
    pthread_rwlock_unlock(&s->fbLock);

    for (INT32 i = 0; i < hwnd->ninvalid; i++) {
        const HGDI_RGN r = &hwnd->cinvalid[i];
        if (r->w > 0) {
            s->cb.damaged(s->cb.user, r->x, r->y, r->w, r->h);
        }
    }
    s->cb.frame_end(s->cb.user);

    hwnd->invalid->null = TRUE;
    hwnd->ninvalid = 0;
    return TRUE;
}

/* Only records the codec; the library does the drawing. */
static BOOL onSurfaceBits(rdpContext *context, const SURFACE_BITS_COMMAND *cmd) {
    Session *s = sessionOf(context);
    if (cmd) {
        s->lastCodec = cmd->bmp.codecID;
    }
    return s->gdiSurfaceBits ? s->gdiSurfaceBits(context, cmd) : TRUE;
}

/* Required: gdi_ResetGraphics asserts on this callback, so a client without
   one aborts on the channel thread instead of failing to resize.

   Called on two threads: the protocol thread, when the server reactivates
   the connection at another size (libfreerdp/core/connection.c), and the
   graphics pipeline channel's thread, on its reset (libfreerdp/gdi/gfx.c),
   which holds the pipeline's lock but not the update lock every paint is
   made under. So the whole resize is made under the update lock, which the
   library exports for this and which is recursive: two resizes don't
   interleave their copies and GDIs, the sizes are reported in the order the
   buffers took them, and no paint on the other thread is between its
   BeginPaint and EndPaint while the buffers change. gdi_resize first ends a
   paint of its own (update_end_paint, into onEndPaint), with the copy
   already at the new size and the GDI still at the old one, which is why
   onEndPaint clamps to the smaller of the two.

   Without the pipeline, this resize also resizes the codecs. They are sized
   at connect, and planar, RemoteFX and NSCodec refuse anything wider
   (planar's "maxWidth 1920 < nSrcWidth 2400"), which ended a bitmap session
   at the first update after a resize past its starting width. In the library
   only the pipeline's reset resizes them (gdi/gfx.c); the core's
   reactivation leaves that to the client, and xfreerdp doesn't do it either.
   With the pipeline open the reset handles them, and they are not touched
   here. */
static BOOL onDesktopResize(rdpContext *context) {
    Session *s = sessionOf(context);
    rdp_update_lock(context->update);
    const UINT32 w = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopHeight);
    const UINT32 bitmapCodecs =
            FREERDP_CODEC_INTERLEAVED | FREERDP_CODEC_PLANAR | FREERDP_CODEC_NSCODEC | FREERDP_CODEC_REMOTEFX;
    const BOOL codecs = s->gfxOpen || !context->codecs
                        || freerdp_client_codecs_reset(context->codecs, bitmapCodecs, w, h);
    if (!codecs) {
        LOGW("the codecs could not be made %ux%u", (unsigned) w, (unsigned) h);
    }
    const BOOL ok = codecs && allocFramebuffer(s, (int) w, (int) h) && gdi_resize(context->gdi, w, h);
    if (ok) {
        s->cb.desktop_size(s->cb.user, (int) w, (int) h);
    }
    rdp_update_unlock(context->update);
    return ok;
}

/* ---- the monitors ------------------------------------------------------- */

/* Whether `n` screens (four ints each) divide a desktop of `w × h`, that is,
   whether their bounding box is the desktop. A layout that doesn't is from
   before the desktop last changed size, and says nothing about the current
   one. */
static bool covers(const int32_t *flat, int n, int w, int h) {
    if (n < 1) {
        return false;
    }
    int64_t right = 0, bottom = 0;
    for (int i = 0; i < n; i++) {
        const int32_t *m = flat + 4 * i;
        if (m[0] < 0 || m[1] < 0) {
            return false;
        }
        const int64_t r = (int64_t) m[0] + m[2], b = (int64_t) m[1] + m[3];
        right = r > right ? r : right;
        bottom = b > bottom ? b : bottom;
    }
    return right == w && bottom == h;
}

/* The maximum width and height of a virtual desktop, and so of any monitor
   in it or any offset from its corner (MS-RDPBCGR 2.2.1.3.6). */
#define MAX_DESKTOP_SIDE 32766

/* Converts `n` MONITOR_DEFs into `flat` as x, y, width, height each,
   relative to their combined top left corner. Returns false, with `flat`
   unspecified, if they are not a valid desktop: a monitor less than a pixel
   wide or tall, or anything more than MAX_DESKTOP_SIDE from that corner. The
   corners are the server's 32-bit values, so the sums are computed in 64
   bits. */
static bool flattenLayout(const MONITOR_DEF *defs, int n, int32_t *flat) {
    int64_t left = 0, top = 0;
    for (int i = 0; i < n; i++) {
        left = i == 0 || defs[i].left < left ? defs[i].left : left;
        top = i == 0 || defs[i].top < top ? defs[i].top : top;
    }
    for (int i = 0; i < n; i++) {
        const int64_t x = (int64_t) defs[i].left - left, y = (int64_t) defs[i].top - top;
        const int64_t w = (int64_t) defs[i].right - defs[i].left + 1;
        const int64_t h = (int64_t) defs[i].bottom - defs[i].top + 1;
        if (w < 1 || h < 1 || x + w > MAX_DESKTOP_SIDE || y + h > MAX_DESKTOP_SIDE) {
            return false;
        }
        flat[4 * i + 0] = (int32_t) x;
        flat[4 * i + 1] = (int32_t) y;
        flat[4 * i + 2] = (int32_t) w;
        flat[4 * i + 3] = (int32_t) h;
    }
    return true;
}

/* Stores the monitors as the server sends them, in its order, which is the
   only identity RDP gives a monitor. A MONITOR_DEF's corners are inclusive
   and relative to the primary monitor, which others may be above or to the
   left of; the desktop's pixel 0,0 is the top left corner of all of them, so
   the session's coordinates start there. The server sends them in two
   places: the Monitor Layout PDU of the connection sequence
   (update->RemoteMonitors), and, on the graphics pipeline, every reset,
   which is how such a server responds to a layout request. */
static void heardLayout(Session *s, const MONITOR_DEF *defs, UINT32 count, const char *where) {
    const int n = count > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : (int) count;
    int32_t flat[4 * VENTO_MAX_MONITORS];
    if (!flattenLayout(defs, n, flat)) {
        /* Invalid corners (a monitor of no width, or one further from the
           others than a desktop is wide), whose sums could overflow the 32
           bits they arrived in. The whole layout is ignored rather than the
           bad monitor: a monitor's position in the list is its identity, and
           renumbering the rest would be wrong in a less visible way. The
           requested layout is used instead (op_monitors). */
        LOGW("%s: the server's %d monitors are not a desktop; their layout is ignored", where, n);
        pthread_mutex_lock(&s->lock);
        s->serverCount = 0;
        pthread_mutex_unlock(&s->lock);
        return;
    }
    for (int i = 0; i < n; i++) {
        LOGI("%s: monitor %d of %d %dx%d+%d+%d%s", where, i + 1, n, flat[4 * i + 2], flat[4 * i + 3],
             flat[4 * i + 0], flat[4 * i + 1],
             (defs[i].flags & MONITOR_PRIMARY) ? ", primary" : "");
    }
    pthread_mutex_lock(&s->lock);
    memcpy(s->serverLayout, flat, sizeof(int32_t) * 4 * (size_t) n);
    s->serverCount = n;
    pthread_mutex_unlock(&s->lock);
}

static BOOL onRemoteMonitors(rdpContext *context, UINT32 count, const MONITOR_DEF *monitors) {
    heardLayout(sessionOf(context), monitors, count, "monitor layout");
    return TRUE;
}

static UINT onGfxResetGraphics(RdpgfxClientContext *context,
                               const RDPGFX_RESET_GRAPHICS_PDU *reset) {
    rdpGdi *gdi = (rdpGdi *) context->custom;
    Session *s = sessionOf(gdi->context);
    if (reset->monitorCount > 0 && reset->monitorDefArray) {
        heardLayout(s, reset->monitorDefArray, reset->monitorCount, "graphics reset");
    }
    return s->gdiResetGraphics ? s->gdiResetGraphics(context, reset) : CHANNEL_RC_OK;
}

/* Converts `n` screens (x, y, width, height each) into a display-control
   layout in `out`, and the desktop that layout produces into `asked`. The
   protocol's rules: the first screen is the primary and is at the origin, so
   all of them are shifted to put it there; a monitor is 200 to 8192 pixels
   each way and its width is even. When that changes a screen's size, the
   screens to its right or below move by the same amount, so adjacent screens
   stay adjacent. Returns how many were written. */
static int buildLayout(const int32_t *rects, int n, int scale, DISPLAY_CONTROL_MONITOR_LAYOUT *out,
                       int32_t *asked) {
    n = n > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : n;
    int32_t w[VENTO_MAX_MONITORS], h[VENTO_MAX_MONITORS];
    for (int i = 0; i < n; i++) {
        const int32_t *r = rects + 4 * i;
        w[i] = r[2] < 200 ? 200 : (r[2] > 8192 ? 8192 : r[2]);
        w[i] &= ~1;
        h[i] = r[3] < 200 ? 200 : (r[3] > 8192 ? 8192 : r[3]);
    }
    int32_t x[VENTO_MAX_MONITORS], y[VENTO_MAX_MONITORS];
    for (int i = 0; i < n; i++) {
        const int32_t *r = rects + 4 * i;
        x[i] = r[0];
        y[i] = r[1];
        for (int j = 0; j < n; j++) {
            const int32_t *o = rects + 4 * j;
            if (o[0] + o[2] <= r[0]) {
                x[i] -= o[2] - w[j];
            }
            if (o[1] + o[3] <= r[1]) {
                y[i] -= o[3] - h[j];
            }
        }
    }
    int32_t left = 0, top = 0;
    for (int i = 0; i < n; i++) {
        left = i == 0 || x[i] < left ? x[i] : left;
        top = i == 0 || y[i] < top ? y[i] : top;
    }
    for (int i = 0; i < n; i++) {
        DISPLAY_CONTROL_MONITOR_LAYOUT *m = &out[i];
        memset(m, 0, sizeof(*m));
        m->Flags = i == 0 ? DISPLAY_CONTROL_MONITOR_PRIMARY : 0;
        m->Left = x[i] - x[0];
        m->Top = y[i] - y[0];
        m->Width = (UINT32) w[i];
        m->Height = (UINT32) h[i];
        m->Orientation = ORIENTATION_LANDSCAPE;
        /* The connection's scale, not 100: a layout restates everything, so
           the default would undo the scale the session was opened with. */
        m->DesktopScaleFactor = (UINT32) scale;
        m->DeviceScaleFactor = (UINT32) scale;
        asked[4 * i + 0] = x[i] - left;
        asked[4 * i + 1] = y[i] - top;
        asked[4 * i + 2] = w[i];
        asked[4 * i + 3] = h[i];
    }
    return n;
}

/* Sends a layout of `n` screens on the display-control channel and stores it
   as the requested layout. The server sends no refusal: it creates the new
   desktop, a different one, or nothing. The server's limits are applied here
   with a log line: the channel's own code would cut a layout down to the
   monitor count without logging anything, and the server may ignore a
   layout over its area. */
static void sendLayout(Session *s, const int32_t *rects, int n) {
    if (!s->disp || !s->disp->SendMonitorLayout || !s->canResize || n < 1) {
        return;
    }
    const int most = (int) atomic_load(&s->dispMonitors);
    if (n > most) {
        LOGW("the server takes at most %d monitors; the last %d of the layout are left out", most,
             n - most);
        n = most;
    }
    DISPLAY_CONTROL_MONITOR_LAYOUT layout[VENTO_MAX_MONITORS];
    int32_t asked[4 * VENTO_MAX_MONITORS];
    n = buildLayout(rects, n, s->scale, layout, asked);
    uint64_t area = 0;
    for (int i = 0; i < n; i++) {
        area += (uint64_t) layout[i].Width * layout[i].Height;
    }
    if (area > atomic_load(&s->dispArea)) {
        LOGW("a layout of %llu pixels is more than the server takes (%llu); it is not asked for",
             (unsigned long long) area, (unsigned long long) atomic_load(&s->dispArea));
        return;
    }
    for (int i = 0; i < n; i++) {
        LOGI("asking for monitor %d of %d: %ux%u%+d%+d", i + 1, n, layout[i].Width, layout[i].Height,
             layout[i].Left, layout[i].Top);
    }
    pthread_mutex_lock(&s->lock);
    memcpy(s->askedLayout, asked, sizeof(int32_t) * 4 * (size_t) n);
    s->askedCount = n;
    pthread_mutex_unlock(&s->lock);
    s->disp->SendMonitorLayout(s->disp, (UINT32) n, layout);
}

/* ---- the remote cursor -------------------------------------------------- */

static void fireCursor(Session *s, const uint32_t *rgba, int w, int h, int hotX, int hotY) {
    s->cb.cursor(s->cb.user, rgba, w, h, hotX, hotY);
}

static BOOL onPointerNew(rdpContext *context, rdpPointer *pointer) {
    (void) context;
    (void) pointer;
    return TRUE;
}

static void onPointerFree(rdpContext *context, rdpPointer *pointer) {
    (void) context;
    (void) pointer;
}

/* Converted when the pointer becomes current rather than when it arrives: a
   server sends shapes it never uses, and an unchanged shape costs nothing on
   the other side anyway (`CursorCache` keys on the hash). */
static BOOL onPointerSet(rdpContext *context, rdpPointer *pointer) {
    Session *s = sessionOf(context);
    const UINT32 w = pointer->width, h = pointer->height;
    if (w == 0 || h == 0 || w > 384 || h > 384) {
        return TRUE;
    }
    uint32_t *rgba = calloc((size_t) w * h, sizeof(uint32_t));
    if (!rgba) {
        return TRUE;
    }
    /* RGBA32 (R, G, B, A in memory), the callback's order for cursors as
       for the framebuffer; any repacking for the platform happens on the
       other side of the call. */
    if (freerdp_image_copy_from_pointer_data((BYTE *) rgba, PIXEL_FORMAT_RGBA32, 0, 0, 0, w, h,
                                             pointer->xorMaskData, pointer->lengthXorMask,
                                             pointer->andMaskData, pointer->lengthAndMask,
                                             pointer->xorBpp, &context->gdi->palette)) {
        fireCursor(s, rgba, (int) w, (int) h, (int) pointer->xPos, (int) pointer->yPos);
    }
    free(rgba);
    return TRUE;
}

/* An empty shape tells the callback the server has hidden the pointer. The
   default arrow is reported the same way: this end draws its own cursor, and
   the server's arrow adds nothing to it. */
static BOOL onPointerSetNull(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

static BOOL onPointerSetDefault(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

/* The far end reporting where it moved the cursor. This app ignores it: the
   cursor is owned by this end and the viewport follows it.

   Set on `update->pointer` rather than on the graphics pointer below, whose
   SetPosition has the same signature but is never called by the library: the
   position update is dispatched in core/update.c and the graphics module only
   sees shapes. */
static BOOL onPointerPosition(rdpContext *context, const POINTER_POSITION_UPDATE *pos) {
    Session *s = sessionOf(context);
    if (s && !s->sawPointerPosition) {
        s->sawPointerPosition = 1;
        LOGI("server moved its pointer to %u,%u (the first of this session)",
             pos->xPos, pos->yPos);
    }
    return TRUE;
}

static BOOL registerPointer(rdpGraphics *graphics) {
    rdpPointer pointer = {0};
    pointer.size = sizeof(pointer);
    pointer.New = onPointerNew;
    pointer.Free = onPointerFree;
    pointer.Set = onPointerSet;
    pointer.SetNull = onPointerSetNull;
    pointer.SetDefault = onPointerSetDefault;
    graphics_register_pointer(graphics, &pointer);
    return TRUE;
}

/* ---- the clipboard ------------------------------------------------------ */

/* Text only, in both directions. CF_UNICODETEXT is NUL-terminated UTF-16,
   which every Windows clipboard holds; the callback takes UTF-8, and the near
   end has no use for a file list or a bitmap. */
static UINT sendFormatList(Session *s, int haveText) {
    CLIPRDR_FORMAT format = {0};
    CLIPRDR_FORMAT_LIST list = {0};
    format.formatId = CF_UNICODETEXT;
    list.common.msgType = CB_FORMAT_LIST;
    list.numFormats = haveText ? 1 : 0;
    list.formats = haveText ? &format : NULL;
    if (!s->cliprdr || !s->cliprdr->ClientFormatList) {
        return ERROR_INTERNAL_ERROR;
    }
    /* The empty list is needed too: on a channel that never sends a format
       list, neither direction works. */
    return s->cliprdr->ClientFormatList(s->cliprdr, &list);
}

static UINT onCliprdrMonitorReady(CliprdrClientContext *cliprdr,
                                  const CLIPRDR_MONITOR_READY *ready) {
    (void) ready;
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_CAPABILITIES caps = {0};
    CLIPRDR_GENERAL_CAPABILITY_SET general = {0};
    general.capabilitySetType = CB_CAPSTYPE_GENERAL;
    general.capabilitySetLength = 12;
    general.version = CB_CAPS_VERSION_2;
    general.generalFlags = CB_USE_LONG_FORMAT_NAMES;
    caps.cCapabilitiesSets = 1;
    caps.capabilitySets = (CLIPRDR_CAPABILITY_SET *) &general;
    const UINT rc = cliprdr->ClientCapabilities(cliprdr, &caps);
    if (rc != CHANNEL_RC_OK) {
        return rc;
    }
    pthread_mutex_lock(&s->lock);
    const int have = s->clipOut != NULL;
    pthread_mutex_unlock(&s->lock);
    return sendFormatList(s, have);
}

static UINT onCliprdrServerCapabilities(CliprdrClientContext *cliprdr,
                                        const CLIPRDR_CAPABILITIES *caps) {
    (void) cliprdr;
    (void) caps;
    return CHANNEL_RC_OK;
}

/* The far end copied something. Request it only if it is text; RDP renders a
   clipboard on demand, so the data arrives only after the request. */
static UINT onCliprdrServerFormatList(CliprdrClientContext *cliprdr,
                                      const CLIPRDR_FORMAT_LIST *list) {
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_FORMAT_LIST_RESPONSE response = {0};
    response.common.msgType = CB_FORMAT_LIST_RESPONSE;
    response.common.msgFlags = CB_RESPONSE_OK;
    const UINT rc = cliprdr->ClientFormatListResponse(cliprdr, &response);
    if (rc != CHANNEL_RC_OK) {
        return rc;
    }
    if (s->viewOnly) {
        return CHANNEL_RC_OK;
    }
    for (UINT32 i = 0; i < list->numFormats; i++) {
        if (list->formats[i].formatId == CF_UNICODETEXT) {
            CLIPRDR_FORMAT_DATA_REQUEST request = {0};
            request.common.msgType = CB_FORMAT_DATA_REQUEST;
            request.requestedFormatId = CF_UNICODETEXT;
            return cliprdr->ClientFormatDataRequest(cliprdr, &request);
        }
    }
    return CHANNEL_RC_OK;
}

static UINT onCliprdrServerFormatListResponse(CliprdrClientContext *cliprdr,
                                              const CLIPRDR_FORMAT_LIST_RESPONSE *response) {
    (void) cliprdr;
    (void) response;
    return CHANNEL_RC_OK;
}

/* The far end is pasting the near end's clipboard. */
static UINT onCliprdrServerFormatDataRequest(CliprdrClientContext *cliprdr,
                                             const CLIPRDR_FORMAT_DATA_REQUEST *request) {
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_FORMAT_DATA_RESPONSE response = {0};
    response.common.msgType = CB_FORMAT_DATA_RESPONSE;

    pthread_mutex_lock(&s->lock);
    BYTE *data = NULL;
    size_t bytes = 0;
    if (request->requestedFormatId == CF_UNICODETEXT && s->clipOut && !s->viewOnly) {
        bytes = (s->clipOutLen + 1) * sizeof(WCHAR);
        data = malloc(bytes);
        if (data) {
            memcpy(data, s->clipOut, bytes);
        } else {
            bytes = 0;
        }
    }
    pthread_mutex_unlock(&s->lock);

    response.common.msgFlags = data ? CB_RESPONSE_OK : CB_RESPONSE_FAIL;
    response.common.dataLen = (UINT32) bytes;
    response.requestedFormatData = data;
    const UINT rc = cliprdr->ClientFormatDataResponse(cliprdr, &response);
    free(data);
    return rc;
}

/* The response to our request: the far end's text. */
static UINT onCliprdrServerFormatDataResponse(CliprdrClientContext *cliprdr,
                                              const CLIPRDR_FORMAT_DATA_RESPONSE *response) {
    Session *s = (Session *) cliprdr->custom;
    if ((response->common.msgFlags & CB_RESPONSE_FAIL) || !response->requestedFormatData) {
        return CHANNEL_RC_OK;
    }
    size_t units = response->common.dataLen / sizeof(WCHAR);
    const WCHAR *text = (const WCHAR *) response->requestedFormatData;
    while (units > 0 && text[units - 1] == 0) {
        units--;
    }
    if (units > 0) {
        fireClipboard(s, text, units);
    }
    return CHANNEL_RC_OK;
}

static void wireCliprdr(Session *s, CliprdrClientContext *cliprdr) {
    s->cliprdr = cliprdr;
    cliprdr->custom = s;
    cliprdr->MonitorReady = onCliprdrMonitorReady;
    cliprdr->ServerCapabilities = onCliprdrServerCapabilities;
    cliprdr->ServerFormatList = onCliprdrServerFormatList;
    cliprdr->ServerFormatListResponse = onCliprdrServerFormatListResponse;
    cliprdr->ServerFormatDataRequest = onCliprdrServerFormatDataRequest;
    cliprdr->ServerFormatDataResponse = onCliprdrServerFormatDataResponse;
}

/* ---- the channels ------------------------------------------------------- */

/* The capability set the server picked from the pipeline's offer. This is the
   only place it is available: the library keeps it private and the settings
   still say what was requested. 8.1 enables H.264 only with the AVC420 flag
   set, 10 and later unless the AVC-disabled flag is set, and 8.0 never. The
   GDI doesn't use this hook, so there is no handler of its to call. */
static UINT onGfxCapsConfirm(RdpgfxClientContext *gfx, const RDPGFX_CAPS_CONFIRM_PDU *confirm) {
    const rdpGdi *gdi = (const rdpGdi *) gfx->custom;
    if (!gdi || !confirm || !confirm->capsSet) {
        return CHANNEL_RC_OK;
    }
    Session *s = sessionOf(gdi->context);
    const UINT32 version = confirm->capsSet->version;
    const UINT32 flags = confirm->capsSet->flags;
    int h264 = 0;
    if (version == RDPGFX_CAPVERSION_81) {
        h264 = (flags & RDPGFX_CAPS_FLAG_AVC420_ENABLED) != 0;
    } else if (version >= RDPGFX_CAPVERSION_10) {
        h264 = (flags & RDPGFX_CAPS_FLAG_AVC_DISABLED) == 0;
    }
    s->gfxH264 = h264;
    s->gfxVersion = version;
    s->gfxCaps = *confirm->capsSet;
    s->gfxRestarting = 0;
    LOGI("graphics pipeline: server confirmed caps 0x%08x, flags 0x%08x", version, flags);
    return CHANNEL_RC_OK;
}

/* Every surface command, H.264 or not, is decoded inside the GDI's handler,
   where the library creates, resets and feeds each surface's H.264 context;
   between vento_h264_enter and vento_h264_leave those contexts belong to this
   session.

   A decoder that loses track of its stream (a hardware one that failed
   mid-stream and was replaced by software, or one whose mode the user
   changed) has none of the reference pictures, so every command fails and
   nothing is drawn until the next key frame. Windows sends one only when a
   stream starts, not for a Refresh Rect or when output is suppressed and
   allowed again. Advertising the capabilities again makes it respond with
   new surfaces and a new stream, whose contexts get new decoders. */
static UINT onGfxSurfaceCommand(RdpgfxClientContext *context, const RDPGFX_SURFACE_COMMAND *cmd) {
    rdpGdi *gdi = (rdpGdi *) context->custom;
    Session *s = sessionOf(gdi->context);
    vento_h264_enter(&s->h264);
    const UINT rc = s->gdiSurfaceCommand(context, cmd);
    vento_h264_leave();
    if (atomic_exchange(&s->h264.restart, false) && !s->gfxRestarting && s->gfxVersion) {
        s->gfxRestarting = 1;
        RDPGFX_CAPSET caps = s->gfxCaps;
        const RDPGFX_CAPS_ADVERTISE_PDU pdu = {.capsSetCount = 1, .capsSets = &caps};
        const UINT error = context->CapsAdvertise(context, &pdu);
        if (error != CHANNEL_RC_OK) {
            LOGW("the capabilities could not be sent again: 0x%08x", error);
        }
    }
    return rc;
}

/* The display-control channel's capabilities, the only indication that the
   desktop can be resized, which depends on the session rather than the
   backend. The server sends them when the channel opens; before that, a
   layout would be cut down to a monitor count the channel hasn't received.
   Called on the dynamic channels' thread; the values are read on the
   protocol thread. */
static UINT onDisplayControlCaps(DispClientContext *disp, UINT32 maxMonitors, UINT32 factorA,
                                 UINT32 factorB) {
    Session *s = (Session *) disp->custom;
    const UINT32 most = maxMonitors > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : maxMonitors;
    /* The area is the two factors' product per monitor. */
    const uint64_t each = (uint64_t) factorA * factorB;
    const uint64_t area = most > 0 && each > UINT64_MAX / most ? UINT64_MAX : each * most;
    LOGI("display control: the server takes %u monitors of %u x %u pixels", maxMonitors, factorA,
         factorB);
    atomic_store(&s->dispMonitors, most);
    atomic_store(&s->dispArea, area);
    pthread_mutex_lock(&s->lock);
    s->canResize = most > 0;
    Cmd held = {0, s->heldWidth, s->heldHeight, 0, NULL, 0};
    if (s->canResize && s->heldWidth > 0) {
        held.type = CMD_RESIZE;
    } else if (s->canResize && s->wantCount > 0) {
        held.type = CMD_LAYOUT;
    }
    s->heldWidth = s->heldHeight = 0;
    pthread_mutex_unlock(&s->lock);
    if (held.type) {
        post(s, &held);
    }
    return CHANNEL_RC_OK;
}

/* A channel's interface arrives in an event rather than from a call, so the
   pointer is kept: this is the only place it is passed. */
static void onChannelConnected(void *context, const ChannelConnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        /* Without this the pipeline negotiates, the channel opens and nothing
           is ever painted. */
        RdpgfxClientContext *gfx = (RdpgfxClientContext *) e->pInterface;
        gdi_graphics_pipeline_init(ctx->gdi, gfx);
        gfx->CapsConfirm = onGfxCapsConfirm;
        /* Our handlers call the GDI's, as for SurfaceBits: the GDI's reset
           handler does the resize. */
        s->gdiResetGraphics = gfx->ResetGraphics;
        gfx->ResetGraphics = onGfxResetGraphics;
        s->gdiSurfaceCommand = gfx->SurfaceCommand;
        gfx->SurfaceCommand = onGfxSurfaceCommand;
        s->gfxOpen = 1;
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        wireCliprdr(s, (CliprdrClientContext *) e->pInterface);
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = (DispClientContext *) e->pInterface;
        s->disp->custom = s;
        s->disp->DisplayControlCaps = onDisplayControlCaps;
    }
}

static void onChannelDisconnected(void *context, const ChannelDisconnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        gdi_graphics_pipeline_uninit(ctx->gdi, (RdpgfxClientContext *) e->pInterface);
        s->gfxOpen = 0;
        s->gfxVersion = 0;
        s->gfxH264 = 0;
        s->gfxRestarting = 0;
        atomic_store(&s->h264.hardware, -1);
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        s->cliprdr = NULL;
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = NULL;
        s->canResize = 0;
        atomic_store(&s->dispMonitors, 0);
    }
}

/* ---- credential and certificate prompts --------------------------------- */

/* Blocks the protocol thread until the session gets an answer, by design: the
   answer comes from a dialog shown to the user. */
static BOOL onAuthenticate(freerdp *instance, char **username, char **password, char **domain,
                           rdp_auth_reason reason) {
    Session *s = sessionOf(instance->context);
    /* The library only calls this when something is missing: the TLS and RDP
       paths skip it when a user name and a password are both set, and NLA
       calls it only for an empty user name or no password. Arriving here with
       both means one of the other reasons, and the saved credentials are the
       answer; the prompt is for when there are none. Forwarding it to the
       user would show a dialog for a saved password. */
    if (*username && **username && *password && **password) {
        return TRUE;
    }
    LOGI("credentials needed, reason %d", (int) reason);
    pthread_mutex_lock(&s->lock);
    s->credAnswered = 0;
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    s->credUser = s->credDomain = s->credPass = NULL;
    pthread_mutex_unlock(&s->lock);

    s->cb.credentials_needed(s->cb.user, true);

    pthread_mutex_lock(&s->lock);
    while (!s->credAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int ok = s->credPass != NULL;
    if (ok) {
        free(*username);
        free(*password);
        free(*domain);
        *username = strdup(s->credUser ? s->credUser : "");
        *password = strdup(s->credPass);
        *domain = strdup(s->credDomain ? s->credDomain : "");
    }
    pthread_mutex_unlock(&s->lock);
    return ok ? TRUE : FALSE;
}

/* The same wait, for the certificate. No CA vouches for an RDP server's
   certificate (a Windows host signs its own), so the identity is the
   fingerprint, and the app decides, from its pin store or by asking the
   user.

   The library is told the certificate is managed externally (see
   applySettings), so this is its only check. Otherwise it would first look
   the server up in its own store under the config path and connect without
   asking if the stored certificate matches, so any PEM ever placed there
   would override the app, and it would write an accepted certificate into
   that store. Here an acceptance applies to this connection only (the
   library keeps it for a redirect or reconnect of the same session) and
   nothing is written.

   The fingerprint is the one the library's own prompt would show, the
   SHA-256 of the server's certificate, so a pin made from that prompt still
   matches. */
static int onVerifyX509Certificate(freerdp *instance, const BYTE *data, size_t length,
                                   const char *hostname, UINT16 port, DWORD flags) {
    (void) hostname; (void) port; (void) flags;
    Session *s = sessionOf(instance->context);

    /* The chain as PEM, the server's certificate first, which is the one
       parsed here. Copied to add a terminator, since only the length bounds
       it. */
    char *pem = data ? malloc(length + 1) : NULL;
    if (!pem) {
        return 0;
    }
    memcpy(pem, data, length);
    pem[length] = '\0';
    rdpCertificate *cert = freerdp_certificate_new_from_pem(pem);
    free(pem);
    char *fingerprint = cert ? freerdp_certificate_get_fingerprint(cert) : NULL;
    freerdp_certificate_free(cert);
    if (!fingerprint) {
        return 0;
    }

    pthread_mutex_lock(&s->lock);
    s->trustAnswered = 0;
    s->trustAccepted = 0;
    pthread_mutex_unlock(&s->lock);

    s->cb.trust_needed(s->cb.user, fingerprint);
    free(fingerprint);

    pthread_mutex_lock(&s->lock);
    while (!s->trustAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int accepted = s->trustAccepted && !s->quit;
    pthread_mutex_unlock(&s->lock);
    /* 0 rather than -1 for a rejection: both refuse, and -1 also makes the
       library log the whole certificate as an error. */
    return accepted ? 1 : 0;
}

/* ---- keysyms to scancodes ----------------------------------------------- */

/*
 * The one place where the protocol doesn't match the rest of the stack, which
 * uses X11 keysyms (RFB's representation, used by the control layer and the
 * clients). RDP's keyboard uses scancodes: set 1, with an E0 prefix for the
 * keys the original PC keyboard didn't have.
 *
 * Three decisions follow:
 *
 *   1. A scancode is a position, not a character: 0x1E is the key left of S,
 *      which is `a` on a US layout and `q` on a French one. So this table is a
 *      US layout, the connection requests a US layout, and the server's layout
 *      maps each position back to a character.
 *   2. Characters the layout has no key for are sent as Unicode keyboard
 *      events, which is how a phone's IME sends `é` and `→` without a layout
 *      in between. They carry no modifier state, so they are only a fallback.
 *   3. Shift is part of the mapping: `!` is not a key but `1` with Shift. The
 *      caller synthesises the modifier and must not synthesise one the user is
 *      already holding.
 */

#define KEY_EXT 0xE000u
#define KEY_NONE 0xFFFFu
/* Pause has no scancode of its own; the library has a separate call for the
   sequence mstsc sends instead. */
#define KEY_PAUSE 0xFFFEu

static UINT16 namedScancode(UINT32 keysym) {
    switch (keysym) {
        case 0xffe1: return 0x2a;             /* Shift_L */
        case 0xffe2: return 0x36;             /* Shift_R */
        case 0xffe3: return 0x1d;             /* Control_L */
        case 0xffe4: return KEY_EXT | 0x1d;   /* Control_R */
        case 0xffe9: return 0x38;             /* Alt_L */
        case 0xffea: case 0xfe03: return KEY_EXT | 0x38; /* Alt_R, ISO_Level3_Shift: both AltGr */
        case 0xffeb: return KEY_EXT | 0x5b;   /* Super_L */
        case 0xffec: return KEY_EXT | 0x5c;   /* Super_R */
        case 0xff67: return KEY_EXT | 0x5d;   /* Menu */
        case 0xffe5: return 0x3a;             /* Caps_Lock */
        case 0xff7f: return 0x45;             /* Num_Lock */
        case 0xff14: return 0x46;             /* Scroll_Lock */
        case 0xff08: return 0x0e;             /* BackSpace */
        case 0xff09: return 0x0f;             /* Tab */
        case 0xff0d: return 0x1c;             /* Return */
        case 0xff1b: return 0x01;             /* Escape */
        case 0xffff: return KEY_EXT | 0x53;   /* Delete */
        case 0xff63: return KEY_EXT | 0x52;   /* Insert */
        case 0xff50: return KEY_EXT | 0x47;   /* Home */
        case 0xff57: return KEY_EXT | 0x4f;   /* End */
        case 0xff55: return KEY_EXT | 0x49;   /* Page_Up */
        case 0xff56: return KEY_EXT | 0x51;   /* Page_Down */
        case 0xff51: return KEY_EXT | 0x4b;   /* Left */
        case 0xff52: return KEY_EXT | 0x48;   /* Up */
        case 0xff53: return KEY_EXT | 0x4d;   /* Right */
        case 0xff54: return KEY_EXT | 0x50;   /* Down */
        case 0xff61: return KEY_EXT | 0x37;   /* Print */
        case 0xff13: return KEY_PAUSE;
        case 0xffc8: return 0x57;             /* F11, which is not after F10 */
        case 0xffc9: return 0x58;             /* F12 */

        /* The keypad. Each key has two keysyms, one per Num Lock state, and
           both map to the same scancode, which is all RDP can express. The far
           end's Num Lock decides which of the two it produces, so this is the
           one place where the near end's lock state doesn't apply: with the
           lock off here and on there, KP_Home arrives as 7. */
        case 0xffb7: case 0xff95: return 0x47;   /* KP_7, KP_Home */
        case 0xffb8: case 0xff97: return 0x48;   /* KP_8, KP_Up */
        case 0xffb9: case 0xff9a: return 0x49;   /* KP_9, KP_Prior */
        case 0xffb4: case 0xff96: return 0x4b;   /* KP_4, KP_Left */
        case 0xffb5: case 0xff9d: return 0x4c;   /* KP_5, KP_Begin */
        case 0xffb6: case 0xff98: return 0x4d;   /* KP_6, KP_Right */
        case 0xffb1: case 0xff9c: return 0x4f;   /* KP_1, KP_End */
        case 0xffb2: case 0xff99: return 0x50;   /* KP_2, KP_Down */
        case 0xffb3: case 0xff9b: return 0x51;   /* KP_3, KP_Next */
        case 0xffb0: case 0xff9e: return 0x52;   /* KP_0, KP_Insert */
        case 0xffae: case 0xff9f: return 0x53;   /* KP_Decimal, KP_Delete */
        case 0xffaa: return 0x37;                /* KP_Multiply */
        case 0xffad: return 0x4a;                /* KP_Subtract */
        case 0xffab: return 0x4e;                /* KP_Add */
        case 0xffaf: return KEY_EXT | 0x35;      /* KP_Divide */
        case 0xff8d: return KEY_EXT | 0x1c;      /* KP_Enter */
        case 0xffac: return 0x7e;                /* KP_Separator, the Brazilian keypad comma */

        /* The three unambiguous conversion keys of a Japanese keyboard, which
           are the ones the control layer sends. */
        case 0xff27: return 0x70;             /* Hiragana_Katakana */
        case 0xff23: return 0x79;             /* Henkan_Mode */
        case 0xff22: return 0x7b;             /* Muhenkan */
        default:
            /* F1–F10 are consecutive; the keypad took the codes after them. */
            if (keysym >= 0xffbe && keysym <= 0xffc7) {
                return (UINT16) (0x3b + (keysym - 0xffbe));
            }
            /* F13–F24, which no PC keyboard has but every layout still has
               scancodes for. F13–F23 run from 0x64; F24 doesn't follow them: a
               Windows far end maps the next scancode (0x6F) to VK_OEM_PA3
               (KBD4_T6F in winpr's table), and F24 is KBD4_T76. */
            if (keysym >= 0xffca && keysym <= 0xffd4) {
                return (UINT16) (0x64 + (keysym - 0xffca));
            }
            if (keysym == 0xffd5) {
                return 0x76;   /* F24 */
            }
            return KEY_NONE;
    }
}

/* The printable keys of a US layout, in keyboard row order rather than
   sorted, so a wrong entry shows up as a wrong neighbour. */
static const struct {
    UINT16 code;
    UINT16 plain, shifted;
} US_LAYOUT[] = {
        {0x29, '`', '~'}, {0x02, '1', '!'}, {0x03, '2', '@'}, {0x04, '3', '#'},
        {0x05, '4', '$'}, {0x06, '5', '%'}, {0x07, '6', '^'}, {0x08, '7', '&'},
        {0x09, '8', '*'}, {0x0a, '9', '('}, {0x0b, '0', ')'}, {0x0c, '-', '_'},
        {0x0d, '=', '+'},
        {0x10, 'q', 'Q'}, {0x11, 'w', 'W'}, {0x12, 'e', 'E'}, {0x13, 'r', 'R'},
        {0x14, 't', 'T'}, {0x15, 'y', 'Y'}, {0x16, 'u', 'U'}, {0x17, 'i', 'I'},
        {0x18, 'o', 'O'}, {0x19, 'p', 'P'}, {0x1a, '[', '{'}, {0x1b, ']', '}'},
        {0x2b, '\\', '|'},
        {0x1e, 'a', 'A'}, {0x1f, 's', 'S'}, {0x20, 'd', 'D'}, {0x21, 'f', 'F'},
        {0x22, 'g', 'G'}, {0x23, 'h', 'H'}, {0x24, 'j', 'J'}, {0x25, 'k', 'K'},
        {0x26, 'l', 'L'}, {0x27, ';', ':'}, {0x28, '\'', '"'},
        {0x2c, 'z', 'Z'}, {0x2d, 'x', 'X'}, {0x2e, 'c', 'C'}, {0x2f, 'v', 'V'},
        {0x30, 'b', 'B'}, {0x31, 'n', 'N'}, {0x32, 'm', 'M'}, {0x33, ',', '<'},
        {0x34, '.', '>'}, {0x35, '/', '?'}, {0x39, ' ', ' '},
};

/* The inverse of `Keysym.fromUnicode`: Latin-1 is itself, everything else is
   0x01000000 | codepoint. */
static UINT32 keysymToUnicode(UINT32 keysym) {
    if ((keysym & 0xff000000u) == 0x01000000u) {
        return keysym & 0x00ffffffu;
    }
    if ((keysym >= 0x20 && keysym <= 0x7e) || (keysym >= 0xa0 && keysym <= 0xff)) {
        return keysym;
    }
    return 0;
}

/* Maps a keysym to one of three results: a scancode, a scancode with Shift,
   or a character with no scancode. */
static void mapKeysym(UINT32 keysym, UINT16 *code, int *shift, WCHAR *unicode) {
    *code = KEY_NONE;
    *shift = 0;
    *unicode = 0;
    const UINT16 named = namedScancode(keysym);
    if (named != KEY_NONE) {
        *code = named;
        return;
    }
    /* The only keypad key with no PC scancode (FreeRDP's xkb table has
       KP_Equal as unknown); it names a character, so it is sent as one. */
    if (keysym == 0xffbd) {
        *unicode = '=';
        return;
    }
    const UINT32 cp = keysymToUnicode(keysym);
    if (cp == 0 || cp > 0xFFFF) {
        return;
    }
    for (size_t i = 0; i < sizeof(US_LAYOUT) / sizeof(US_LAYOUT[0]); i++) {
        if (US_LAYOUT[i].plain == cp) {
            *code = US_LAYOUT[i].code;
            return;
        }
        if (US_LAYOUT[i].shifted == cp) {
            *code = US_LAYOUT[i].code;
            *shift = 1;
            return;
        }
    }
    *unicode = (WCHAR) cp;
}

/* ---- input, on the protocol thread -------------------------------------- */

static void sendScancode(Session *s, UINT16 code, int down) {
    rdpInput *input = s->instance->context->input;
    if (code == KEY_PAUSE) {
        /* The whole sequence is sent on the press, so the release sends
           nothing. The library sends what mstsc does (an E1-flagged Ctrl
           around a Num Lock) rather than the E1 1D 45 a keyboard sends. */
        if (down) {
            freerdp_input_send_keyboard_pause_event(input);
        }
        return;
    }
    UINT16 flags = down ? KBD_FLAGS_DOWN : KBD_FLAGS_RELEASE;
    if (code & KEY_EXT) {
        flags |= KBD_FLAGS_EXTENDED;
    }
    freerdp_input_send_keyboard_event(input, flags, (UINT8) (code & 0xFFu));
}

static void doKeyDown(Session *s, UINT32 keysym, INT64 keyId) {
    if (s->viewOnly || !s->connected) {
        return;
    }
    UINT16 code;
    int shift;
    WCHAR unicode;
    mapKeysym(keysym, &code, &shift, &unicode);
    if (code == KEY_NONE && unicode == 0) {
        return;
    }

    /* A second press of a key already down is auto-repeat, which RDP sends as
       another key down. Recording it again would fill the table and then
       silently drop every later key. */
    for (int i = 0; i < s->nkeys; i++) {
        if (s->keys[i].id == keyId) {
            if (s->keys[i].unicode) {
                freerdp_input_send_unicode_keyboard_event(s->instance->context->input,
                                                          0, s->keys[i].unicode);
            } else {
                sendScancode(s, s->keys[i].code, 1);
            }
            return;
        }
    }
    if (s->nkeys >= MAX_HELD_KEYS) {
        return;
    }

    if (unicode) {
        freerdp_input_send_unicode_keyboard_event(s->instance->context->input, 0, unicode);
    } else {
        /* Synthesised only when the user isn't already holding Shift:
           otherwise releasing a capital typed with Shift down would also
           release Shift, and the far end would see it released mid-word. */
        if (shift && !s->shiftHeld) {
            sendScancode(s, 0x2a, 1);
        }
        sendScancode(s, code, 1);
        if (keysym == 0xffe1 || keysym == 0xffe2) {
            s->shiftHeld = 1;
        }
    }
    s->keys[s->nkeys].id = keyId;
    s->keys[s->nkeys].code = code;
    s->keys[s->nkeys].shift = shift && !s->shiftHeld;
    s->keys[s->nkeys].unicode = unicode;
    s->nkeys++;
}

static void releaseAt(Session *s, int index) {
    if (s->keys[index].unicode) {
        /* A character needs a release as a scancode does: Windows turns a
           Unicode event into a key whose down and up go to the application
           like any other key's (xfreerdp sends the release too). */
        freerdp_input_send_unicode_keyboard_event(s->instance->context->input, KBD_FLAGS_RELEASE,
                                                  s->keys[index].unicode);
    } else {
        sendScancode(s, s->keys[index].code, 0);
        if (s->keys[index].shift) {
            sendScancode(s, 0x2a, 0);
        }
        if (s->keys[index].code == 0x2a || s->keys[index].code == 0x36) {
            s->shiftHeld = 0;
        }
    }
    s->keys[index] = s->keys[--s->nkeys];
}

static void doKeyUp(Session *s, INT64 keyId) {
    for (int i = 0; i < s->nkeys; i++) {
        if (s->keys[i].id == keyId) {
            if (!s->viewOnly && s->connected) {
                releaseAt(s, i);
            } else {
                s->keys[i] = s->keys[--s->nkeys];
            }
            return;
        }
    }
}

static void doReleaseAll(Session *s) {
    while (s->nkeys > 0) {
        if (!s->viewOnly && s->connected) {
            releaseAt(s, s->nkeys - 1);
        } else {
            s->nkeys--;
        }
    }
    s->shiftHeld = 0;
}

/* An absolute position and an RFB button mask, as used throughout the control
   stack. Bits 0x01 to 0x04, 0x80 and 0x100 are buttons and 0x08 to 0x40 are
   wheel notches. RDP has real wheel events rather than RFB's wheel buttons,
   so a button is sent on every change and a wheel notch only when its bit is
   newly set. */
static void doPointer(Session *s, int x, int y, UINT16 mask) {
    if (s->viewOnly || !s->connected) {
        return;
    }
    rdpInput *input = s->instance->context->input;
    const UINT16 previous = s->buttons;
    s->buttons = mask;

    /* The move is sent first, so a click lands at the new position. */
    freerdp_input_send_mouse_event(input, PTR_FLAGS_MOVE, (UINT16) x, (UINT16) y);

    static const struct { UINT16 bit, flag; } BUTTONS[] = {
            {0x01, PTR_FLAGS_BUTTON1},
            {0x02, PTR_FLAGS_BUTTON3}, /* middle: RDP numbers them differently */
            {0x04, PTR_FLAGS_BUTTON2},
    };
    for (size_t i = 0; i < sizeof(BUTTONS) / sizeof(BUTTONS[0]); i++) {
        const int now = (mask & BUTTONS[i].bit) != 0;
        const int was = (previous & BUTTONS[i].bit) != 0;
        if (now != was) {
            freerdp_input_send_mouse_event(input,
                                           (UINT16) (BUTTONS[i].flag | (now ? PTR_FLAGS_DOWN : 0)),
                                           (UINT16) x, (UINT16) y);
        }
    }
    /* Buttons 8 and 9, RDP's X1 and X2, which Windows uses for browser back
       and forward. */
    static const struct { UINT16 bit, flag; } EXTENDED[] = {
            {0x80, PTR_XFLAGS_BUTTON1},
            {0x100, PTR_XFLAGS_BUTTON2},
    };
    for (size_t i = 0; i < sizeof(EXTENDED) / sizeof(EXTENDED[0]); i++) {
        const int now = (mask & EXTENDED[i].bit) != 0;
        const int was = (previous & EXTENDED[i].bit) != 0;
        if (now != was) {
            freerdp_input_send_extended_mouse_event(input,
                                                    (UINT16) (EXTENDED[i].flag
                                                              | (now ? PTR_XFLAGS_DOWN : 0)),
                                                    (UINT16) x, (UINT16) y);
        }
    }
    /* RDP counts wheel rotation in 120ths of a click, as Windows does
       everywhere; RFB counts clicks, as button presses. */
    static const struct { UINT16 bit, flags; } WHEEL[] = {
            {0x08, PTR_FLAGS_WHEEL | 120},
            {0x10, PTR_FLAGS_WHEEL | PTR_FLAGS_WHEEL_NEGATIVE | 0x88}, /* -120 in nine bits */
            {0x20, PTR_FLAGS_HWHEEL | PTR_FLAGS_WHEEL_NEGATIVE | 0x88},
            {0x40, PTR_FLAGS_HWHEEL | 120},
    };
    for (size_t i = 0; i < sizeof(WHEEL) / sizeof(WHEEL[0]); i++) {
        if ((mask & WHEEL[i].bit) && !(previous & WHEEL[i].bit)) {
            freerdp_input_send_mouse_event(input, WHEEL[i].flags, (UINT16) x, (UINT16) y);
        }
    }
}

static void doClipboard(Session *s, WCHAR *text, size_t len) {
    pthread_mutex_lock(&s->lock);
    free(s->clipOut);
    s->clipOut = text;
    s->clipOutLen = len;
    pthread_mutex_unlock(&s->lock);
    if (s->cliprdr && !s->viewOnly) {
        sendFormatList(s, text != NULL);
    }
}

/* Unlike an RFB client, which pauses by not requesting updates, an RDP client
   receives updates unrequested, so losing focus sends a Suppress Output PDU
   telling the server to stop, and the server redraws when output is allowed
   again. It affects only the picture; sound keeps playing (see
   applySettings). */
static void doFocus(Session *s, int focused) {
    if (!s->connected || !s->instance->context->gdi) {
        return;
    }
    gdi_send_suppress_output(s->instance->context->gdi, focused ? FALSE : TRUE);
}

/* Holds a size or layout requested before the server has sent its
   display-control capabilities (onDisplayControlCaps), which on Windows arrive
   a second or so after the session connects. Only the latest is kept, and it
   is sent when the capabilities arrive, or never if the server has no such
   channel. `width` 0 means the layout op_request_layout stored, which stays
   in place. Returns whether it was held. Checked under `lock`, as the
   capabilities are stored, so they can't arrive between the check and the
   hold. */
static bool holdUntilCaps(Session *s, int width, int height) {
    pthread_mutex_lock(&s->lock);
    const bool hold = !s->canResize;
    if (hold) {
        s->heldWidth = width;
        s->heldHeight = height;
        if (width > 0) {
            s->wantCount = 0;
        }
    }
    pthread_mutex_unlock(&s->lock);
    if (hold) {
        LOGI("the desktop's new shape waits for the server to say what it takes");
    }
    return hold;
}

/* A desktop of `width × height` in total, not per monitor: everywhere else in
   the session interface the size is the framebuffer's. So the requested
   monitors divide the width between them side by side, the last taking the
   remainder; requesting the whole width for each made a two-monitor desktop
   twice as wide as requested. (IronRDP's backend divides it the same way.) */
static void doResize(Session *s, int width, int height) {
    if (holdUntilCaps(s, width, height)) {
        return;
    }
    /* No more monitors than the server supports, so a single-monitor server
       still gets the whole size rather than its first part. */
    const int most = (int) atomic_load(&s->dispMonitors);
    int count = s->monitors < 1 ? 1
                : (s->monitors > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : s->monitors);
    count = most > 0 && count > most ? most : count;
    const int each = width / count;
    int32_t rects[4 * VENTO_MAX_MONITORS];
    for (int i = 0; i < count; i++) {
        rects[4 * i + 0] = i * each;
        rects[4 * i + 1] = 0;
        rects[4 * i + 2] = i == count - 1 ? width - each * (count - 1) : each;
        rects[4 * i + 3] = height;
    }
    sendLayout(s, rects, count);
}

/* Sends the latest layout op_request_layout stored: requests made faster
   than the thread drains them supersede each other. */
static void doLayout(Session *s) {
    int32_t rects[4 * VENTO_MAX_MONITORS];
    if (holdUntilCaps(s, 0, 0)) {
        return;
    }
    pthread_mutex_lock(&s->lock);
    const int n = s->wantCount;
    memcpy(rects, s->wantLayout, sizeof(int32_t) * 4 * (size_t) n);
    s->wantCount = 0;
    pthread_mutex_unlock(&s->lock);
    sendLayout(s, rects, n);
}

static void drainCommands(Session *s) {
    for (;;) {
        Cmd c;
        pthread_mutex_lock(&s->lock);
        if (s->head == s->tail) {
            const int dropped = s->dropped;
            s->dropped = 0;
            pthread_mutex_unlock(&s->lock);
            if (dropped > 0) {
                LOGW("input: caught up, %d commands dropped on the way", dropped);
            }
            return;
        }
        c = s->ring[s->tail];
        s->tail = (s->tail + 1) % CMD_RING;
        pthread_mutex_unlock(&s->lock);

        switch (c.type) {
            case CMD_POINTER: doPointer(s, c.a, c.b, (UINT16) c.c); break;
            case CMD_KEY_DOWN: doKeyDown(s, (UINT32) c.a, c.c); break;
            case CMD_KEY_UP: doKeyUp(s, c.c); break;
            case CMD_RELEASE_KEYS: doReleaseAll(s); break;
            case CMD_CLIPBOARD: doClipboard(s, c.text, c.textLen); c.text = NULL; break;
            case CMD_FOCUS: doFocus(s, c.a); break;
            case CMD_RESIZE: doResize(s, c.a, c.b); break;
            case CMD_LAYOUT: doLayout(s); break;
            default: break;
        }
        free(c.text);
    }
}

/* ---- the connecting screen's status --------------------------------------- */

/* How long a step of the connection lasts before the connecting screen names
   it: most are over in milliseconds, and a line that flashes past unread is
   noise. Those that can last are the authentication (a slow domain
   controller) and the steps after it, where Windows prepares the session and
   may wait for its licence server, and any of them over a slow link. */
#define STATUS_DELAY_MS 1000

static const char STATUS_AUTHENTICATING[] = "Authenticating…";
static const char STATUS_STARTING[] = "Starting the session…";

/* What a connection state is worth saying, or NULL: the authentication, and
   everything after it until the session is active. */
static const char *statusOfState(int state) {
    if (state == CONNECTION_STATE_NLA || state == CONNECTION_STATE_AAD) {
        return STATUS_AUTHENTICATING;
    }
    if (state >= CONNECTION_STATE_MCS_CREATE_REQUEST && state < CONNECTION_STATE_ACTIVE) {
        return STATUS_STARTING;
    }
    return NULL;
}

/* Shows the pending status once it is due. A thread of its own, since the
   protocol thread is inside freerdp_connect for the whole of the wait. */
static void *statusThread(void *arg) {
    Session *s = (Session *) arg;
    pthread_mutex_lock(&s->lock);
    while (!s->statusStop) {
        if (!s->statusPending) {
            pthread_cond_wait(&s->statusCond, &s->lock);
            continue;
        }
        const struct timespec due = s->statusDue;
        if (pthread_cond_timedwait(&s->statusCond, &s->lock, &due) != ETIMEDOUT) {
            continue;
        }
        if (s->statusStop || !s->statusPending) {
            continue;
        }
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec < s->statusDue.tv_sec
            || (now.tv_sec == s->statusDue.tv_sec && now.tv_nsec < s->statusDue.tv_nsec)) {
            continue; /* moved while this waited */
        }
        const char *text = s->statusPending;
        s->statusPending = NULL;
        s->statusShown = text;
        pthread_mutex_unlock(&s->lock);
        LOGI("status: %s", text);
        s->cb.status(s->cb.user, text);
        pthread_mutex_lock(&s->lock);
    }
    pthread_mutex_unlock(&s->lock);
    return NULL;
}

/* The protocol thread's: ends the status thread, whose last status then
   comes before whatever this thread reports next. */
static void stopStatus(Session *s) {
    pthread_mutex_lock(&s->lock);
    const int started = s->statusStarted;
    s->statusStop = 1;
    s->statusPending = NULL;
    pthread_cond_broadcast(&s->statusCond);
    pthread_mutex_unlock(&s->lock);
    if (started) {
        pthread_join(s->statusThread, NULL);
        pthread_mutex_lock(&s->lock);
        s->statusStarted = 0;
        pthread_mutex_unlock(&s->lock);
    }
}

/* Each step of the connection sequence: the status it names is due a
   second from now, or at once if one is already up (the wait is already a
   long one), and a status that no longer applies is cleared at once.
   StateChanged rather than ConnectionStateChange, which the library declares
   but never registers, so a handler for it is never called. */
static void onStateChanged(void *context, const StateChangedEventArgs *e) {
    Session *s = sessionOf((rdpContext *) context);
    const char *text = statusOfState((int) e->newState);
    int clear = 0;
    const char *now = NULL;
    pthread_mutex_lock(&s->lock);
    const char *coming = s->statusPending ? s->statusPending : s->statusShown;
    if (s->statusStop || text == coming) {
        /* nothing new */
    } else if (text == s->statusShown || e->newState == CONNECTION_STATE_ACTIVE) {
        /* Back to what is shown, or connected, where the screen drops what
           was shown by itself. */
        s->statusPending = NULL;
    } else if (s->statusShown && text) {
        s->statusPending = NULL;
        s->statusShown = text;
        now = text;
    } else {
        clear = s->statusShown != NULL;
        s->statusShown = NULL;
        s->statusPending = text;
        if (text) {
            clock_gettime(CLOCK_MONOTONIC, &s->statusDue);
            s->statusDue.tv_sec += STATUS_DELAY_MS / 1000;
            s->statusDue.tv_nsec += (long) (STATUS_DELAY_MS % 1000) * 1000000L;
            if (s->statusDue.tv_nsec >= 1000000000L) {
                s->statusDue.tv_sec++;
                s->statusDue.tv_nsec -= 1000000000L;
            }
            if (!s->statusStarted) {
                s->statusStarted = pthread_create(&s->statusThread, NULL, statusThread, s) == 0;
            }
        }
        pthread_cond_broadcast(&s->statusCond);
    }
    pthread_mutex_unlock(&s->lock);
    if (now) {
        LOGI("status: %s", now);
    }
    if (now || clear) {
        s->cb.status(s->cb.user, now);
    }
}

/* ---- the connection ----------------------------------------------------- */

static BOOL onPreConnect(freerdp *instance) {
    rdpContext *ctx = instance->context;
    PubSub_SubscribeChannelConnected(ctx->pubSub, onChannelConnected);
    PubSub_SubscribeChannelDisconnected(ctx->pubSub, onChannelDisconnected);
    PubSub_SubscribeStateChanged(ctx->pubSub, onStateChanged);
    /* Set before connecting, since the Monitor Layout PDU arrives in the
       connection sequence. */
    ctx->update->RemoteMonitors = onRemoteMonitors;
    return TRUE;
}

/* Idempotent on purpose: upstream calls this callback from two places (the
   activation sequence and the end of freerdp_connect), and which one runs
   depends on the server. A second call would re-initialise the GDI under a
   framebuffer the first had already published. */
static BOOL onPostConnect(freerdp *instance) {
    Session *s = sessionOf(instance->context);
    if (s->connected) {
        return TRUE;
    }
    rdpSettings *settings = instance->context->settings;
    const UINT32 w = freerdp_settings_get_uint32(settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(settings, FreeRDP_DesktopHeight);

    /* BGRX because every codec here decodes to it, so the library's copy into
       this buffer is a memcpy per row. The session's framebuffer is RGBX,
       which differs by two swapped bytes, and onEndPaint swaps them in a pass
       it makes anyway. Asking the GDI for RGBX instead moves the swap into
       FreeRDP's generic converter, which works a pixel at a time and took a
       fifth of the decode thread when measured. */
    if (!gdi_init(instance, PIXEL_FORMAT_BGRX32)) {
        return FALSE;
    }
    if (!registerPointer(instance->context->graphics) || !allocFramebuffer(s, (int) w, (int) h)) {
        return FALSE;
    }
    rdpUpdate *update = instance->context->update;
    update->BeginPaint = onBeginPaint;
    update->EndPaint = onEndPaint;
    update->DesktopResize = onDesktopResize;
    update->pointer->PointerPosition = onPointerPosition;
    /* Ours calls the GDI's handler rather than replacing it: the GDI's
       handler does the drawing. */
    s->gdiSurfaceBits = update->SurfaceBits;
    update->SurfaceBits = onSurfaceBits;

    /* The input the server accepts. Each is the client's request ANDed with
       the server's offer during capability exchange, so after PostConnect
       they describe this session rather than the client. The relative pointer
       is not used yet; whether servers offer it decides whether a relative
       pointer over RDP is possible. */
    LOGI("server input: relative pointer %s, horizontal wheel %s, unicode %s",
         freerdp_settings_get_bool(settings, FreeRDP_HasRelativeMouseEvent) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_HasHorizontalWheel) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_UnicodeInput) ? "yes" : "no");

    s->connected = 1;
    s->cb.connected(s->cb.user, (int) w, (int) h);
    return TRUE;
}

static void onPostDisconnect(freerdp *instance) {
    gdi_free(instance);
}

static void applySettings(Session *s, rdpSettings *settings) {
    freerdp_settings_set_string(settings, FreeRDP_ServerHostname, s->host);
    freerdp_settings_set_uint32(settings, FreeRDP_ServerPort, (UINT32) s->port);
    freerdp_settings_set_string(settings, FreeRDP_Username, s->user ? s->user : "");
    freerdp_settings_set_string(settings, FreeRDP_Domain, s->domain ? s->domain : "");
    if (s->password) {
        freerdp_settings_set_string(settings, FreeRDP_Password, s->password);
    }
    freerdp_settings_set_string(settings, FreeRDP_ClientHostname, s->clientName);
    /* The major type is the near end's platform, as FreeRDP's X11 and Wayland
       clients and IronRDP send it. The minor type stays unspecified, as in
       IronRDP: FreeRDP's own clients put their display stack there (an X
       server, Wayland, SDL), but this library draws into a buffer for
       whichever window system the client uses, and Android has no minor
       type. */
    freerdp_settings_set_uint32(settings, FreeRDP_OsMajorType, s->osMajorType);
    freerdp_settings_set_uint32(settings, FreeRDP_OsMinorType, OSMINORTYPE_UNSPECIFIED);
    freerdp_settings_set_uint32(settings, FreeRDP_KeyboardLayout, (UINT32) s->keyboardLayout);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth, (UINT32) s->width);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopHeight, (UINT32) s->height);
    freerdp_settings_set_uint32(settings, FreeRDP_ColorDepth, 32);
    freerdp_settings_set_bool(settings, FreeRDP_SoftwareGdi, TRUE);
    freerdp_settings_set_uint32(settings, FreeRDP_TcpConnectTimeout,
                                (UINT32) (s->connectTimeoutMs > 0 ? s->connectTimeoutMs : 20000));
    /* No proxy. With the type left at PROXY_TYPE_NONE, the library reads
       https_proxy, HTTPS_PROXY and no_proxy from the environment at connect
       and sends the connection through the proxy named there, without the
       session showing it. */
    freerdp_settings_set_uint32(settings, FreeRDP_ProxyType, PROXY_TYPE_IGNORE);

    /* Where the library would keep its certificate store. Nothing is kept
       there (the certificate is managed outside the library, below), but the
       TLS layer still sets up a store at connect, and without a path for one
       the connection fails before it starts. */
    if (s->configPath) {
        freerdp_settings_set_string(settings, FreeRDP_ConfigPath, s->configPath);
    }
    /* Already off, since nothing in the library turns it on; set explicitly
       because when on, freerdp_client_start puts command-line prompts back
       over every callback, and a phone has no terminal for them. */
    freerdp_settings_set_bool(settings, FreeRDP_UseCommonStdioCallbacks, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_IgnoreCertificate, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_AutoAcceptCertificate, FALSE);
    /* Every certificate goes to onVerifyX509Certificate only, not to the
       known-hosts store under the config path, its JSON certificate list, or
       a certificate authority. The app alone decides, and the library writes
       nothing. */
    freerdp_settings_set_bool(settings, FreeRDP_ExternalCertificateManagement, TRUE);

    /* Network Level Authentication: preferred, required, or refused. Nothing
       here turns off TLS: a server that offers only the RDP security layer
       sends the password protected only by a 1990s cipher, and the connection
       should fail.

       NLA has been two switches since the library turned ExtSecurity on by
       default in 3.32.0. That is HYBRID_EX: the same CredSSP plus the Early
       User Authorization Result PDU, which reports "this account may not log
       in here" as an error instead of a login screen saying so. It has its
       own switch and is negotiated before plain NLA, so "Never" has to turn
       off both or CredSSP happens anyway; "If supported" tries HYBRID_EX
       first, as every client of this library now does. */
    const int nlaOff = s->nla && strcmp(s->nla, "off") == 0;
    const int nlaRequired = s->nla && strcmp(s->nla, "require") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_NlaSecurity, !nlaOff);
    freerdp_settings_set_bool(settings, FreeRDP_ExtSecurity, !nlaOff);
    freerdp_settings_set_bool(settings, FreeRDP_TlsSecurity, !nlaRequired);
    freerdp_settings_set_bool(settings, FreeRDP_RdpSecurity, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_UseRdpSecurityLayer, FALSE);

    /* The minimum TLS version. 3.27.0 raised it to TLS 1.2 at OpenSSL's
       security level 2, from TLS 1.0 at level 1 set in 3.19.1. Level 1
       already refuses TLS 1.0 and 1.1 in OpenSSL 3, along with their SHA-1
       signatures, so for this app the raise only removed keys under 2048
       bits, and this option restores more than that. Level 0, because it is
       the only level at which a handshake can use the lower versions. With
       the option off, both are the library's defaults. */
    if (s->legacyTls) {
        freerdp_settings_set_uint16(settings, FreeRDP_TLSMinVersion, TLS1_VERSION);
        freerdp_settings_set_uint32(settings, FreeRDP_TlsSecLevel, FREERDP_TLS_SECLEVEL_0);
    }

    freerdp_settings_set_bool(settings, FreeRDP_CompressionEnabled, s->compression != 0);
    freerdp_settings_set_bool(settings, FreeRDP_RedirectClipboard, TRUE);
    freerdp_settings_set_bool(settings, FreeRDP_SupportDisplayControl, TRUE);

    /* The picture. Windows 8 and later prefer the graphics pipeline, and it is
       the only path that carries H.264, which this build decodes with the
       platform's decoders (h264_vento.h): MediaCodec on Android, FFmpeg on
       Linux. RemoteFX is the older codec, and bitmaps are the fallback when a
       server supports neither. */
    const char *g = s->graphics ? s->graphics : "gfx";
    const int gfx = strcmp(g, "gfx") == 0 || strcmp(g, "gfx-novideo") == 0;
    /* Without decoders (as in the probe), nothing that needs one is
       offered. */
    const int h264 = strcmp(g, "gfx") == 0 && s->h264.available;
    const int rfx = gfx || strcmp(g, "rfx") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteFxCodec, rfx);
    /* One switch, not two: with H264 on and AVC444 off, the client advertises
       capability versions up to 8.1 only (every version from 10 up is inside
       a branch that needs both), so "H.264 but only AVC420" also means no
       progressive v2, no scaling and no error-reporting caps. The useful
       choice is video or no video, which is this pair. */
    freerdp_settings_set_bool(settings, FreeRDP_GfxH264, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxAVC444, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxProgressive, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxPlanar, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxSmallCache, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_GfxThinClient, FALSE);
    /* Off, the library's default, set explicitly because turning it on looks
       like an obvious improvement: only the legacy path uses cache orders, and
       on the damage workload the cache misses every time and costs 0.97 MiB
       against 0.77. */
    freerdp_settings_set_bool(settings, FreeRDP_BitmapCacheEnabled, FALSE);

    /* Sound: the two settings decide what the client info tells the server
       (play it here, play it on the remote machine, or play nothing), and the
       library loads the channel that carries it from the first one, as both a
       static and a dynamic channel, by its own design. Here means the
       platform's output, through the module's device (rdpsnd_vento.h), which
       both channels are told to use by name. A session given no output
       doesn't ask for the server's sound at all, rather than have the server
       send it only to be discarded.

       "off" and "remote" get a device that discards every sample: autodetect,
       which is on below, makes the library register device redirection, and
       device redirection without rdpsnd requested adds rdpsnd with the
       library's silent device anyway, on a channel the client info has
       already told the server not to play anything on.

       Hiding the window doesn't stop it. The Suppress Output PDU that pauses
       the picture doesn't cover sound, and the server keeps sending it as it
       plays, which is right for a minimised window. Since nothing is held
       back while hidden, nothing arrives late on restore either. */
    const int soundAsked = s->sound && strcmp(s->sound, "local") == 0;
    const int soundHere = soundAsked && s->audio.available;
    const int soundThere = s->sound && strcmp(s->sound, "remote") == 0;
    if (soundAsked && !soundHere) {
        LOGI("sound: this session has no output, so the server is not asked for its sound");
    }
    if (soundHere) {
        char latency[32];
        snprintf(latency, sizeof latency, "latency:%d", VENTO_RDPSND_LATENCY_MS);
        const char *const args[] = {RDPSND_CHANNEL_NAME, "sys:" VENTO_RDPSND_SUBSYSTEM, latency};
        if (!freerdp_client_add_static_channel(settings, ARRAYSIZE(args), args)
                || !freerdp_client_add_dynamic_channel(settings, ARRAYSIZE(args), args)) {
            LOGW("sound: could not name the sound device");
        }
    }
    freerdp_settings_set_bool(settings, FreeRDP_AudioPlayback, soundHere);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteConsoleAudio, soundThere);
    /* The microphone is the audin channel's capture side, which the library
       loads when this is set and opens through OpenSL ES (the only capture
       device built, on Android only). */
    freerdp_settings_set_bool(settings, FreeRDP_AudioCapture, s->microphone);

    /* The scale the far end draws its interface at. The protocol allows
       100–500 for the desktop factor and exactly 100, 140 or 180 for the
       device factor, and a value outside either range makes a server ignore
       both, so one option sets both to the same three values. These are the
       values in the client core data, which Windows reads and ignores; the
       ones it uses are in the monitor block below. */
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopScaleFactor, (UINT32) s->scale);
    freerdp_settings_set_uint32(settings, FreeRDP_DeviceScaleFactor, (UINT32) s->scale);

    /* How much of the link the remote machine may spend on visual effects.
       The flags are a hint it may ignore, and it decides some of the rest
       itself from the connection type. */
    UINT32 performance = PERF_FLAG_NONE;
    UINT32 connectionType = CONNECTION_TYPE_AUTODETECT;
    if (s->experience && strcmp(s->experience, "plain") == 0) {
        performance = PERF_DISABLE_WALLPAPER | PERF_DISABLE_FULLWINDOWDRAG
                | PERF_DISABLE_MENUANIMATIONS | PERF_DISABLE_THEMING
                | PERF_DISABLE_CURSOR_SHADOW | PERF_DISABLE_CURSORSETTINGS;
        connectionType = CONNECTION_TYPE_MODEM;
    } else if (!s->experience || strcmp(s->experience, "balanced") == 0) {
        performance = PERF_DISABLE_MENUANIMATIONS | PERF_DISABLE_FULLWINDOWDRAG
                | PERF_DISABLE_CURSOR_SHADOW;
        /* LAN, though the option is only about effects, because a server
           reads this as the bandwidth available: xrdp offers RemoteFX to a
           client that reports LAN and plain bitmaps to one that reports
           broadband, so anything less here silently loses the codec. IronRDP
           sends the same value for the same option, which keeps the two
           comparable. */
        connectionType = CONNECTION_TYPE_LAN;
    } else {
        performance = PERF_ENABLE_FONT_SMOOTHING | PERF_ENABLE_DESKTOP_COMPOSITION;
        connectionType = CONNECTION_TYPE_LAN;
    }
    freerdp_settings_set_uint32(settings, FreeRDP_PerformanceFlags, performance);
    freerdp_settings_set_uint32(settings, FreeRDP_ConnectionType, connectionType);
    /* On regardless of the connection type, because a Windows host sends RTT
       measure requests unprompted, and a client without autodetect enabled
       treats one as a protocol error and drops the connection, ending the
       session half a second after it starts. It also provides the only line
       speed measurement this protocol has. */
    freerdp_settings_set_bool(settings, FreeRDP_NetworkAutoDetect, TRUE);

    /* The monitor layout, which a scale other than 100 needs as much as a
       second screen does. Two library conditions gate the block carrying each
       monitor's scale factors: the layout is only written under UseMultimon,
       and the attributes inside it only under HasMonitorAttributes, which the
       client path never sets. So a client of this library otherwise sends its
       scale in the core data only, and Windows uses neither value until the
       monitor block includes them. Both are therefore set for a single
       monitor too, at the cost of a layout block listing one screen. */
    if (s->monitors > 1 || s->scale != 100) {
        freerdp_settings_set_bool(settings, FreeRDP_UseMultimon, TRUE);
        freerdp_settings_set_bool(settings, FreeRDP_HasMonitorAttributes, TRUE);
        freerdp_settings_set_uint32(settings, FreeRDP_MonitorCount, (UINT32) s->monitors);
        if (s->monitors > 1) {
            freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth,
                                        (UINT32) (s->width * s->monitors));
        }
        if (freerdp_settings_set_pointer_len(settings, FreeRDP_MonitorDefArray, NULL,
                                             (size_t) s->monitors)) {
            for (int i = 0; i < s->monitors; i++) {
                rdpMonitor *m = freerdp_settings_get_pointer_array_writable(
                        settings, FreeRDP_MonitorDefArray, (size_t) i);
                if (!m) {
                    continue;
                }
                m->x = i * s->width;
                m->y = 0;
                m->width = s->width;
                m->height = s->height;
                m->is_primary = i == 0;
                m->orig_screen = i;
                m->attributes.physicalWidth = 0;
                m->attributes.physicalHeight = 0;
                m->attributes.orientation = 0;
                m->attributes.desktopScaleFactor = (UINT32) s->scale;
                m->attributes.deviceScaleFactor = (UINT32) s->scale;
            }
        }
        /* The connection's layout is the first requested layout, used until
           the server sends one. */
        pthread_mutex_lock(&s->lock);
        s->askedCount = s->monitors > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : s->monitors;
        for (int i = 0; i < s->askedCount; i++) {
            s->askedLayout[4 * i + 0] = i * s->width;
            s->askedLayout[4 * i + 1] = 0;
            s->askedLayout[4 * i + 2] = s->width;
            s->askedLayout[4 * i + 3] = s->height;
        }
        pthread_mutex_unlock(&s->lock);
    }
}

static void *protocolThread(void *arg) {
    Session *s = (Session *) arg;
    ShimContext *shim = s->ctx;
    rdpContext *ctx = &shim->client.context;

    if (s->addressError[0]) {
        fireClosed(s, s->addressError);
        return NULL;
    }

    ctx->instance->PreConnect = onPreConnect;
    ctx->instance->PostConnect = onPostConnect;
    ctx->instance->PostDisconnect = onPostDisconnect;
    ctx->instance->AuthenticateEx = onAuthenticate;
    /* The only certificate check, with the library's own store bypassed (see
       onVerifyX509Certificate). */
    ctx->instance->VerifyX509Certificate = onVerifyX509Certificate;

    applySettings(s, ctx->settings);

    if (freerdp_client_start(ctx) != CHANNEL_RC_OK) {
        fireClosed(s, "Could not start the connection");
        return NULL;
    }
    s->started = 1;

    if (!freerdp_connect(ctx->instance)) {
        const UINT32 err = freerdp_get_last_error(ctx);
        const char *detail = freerdp_get_last_error_string(err);
        LOGW("connect failed: 0x%08x (%s)", err, detail ? detail : "?");
        if (err == FREERDP_ERROR_CONNECT_LOGON_FAILURE || err == FREERDP_ERROR_CONNECT_WRONG_PASSWORD) {
            detail = VENTO_LOGON_FAILED;
        }
        pthread_mutex_lock(&s->lock);
        const int asked = s->quit;
        pthread_mutex_unlock(&s->lock);
        fireClosed(s, asked ? "" : (detail ? detail : "The connection failed"));
        return NULL;
    }
    LOGI("handshake done: %s", freerdp_settings_get_string(ctx->settings, FreeRDP_ServerHostname));

    /* The library's handles plus our wake event. Setting a flag doesn't wake
       a thread inside WaitForMultipleObjects, so the event has to be in the
       set being waited on. */
    while (!freerdp_shall_disconnect_context(ctx)) {
        pthread_mutex_lock(&s->lock);
        const int quit = s->quit;
        pthread_mutex_unlock(&s->lock);
        if (quit) {
            break;
        }
        HANDLE handles[64];
        handles[0] = s->wake;
        const DWORD count = freerdp_get_event_handles(ctx, &handles[1], 63);
        if (count == 0) {
            break;
        }
        const DWORD status = WaitForMultipleObjects(count + 1, handles, FALSE, 1000);
        if (status == WAIT_FAILED) {
            break;
        }
        /* Before the drain, not after: a command posted in between only costs
           a wakeup that finds nothing, while the other order delays it until
           the timeout. The reset is done here because a WinPR event handle has
           no cleanup hook, so no wait resets one even if it was created as
           auto-reset, and an event left signalled makes this loop spin. */
        ResetEvent(s->wake);
        drainCommands(s);
        if (!freerdp_check_event_handles(ctx)) {
            break;
        }
    }

    pthread_mutex_lock(&s->lock);
    const int asked = s->quit;
    pthread_mutex_unlock(&s->lock);
    s->connected = 0;

    freerdp_disconnect(ctx->instance);
    const UINT32 err = freerdp_get_last_error(ctx);
    const char *detail = err != FREERDP_ERROR_SUCCESS ? freerdp_get_last_error_string(err) : NULL;
    fireClosed(s, asked ? "" : (detail ? detail : "The connection was lost"));
    return NULL;
}

/* ---- the session interface --------------------------------------------- */

static Session *sessionOfBase(vento_session *base) {
    return (Session *) base;
}

static char *dupOrNull(const char *s) {
    return s ? strdup(s) : NULL;
}

static void freeSession(Session *s) {
    if (s->wake) {
        CloseHandle(s->wake);
    }
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        free(s->ring[i].text);
    }
    free(s->fb);
    free(s->clipOut);
    free(s->host);
    free(s->user);
    free(s->domain);
    free(s->password);
    free(s->nla);
    free(s->graphics);
    free(s->experience);
    free(s->sound);
    free(s->clientName);
    free(s->configPath);
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_cond_destroy(&s->statusCond);
    pthread_rwlock_destroy(&s->fbLock);
    vento_h264_release(&s->h264);
    vento_sound_release(&s->audio);
    free(s);
}

const char *vento_freerdp_version(void) {
    static char buf[64];
    snprintf(buf, sizeof(buf), "FreeRDP %s", freerdp_get_version_string());
    return buf;
}

#ifdef VENTO_FREERDP_WLOG
/* WinPR's log (WLog) through vento_log, under this backend's tag with the
   logger's name and the function in front. Only on a desktop
   (VENTO_FREERDP_WLOG, CMakeLists.txt), where WLog's console appender writes
   to stdout and stderr in a layout of its own; on Android it writes to logcat.
   Not where the environment picks an appender (WLOG_APPENDER) or a prefix
   (WLOG_PREFIX). The level is still WLog's: warnings and errors unless
   WLOG_LEVEL says otherwise. */
static BOOL wlogMessage(const wLogMessage *msg) {
    vento_log(msg->Level >= WLOG_WARN ? VENTO_LOG_WARN : VENTO_LOG_INFO, TAG, "%s%s",
              msg->PrefixString ? msg->PrefixString : "", msg->TextString ? msg->TextString : "");
    return TRUE;
}

static void routeWLog(void) {
    if (getenv("WLOG_APPENDER")) {
        return;
    }
    wLog *root = WLog_GetRoot();
    if (!root || !WLog_SetLogAppenderType(root, WLOG_APPENDER_CALLBACK)) {
        return;
    }
    wLogCallbacks callbacks = {.message = wlogMessage};
    if (!WLog_ConfigureAppender(WLog_GetLogAppender(root), "callbacks", &callbacks)) {
        (void) WLog_SetLogAppenderType(root, WLOG_APPENDER_CONSOLE);
        return;
    }
    if (!getenv("WLOG_PREFIX")) {
        (void) WLog_Layout_SetPrefixFormat(root, WLog_GetLogLayout(root), "%mn: [%fn]%{[%ctx]%}: ");
    }
}
#endif

vento_session *vento_freerdp_create(const vento_freerdp_config *cfg, const vento_callbacks *cb) {
#ifdef VENTO_FREERDP_WLOG
    static pthread_once_t wlogOnce = PTHREAD_ONCE_INIT;
    pthread_once(&wlogOnce, routeWLog);
#endif
    Session *s = calloc(1, sizeof(Session));
    if (!s) {
        if (cfg->video && cfg->video->release) {
            cfg->video->release(cfg->video->user);
        }
        if (cfg->audio && cfg->audio->release) {
            cfg->audio->release(cfg->audio->user);
        }
        return NULL;
    }
    /* The session owns the decoders and the sound output from here;
       freeSession releases them on every exit path. */
    vento_h264_init(&s->h264, cfg->video);
    vento_sound_init(&s->audio, cfg->audio);
    s->base.ops = &OPS;
    s->cb = *cb;
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    {
        pthread_condattr_t monotonic;
        pthread_condattr_init(&monotonic);
        pthread_condattr_setclock(&monotonic, CLOCK_MONOTONIC);
        pthread_cond_init(&s->statusCond, &monotonic);
        pthread_condattr_destroy(&monotonic);
    }
    pthread_rwlock_init(&s->fbLock, NULL);
    s->wake = CreateEvent(NULL, FALSE, FALSE, NULL);
    if (!s->wake) {
        LOGW("no wake event");
        freeSession(s);
        return NULL;
    }

    char *addr = dupOrNull(cfg->address);
    s->port = 3389;
    if (addr) {
        s->host = addr;
        splitAddress(addr, &s->port, s->addressError, sizeof s->addressError);
    }
    s->user = dupOrNull(cfg->user_name);
    s->domain = dupOrNull(cfg->domain);
    s->password = dupOrNull(cfg->password);
    s->nla = dupOrNull(cfg->nla);
    s->legacyTls = cfg->legacy_tls;
    s->graphics = dupOrNull(cfg->graphics);
    s->experience = dupOrNull(cfg->experience);
    s->sound = dupOrNull(cfg->sound);
    s->microphone = cfg->microphone;
    s->clientName = dupOrNull(cfg->client_name);
    s->osMajorType = OSMAJORTYPE_UNSPECIFIED;
    if (cfg->platform && strcmp(cfg->platform, "android") == 0) {
        s->osMajorType = OSMAJORTYPE_ANDROID;
    } else if (cfg->platform && strcmp(cfg->platform, "linux") == 0) {
        s->osMajorType = OSMAJORTYPE_UNIX;
    }
    s->configPath = dupOrNull(cfg->config_path);
    s->compression = cfg->compression;
    /* A server ignores a scale outside the protocol's range, so an invalid
       value becomes 100. */
    s->scale = cfg->scale == 140 || cfg->scale == 180 ? cfg->scale : 100;
    s->width = cfg->width;
    s->height = cfg->height;
    s->monitors = cfg->monitors;
    s->serverCount = -1;
    s->keyboardLayout = cfg->keyboard_layout;
    s->connectTimeoutMs = cfg->connect_timeout_ms;

    /* WinPR reads $HOME through GetKnownPath, and an Android app's process has
       no HOME, which stops a session from starting: freerdp_settings_new fails
       when the home path is null, so the context below is never allocated.
       Only when it is unset does it become the config directory, before any
       FreeRDP code runs: the environment belongs to the whole process, and on
       a desktop HOME is the user's, which other threads may be reading and
       this code must not change. When HOME is set, the library uses it only
       for paths the session sets itself (FreeRDP_ConfigPath is set from the
       same directory before connecting) or never uses (drive redirection,
       and the user config of Azure AD and FFmpeg's decoder, neither of which
       this build has). The first session sets it, and every later one finds
       it set to the same directory. */
    if (s->configPath && !getenv("HOME")) {
        setenv("HOME", s->configPath, 0);
    }

    /* ContextSize: see ShimContext. */
    RDP_CLIENT_ENTRY_POINTS entry = {0};
    entry.Size = sizeof(RDP_CLIENT_ENTRY_POINTS_V1);
    entry.Version = RDP_CLIENT_INTERFACE_VERSION;
    entry.ContextSize = sizeof(ShimContext);
    rdpContext *ctx = freerdp_client_context_new(&entry);
    if (!ctx) {
        LOGW("freerdp_client_context_new failed");
        freeSession(s);
        return NULL;
    }
    s->ctx = (ShimContext *) ctx;
    s->ctx->session = s;
    s->instance = ctx->instance;
    /* Creating the context restored the library's own addin provider, which
       doesn't know the module's rdpsnd device. */
    if (s->audio.available && !vento_sound_register()) {
        LOGW("could not register the sound device; the server is not asked for its sound");
        vento_sound_release(&s->audio);
    }

    if (pthread_create(&s->thread, NULL, protocolThread, s) != 0) {
        freerdp_client_context_free(ctx);
        s->ctx = NULL;
        freeSession(s);
        return NULL;
    }
    return &s->base;
}

static void op_answer_credentials(vento_session *base, const char *user, const char *domain,
                                  const char *password) {
    Session *s = sessionOfBase(base);
    char *u = dupOrNull(user);
    char *d = dupOrNull(domain);
    char *p = dupOrNull(password);
    pthread_mutex_lock(&s->lock);
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    s->credUser = u;
    s->credDomain = d;
    s->credPass = p;
    s->credAnswered = 1;
    if (!p) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

static void op_answer_trust(vento_session *base, bool accept) {
    Session *s = sessionOfBase(base);
    pthread_mutex_lock(&s->lock);
    s->trustAccepted = accept;
    s->trustAnswered = 1;
    if (!s->trustAccepted) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

/* Both ways a session ends set the flag, wake the wait *and* abort the
   connect: during a handshake the thread is inside the library's socket read,
   whose wait doesn't include our event. */
static void stop(Session *s) {
    pthread_mutex_lock(&s->lock);
    s->quit = 1;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    if (s->ctx) {
        freerdp_abort_connect_context(&s->ctx->client.context);
    }
    SetEvent(s->wake);
}

static void op_disconnect(vento_session *base) {
    stop(sessionOfBase(base));
}

static void op_destroy(vento_session *base) {
    Session *s = sessionOfBase(base);
    stop(s);
    pthread_join(s->thread, NULL);
    /* Already stopped by `closed`, unless the thread ended without one. */
    stopStatus(s);

    if (s->ctx) {
        if (s->started) {
            freerdp_client_stop(&s->ctx->client.context);
        }
        freerdp_client_context_free(&s->ctx->client.context);
        s->ctx = NULL;
    }
    freeSession(s);
}

static void op_pointer(vento_session *base, int x, int y, int buttonMask) {
    Cmd c = {CMD_POINTER, x, y, buttonMask, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_key_down(vento_session *base, uint32_t keysym, int64_t keyId) {
    Cmd c = {CMD_KEY_DOWN, (int) keysym, 0, keyId, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_key_up(vento_session *base, int64_t keyId) {
    Cmd c = {CMD_KEY_UP, 0, 0, keyId, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_release_all_keys(vento_session *base) {
    Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_focus(vento_session *base, bool focused) {
    Cmd c = {CMD_FOCUS, focused, 0, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_view_only(vento_session *base, bool viewOnly) {
    Session *s = sessionOfBase(base);
    s->viewOnly = viewOnly;
    if (s->viewOnly) {
        /* Keys held at the far end were pressed by this session, so it
           releases them when it stops sending input. */
        Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
        post(s, &c);
    }
}

static bool op_can_resize(vento_session *base) {
    return sessionOfBase(base)->canResize != 0;
}

static void op_request_desktop_size(vento_session *base, int width, int height) {
    Cmd c = {CMD_RESIZE, width, height, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

void vento_freerdp_set_monitor_count(vento_session *base, int count) {
    sessionOfBase(base)->monitors = count < 1 ? 1 : (count > 16 ? 16 : count);
}

/* The desktop's monitors in the server's order: as the server last sent
   them, or else as last requested, whichever covers the current desktop (see
   `covers`). None for a single-monitor desktop, which is not a layout, or for
   a desktop neither covers. */
static int op_monitors(vento_session *base, int32_t *out) {
    Session *s = sessionOfBase(base);
    if (!s->connected) {
        return 0;
    }
    pthread_rwlock_rdlock(&s->fbLock);
    const int w = s->fbW, h = s->fbH;
    pthread_rwlock_unlock(&s->fbLock);
    pthread_mutex_lock(&s->lock);
    const int32_t *from = NULL;
    int n = 0;
    if (s->serverCount > 0 && covers(s->serverLayout, s->serverCount, w, h)) {
        from = s->serverLayout;
        n = s->serverCount;
    } else if (covers(s->askedLayout, s->askedCount, w, h)) {
        from = s->askedLayout;
        n = s->askedCount;
    }
    if (n < 2) {
        n = 0;
    } else {
        memcpy(out, from, sizeof(int32_t) * 4 * (size_t) n);
    }
    pthread_mutex_unlock(&s->lock);
    return n;
}

/* RDP identifies a monitor only by its position in the list, so this session
   has no `screens` op (a screen's id is its index) and a layout is sent in
   the order given: the first is the primary, and a screen past the far end's
   last is a new one. The desktop's size is the layout's bounding box, which
   `width` and `height` describe anyway. */
static void op_request_layout(vento_session *base, int width, int height, const int32_t *screens,
                              int n) {
    (void) width;
    (void) height;
    Session *s = sessionOfBase(base);
    if (n < 1) {
        return;
    }
    n = n > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : n;
    pthread_mutex_lock(&s->lock);
    for (int i = 0; i < n; i++) {
        memcpy(s->wantLayout + 4 * i, screens + 5 * i + 1, sizeof(int32_t) * 4);
    }
    s->wantCount = n;
    pthread_mutex_unlock(&s->lock);
    Cmd c = {CMD_LAYOUT, 0, 0, 0, NULL, 0};
    post(s, &c);
}

/* Converted to wire format (NUL-terminated UTF-16 with CRLF) on arrival, once
   rather than on every paste. Text over VENTO_MAX_CLIPBOARD is not offered at
   all, as in the IronRDP backend. */
static void op_clipboard(vento_session *base, const char *utf8) {
    if (!utf8) {
        return;
    }
    if (strlen(utf8) > VENTO_MAX_CLIPBOARD) {
        LOGW("this end's clipboard is over the %d bytes it offers; not offered",
             VENTO_MAX_CLIPBOARD);
        return;
    }
    char *crlf = vento_text_to_crlf(utf8);
    if (!crlf) {
        return;
    }
    size_t units = 0;
    WCHAR *copy = ConvertUtf8ToWCharAlloc(crlf, &units);
    free(crlf);
    if (!copy) {
        return;
    }
    Cmd c = {CMD_CLIPBOARD, 0, 0, 0, copy, units};
    post(sessionOfBase(base), &c);
}

/* The facts: protocol, connection, security, encoding, line speed, server and
   viewer pixels, in the order the Rust adapter's `info` labels them. Values
   the protocol has no concept of are omitted rather than left empty, so there
   is no desktop name: RDP has none. */
static bool op_info(vento_session *base, vento_strings *out) {
    Session *s = sessionOfBase(base);
    if (!s->connected || !s->ctx) {
        return false;
    }
    rdpSettings *settings = s->ctx->client.context.settings;
    char protocol[64], connection[128], security[64], lineSpeed[64], server[64], viewer[64];

    snprintf(protocol, sizeof(protocol), "RDP (FreeRDP %s)", freerdp_get_version_string());
    /* Formatted as it would be typed, with brackets for IPv6. */
    const char *host = s->host ? s->host : "";
    if (strchr(host, ':')) {
        snprintf(connection, sizeof(connection), "[%s]:%d", host, s->port);
    } else {
        snprintf(connection, sizeof(connection), "%s:%d", host, s->port);
    }
    const char *layer;
    switch (freerdp_settings_get_uint32(settings, FreeRDP_SelectedProtocol)) {
        case 0: layer = "RDP"; break;
        case 1: layer = "TLS"; break;
        case 2: layer = "NLA (CredSSP)"; break;
        case 4: layer = "RDSTLS"; break;
        case 8: layer = "NLA (CredSSP, extended)"; break;
        default: layer = "?"; break;
    }
    snprintf(security, sizeof(security), "%s", layer);

    /* The autodetect exchange's bandwidth in kilobits, empty until the server
       has run one, which it does at a time of its choosing rather than at
       connect, so this row appears part way into a session. */
    const rdpAutoDetect *detect = autodetect_get(&s->ctx->client.context);
    const UINT32 kbps = detect ? detect->netCharBandwidth : 0;
    if (kbps >= 1000) {
        snprintf(lineSpeed, sizeof(lineSpeed), "%u.%u Mbit/s", kbps / 1000, (kbps % 1000) / 100);
    } else if (kbps > 0) {
        snprintf(lineSpeed, sizeof(lineSpeed), "%u kbit/s", kbps);
    } else {
        lineSpeed[0] = 0;
    }

    const UINT32 w = freerdp_settings_get_uint32(settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(settings, FreeRDP_DesktopHeight);
    snprintf(server, sizeof(server), "%u×%u, %u bpp", w, h,
             freerdp_settings_get_uint32(settings, FreeRDP_ColorDepth));
    snprintf(viewer, sizeof(viewer), "%u×%u, 32 bpp", w, h);

    /* The encoding the picture arrives in rather than the one requested: the
       pipeline's channel opening means the server accepted it, and the version
       and H.264 are what the server confirmed, since the settings that
       requested them stay unchanged whatever it answers. Without the pipeline
       the codec is read from the surfaces for the same reason: the RemoteFX
       setting stays set when the server declines it. */
    char encoding[64];
    if (s->gfxOpen) {
        const UINT32 version = s->gfxVersion;
        if (version) {
            const int hardware = atomic_load(&s->h264.hardware);
            snprintf(encoding, sizeof(encoding), "EGFX %u.%u%s%s", (version >> 16) & 0xFFFFu,
                     (version >> 8) & 0xFFu, s->gfxH264 ? ", H.264" : "",
                     !s->gfxH264 || hardware < 0 ? "" : hardware ? " (hardware)" : " (software)");
        } else {
            snprintf(encoding, sizeof(encoding), "EGFX");
        }
    } else if (s->lastCodec == RDP_CODEC_ID_REMOTEFX) {
        snprintf(encoding, sizeof(encoding), "RemoteFX");
    } else if (s->lastCodec == RDP_CODEC_ID_NSCODEC) {
        snprintf(encoding, sizeof(encoding), "NSCodec");
    } else {
        snprintf(encoding, sizeof(encoding), "Bitmaps");
    }

    const char *values[] = {protocol, connection, security, encoding, lineSpeed,
                            server, viewer};
    return vento_strings_set(out, values, sizeof(values) / sizeof(values[0]));
}

/* The overlay's H.264 line: the last decoder's status, as for RustDesk and
   TigerVNC ("H.264 vaapi map 4.1 ms", "H.264 sw 3.2 ms (vaapi: no
   device)"). */
static bool op_decoder_status(vento_session *base, char *out, size_t len) {
    return vento_h264_status(&sessionOfBase(base)->h264, out, len);
}

/* Both directions from the library's statistics, counted where the transport
   reads and writes its front BIO, so they measure the protocol inside TLS and
   outside TCP, where every other backend here counts. This uses the library's
   public API, with no patch or private header. */
static bool op_traffic(vento_session *base, uint64_t out[2]) {
    Session *s = sessionOfBase(base);
    if (!s->connected || !s->ctx) {
        return false;
    }
    UINT64 in = 0, sent = 0;
    if (!freerdp_get_stats(s->ctx->client.context.rdp, &in, &sent, NULL, NULL)) {
        return false;
    }
    out[0] = in;
    out[1] = sent;
    return true;
}

static bool op_read_region(vento_session *base, int x, int y, int w, int h, void *dst,
                           size_t stride, int dstX, int dstY) {
    Session *s = sessionOfBase(base);
    if (w <= 0 || h <= 0 || dstX < 0 || dstY < 0) {
        return false;
    }
    bool ok = false;
    pthread_rwlock_rdlock(&s->fbLock);
    /* Subtracted rather than added: `x + w` near INT_MAX wraps negative and
       would pass, and the rows would be read from before the framebuffer. */
    if (s->fb && x >= 0 && y >= 0 && x <= s->fbW && y <= s->fbH && w <= s->fbW - x
            && h <= s->fbH - y) {
        for (int row = 0; row < h; row++) {
            const uint8_t *src = s->fb + ((size_t) (y + row) * (size_t) s->fbW + x) * 4u;
            uint8_t *out = (uint8_t *) dst + (size_t) (dstY + row) * stride + (size_t) dstX * 4u;
            memcpy(out, src, (size_t) w * 4u);
        }
        ok = true;
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}

static bool op_read_thumbnail(vento_session *base, int step, void *dst, size_t stride, int dstW,
                              int dstH) {
    Session *s = sessionOfBase(base);
    if (step < 1) {
        return false;
    }
    bool ok = false;
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->fb && s->fbW > 0 && s->fbH > 0) {
        const int tw = (s->fbW + step - 1) / step;
        const int th = (s->fbH + step - 1) / step;
        if (tw <= dstW && th <= dstH) {
            for (int row = 0; row < th; row++) {
                const uint32_t *src = (const uint32_t *) (s->fb
                        + (size_t) (row * step) * (size_t) s->fbW * 4u);
                uint32_t *out = (uint32_t *) ((uint8_t *) dst + (size_t) row * stride);
                for (int col = 0; col < tw; col++) {
                    out[col] = src[col * step];
                }
            }
            ok = true;
        }
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}

static const vento_session_ops OPS = {
        .size = sizeof(vento_session_ops),
        .pointer = op_pointer,
        .key_down = op_key_down,
        .key_up = op_key_up,
        .release_all_keys = op_release_all_keys,
        .focus = op_focus,
        .view_only = op_view_only,
        .can_resize = op_can_resize,
        .monitors = op_monitors,
        .request_desktop_size = op_request_desktop_size,
        .clipboard = op_clipboard,
        .info = op_info,
        .traffic = op_traffic,
        .read_region = op_read_region,
        .read_thumbnail = op_read_thumbnail,
        .answer_credentials = op_answer_credentials,
        .answer_trust = op_answer_trust,
        .disconnect = op_disconnect,
        .destroy = op_destroy,
        .request_layout = op_request_layout,
        .decoder_status = op_decoder_status,
};
