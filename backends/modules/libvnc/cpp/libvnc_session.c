// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// LibVNCServer's libvncclient as a `vento_session`, so that above this file it
// has the same interface as our own RFB client and the two clients can be
// compared directly. Nothing in here is platform-specific: on the phone, the
// Rust adapter in the module's `src/` is linked into the same library and the
// shared JNI layer in `backends/platform/android` wraps it, as it does every
// backend.
//
// Three parts of this shim are required by libvncclient's API:
//
//   1. One thread owns the rfbClient, and every call into the session is a
//      queued command it drains. Nothing in libvncclient is safe to call from
//      a second thread (reads and writes go through the same buffered socket
//      and the one framebuffer), so a pointer event sent from the main thread
//      would race the protocol thread's own writes.
//   2. There are two framebuffers. libvncclient decodes into its own without
//      a lock, and `vento/backend.h` allows pixels to be read from the drawing
//      thread at any moment; so the damaged rectangles are copied under a
//      writer lock once the update message is complete, and reads are served
//      from that copy. With one buffer, the reader would wait for
//      HandleRFBServerMessage, which puts the drawing thread behind a socket
//      read.
//   3. The alpha byte is filled in during that copy. RFB's 32-bit formats leave
//      the fourth byte undefined and the decoder writes whatever the wire said;
//      `vento/backend.h` requires it to be opaque.
//
// Two library features are deliberately not wired: **xvp**, which shuts down
// or reboots the far end's machine, and **text chat**. Nothing in this app
// asks a session for either, and an unused protocol capability is more code to
// maintain.

#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include <gnutls/gnutls.h>
#include <rfb/rfbclient.h>

#include <vento/backend.h>
#include <vento/libvnc.h>
#include <vento/log.h>
#include <vento/text.h>

#define TAG "LibVnc"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)

#define CMD_RING 512
#define MAX_HELD_KEYS 64

enum {
    CMD_POINTER = 1,
    CMD_KEY_DOWN,
    CMD_KEY_UP,
    CMD_RELEASE_KEYS,
    CMD_CLIPBOARD,
    CMD_RESIZE,
    CMD_LAYOUT,
};

typedef struct {
    int type;
    int a, b, c;
    int64_t id;  /* the key id from the session API, not the keysym */
    char *text;  /* owned by the queue */
    int32_t *screens; /* a layout: `c` screens of five ints; owned by the queue */
} Cmd;

static void cmdFree(Cmd *c) {
    free(c->text);
    free(c->screens);
    c->text = NULL;
    c->screens = NULL;
}

static char *dupOrNull(const char *s) {
    return s ? strdup(s) : NULL;
}

typedef struct {
    int x, y, w, h;
} Rect;

/* What op_info reports of a connection besides its encodings. */
typedef struct {
    bool known;
    /* At most 255 bytes from the server, as UTF-8: up to twice as many if
       they were Latin-1. */
    char desktop[512];
    char protocol[64], security[96], serverPixels[64], viewerPixels[64];
} Facts;

typedef struct {
    vento_session base; /* first, so that a pointer to this is a pointer to that */
    vento_callbacks cb;

    rfbClient *cl; /* protocol thread only */

    pthread_t thread;
    int wake[2]; /* self-pipe that wakes the protocol thread */

    pthread_mutex_t lock;
    pthread_cond_t cond;

    Cmd ring[CMD_RING];
    int head, tail;

    int quit;
    int paused;
    _Atomic int viewOnly; /* written from any thread, read from the protocol thread */
    int connected;
    int closedReported;
    _Atomic int canResize; /* an ExtDesktopSize rectangle has arrived; written from the protocol thread */

    /* The far end's screen layout, five ints per screen (the server's id,
       then x, y, width, height), copied out of the library's array under
       `lock` because that array belongs to the protocol thread and this copy
       is read from any thread. */
    int32_t *screens;
    int nscreens;

    /* the credential prompt, answered through the session */
    int credWaiting, credAnswered;
    char *credUser, *credPass;

    /* The certificate prompt, which works the same way. The anonymous-TLS
       question uses the same pair: only one of the two can be outstanding,
       since one is asked in place of the other. */
    int trustAnswered, trustAccepted;

    char *storedUser, *storedPass;
    int storedPassUsed;

    /* the copy read_region is served from */
    pthread_rwlock_t fbLock;
    uint8_t *fb;
    int fbW, fbH;

    /* damage of the update message being handled */
    Rect *dirty;
    int ndirty, capdirty;

    struct {
        int64_t id;
        uint32_t sym;
    } keys[MAX_HELD_KEYS];
    int nkeys;

    char *host;
    int port;
    /* Why the address is invalid, or empty if it is valid. A session is
       created and started in one call, so the protocol thread is the first
       place to report it. */
    char addressError[160];
    int shared;
    int anonymousTls;
    /* The most recently requested encodings, under `lock`;
       vento_libvnc_set_encodings replaces and frees it there, and sets
       `encodingsChanged` for the protocol thread, which sends them the next
       time it drains the queue. A flag rather than a queued command, since a
       full queue drops its oldest entry and an encodings change must not be
       lost. */
    char *encodings;
    int compressLevel;
    int qualityLevel;
    int encodingsChanged;
    /* The protocol thread's own copy of `encodings`. libvncclient holds it in
       appData.encodingsString and reads it at every SetFormatAndEncodings, and
       op_info reads it under the framebuffer lock. Kept separate because the
       library holds it for the whole session without saying when it is done
       with it, so the string set_encodings replaces must not be the one the
       library has. Replaced, and the previous one freed, only on the protocol
       thread and under the writer lock. */
    char *applied;
    int colorLevel;
    int connectTimeoutMs;

    /* The connection's facts for op_info, under the writer lock: published
       by the protocol thread once the handshake is done (publishFacts), and
       empty before that. */
    Facts facts;

    /* Every 8-bit pixel value, converted to the shadow buffer's format. A
       reduced format then costs one table lookup per pixel in the existing
       copy rather than arithmetic. Unused at 32 bits. */
    uint32_t palette[256];
} Session;

static const vento_session_ops OPS;
static void publishFacts(Session *s, rfbClient *cl);

/* `s->cl` is read by `op_traffic` under the framebuffer lock and by the two
   teardown calls under `s->lock`, so both are held to change it. This is
   required because a failed handshake frees the client (see `initClient`): a
   reader that sees the old pointer afterwards would shut down a descriptor
   that may already be reused. */
static void publishClient(Session *s, rfbClient *cl) {
    pthread_rwlock_wrlock(&s->fbLock);
    pthread_mutex_lock(&s->lock);
    s->cl = cl;
    pthread_mutex_unlock(&s->lock);
    pthread_rwlock_unlock(&s->fbLock);
}

/* ---- the command queue -------------------------------------------------- */

static void wake(Session *s) {
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

/* Caller holds the lock. */
static void ring_push(Session *s, const Cmd *c) {
    const int next = (s->head + 1) % CMD_RING;
    if (next == s->tail) {
        /* Full. Dropping the oldest keeps the newest pointer position, which
           is the one that matters; a dropped button change would be lost, but
           this is reached only if the protocol thread is stuck. */
        cmdFree(&s->ring[s->tail]);
        s->tail = (s->tail + 1) % CMD_RING;
    }
    s->ring[s->head] = *c;
    s->head = next;
}

static void post(Session *s, const Cmd *c) {
    pthread_mutex_lock(&s->lock);
    if (s->quit) {
        pthread_mutex_unlock(&s->lock);
        free(c->text);
        free(c->screens);
        return;
    }
    /* Motion is coalesced at the end of the queue: the protocol thread can be
       inside a decode, and by the time it drains, every position but the last
       is stale. A button change is a different event and is never merged. */
    if (c->type == CMD_POINTER && s->head != s->tail) {
        const int last = (s->head + CMD_RING - 1) % CMD_RING;
        if (s->ring[last].type == CMD_POINTER && s->ring[last].c == c->c) {
            s->ring[last] = *c;
            pthread_mutex_unlock(&s->lock);
            wake(s);
            return;
        }
    }
    ring_push(s, c);
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    wake(s);
}

/* ---- callbacks out ------------------------------------------------------ */

static void fireClosed(Session *s, const char *detail) {
    pthread_mutex_lock(&s->lock);
    const int already = s->closedReported;
    s->closedReported = 1;
    pthread_mutex_unlock(&s->lock);
    if (already) {
        return;
    }
    /* The library's last message can quote the server: an "Unknown
       authentication scheme" carries bytes from the far end, and the session
       API requires UTF-8. */
    char *utf8 = vento_utf8_from_bytes(detail);
    s->cb.closed(s->cb.user, utf8 ? utf8 : "");
    free(utf8);
}

/* ---- libvncclient callbacks (protocol thread) --------------------------- */

static rfbClientData sessionTag;

static Session *sessionOf(rfbClient *cl) {
    return (Session *) rfbClientGetClientData(cl, &sessionTag);
}

/* Sets the pixel format requested on the wire, and the table that converts a
   reduced one back to the shadow buffer's 32 bits. Level 0 is the format that
   makes the copy below a plain copy; 1 and 2 are 64 and 8 colours, which is
   what the RealVNC and TigerVNC backends' colour rows mean by rgb222 and
   rgb111 (the formats TigerVNC's own viewer reduces to). libvncclient's
   `useBGR233`, `forceTrueColour` and `requestedDepth` cannot be used for this:
   the library never reads them, and it sends whatever `cl->format` says. */
static void setColorFormat(Session *s, rfbClient *cl) {
    cl->format.bigEndian = FALSE;
    cl->format.trueColour = TRUE;
    if (s->colorLevel <= 0) {
        /* 32 bits little-endian with red at shift 0 puts the bytes in memory as
           R, G, B, _, which is the session API's order once the fourth byte is
           filled in. */
        cl->format.bitsPerPixel = 32;
        cl->format.depth = 24;
        cl->format.redMax = 255;
        cl->format.greenMax = 255;
        cl->format.blueMax = 255;
        cl->format.redShift = 0;
        cl->format.greenShift = 8;
        cl->format.blueShift = 16;
        return;
    }
    const int bits = s->colorLevel == 1 ? 2 : 1;
    const int max = (1 << bits) - 1;
    cl->format.bitsPerPixel = 8;
    cl->format.depth = bits * 3;
    cl->format.redMax = (uint16_t) max;
    cl->format.greenMax = (uint16_t) max;
    cl->format.blueMax = (uint16_t) max;
    cl->format.redShift = (uint8_t) (bits * 2);
    cl->format.greenShift = (uint8_t) bits;
    cl->format.blueShift = 0;
    for (int i = 0; i < 256; i++) {
        const unsigned r = ((unsigned) i >> cl->format.redShift) & (unsigned) max;
        const unsigned g = ((unsigned) i >> cl->format.greenShift) & (unsigned) max;
        const unsigned b = (unsigned) i & (unsigned) max;
        /* Scaled to the full range rather than shifted: a one- or two-bit
           channel has to reach 255, or white comes out grey. */
        s->palette[i] = 0xFF000000u | (b * 255u / (unsigned) max) << 16
                | (g * 255u / (unsigned) max) << 8 | (r * 255u / (unsigned) max);
    }
}

static rfbBool onMallocFrameBuffer(rfbClient *cl) {
    Session *s = sessionOf(cl);
    const int w = cl->width, h = cl->height;
    if (w <= 0 || h <= 0) {
        return FALSE;
    }
    const size_t bytes = (size_t) w * (size_t) h * 4u;

    uint8_t *decode = calloc(1, (size_t) w * (size_t) h * (cl->format.bitsPerPixel / 8u));
    uint8_t *shadow = malloc(bytes);
    if (!decode || !shadow) {
        free(decode);
        free(shadow);
        return FALSE;
    }
    memset(shadow, 0, bytes);
    /* Opaque from the first frame, so a region read before anything has been
       decoded is black rather than transparent. */
    for (size_t i = 3; i < bytes; i += 4) {
        shadow[i] = 0xFF;
    }

    pthread_rwlock_wrlock(&s->fbLock);
    free(cl->frameBuffer);
    free(s->fb);
    cl->frameBuffer = decode;
    s->fb = shadow;
    s->fbW = w;
    s->fbH = h;
    pthread_rwlock_unlock(&s->fbLock);

    if (s->connected) {
        s->cb.desktop_size(s->cb.user, w, h);
    } else {
        s->cb.connected(s->cb.user, w, h);
    }
    s->connected = 1;
    return TRUE;
}

static void onGotUpdate(rfbClient *cl, int x, int y, int w, int h) {
    Session *s = sessionOf(cl);
    if (w <= 0 || h <= 0) {
        return;
    }
    if (s->ndirty == s->capdirty) {
        const int cap = s->capdirty ? s->capdirty * 2 : 64;
        Rect *grown = realloc(s->dirty, (size_t) cap * sizeof(Rect));
        if (!grown) {
            return;
        }
        s->dirty = grown;
        s->capdirty = cap;
    }
    s->dirty[s->ndirty++] = (Rect) {x, y, w, h};
}

/* Copies the library's screen list, which patches/libvncserver's 0003 adds
   (upstream keeps only one screen). The fields are in network byte order
   there, as in `client->screen`, so they are swapped here and in sendLayout
   only. The ids are kept, since a later layout request sends them back.
   Called once per update message, and copies only when the layout has
   changed. */
static void layoutChanged(Session *s, rfbClient *client) {
    const int n = client->numScreens;
    int32_t stack[5 * VENTO_MAX_MONITORS];
    int32_t *flat = stack;
    if (n > VENTO_MAX_MONITORS) {
        return; /* beyond any real setup; a fixed array avoids a malloc per update */
    }
    for (int i = 0; i < n; i++) {
        flat[5 * i + 0] = (int32_t) rfbClientSwap32IfLE(client->screens[i].id);
        flat[5 * i + 1] = (int32_t) rfbClientSwap16IfLE(client->screens[i].x);
        flat[5 * i + 2] = (int32_t) rfbClientSwap16IfLE(client->screens[i].y);
        flat[5 * i + 3] = (int32_t) rfbClientSwap16IfLE(client->screens[i].width);
        flat[5 * i + 4] = (int32_t) rfbClientSwap16IfLE(client->screens[i].height);
    }
    const size_t bytes = (size_t) n * 5 * sizeof(int32_t);
    pthread_mutex_lock(&s->lock);
    if (s->nscreens != n || (n > 0 && memcmp(s->screens, flat, bytes) != 0)) {
        int32_t *copy = n > 0 ? malloc(bytes) : NULL;
        if (n == 0 || copy != NULL) {
            if (copy != NULL) {
                memcpy(copy, flat, bytes);
            }
            free(s->screens);
            s->screens = copy;
            s->nscreens = n;
        }
    }
    pthread_mutex_unlock(&s->lock);
}

/* The entire cost of the second framebuffer: one copy per damaged rectangle
   per update message, under the writer lock. */
static void onFinishedUpdate(rfbClient *cl) {
    Session *s = sessionOf(cl);
    /* libvncclient has no callback for the ExtDesktopSize rectangle and no
       way to ask whether the server can resize; it only stores the screen it
       read from one, and a client that has never received one has a screen
       of zero size. This is checked when an update ends, the first point
       after the rectangle where the shim runs. */
    s->canResize = cl->screen.width != 0;
    layoutChanged(s, cl);
    if (s->ndirty == 0) {
        return;
    }
    const int fw = s->fbW, fh = s->fbH;

    pthread_rwlock_wrlock(&s->fbLock);
    for (int i = 0; i < s->ndirty; i++) {
        Rect r = s->dirty[i];
        if (r.x < 0) { r.w += r.x; r.x = 0; }
        if (r.y < 0) { r.h += r.y; r.y = 0; }
        if (r.x + r.w > fw) { r.w = fw - r.x; }
        if (r.y + r.h > fh) { r.h = fh - r.y; }
        if (r.w <= 0 || r.h <= 0) {
            s->dirty[i].w = 0;
            continue;
        }
        s->dirty[i] = r;
        for (int row = 0; row < r.h; row++) {
            uint32_t *dst = (uint32_t *) (s->fb
                    + (size_t) (r.y + row) * (size_t) fw * 4u) + r.x;
            if (s->colorLevel <= 0) {
                const uint32_t *src = (const uint32_t *) (cl->frameBuffer
                        + (size_t) (r.y + row) * (size_t) fw * 4u) + r.x;
                for (int col = 0; col < r.w; col++) {
                    dst[col] = src[col] | 0xFF000000u;
                }
            } else {
                const uint8_t *src = cl->frameBuffer
                        + (size_t) (r.y + row) * (size_t) fw + r.x;
                for (int col = 0; col < r.w; col++) {
                    dst[col] = s->palette[src[col]];
                }
            }
        }
    }
    pthread_rwlock_unlock(&s->fbLock);

    for (int i = 0; i < s->ndirty; i++) {
        if (s->dirty[i].w > 0) {
            s->cb.damaged(s->cb.user, s->dirty[i].x, s->dirty[i].y, s->dirty[i].w,
                          s->dirty[i].h);
        }
    }
    s->cb.frame_end(s->cb.user);
    s->ndirty = 0;
}

/* A 0x0 shape means the server hid the pointer, which the library reports
   only with patches/libvncserver's 0007; it is passed on with no pixels, which
   is how the session API represents a hidden pointer. */
static void onGotCursor(rfbClient *cl, int xhot, int yhot, int w, int h, int bpp) {
    Session *s = sessionOf(cl);
    uint32_t *rgba = NULL;
    /* The cursor arrives in the framebuffer's format, so a reduced format is
       one byte per pixel here too. */
    if (w > 0 && h > 0 && cl->rcSource && (bpp == 4 || bpp == 1)) {
        const int n = w * h;
        rgba = malloc((size_t) n * sizeof(uint32_t));
        if (rgba) {
            for (int i = 0; i < n; i++) {
                /* The framebuffer format is R,G,B,_ in memory, and the palette
                   uses the same packing, which is the session API's order; the
                   mask is one byte per pixel, nonzero where the cursor is
                   drawn, and becomes the alpha. */
                const uint32_t p = bpp == 4
                        ? ((const uint32_t *) cl->rcSource)[i]
                        : s->palette[cl->rcSource[i]];
                const uint32_t a = (cl->rcMask && !cl->rcMask[i]) ? 0u : 0xFFu;
                rgba[i] = (a << 24) | (p & 0x00FFFFFFu);
            }
        }
    }
    s->cb.cursor(s->cb.user, rgba, w, h, xhot, yhot);
    free(rgba);
}

static void onBell(rfbClient *cl) {
    Session *s = sessionOf(cl);
    s->cb.bell(s->cb.user);
}

static void onCutText(rfbClient *cl, const char *text, int len) {
    Session *s = sessionOf(cl);
    if (!text || len <= 0) {
        return;
    }
    /* ServerCutText is Latin-1, one code point per byte, which is at most two
       bytes of UTF-8 each. */
    char *utf8 = malloc((size_t) len * 2 + 1);
    if (!utf8) {
        return;
    }
    size_t w = 0;
    for (int i = 0; i < len; i++) {
        const uint8_t c = (uint8_t) text[i];
        if (c < 0x80) {
            utf8[w++] = (char) c;
        } else {
            utf8[w++] = (char) (0xC0 | (c >> 6));
            utf8[w++] = (char) (0x80 | (c & 0x3F));
        }
    }
    utf8[w] = '\0';
    s->cb.clipboard(s->cb.user, utf8);
    free(utf8);
}

/* Blocks the protocol thread until the prompt is answered, by design: the
   answer comes from a dialog shown to the user. */
static int askCredentials(Session *s, int needsUserName) {
    pthread_mutex_lock(&s->lock);
    s->credWaiting = 1;
    s->credAnswered = 0;
    free(s->credUser);
    free(s->credPass);
    s->credUser = NULL;
    s->credPass = NULL;
    pthread_mutex_unlock(&s->lock);

    s->cb.credentials_needed(s->cb.user, needsUserName != 0);

    pthread_mutex_lock(&s->lock);
    while (!s->credAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int ok = s->credPass != NULL;
    s->credWaiting = 0;
    pthread_mutex_unlock(&s->lock);
    return ok;
}

/* The same wait, for the other two questions a handshake can ask: whether to
   accept a fingerprint, or a far end with no identity at all. Both are
   answered through answer_trust, since only one of them is ever
   outstanding. */
static int askIdentity(Session *s, void (*ask)(void *, const char *), const char *arg) {
    pthread_mutex_lock(&s->lock);
    s->trustAnswered = 0;
    s->trustAccepted = 0;
    pthread_mutex_unlock(&s->lock);

    ask(s->cb.user, arg);

    pthread_mutex_lock(&s->lock);
    while (!s->trustAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int accepted = s->trustAccepted && !s->quit;
    pthread_mutex_unlock(&s->lock);
    return accepted;
}

/* Called for every certificate. One that fails to verify against the system's
   authorities (the phone's store, or the host's) arrives here as upstream
   always sent it, which is nearly every one, since authorities do not sign VNC
   servers' certificates. One that verifies arrives here too, whether or not it
   is for the name dialled, through the patches on the library's
   verify_certificate_callback: an authority's signature shows who the server
   claims to be, not that it is the same server as last time. Either way the
   identity is the fingerprint, and the answer comes from the pin store or the
   user, as for the TigerVNC backend, whose library is patched the same way.

   Formatted as `openssl x509 -fingerprint -sha256` prints it, which is what
   the other two backends pin, so a host reached through two of them has one
   entry rather than two. */
static rfbBool onCertMismatch(rfbClient *cl, const char *subject, time_t from, time_t until,
                              const uint8_t *sha256, size_t len) {
    Session *s = sessionOf(cl);
    (void) from;
    (void) until;
    if (len != 32) {
        return FALSE;
    }
    char hex[32 * 3 + 1];
    for (size_t i = 0; i < len; i++) {
        snprintf(hex + i * 3, 4, "%02X:", sha256[i]);
    }
    hex[len * 3 - 1] = '\0';
    LOGI("certificate %s (%s)", hex, subject ? subject : "no subject");
    return askIdentity(s, s->cb.trust_needed, hex) ? TRUE : FALSE;
}

/* The anonymous VeNCrypt sub-types. In the patched library, setting this hook
   adds them to the acceptable set, and it is called once the server has
   agreed one of them, before the TLS handshake. No certificate follows and
   there is nothing more to learn, so this is both the last point worth asking
   and the first point possible.

   Always installed, even for a connection that does not allow them, so that
   the refusal gives this file's reason rather than the library's "unknown
   authentication scheme", which is what the same server produced before and
   is not what happened. The TigerVNC backend refuses at the same point for
   its own reason. */
static rfbBool onConfirmAnonymousTLS(rfbClient *cl) {
    Session *s = sessionOf(cl);
    LOGI("anonymous VeNCrypt sub-type %u", (unsigned) cl->subAuthScheme);
    if (!s->anonymousTls) {
        rfbClientErr("This server only offers encryption without a peer certificate, which is not enabled for this connection.\n");
        return FALSE;
    }
    return askIdentity(s, s->cb.unverified,
                       "This server offers encryption without a peer certificate.")
                   ? TRUE
                   : FALSE;
}

static char *onGetPassword(rfbClient *cl) {
    Session *s = sessionOf(cl);
    pthread_mutex_lock(&s->lock);
    if (s->storedPass && !s->storedPassUsed) {
        s->storedPassUsed = 1;
        char *p = strdup(s->storedPass);
        pthread_mutex_unlock(&s->lock);
        return p;
    }
    pthread_mutex_unlock(&s->lock);

    if (!askCredentials(s, 0)) {
        return NULL;
    }
    pthread_mutex_lock(&s->lock);
    char *p = s->credPass ? strdup(s->credPass) : NULL;
    pthread_mutex_unlock(&s->lock);
    return p;
}

static rfbCredential *onGetCredential(rfbClient *cl, int type) {
    Session *s = sessionOf(cl);
    if (type == rfbCredentialTypeX509) {
        /* Every field left null: no CA file, so the library checks against
           the system's authorities, and no fingerprint given in advance, so
           every certificate, verified or not, arrives at onCertMismatch, the
           only place this backend decides on one. */
        return calloc(1, sizeof(rfbCredential));
    }
    if (type != rfbCredentialTypeUser) {
        return NULL;
    }
    /* VeNCrypt's top-level Plain has no TLS under it: the user name and
       password would go to the server in the clear. The library accepts it,
       over VncAuth or an anonymous TLS sub-type listed before it too, and asks
       for the credentials before sending them, so this is where it is
       refused. The TLS and X509 Plain sub-types come here as well, inside
       their tunnel, and are answered as usual. */
    if (cl->subAuthScheme == rfbVeNCryptPlain) {
        rfbClientErr("The remote server is requesting plaintext credentials (VeNCrypt Plain), which are not supported by this app.\n");
        return NULL;
    }
    char *user = NULL, *pass = NULL;
    pthread_mutex_lock(&s->lock);
    if (s->storedPass && !s->storedPassUsed) {
        s->storedPassUsed = 1;
        user = s->storedUser ? strdup(s->storedUser) : strdup("");
        pass = strdup(s->storedPass);
    }
    pthread_mutex_unlock(&s->lock);

    if (!pass) {
        if (!askCredentials(s, 1)) {
            return NULL;
        }
        pthread_mutex_lock(&s->lock);
        user = strdup(s->credUser ? s->credUser : "");
        pass = strdup(s->credPass ? s->credPass : "");
        pthread_mutex_unlock(&s->lock);
    }

    rfbCredential *c = calloc(1, sizeof(rfbCredential));
    if (!c) {
        free(user);
        free(pass);
        return NULL;
    }
    c->userCredential.username = user;
    c->userCredential.password = pass;
    return c;
}

/* ---- logging ------------------------------------------------------------ */

/* The last message libvncclient logged, so that a refused connection can be
   reported in its words rather than ours. Its diagnosis is far more specific
   than anything this shim can work out ("Unknown authentication scheme from
   VNC server: 13, 133, 5, 129, 6, 130" rather than "Could not connect"), and
   it leaves the library only through these two hooks, which have no user
   pointer. Thread-local in place of that pointer: a session does its whole
   handshake on its own thread, so the last line logged there is its own,
   whereas with two sessions failing at once a global would report one's
   reason for the other. */
static _Thread_local char lastError[256];
static _Thread_local char lastLine[256];

/* Trims the trailing newline, since the text is shown on screen. */
static void remember(char *slot, const char *line) {
    snprintf(slot, 256, "%s", line);
    size_t n = strlen(slot);
    while (n > 0 && (slot[n - 1] == '\n' || slot[n - 1] == '\r')) {
        slot[--n] = '\0';
    }
}

static void logInfo(const char *fmt, ...) {
    char line[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    remember(lastLine, line);
    vento_log(VENTO_LOG_INFO, TAG, "%s", line);
}

static void logErr(const char *fmt, ...) {
    char line[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    remember(lastError, line);
    vento_log(VENTO_LOG_WARN, TAG, "%s", line);
}

static void forgetLastError(void) {
    lastError[0] = '\0';
    lastLine[0] = '\0';
}

/* Copies out, because the next line logged overwrites it. */
static void takeLastError(char *out, size_t size) {
    /* The error hook first, then the plain one: libvncclient logs most
       messages through `rfbClientLog`, including the one that explains a
       refused connection, and uses `rfbClientErr` for few of them. Whichever
       it used, its last message before giving up is the reason. */
    snprintf(out, size, "%s", lastError[0] ? lastError : lastLine);
}

/* ---- the protocol thread ------------------------------------------------ */

/* Converts to what RFB's original ClientCutText can carry: everything above
   U+00FF becomes a question mark, since the extended clipboard is not used
   (see GotXCutText in protocolThread). */
static char *utf8ToLatin1(const char *utf8) {
    const size_t n = strlen(utf8);
    char *out = malloc(n + 1);
    if (!out) {
        return NULL;
    }
    size_t w = 0;
    for (size_t i = 0; i < n;) {
        const uint8_t c = (uint8_t) utf8[i];
        if (c < 0x80) {
            out[w++] = (char) c;
            i++;
        } else if ((c & 0xE0) == 0xC0 && i + 1 < n) {
            const uint32_t cp = ((c & 0x1Fu) << 6) | ((uint8_t) utf8[i + 1] & 0x3Fu);
            out[w++] = cp <= 0xFFu ? (char) cp : '?';
            i += 2;
        } else {
            out[w++] = '?';
            i += (c & 0xF0) == 0xE0 ? 3 : 4;
        }
    }
    out[w] = '\0';
    return out;
}

/* Copies the requested encodings into the client's appData, through the
   protocol thread's own copy (`applied`). Protocol thread only. */
static void takeEncodings(Session *s, rfbClient *cl) {
    pthread_mutex_lock(&s->lock);
    char *fresh = dupOrNull(s->encodings);
    const int compressLevel = s->compressLevel;
    const int qualityLevel = s->qualityLevel;
    s->encodingsChanged = 0;
    pthread_mutex_unlock(&s->lock);

    pthread_rwlock_wrlock(&s->fbLock);
    char *old = s->applied;
    s->applied = fresh;
    cl->appData.encodingsString = fresh;
    cl->appData.compressLevel = compressLevel;
    cl->appData.qualityLevel = qualityLevel < 0 ? 9 : qualityLevel;
    /* Required: the quality pseudo-encoding is what lets Tight send JPEG at
       all, so "lossless" means not sending it rather than a value of it. */
    cl->appData.enableJPEG = (qualityLevel >= 0);
    pthread_rwlock_unlock(&s->fbLock);
    free(old);
}

static void applyEncodings(Session *s) {
    rfbClient *cl = s->cl;
    if (!cl) {
        return;
    }
    takeEncodings(s, cl);
    SetFormatAndEncodings(cl);
}

/* The server's screen with this id, in the library's list, or NULL. */
static const rfbExtDesktopScreen *serverScreen(rfbClient *client, uint32_t id) {
    for (int i = 0; i < client->numScreens; i++) {
        if (rfbClientSwap32IfLE(client->screens[i].id) == id) {
            return &client->screens[i];
        }
    }
    return NULL;
}

/* Converts a requested layout (`n` screens of five ints: id, x, y, width,
   height) into the screens to send, in network order, matched against the
   server's layout. A screen whose id the server has is that screen, moved or
   resized, and keeps its flags, so the request changes the existing desktop
   rather than creating a new one. A screen with an id the server does not
   have, or one already used, is new and gets the lowest id in neither
   layout, as TigerVNC's viewer assigns new screens a fresh id. Returns false
   if the result is not a valid layout of a `width × height` desktop, as
   checked by TigerVNC's ScreenSet::validate before sending: no screen empty or
   outside the desktop, and at most 255 of them. */
static bool buildLayout(rfbClient *client, int width, int height, const int32_t *flat, int n,
                        rfbExtDesktopScreen *out) {
    if (n <= 0 || n > 255) {
        return false;
    }
    bool kept[255] = {false};
    for (int i = 0; i < n; i++) {
        const uint32_t id = (uint32_t) flat[5 * i];
        bool repeat = false;
        for (int j = 0; j < i; j++) {
            repeat |= kept[j] && (uint32_t) flat[5 * j] == id;
        }
        kept[i] = !repeat && serverScreen(client, id) != NULL;
    }
    uint32_t next = 0;
    for (int i = 0; i < n; i++) {
        const int32_t *m = &flat[5 * i];
        if (m[1] < 0 || m[2] < 0 || m[3] <= 0 || m[4] <= 0 || m[1] > width - m[3]
                || m[2] > height - m[4]) {
            return false;
        }
        uint32_t id;
        uint32_t flags = 0;
        if (kept[i]) {
            id = (uint32_t) m[0];
            flags = rfbClientSwap32IfLE(serverScreen(client, id)->flags);
        } else {
            for (;; next++) {
                bool used = serverScreen(client, next) != NULL;
                for (int j = 0; j < n && !used; j++) {
                    used = kept[j] && (uint32_t) flat[5 * j] == next;
                }
                if (!used) {
                    break;
                }
            }
            id = next++;
        }
        out[i].id = rfbClientSwap32IfLE(id);
        out[i].x = (uint16_t) rfbClientSwap16IfLE((uint16_t) m[1]);
        out[i].y = (uint16_t) rfbClientSwap16IfLE((uint16_t) m[2]);
        out[i].width = (uint16_t) rfbClientSwap16IfLE((uint16_t) m[3]);
        out[i].height = (uint16_t) rfbClientSwap16IfLE((uint16_t) m[4]);
        out[i].flags = rfbClientSwap32IfLE(flags);
    }
    return true;
}

/* Sends a SetDesktopSize of `n` screens, already in network order, built here
   rather than with the library's SendExtDesktopSize: that sends a single
   screen, `client->screen`, which the library's reader leaves at the *last*
   screen of the server's list, so with two screens it would request the
   second one's id and position at the whole desktop's size. What it does
   around the message is kept: a full update request at the new size, then no
   more until the server's answer clears `requestedResize`, so that an update
   at the old size is not decoded into a framebuffer about to be reallocated.
   Nothing is sent if the layout matches the server's current one. */
static rfbBool sendLayout(rfbClient *client, int width, int height,
                          const rfbExtDesktopScreen *screens, int n) {
    if (width == client->width && height == client->height && n == client->numScreens
            && memcmp(screens, client->screens, (size_t) n * sizeof(rfbExtDesktopScreen)) == 0) {
        return TRUE;
    }
    rfbSetDesktopSizeMsg sdm;
    memset(&sdm, 0, sizeof(sdm));
    sdm.type = rfbSetDesktopSize;
    sdm.width = (uint16_t) rfbClientSwap16IfLE((uint16_t) width);
    sdm.height = (uint16_t) rfbClientSwap16IfLE((uint16_t) height);
    sdm.numberOfScreens = (uint8_t) n;
    if (!WriteToRFBServer(client, (char *) &sdm, sz_rfbSetDesktopSizeMsg)) {
        return FALSE;
    }
    for (int i = 0; i < n; i++) {
        if (!WriteToRFBServer(client, (char *) &screens[i], sz_rfbExtDesktopScreen)) {
            return FALSE;
        }
    }
    client->requestedResize = FALSE;
    SendFramebufferUpdateRequest(client, 0, 0, width, height, FALSE);
    client->requestedResize = TRUE;
    return TRUE;
}

/* Caller must not hold the lock: these call into libvncclient. */
static void runCmd(Session *s, Cmd *c) {
    rfbClient *cl = s->cl;
    switch (c->type) {
        case CMD_POINTER:
            if (cl && !s->viewOnly) {
                SendPointerEvent(cl, c->a, c->b, c->c);
            }
            break;
        case CMD_KEY_DOWN:
            if (cl && !s->viewOnly) {
                /* One entry per key, not per press: a key already down going
                   down again is auto-repeat, and appending would fill the table
                   within a few seconds of holding a key, after which the next
                   key pressed would not be recorded and never released. */
                int slot = -1;
                for (int i = 0; i < s->nkeys; i++) {
                    if (s->keys[i].id == c->id) {
                        slot = i;
                        break;
                    }
                }
                if (slot < 0 && s->nkeys < MAX_HELD_KEYS) {
                    slot = s->nkeys++;
                }
                if (slot >= 0) {
                    s->keys[slot].id = c->id;
                    s->keys[slot].sym = (uint32_t) c->a;
                }
                SendKeyEvent(cl, (uint32_t) c->a, TRUE);
            }
            break;
        case CMD_KEY_UP:
            /* The keysym recorded at press time, not one derived again from
               the key: the session API requires a release to match its press,
               and libvncclient's SendKeyEvent identifies keys only by keysym. */
            for (int i = 0; i < s->nkeys; i++) {
                if (s->keys[i].id == c->id) {
                    if (cl && !s->viewOnly) {
                        SendKeyEvent(cl, s->keys[i].sym, FALSE);
                    }
                    s->keys[i] = s->keys[--s->nkeys];
                    break;
                }
            }
            break;
        case CMD_RELEASE_KEYS:
            for (int i = 0; i < s->nkeys; i++) {
                if (cl && !s->viewOnly) {
                    SendKeyEvent(cl, s->keys[i].sym, FALSE);
                }
            }
            s->nkeys = 0;
            break;
        case CMD_CLIPBOARD:
            if (cl && c->text && !s->viewOnly) {
                /* ClientCutText is Latin-1, and the string arrives as UTF-8. */
                char *latin1 = utf8ToLatin1(c->text);
                if (latin1) {
                    SendClientCutText(cl, latin1, (int) strlen(latin1));
                    free(latin1);
                }
            }
            break;
        case CMD_RESIZE:
            /* One screen covering the desktop, which is TigerVNC's windowed
               case and the only one a phone has: the server's first screen,
               so that its id and flags are sent back unchanged, or a new one
               if the server has reported none. A refused request leaves the
               size unchanged, so nothing happens here and the panel keeps
               reporting the current size. */
            if (cl && !s->viewOnly && s->canResize) {
                rfbClient *client = cl; /* the byte-order macros refer to `client` */
                const int32_t one[5] = {
                        client->numScreens > 0 ? (int32_t) rfbClientSwap32IfLE(client->screens[0].id)
                                               : 0,
                        0, 0, c->a, c->b};
                rfbExtDesktopScreen screen;
                if (buildLayout(cl, c->a, c->b, one, 1, &screen)) {
                    sendLayout(cl, c->a, c->b, &screen, 1);
                }
            }
            break;
        case CMD_LAYOUT:
            if (cl && !s->viewOnly && s->canResize && c->screens) {
                rfbExtDesktopScreen screens[255];
                if (buildLayout(cl, c->a, c->b, c->screens, c->c, screens)) {
                    sendLayout(cl, c->a, c->b, screens, c->c);
                } else {
                    LOGI("not asking for %dx%d in %d screens: that is not a valid layout", c->a, c->b,
                         c->c);
                }
            }
            break;
        default:
            break;
    }
    cmdFree(c);
}

static int drain(Session *s) {
    for (;;) {
        Cmd c;
        pthread_mutex_lock(&s->lock);
        if (s->tail == s->head) {
            const int quit = s->quit;
            const int encodings = s->encodingsChanged;
            pthread_mutex_unlock(&s->lock);
            if (encodings && !quit) {
                applyEncodings(s);
            }
            return quit;
        }
        c = s->ring[s->tail];
        s->ring[s->tail].text = NULL;
        s->ring[s->tail].screens = NULL;
        s->tail = (s->tail + 1) % CMD_RING;
        pthread_mutex_unlock(&s->lock);
        runCmd(s, &c);
    }
}

static void drainWake(Session *s) {
    char buf[64];
    while (read(s->wake[0], buf, sizeof(buf)) > 0) {
        /* nothing */
    }
}

/* Whether the session has been asked to end. */
static int quitting(Session *s) {
    pthread_mutex_lock(&s->lock);
    const int quit = s->quit;
    pthread_mutex_unlock(&s->lock);
    return quit;
}

/* The second half of a non-blocking connect, polled together with the wake
   pipe: a writable socket means the connect finished (`SO_ERROR` gives the
   result), and a wake with `quit` set means the session was ended. Returns 0
   and sets `*err` if it did not connect. */
static int waitConnected(Session *s, int fd, int timeoutMs, int *err) {
    struct timespec start;
    clock_gettime(CLOCK_MONOTONIC, &start);
    for (;;) {
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        const long elapsed = (now.tv_sec - start.tv_sec) * 1000L + (now.tv_nsec - start.tv_nsec) / 1000000L;
        if (elapsed >= timeoutMs) {
            *err = ETIMEDOUT;
            return 0;
        }
        struct pollfd fds[2] = {{fd, POLLOUT, 0}, {s->wake[0], POLLIN, 0}};
        const int r = poll(fds, 2, (int) (timeoutMs - elapsed));
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            *err = errno;
            return 0;
        }
        if (fds[1].revents) {
            /* Any other wake is a command, and the ring is drained once the
               session is up whether or not the pipe still has bytes. */
            drainWake(s);
            if (quitting(s)) {
                *err = ECANCELED;
                return 0;
            }
        }
        if (fds[0].revents) {
            int soError = 0;
            socklen_t len = sizeof soError;
            if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &soError, &len) != 0) {
                soError = errno;
            }
            *err = soError;
            return soError == 0;
        }
    }
}

/* libvncclient's dial, done here so that a disconnect can end it. The
   library's (`ConnectClientToTcpAddr6WithTimeout`) keeps its socket private
   and waits out `connectTimeout` in its own `select`, so a session ended
   while dialling an unreachable address waited for the full timeout, and so
   did `op_destroy`'s join. Otherwise this is the same dial (every address in
   turn, each with the whole timeout, the socket left non-blocking with
   `TCP_NODELAY` as the library leaves it), with the wake pipe polled
   alongside the socket. */
static rfbSocket dial(Session *s, const char *host, int port, int timeoutMs) {
    char service[8];
    snprintf(service, sizeof service, "%d", port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    struct addrinfo *res = NULL;
    const int gai = getaddrinfo(host[0] ? host : "localhost", service, &hints, &res);
    if (gai != 0) {
        logErr("Could not find %s: %s", host, gai_strerror(gai));
        return RFB_INVALID_SOCKET;
    }
    int err = EHOSTUNREACH;
    rfbSocket sock = RFB_INVALID_SOCKET;
    for (struct addrinfo *ai = res; ai && sock == RFB_INVALID_SOCKET && err != ECANCELED; ai = ai->ai_next) {
        const int fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (fd < 0) {
            err = errno;
            continue;
        }
        const int fl = fcntl(fd, F_GETFL, 0);
        if (fl < 0 || fcntl(fd, F_SETFL, fl | O_NONBLOCK) < 0) {
            err = errno;
            close(fd);
            continue;
        }
        if (connect(fd, ai->ai_addr, ai->ai_addrlen) == 0) {
            sock = fd;
        } else if (errno != EINPROGRESS) {
            err = errno;
            close(fd);
        } else if (waitConnected(s, fd, timeoutMs, &err)) {
            sock = fd;
        } else {
            close(fd);
        }
    }
    freeaddrinfo(res);
    if (sock == RFB_INVALID_SOCKET) {
        /* A dial ended by the session itself is not reported. */
        if (err != ECANCELED) {
            logErr("Unable to connect to VNC server: %s", strerror(err));
        }
        return RFB_INVALID_SOCKET;
    }
    const int one = 1;
    setsockopt(sock, IPPROTO_TCP, TCP_NODELAY, (const char *) &one, sizeof one);
    return sock;
}

/* `rfbInitClient` with our own connect step (see `dial`): the library's two
   halves of it, `rfbClientConnect` and `rfbClientInitialise`, with the first
   replaced, and the client freed on failure as the library frees it, after
   it has been unpublished so that `stop` cannot reach a freed client. A UNIX
   socket path, which the library's connect also accepts as a host, is still
   dialled by the library: it connects or fails at once, with no wait. */
static int initClient(Session *s, rfbClient *cl) {
    struct stat st;
    int ok;
    if (stat(cl->serverHost, &st) == 0 && S_ISSOCK(st.st_mode)) {
        ok = rfbClientConnect(cl);
    } else {
        const int timeoutMs = s->connectTimeoutMs > 0 ? s->connectTimeoutMs : 20000;
        const rfbSocket sock = dial(s, cl->serverHost, cl->serverPort, timeoutMs);
        ok = sock != RFB_INVALID_SOCKET;
        if (ok) {
            /* Under the lock `stop` holds: from here a disconnect shuts down
               this socket, and a disconnect that came first means it is not
               used at all. */
            pthread_mutex_lock(&s->lock);
            if (s->quit) {
                close(sock);
                ok = 0;
            } else {
                cl->sock = sock;
            }
            pthread_mutex_unlock(&s->lock);
        }
    }
    if (ok && rfbClientInitialise(cl)) {
        return 1;
    }
    publishClient(s, NULL);
    rfbClientCleanup(cl);
    return 0;
}

/* Whether the TLS session holds decrypted bytes the library has not read. A
   record is read into the library's buffer only as far as the current message
   needs, so the rest of it (a whole message, the next one, or the end of one)
   is in neither that buffer nor the socket, and poll() would wait for the
   server to send more before any of it was handled. Upstream's own loop has
   the same gap. */
static bool tlsPending(rfbClient *cl) {
    return cl->tlsSession != NULL
           && gnutls_record_check_pending((gnutls_session_t) cl->tlsSession) > 0;
}

static void *protocolThread(void *arg) {
    Session *s = (Session *) arg;

    if (s->addressError[0]) {
        fireClosed(s, s->addressError);
        return NULL;
    }

    rfbClient *cl = rfbGetClient(8, 3, 4);
    if (!cl) {
        fireClosed(s, "Out of memory");
        return NULL;
    }
    rfbClientSetClientData(cl, &sessionTag, s);

    setColorFormat(s, cl);

    /* Despite the name, this asks for the cursor pseudo-encodings, so the
       shape arrives to be drawn locally. It also adds PointerPos to the list,
       and the LED state pseudo-encoding is always sent with it. Neither can be
       removed without patching the library, and both are ignored here, but
       they are the difference between the two RFB backends' encoding lists,
       which matters when comparing them. */
    cl->appData.useRemoteCursor = TRUE;
    cl->appData.shareDesktop = s->shared;
    /* Without this the original DesktopSize pseudo-encoding is never
       requested, and only a server that supports ExtendedDesktopSize can
       report a resize. */
    cl->canHandleNewFBSize = TRUE;
    takeEncodings(s, cl);
    cl->connectTimeout = (unsigned) (s->connectTimeoutMs > 0 ? (s->connectTimeoutMs + 999) / 1000 : 20);

    cl->MallocFrameBuffer = onMallocFrameBuffer;
    cl->GotFrameBufferUpdate = onGotUpdate;
    cl->FinishedFrameBufferUpdate = onFinishedUpdate;
    cl->GotCursorShape = onGotCursor;
    cl->GotXCutText = onCutText;
    /* Deliberately *not* GotXCutTextUTF8, which would request the extended
       clipboard and break the server-to-client direction. libvncclient's
       implementation acts only on an unsolicited `provide`: it ignores the
       `notify` a server sends when its clipboard changes, and has no call to
       answer one with a `request`. Against a server that sends `notify`
       rather than `provide`, such as TigerVNC's, enabling it replaces a
       working Latin-1 path with nothing. Tested: with it on, a clipboard set
       on the far end reaches this client as "not provide type. ignore" and
       goes no further. Answering a notify needs a patch to libvncclient, not
       a flag. */
    cl->Bell = onBell;
    cl->GetPassword = onGetPassword;
    cl->GetCredential = onGetCredential;
    cl->GetX509CertFingerprintMismatchDecision = onCertMismatch;
    cl->ConfirmAnonymousTLS = onConfirmAnonymousTLS;

    free(cl->serverHost);
    cl->serverHost = strdup(s->host ? s->host : "");
    cl->serverPort = s->port;

    publishClient(s, cl);

    /* A failed handshake frees the client; `initClient` unpublishes it
       first, so that neither teardown call reads a freed client. */
    forgetLastError();
    const int connected = initClient(s, cl);
    if (!connected) {
        pthread_mutex_lock(&s->lock);
        const int cancelled = s->quit;
        pthread_mutex_unlock(&s->lock);
        char why[256];
        takeLastError(why, sizeof why);
        fireClosed(s, cancelled ? "" : (why[0] ? why : "Could not connect"));
        return NULL;
    }

    publishFacts(s, cl);

    /* An IPv6 literal is bracketed, as it would be typed, so that it stays
       readable with a port after it. */
    if (strchr(cl->serverHost, ':')) {
        LOGI("connected to [%s]:%d, %dx%d", cl->serverHost, cl->serverPort, cl->width, cl->height);
    } else {
        LOGI("connected to %s:%d, %dx%d", cl->serverHost, cl->serverPort, cl->width, cl->height);
    }

    for (;;) {
        if (drain(s)) {
            break;
        }

        pthread_mutex_lock(&s->lock);
        const int paused = s->paused;
        pthread_mutex_unlock(&s->lock);

        if (!paused && (cl->buffered > 0 || tlsPending(cl))) {
            if (!HandleRFBServerMessage(cl)) {
                break;
            }
            continue;
        }

        /* The socket is polled only while the session has focus. An RFB
           server sends only what was requested, and libvncclient requests the
           next update at the end of each one it handles, so leaving the last
           one unread is enough to pause: at most one update waits in the
           socket and the server then stops sending. */
        struct pollfd fds[2];
        int n = 0;
        fds[n].fd = s->wake[0];
        fds[n].events = POLLIN;
        n++;
        if (!paused && cl->sock != RFB_INVALID_SOCKET) {
            fds[n].fd = cl->sock;
            fds[n].events = POLLIN;
            n++;
        }
        const int r = poll(fds, (nfds_t) n, -1);
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            break;
        }
        if (fds[0].revents) {
            drainWake(s);
        }
        if (n > 1 && (fds[1].revents & POLLIN)) {
            if (!HandleRFBServerMessage(cl)) {
                break;
            }
        } else if (n > 1 && (fds[1].revents & (POLLERR | POLLHUP))) {
            break;
        }
    }

    pthread_mutex_lock(&s->lock);
    const int asked = s->quit;
    pthread_mutex_unlock(&s->lock);

    publishClient(s, NULL);
    free(cl->frameBuffer);
    cl->frameBuffer = NULL;
    rfbClientCleanup(cl);

    fireClosed(s, asked ? "" : "The connection was lost");
    return NULL;
}

/* ---- the address -------------------------------------------------------- */

/* Whether `address` is an unbracketed IPv6 literal. `host::port` is the only
   unbracketed address with two colons that is not one: there, the last colon
   directly follows the first, and there is no third. */
static int bareLiteral(const char *address) {
    const char *first = strchr(address, ':');
    return first && strrchr(address, ':') > first + 1;
}

/* An address is `host`, `host:port`, `host::port`, `[literal]` or
   `[literal]:port`, and a bare IPv6 literal is refused with an error rather
   than guessed at: a number under 100 is a display here, so `::1:1` is
   ambiguous, and the previous guess produced `::`, the wildcard address,
   which does connect to something. The host is left at the start of
   `address`, which remains the caller's buffer to free; `why` is empty unless
   there is nothing to dial. */
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
    if (len == 0 || address[0] == ':') {
        snprintf(why, whyLen, "No host in address");
        return;
    }

    const char *host = address;
    size_t hostLen = len;
    const char *number = NULL;
    int display = 1;  /* whether a number under 100 is a display rather than a port */

    if (address[0] == '[') {
        const char *close = strchr(address, ']');
        if (!close) {
            snprintf(why, whyLen, "%s missing closing bracket", address);
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
    } else if (bareLiteral(address)) {
        snprintf(why, whyLen, "IPv6 addresses must be bracketed, like [::1]:5900, not %s", address);
        return;
    } else {
        const char *colon = strrchr(address, ':');
        if (colon) {
            /* `host::port` means the number is a port, not a display. */
            display = colon[-1] != ':';
            hostLen = (size_t) (colon - address) - !display;
            number = colon + 1;
        }
    }

    if (hostLen == 0) {
        snprintf(why, whyLen, "No host in address");
        return;
    }
    if (number) {
        char *end = NULL;
        const long n = strtol(number, &end, 10);
        const long p = display && n >= 0 && n < 100 ? 5900 + n : n;
        if (end == number || *end || p <= 0 || p > 65535) {
            snprintf(why, whyLen, "%s does not end in a port number", address);
            return;
        }
        *port = (int) p;
    }
    memmove(address, host, hostLen);
    address[hostLen] = 0;
}

/* ---- the session's surface ---------------------------------------------- */

static Session *sessionOfBase(vento_session *base) {
    return (Session *) base;
}

static void freeSession(Session *s) {
    for (int i = 0; i < 2; i++) {
        if (s->wake[i] >= 0) {
            close(s->wake[i]);
        }
    }
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        cmdFree(&s->ring[i]);
    }
    free(s->dirty);
    free(s->fb);
    free(s->screens);
    free(s->host);
    free(s->storedUser);
    free(s->storedPass);
    free(s->encodings);
    free(s->applied);
    free(s->credUser);
    free(s->credPass);
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
    free(s);
}

const char *vento_libvnc_version(void) {
    return "LibVNCServer " LIBVNCSERVER_VERSION;
}

vento_session *vento_libvnc_create(const vento_libvnc_config *cfg, const vento_callbacks *cb) {
    Session *s = calloc(1, sizeof(Session));
    if (!s) {
        return NULL;
    }
    s->base.ops = &OPS;
    s->cb = *cb;
    s->wake[0] = s->wake[1] = -1;
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    pthread_rwlock_init(&s->fbLock, NULL);
    if (pipe(s->wake) != 0) {
        freeSession(s);
        return NULL;
    }
    for (int i = 0; i < 2; i++) {
        const int fl = fcntl(s->wake[i], F_GETFL, 0);
        fcntl(s->wake[i], F_SETFL, fl | O_NONBLOCK);
    }

    char *addr = dupOrNull(cfg->address);
    s->port = 5900;
    if (addr) {
        s->host = addr;
        splitAddress(addr, &s->port, s->addressError, sizeof s->addressError);
    }
    s->storedUser = dupOrNull(cfg->user_name);
    s->storedPass = dupOrNull(cfg->password);
    s->encodings = dupOrNull(cfg->encoding);
    s->compressLevel = cfg->compress_level;
    s->qualityLevel = cfg->quality_level;
    s->colorLevel = cfg->color_level;
    s->shared = cfg->shared;
    s->anonymousTls = cfg->anonymous_tls;
    s->connectTimeoutMs = cfg->connect_timeout_ms;

    rfbClientLog = logInfo;
    rfbClientErr = logErr;

    if (pthread_create(&s->thread, NULL, protocolThread, s) != 0) {
        freeSession(s);
        return NULL;
    }
    return &s->base;
}

static void op_answer_credentials(vento_session *base, const char *user, const char *domain,
                                  const char *password) {
    Session *s = sessionOfBase(base);
    (void) domain;
    char *u = dupOrNull(user);
    char *p = dupOrNull(password);
    pthread_mutex_lock(&s->lock);
    free(s->credUser);
    free(s->credPass);
    s->credUser = u;
    s->credPass = p;
    s->credAnswered = 1;
    if (!p) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    wake(s);
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
    wake(s);
}

/* Both ways a session ends set the flag, write to the wake pipe *and* shut
   down the socket. The wake pipe ends the dial (see `dial`) and wakes the
   loop's poll, but nothing else polls it: the protocol thread can be blocked
   in the library's read, which waits for the rest of a message for as long
   as the server takes to send it, during the handshake and after it alike.
   Only shutting down the socket interrupts that. The client is unpublished
   before it is cleaned up, so a socket reached through `s->cl` under `lock`
   is still open. */
static void stop(Session *s) {
    pthread_mutex_lock(&s->lock);
    s->quit = 1;
    s->paused = 0;
    if (s->cl && s->cl->sock >= 0) {
        shutdown(s->cl->sock, SHUT_RDWR);
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    wake(s);
}

static void op_disconnect(vento_session *base) {
    stop(sessionOfBase(base));
}

/* Shuts down the socket as `op_disconnect` does, which matters more here
   because this one *joins*. A session destroyed while still connecting (the
   panel's Reconnect, or a second session started over the first) would
   otherwise block the caller, the main thread on the phone, until the
   connect timeout ran out. */
static void op_destroy(vento_session *base) {
    Session *s = sessionOfBase(base);
    stop(s);
    pthread_join(s->thread, NULL);
    freeSession(s);
}

static void op_pointer(vento_session *base, int x, int y, int buttonMask) {
    Cmd c = {CMD_POINTER, x, y, buttonMask, 0, NULL, NULL};
    post(sessionOfBase(base), &c);
}

static void op_key_down(vento_session *base, uint32_t keysym, int64_t keyId) {
    Cmd c = {CMD_KEY_DOWN, (int) keysym, 0, 0, keyId, NULL, NULL};
    post(sessionOfBase(base), &c);
}

static void op_key_up(vento_session *base, int64_t keyId) {
    Cmd c = {CMD_KEY_UP, 0, 0, 0, keyId, NULL, NULL};
    post(sessionOfBase(base), &c);
}

static void op_release_all_keys(vento_session *base) {
    Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, 0, NULL, NULL};
    post(sessionOfBase(base), &c);
}

static void op_focus(vento_session *base, bool focused) {
    Session *s = sessionOfBase(base);
    pthread_mutex_lock(&s->lock);
    s->paused = !focused;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    wake(s);
}

static void op_view_only(vento_session *base, bool viewOnly) {
    sessionOfBase(base)->viewOnly = viewOnly ? 1 : 0;
}

/* Answers for any number of screens, since this session implements
   `request_layout` (see `can_resize` in vento/backend.h). */
static bool op_can_resize(vento_session *base) {
    Session *s = sessionOfBase(base);
    return s->canResize && !s->viewOnly;
}

/* Four ints per screen without the id, or all five with it. */
static int copyScreens(Session *s, int32_t *out, bool ids) {
    pthread_mutex_lock(&s->lock);
    const int n = s->nscreens;
    for (int i = 0; i < n; i++) {
        const int32_t *m = &s->screens[5 * i];
        if (ids) {
            memcpy(&out[5 * i], m, 5 * sizeof(int32_t));
        } else {
            memcpy(&out[4 * i], m + 1, 4 * sizeof(int32_t));
        }
    }
    pthread_mutex_unlock(&s->lock);
    return n;
}

static int op_monitors(vento_session *base, int32_t *out) {
    return copyScreens(sessionOfBase(base), out, false);
}

static int op_screens(vento_session *base, int32_t *out) {
    return copyScreens(sessionOfBase(base), out, true);
}

static void op_request_desktop_size(vento_session *base, int width, int height) {
    if (width > 0 && height > 0 && width <= 65535 && height <= 65535) {
        Cmd c = {CMD_RESIZE, width, height, 0, 0, NULL, NULL};
        post(sessionOfBase(base), &c);
    }
}

static void op_request_layout(vento_session *base, int width, int height, const int32_t *screens,
                              int n) {
    /* RFB's limits: the count is one byte and every number sixteen bits;
       anything outside them would be sent truncated, as a different layout. */
    if (width <= 0 || height <= 0 || width > 65535 || height > 65535 || !screens || n <= 0
            || n > 255) {
        return;
    }
    Cmd c = {CMD_LAYOUT, width, height, n, 0, NULL, malloc((size_t) n * 5 * sizeof(int32_t))};
    if (c.screens) {
        memcpy(c.screens, screens, (size_t) n * 5 * sizeof(int32_t));
        post(sessionOfBase(base), &c);
    }
}

static void op_clipboard(vento_session *base, const char *utf8) {
    Cmd c = {CMD_CLIPBOARD, 0, 0, 0, 0, dupOrNull(utf8), NULL};
    post(sessionOfBase(base), &c);
}

void vento_libvnc_set_encodings(vento_session *base, const char *encoding, int compressLevel,
                                int qualityLevel) {
    Session *s = sessionOfBase(base);
    char *e = dupOrNull(encoding);
    pthread_mutex_lock(&s->lock);
    char *old = s->encodings;
    s->encodings = e;
    s->compressLevel = compressLevel;
    s->qualityLevel = qualityLevel;
    s->encodingsChanged = 1;
    const int quit = s->quit;
    pthread_mutex_unlock(&s->lock);
    /* Freed immediately, since libvncclient never holds this string: it holds
       the protocol thread's copy, which is replaced when the protocol thread
       next drains (at once if it is waiting, or after the handshake if it is
       still in one). */
    free(old);
    if (!quit) {
        wake(s);
    }
}

/* Uses the wording of TigerVNC's PixelFormat::print, so that this row reads
   the same as the TigerVNC backend's. */
static void describeFormat(char *out, size_t size, const rfbPixelFormat *f) {
    if (!f->trueColour) {
        snprintf(out, size, "depth %d (%dbpp) colour map", f->depth, f->bitsPerPixel);
        return;
    }
    unsigned r = 0, g = 0, b = 0;
    for (unsigned m = f->redMax; m; m >>= 1) { r++; }
    for (unsigned m = f->greenMax; m; m >>= 1) { g++; }
    for (unsigned m = f->blueMax; m; m >>= 1) { b++; }
    snprintf(out, size, "depth %d (%dbpp) %s-endian rgb%u%u%u", f->depth, f->bitsPerPixel,
             f->bigEndian ? "big" : "little", r, g, b);
}

/* Describes a VeNCrypt sub-type for the panel's Security row: which
   authentication ran and, for the three without a certificate, that the
   encryption verified nothing about the server. Reporting only "TLS" would
   imply an identity check these sub-types do not have. */
static void describeVeNCrypt(char *out, size_t size, uint32_t subType) {
    const char *auth;
    switch (subType) {
        case rfbVeNCryptTLSNone:
        case rfbVeNCryptX509None: auth = "None"; break;
        case rfbVeNCryptTLSVNC:
        case rfbVeNCryptX509VNC: auth = "VNC password"; break;
        case rfbVeNCryptTLSPlain:
        case rfbVeNCryptX509Plain: auth = "User name and password"; break;
        default: snprintf(out, size, "VeNCrypt type %u", subType); return;
    }
    const int anonymous = subType == rfbVeNCryptTLSNone || subType == rfbVeNCryptTLSVNC
                          || subType == rfbVeNCryptTLSPlain;
    snprintf(out, size, "%s (VeNCrypt)%s", auth, anonymous ? ", encrypted and unverified" : "");
}

/* Copies the facts of a connection whose handshake is done out of the client,
   on the protocol thread, the only thread that may read it: the handshake
   writes the name, version, security and formats as it goes, without taking
   any lock op_info could hold. None of them changes afterwards. op_info
   returns them in the order the Rust adapter's `info` labels them: desktop
   name, protocol, connection, security, encoding, server and viewer pixels. */
static void publishFacts(Session *s, rfbClient *cl) {
    char desktop[256], protocol[64], security[96], serverPixels[64], viewerPixels[64];
    snprintf(desktop, sizeof(desktop), "%s", cl->desktopName ? cl->desktopName : "");
    snprintf(protocol, sizeof(protocol), "RFB %d.%d", cl->major, cl->minor);
    /* VeNCrypt leaves authScheme at 19 and stores the sub-type separately,
       and the sub-type says which authentication ran and whether the tunnel
       verified anything about the server. */
    switch (cl->authScheme) {
        case rfbNoAuth: snprintf(security, sizeof(security), "None"); break;
        case rfbVncAuth: snprintf(security, sizeof(security), "VNC password"); break;
        case rfbVeNCrypt: describeVeNCrypt(security, sizeof(security), cl->subAuthScheme); break;
        default: snprintf(security, sizeof(security), "Type %u", cl->authScheme); break;
    }
    describeFormat(serverPixels, sizeof(serverPixels), &cl->si.format);
    describeFormat(viewerPixels, sizeof(viewerPixels), &cl->format);
    /* Only the desktop name comes from the server; the rest is written here. */
    char *name = vento_utf8_from_bytes(desktop);

    pthread_rwlock_wrlock(&s->fbLock);
    snprintf(s->facts.desktop, sizeof(s->facts.desktop), "%s", name ? name : "");
    memcpy(s->facts.protocol, protocol, sizeof(protocol));
    memcpy(s->facts.security, security, sizeof(security));
    memcpy(s->facts.serverPixels, serverPixels, sizeof(serverPixels));
    memcpy(s->facts.viewerPixels, viewerPixels, sizeof(viewerPixels));
    s->facts.known = true;
    pthread_rwlock_unlock(&s->fbLock);
    free(name);
}

static bool op_info(vento_session *base, vento_strings *out) {
    Session *s = sessionOfBase(base);
    char encoding[64] = "";

    pthread_rwlock_rdlock(&s->fbLock);
    if (!s->facts.known) {
        pthread_rwlock_unlock(&s->fbLock);
        return false;
    }
    const Facts facts = s->facts;
    /* The requested encodings, not the one in use: libvncclient does not
       report which encoding decoded a rectangle, so this row is the offer and
       its label says so. The other backends report the encoding in use. */
    snprintf(encoding, sizeof(encoding), "%s", s->applied ? s->applied : "");
    pthread_rwlock_unlock(&s->fbLock);

    const char *values[] = {facts.desktop, facts.protocol, "TCP", facts.security, encoding,
                            facts.serverPixels, facts.viewerPixels};
    return vento_strings_set(out, values, sizeof(values) / sizeof(values[0]));
}

/* The counters kept by the patched sockets.c (patches/libvncserver's 0004),
   read under the framebuffer lock because a failed handshake may free the
   client concurrently. */
static bool op_traffic(vento_session *base, uint64_t out[2]) {
    Session *s = sessionOfBase(base);
    bool have = false;
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->cl) {
        out[0] = (uint64_t) s->cl->bytesRcvd;
        out[1] = (uint64_t) s->cl->bytesSent;
        have = true;
    }
    pthread_rwlock_unlock(&s->fbLock);
    return have;
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
        .screens = op_screens,
        .request_layout = op_request_layout,
};
