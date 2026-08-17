// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// LibVNCServer's libvncclient behind the same JNI surface the RFB client we
// wrote has, so the two are interchangeable above this file and the comparison
// is between the clients rather than between two ways of binding one.
//
// Three things here are the shim's rather than the library's, and each is
// forced by libvncclient's API rather than chosen:
//
//   1. One thread owns the rfbClient, and every call from Java is a queued
//      command it drains. Nothing in libvncclient is safe to call from a second
//      thread — reads and writes go through the same buffered socket and the
//      one framebuffer — so a pointer event sent from the main thread would
//      race the protocol thread's own writes.
//   2. There are two framebuffers. libvncclient decodes into its own and has no
//      lock over it, and the seam promises that pixels may be read from the
//      drawing thread at any moment; so the damaged rectangles are copied under
//      a writer lock once the update message is complete, and reads are served
//      from that copy. The alternative — one buffer, and the reader waiting for
//      HandleRFBServerMessage — puts the drawing thread behind a socket read.
//   3. The alpha byte is filled in during that copy. RFB's 32-bit formats leave
//      the fourth byte undefined and the decoder writes whatever the wire said;
//      Android's ARGB_8888 reads it.
//
// Two of their features are available and deliberately not wired: **xvp**, which
// shuts down or reboots the machine at the far end, and **text chat**. Neither
// is missing because it would be hard — nothing in this app asks a session to do
// either, and a protocol capability with no caller is a surface to maintain.

#include <android/bitmap.h>
#include <android/log.h>
#include <errno.h>
#include <fcntl.h>
#include <jni.h>
#include <poll.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include <rfb/rfbclient.h>

#define TAG "LibVnc"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, TAG, __VA_ARGS__)

#define CMD_RING 512
#define MAX_HELD_KEYS 64

enum {
    CMD_POINTER = 1,
    CMD_KEY_DOWN,
    CMD_KEY_UP,
    CMD_RELEASE_KEYS,
    CMD_CLIPBOARD,
    CMD_ENCODINGS,
    CMD_RESIZE,
    CMD_DISCONNECT,
};

typedef struct {
    int type;
    int a, b, c;
    char *text; /* owned by the queue */
} Cmd;

typedef struct {
    int x, y, w, h;
} Rect;

typedef struct {
    JavaVM *vm;
    jobject callbacks; /* global ref */

    rfbClient *cl; /* protocol thread only */

    pthread_t thread;
    int wake[2]; /* self-pipe: the queue's doorbell */

    pthread_mutex_t lock;
    pthread_cond_t cond;

    Cmd ring[CMD_RING];
    int head, tail;

    int quit;
    int paused;
    _Atomic int viewOnly; /* written from the Java thread, read from the protocol one */
    int connected;
    int closedReported;
    _Atomic int canResize; /* an ExtDesktopSize rectangle has arrived; written from the protocol thread */

    /* The far end's screen layout, four ints per screen, copied out of the
       library's own array under `lock` because that array is the protocol
       thread's and this is read from whichever thread asks. */
    jint *screens;
    int nscreens;

    /* the credential prompt, answered from Java */
    int credWaiting, credAnswered;
    char *credUser, *credPass;

    /* and the certificate one, which is the same shape. The anonymous-TLS
       question rides on the same pair: only one of the two can be outstanding,
       since one is asked in place of the other. */
    int trustAnswered, trustAccepted;

    /* true while rfbInitClient has not returned; see nativeDisconnect */
    int handshaking;
    char *storedUser, *storedPass;
    int storedPassUsed;

    /* the copy readRegion is served from */
    pthread_rwlock_t fbLock;
    uint8_t *fb;
    int fbW, fbH;

    /* damage of the update message being handled */
    Rect *dirty;
    int ndirty, capdirty;

    struct {
        int id;
        uint32_t sym;
    } keys[MAX_HELD_KEYS];
    int nkeys;

    char *host;
    int port;
    /* Why the address was not one, empty if it was: a session is created and
       started in one call, so the protocol thread is the first place there is
       to say it. */
    char addressError[160];
    int shared;
    int anonymousTls;
    char *encodings;
    int compressLevel;
    int qualityLevel;
    int colorLevel;
    int connectTimeoutMs;
    char lastEncoding[32];

    /* Every value an 8-bit pixel can take, as the shadow buffer wants it. A
       reduced format is a table lookup per pixel in the copy that was already
       there, rather than arithmetic in it. Unused at 32 bits. */
    uint32_t palette[256];
} Session;

static jclass gCallbacksClass;
static jmethodID mConnected, mDesktopSize, mDamage, mFrameEnd, mCursor, mBell,
        mClipboard, mCredentialsNeeded, mTrustNeeded, mUnverified, mClosed;

/* ---- JNI plumbing ------------------------------------------------------- */

/* Anything a callback throws is pending on this thread when it returns, and
   every JNI call made while an exception is pending is undefined — including
   the detach below, which runs the thread's uncaught-exception handler and
   takes the process with it. So it is reported and cleared here, and a Java
   fault costs a frame rather than the session. */
static void cleared(JNIEnv *env) {
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionDescribe(env);
        (*env)->ExceptionClear(env);
    }
}

static JNIEnv *attach(Session *s, int *attached) {
    JNIEnv *env = NULL;
    *attached = 0;
    if ((*s->vm)->GetEnv(s->vm, (void **) &env, JNI_VERSION_1_6) == JNI_OK) {
        return env;
    }
    if ((*s->vm)->AttachCurrentThread(s->vm, &env, NULL) != JNI_OK) {
        return NULL;
    }
    *attached = 1;
    return env;
}

static void detach(Session *s, int attached) {
    if (attached) {
        (*s->vm)->DetachCurrentThread(s->vm);
    }
}

/* `s->cl` has two kinds of reader — `nativeInfo` under the framebuffer lock,
   and the two teardown calls under `s->lock` — so both are held to change it.
   What makes that necessary rather than tidy is that `rfbInitClient` frees the
   client itself when it fails: a reader that sees the old pointer afterwards
   shuts down a descriptor that is no longer anybody's. */
static void publishClient(Session *s, rfbClient *cl) {
    pthread_rwlock_wrlock(&s->fbLock);
    pthread_mutex_lock(&s->lock);
    s->cl = cl;
    pthread_mutex_unlock(&s->lock);
    pthread_rwlock_unlock(&s->fbLock);
}

/* ---- the command queue -------------------------------------------------- */

/* Caller holds the lock. */
static void ring_push(Session *s, const Cmd *c) {
    const int next = (s->head + 1) % CMD_RING;
    if (next == s->tail) {
        /* Full. Dropping the oldest keeps the newest pointer position, which
           is the one that matters; a dropped button change would not be, so
           this is only ever reached if the protocol thread is wedged. */
        free(s->ring[s->tail].text);
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
        return;
    }
    /* Motion coalesces onto the tail of the queue: the protocol thread can be
       inside a decode, and by the time it drains, every position but the last
       is somewhere the pointer no longer is. A button change is a different
       event and is never merged away. */
    if (c->type == CMD_POINTER && s->head != s->tail) {
        const int last = (s->head + CMD_RING - 1) % CMD_RING;
        if (s->ring[last].type == CMD_POINTER && s->ring[last].c == c->c) {
            s->ring[last] = *c;
            pthread_mutex_unlock(&s->lock);
            const char one = 1;
            (void) !write(s->wake[1], &one, 1);
            return;
        }
    }
    ring_push(s, c);
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

/* Every string the *server* chose crosses here, not just the clipboard.
   `NewStringUTF` takes modified UTF-8, so a Latin-1 byte over 0x7F is not a
   string it can be given at all and a four-byte UTF-8 sequence is not one
   either — and a build with CheckJNI on aborts rather than producing mojibake.
   So anything from the wire decodes to UTF-16 here and uses `NewString`; only a
   string this file wrote itself may go the short way. */
static jstring toJava(JNIEnv *env, const char *bytes) {
    if (!bytes) {
        return NULL;
    }
    const size_t n = strlen(bytes);
    jchar *out = malloc((n + 1) * sizeof(jchar));
    if (!out) {
        return NULL;
    }
    /* Valid UTF-8 as UTF-8 and anything else as Latin-1, which is the rule
       TigerVNC's reader applies to the same field: RFB 3.8 says the desktop
       name is Latin-1 and every server written since sends UTF-8, so the bytes
       alone have to say which they are. Decoded in one pass with a rewind,
       since "is this valid UTF-8" is only answerable by decoding it. */
    size_t w = 0;
    int utf8 = 1;
    for (size_t i = 0; i < n && utf8;) {
        const uint8_t c = (uint8_t) bytes[i];
        int extra;
        uint32_t cp;
        if (c < 0x80) { cp = c; extra = 0; }
        else if ((c & 0xE0) == 0xC0) { cp = c & 0x1Fu; extra = 1; }
        else if ((c & 0xF0) == 0xE0) { cp = c & 0x0Fu; extra = 2; }
        else if ((c & 0xF8) == 0xF0) { cp = c & 0x07u; extra = 3; }
        else { utf8 = 0; break; }
        if (i + (size_t) extra >= n) {
            utf8 = 0;
            break;
        }
        for (int k = 1; k <= extra; k++) {
            const uint8_t cc = (uint8_t) bytes[i + (size_t) k];
            if ((cc & 0xC0) != 0x80) {
                utf8 = 0;
                break;
            }
            cp = (cp << 6) | (cc & 0x3Fu);
        }
        if (!utf8) {
            break;
        }
        i += (size_t) extra + 1;
        if (cp >= 0x10000) {
            cp -= 0x10000;
            out[w++] = (jchar) (0xD800 + (cp >> 10));
            out[w++] = (jchar) (0xDC00 + (cp & 0x3FF));
        } else {
            out[w++] = (jchar) cp;
        }
    }
    if (!utf8) {
        /* One code point a byte, which is one UTF-16 unit each. */
        for (w = 0; w < n; w++) {
            out[w] = (jchar) (uint8_t) bytes[w];
        }
    }
    jstring js = (*env)->NewString(env, out, (jsize) w);
    free(out);
    return js;
}

/* ---- callbacks into Java ------------------------------------------------ */

static void fireDamage(Session *s, JNIEnv *env, int x, int y, int w, int h) {
    (*env)->CallVoidMethod(env, s->callbacks, mDamage, x, y, w, h);
    cleared(env);
}

static void fireClosed(Session *s, const char *detail) {
    pthread_mutex_lock(&s->lock);
    const int already = s->closedReported;
    s->closedReported = 1;
    pthread_mutex_unlock(&s->lock);
    if (already) {
        return;
    }
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        /* The library's own last words, which quote the server's: an
           "Unknown authentication scheme" carries bytes somebody else chose. */
        jstring d = toJava(env, detail ? detail : "");
        (*env)->CallVoidMethod(env, s->callbacks, mClosed, d);
        cleared(env);
        (*env)->DeleteLocalRef(env, d);
        detach(s, attached);
    }
}

/* ---- libvncclient callbacks (protocol thread) --------------------------- */

static rfbClientData sessionTag;

static Session *sessionOf(rfbClient *cl) {
    return (Session *) rfbClientGetClientData(cl, &sessionTag);
}

/* The format asked for on the wire, and the table that puts a reduced one back
   into the shadow buffer's 32 bits. Level 0 is the format that makes the copy
   below a memcpy; 1 and 2 are 64 and 8 colours, which is what the two other RFB
   backends' colour rows mean by rgb222 and rgb111 — the same formats TigerVNC's
   own viewer reduces to. libvncclient's `useBGR233`, `forceTrueColour` and
   `requestedDepth` are not the way to ask for this: the library never reads
   them, and the format it sends is whatever this field says. */
static void setColorFormat(Session *s, rfbClient *cl) {
    cl->format.bigEndian = FALSE;
    cl->format.trueColour = TRUE;
    if (s->colorLevel <= 0) {
        /* 32 bits little-endian with red at shift 0 puts the bytes in memory as
           R, G, B, _ — which is what Android's ARGB_8888 is, once the fourth
           byte is filled in. */
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
        /* Full range rather than a shift: three bits of red have to reach 255,
           or white comes out grey. */
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

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, s->connected ? mDesktopSize : mConnected, w, h);
        cleared(env);
        detach(s, attached);
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

/* Take a copy of the library's screen list, which is the patch's whole point:
   upstream keeps one screen and the layout is the list. The fields are in
   network byte order there, as `client->screen`'s are, so they are swapped here and
   nowhere else. Called once per update message, and copies only when the
   layout has actually moved. */
static void layoutChanged(Session *s, rfbClient *client) {
    const int n = client->numScreens;
    jint stack[4 * 16];
    jint *flat = stack;
    if (n > (int) (sizeof(stack) / sizeof(stack[0])) / 4) {
        return; /* sixteen heads is already absurd; the alternative is a malloc per update */
    }
    for (int i = 0; i < n; i++) {
        flat[4 * i + 0] = (jint) rfbClientSwap16IfLE(client->screens[i].x);
        flat[4 * i + 1] = (jint) rfbClientSwap16IfLE(client->screens[i].y);
        flat[4 * i + 2] = (jint) rfbClientSwap16IfLE(client->screens[i].width);
        flat[4 * i + 3] = (jint) rfbClientSwap16IfLE(client->screens[i].height);
    }
    const size_t bytes = (size_t) n * 4 * sizeof(jint);
    pthread_mutex_lock(&s->lock);
    if (s->nscreens != n || (n > 0 && memcmp(s->screens, flat, bytes) != 0)) {
        jint *copy = n > 0 ? malloc(bytes) : NULL;
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

/* The whole of the second framebuffer's cost, and the only place it is paid:
   once per damaged rectangle per update message, under the writer lock. */
static void onFinishedUpdate(rfbClient *cl) {
    Session *s = sessionOf(cl);
    /* libvncclient has no callback for the ExtDesktopSize rectangle and no
       "does this server resize" of its own; what it leaves behind is the screen
       it read out of one, and a client that has never seen one has a screen of
       zero size. So this is the announcement, read once an update ends because
       that is the first place after it where the shim runs. */
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

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        for (int i = 0; i < s->ndirty; i++) {
            if (s->dirty[i].w > 0) {
                fireDamage(s, env, s->dirty[i].x, s->dirty[i].y, s->dirty[i].w, s->dirty[i].h);
            }
        }
        (*env)->CallVoidMethod(env, s->callbacks, mFrameEnd);
        cleared(env);
        detach(s, attached);
    }
    s->ndirty = 0;
}

/* FNV-1a over a cursor's pixels, which is its identity in CursorCache — so
   this, the other shims' copies of it and the Java one all have to agree.
   Computed while the pixels are still hot from the conversion above; what it
   saves on the other side is a bitmap and a texture per cursor change. */
static jlong cursorHash(const jint *argb, int width, int height) {
    uint64_t h = 0xcbf29ce484222325ull;
    h = (h ^ (uint32_t) width) * 0x100000001b3ull;
    h = (h ^ (uint32_t) height) * 0x100000001b3ull;
    for (int i = 0; i < width * height; i++) {
        h = (h ^ (uint32_t) argb[i]) * 0x100000001b3ull;
    }
    return (jlong) h;
}

static void onGotCursor(rfbClient *cl, int xhot, int yhot, int w, int h, int bpp) {
    Session *s = sessionOf(cl);
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (!env) {
        return;
    }
    jintArray argb = NULL;
    jlong hash = 0;
    /* The cursor arrives in the framebuffer's format, so a reduced one is one
       byte a pixel here too. */
    if (w > 0 && h > 0 && cl->rcSource && (bpp == 4 || bpp == 1)) {
        const int n = w * h;
        argb = (*env)->NewIntArray(env, n);
        if (argb) {
            jint *out = (*env)->GetIntArrayElements(env, argb, NULL);
            for (int i = 0; i < n; i++) {
                /* The framebuffer format is R,G,B,_ in memory, and the palette
                   is the same packing; Java's Color is ARGB packed, so the
                   channels come back apart here. The mask is one byte a pixel,
                   nonzero where the cursor is drawn. */
                const uint32_t p = bpp == 4
                        ? ((const uint32_t *) cl->rcSource)[i]
                        : s->palette[cl->rcSource[i]];
                const uint32_t r = p & 0xFFu, g = (p >> 8) & 0xFFu, b = (p >> 16) & 0xFFu;
                const uint32_t a = (cl->rcMask && !cl->rcMask[i]) ? 0u : 0xFFu;
                out[i] = (jint) ((a << 24) | (r << 16) | (g << 8) | b);
            }
            hash = cursorHash(out, w, h);
            (*env)->ReleaseIntArrayElements(env, argb, out, 0);
        }
    }
    (*env)->CallVoidMethod(env, s->callbacks, mCursor, argb, w, h, xhot, yhot, hash);
    cleared(env);
    if (argb) {
        (*env)->DeleteLocalRef(env, argb);
    }
    detach(s, attached);
}

static void onBell(rfbClient *cl) {
    Session *s = sessionOf(cl);
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, mBell);
        cleared(env);
        detach(s, attached);
    }
}

static void fireClipboard(Session *s, const jchar *chars, size_t n) {
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (!env) {
        return;
    }
    jstring js = (*env)->NewString(env, chars, (jsize) n);
    if (js) {
        (*env)->CallVoidMethod(env, s->callbacks, mClipboard, js);
        cleared(env);
        (*env)->DeleteLocalRef(env, js);
    }
    detach(s, attached);
}

static void onCutText(rfbClient *cl, const char *text, int len) {
    Session *s = sessionOf(cl);
    if (!text || len <= 0) {
        return;
    }
    jchar *out = malloc((size_t) len * sizeof(jchar));
    if (!out) {
        return;
    }
    /* Latin-1 is one code point a byte, which is one UTF-16 unit each. */
    for (int i = 0; i < len; i++) {
        out[i] = (jchar) (uint8_t) text[i];
    }
    fireClipboard(s, out, (size_t) len);
    free(out);
}

/* Blocks the protocol thread until Java answers, which is the design: on the
   other side of it is a dialog and a person. */
static int askCredentials(Session *s, int needsUserName) {
    pthread_mutex_lock(&s->lock);
    s->credWaiting = 1;
    s->credAnswered = 0;
    free(s->credUser);
    free(s->credPass);
    s->credUser = NULL;
    s->credPass = NULL;
    pthread_mutex_unlock(&s->lock);

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, mCredentialsNeeded, (jboolean) (needsUserName != 0));
        cleared(env);
        detach(s, attached);
    }

    pthread_mutex_lock(&s->lock);
    while (!s->credAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int ok = s->credPass != NULL;
    s->credWaiting = 0;
    pthread_mutex_unlock(&s->lock);
    return ok;
}

/* The same wait, for the other two questions a handshake can ask: a
   fingerprint to accept, or a far end with no identity to prove at all. Both
   are answered through nativeAnswerTrust, since only one of them is ever
   outstanding. */
static int askIdentity(Session *s, jmethodID method, const char *arg) {
    pthread_mutex_lock(&s->lock);
    s->trustAnswered = 0;
    s->trustAccepted = 0;
    pthread_mutex_unlock(&s->lock);

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        jstring js = (*env)->NewStringUTF(env, arg);
        (*env)->CallVoidMethod(env, s->callbacks, method, js);
        cleared(env);
        (*env)->DeleteLocalRef(env, js);
        detach(s, attached);
    }

    pthread_mutex_lock(&s->lock);
    while (!s->trustAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int accepted = s->trustAccepted && !s->quit;
    pthread_mutex_unlock(&s->lock);
    return accepted;
}

/* Reached once the certificate has failed to verify against anything, which on
   Android is every certificate: there is no trust store in this build, because
   no authority signs a VNC server's own. So the identity is the fingerprint,
   and the answer comes from the pin store or from a person.

   Printed the way `openssl x509 -fingerprint -sha256` prints it, which is what
   the other two backends pin, so one host reached through two of them is one
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
    return askIdentity(s, mTrustNeeded, hex) ? TRUE : FALSE;
}

/* The anonymous VeNCrypt sub-types: in the patched library this hook's presence
   is what puts them in the acceptable set, and it is called once the server has
   agreed one of them and before the handshake. There is no certificate coming
   and nothing further to learn, so this is both the last moment the question is
   worth asking and the first at which it can be.

   Always installed, even for a connection that does not allow them, so that
   the refusal is this file's sentence rather than the library's "unknown
   authentication scheme" — which is what the same server said before, and is
   not what happened. The TigerVNC backend refuses at the same point for a
   reason of its own. */
static rfbBool onConfirmAnonymousTLS(rfbClient *cl) {
    Session *s = sessionOf(cl);
    LOGI("anonymous VeNCrypt sub-type %u", (unsigned) cl->subAuthScheme);
    if (!s->anonymousTls) {
        rfbClientErr("This server only offers encryption without a peer certificate, which is not enabled for this connection.\n");
        return FALSE;
    }
    return askIdentity(s, mUnverified,
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
        /* Every field left null: no CA to check against and no fingerprint
           given ahead of time, so verification fails and the decision arrives
           at onCertMismatch, which is the one place this backend answers it. */
        return calloc(1, sizeof(rfbCredential));
    }
    if (type != rfbCredentialTypeUser) {
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

/* The last thing libvncclient said, so that a refused connection can be
   reported in its words rather than in ours.
   Its own diagnosis is far better than anything this shim can work out from
   the outside — "Unknown authentication scheme from VNC server: 13, 133, 5,
   129, 6, 130" against "Could not connect" — and the only way it leaves the
   library is through these two hooks, which have no user pointer.
   Per thread, which is what stands in for the pointer they lack: a session
   does its whole handshake on its own thread, so the last line logged there is
   its own — and with two sessions failing at once, a global would report one's
   reason for the other. */
static _Thread_local char lastError[256];
static _Thread_local char lastLine[256];

/* Trimmed, because its lines end in a newline and something going on a screen
   must not. */
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
    __android_log_print(ANDROID_LOG_INFO, TAG, "%s", line);
}

static void logErr(const char *fmt, ...) {
    char line[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    remember(lastError, line);
    __android_log_print(ANDROID_LOG_WARN, TAG, "%s", line);
}

static void forgetLastError(void) {
    lastError[0] = '\0';
    lastLine[0] = '\0';
}

/* Copies out, because the next line logged overwrites it. */
static void takeLastError(char *out, size_t size) {
    /* The error hook first, and the plain one behind it: libvncclient says most
       of what it thinks through `rfbClientLog`, including the sentence that
       explains a refused connection, and reserves `rfbClientErr` for rather
       fewer of them. Whichever it used, the last thing it said before giving up
       is the reason it gave up. */
    snprintf(out, size, "%s", lastError[0] ? lastError : lastLine);
}

/* ---- the protocol thread ------------------------------------------------ */

/* What RFB's original ClientCutText can carry: everything above U+00FF becomes
   a question mark, which is what a clipboard sent to a server that never agreed
   to the extended one is worth. */
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

static void applyEncodings(Session *s) {
    rfbClient *cl = s->cl;
    if (!cl) {
        return;
    }
    cl->appData.encodingsString = s->encodings;
    cl->appData.compressLevel = s->compressLevel;
    cl->appData.qualityLevel = s->qualityLevel < 0 ? 9 : s->qualityLevel;
    /* Not a nicety: the quality pseudo-encoding is what lets Tight send JPEG at
       all, so "lossless" is the absence of it rather than a value of it. */
    cl->appData.enableJPEG = (s->qualityLevel >= 0);
    SetFormatAndEncodings(cl);
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
                   in a second and a half of somebody leaning on a key — after
                   which the next key pressed is not recorded at all and never
                   released. */
                int slot = -1;
                for (int i = 0; i < s->nkeys; i++) {
                    if (s->keys[i].id == c->b) {
                        slot = i;
                        break;
                    }
                }
                if (slot < 0 && s->nkeys < MAX_HELD_KEYS) {
                    slot = s->nkeys++;
                }
                if (slot >= 0) {
                    s->keys[slot].id = c->b;
                    s->keys[slot].sym = (uint32_t) c->a;
                }
                SendKeyEvent(cl, (uint32_t) c->a, TRUE);
            }
            break;
        case CMD_KEY_UP:
            /* The keysym recorded at press time, not one worked out again from
               the key: the seam's whole point is that a release names the key
               its press named, and libvncclient's SendKeyEvent has only the
               keysym to say it with. */
            for (int i = 0; i < s->nkeys; i++) {
                if (s->keys[i].id == c->a) {
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
        case CMD_ENCODINGS:
            applyEncodings(s);
            break;
        case CMD_RESIZE:
            /* SendExtDesktopSize stops asking for updates until the server's
               answer arrives, which is what keeps a screenful of the old size
               from being decoded into a framebuffer that is about to be
               reallocated. It sends nothing at all if the size already matches,
               and the answer to a refusal is an unchanged size — so a refusal
               is silence here, and the panel goes on reporting the size that
               is. */
            if (cl && !s->viewOnly && s->canResize) {
                SendExtDesktopSize(cl, (uint16_t) c->a, (uint16_t) c->b);
            }
            break;
        case CMD_DISCONNECT:
            s->quit = 1;
            break;
        default:
            break;
    }
    free(c->text);
    c->text = NULL;
}

static int drain(Session *s) {
    for (;;) {
        Cmd c;
        pthread_mutex_lock(&s->lock);
        if (s->tail == s->head) {
            pthread_mutex_unlock(&s->lock);
            return s->quit;
        }
        c = s->ring[s->tail];
        s->ring[s->tail].text = NULL;
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

static void *protocolThread(void *arg) {
    Session *s = (Session *) arg;

    /* Attached for the thread's whole life rather than per callback, which is
       what the sibling shim does: registering a thread is not free and
       onFinishedUpdate is once a frame. `attach` then always takes its GetEnv
       path and `detach` is a no-op, so the callbacks are unchanged. */
    JNIEnv *unused = NULL;
    const int ours = (*s->vm)->AttachCurrentThread(s->vm, &unused, NULL) == JNI_OK;

    if (s->addressError[0]) {
        fireClosed(s, s->addressError);
        goto done;
    }

    rfbClient *cl = rfbGetClient(8, 3, 4);
    if (!cl) {
        fireClosed(s, "Out of memory");
        goto done;
    }
    rfbClientSetClientData(cl, &sessionTag, s);

    setColorFormat(s, cl);

    /* Their name for the opposite of what it says: it asks for the cursor
       pseudo-encodings, which is how the shape arrives to be drawn here. It
       also drags PointerPos into the list, and the LED state goes out
       unconditionally beside it — neither separable without patching their
       code, and both ignored here, but the two RFB backends' encoding lists
       differ by exactly that, which matters when comparing them. */
    cl->appData.useRemoteCursor = TRUE;
    cl->appData.shareDesktop = s->shared;
    /* Both halves of "the desktop may change size": without this one the
       original DesktopSize pseudo-encoding is never asked for and only a
       server that speaks the extended one can say it has resized. */
    cl->canHandleNewFBSize = TRUE;
    cl->appData.encodingsString = s->encodings;
    cl->appData.compressLevel = s->compressLevel;
    cl->appData.qualityLevel = s->qualityLevel < 0 ? 9 : s->qualityLevel;
    cl->appData.enableJPEG = (s->qualityLevel >= 0);
    cl->connectTimeout = (unsigned) (s->connectTimeoutMs > 0 ? (s->connectTimeoutMs + 999) / 1000 : 20);

    cl->MallocFrameBuffer = onMallocFrameBuffer;
    cl->GotFrameBufferUpdate = onGotUpdate;
    cl->FinishedFrameBufferUpdate = onFinishedUpdate;
    cl->GotCursorShape = onGotCursor;
    cl->GotXCutText = onCutText;
    /* And deliberately *not* GotXCutTextUTF8, which is what would ask for the
       extended clipboard — because asking for it here breaks the direction it
       was supposed to improve. libvncclient's implementation of it acts on an
       unsolicited `provide` and on nothing else: it ignores the `notify` a
       server sends when its clipboard changes, and there is no call to answer
       one with a `request`. Against a server that announces rather than pushes,
       which TigerVNC's does, offering the encoding replaces a working Latin-1
       path with silence. Measured, not assumed: with it on, a clipboard set on
       the far end reaches this client as "not provide type. ignore" and stops
       there. Teaching it to answer a notify is a patch to their code rather
       than a flag, and it is on the deferred list. */
    cl->Bell = onBell;
    cl->GetPassword = onGetPassword;
    cl->GetCredential = onGetCredential;
    cl->GetX509CertFingerprintMismatchDecision = onCertMismatch;
    cl->ConfirmAnonymousTLS = onConfirmAnonymousTLS;

    free(cl->serverHost);
    cl->serverHost = strdup(s->host ? s->host : "");
    cl->serverPort = s->port;

    publishClient(s, cl);

    /* rfbInitClient frees the client itself on failure, so the handle has to go
       before anything else can touch it — and in the same breath as the flag
       the teardown calls test, or they read a freed client in between. */
    forgetLastError();
    pthread_mutex_lock(&s->lock);
    s->handshaking = 1;
    pthread_mutex_unlock(&s->lock);
    const int connected = rfbInitClient(cl, NULL, NULL);
    if (!connected) {
        publishClient(s, NULL);
    }
    pthread_mutex_lock(&s->lock);
    s->handshaking = 0;
    pthread_mutex_unlock(&s->lock);
    if (!connected) {
        pthread_mutex_lock(&s->lock);
        const int cancelled = s->quit;
        pthread_mutex_unlock(&s->lock);
        char why[256];
        takeLastError(why, sizeof why);
        fireClosed(s, cancelled ? "" : (why[0] ? why : "Could not connect"));
        goto done;
    }

    /* Bracketed the way it would have to be typed, since a literal with a port
       after it is otherwise two colons in a row nobody can read. */
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

        if (!paused && cl->buffered > 0) {
            if (!HandleRFBServerMessage(cl)) {
                break;
            }
            continue;
        }

        /* The socket is watched only while the session is in front. An RFB
           server sends nothing that was not asked for and libvncclient asks
           for the next update at the end of each one it handles, so leaving
           the last one unread is the whole of the pause: at most one update
           sits in the socket and the server then waits. */
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

done:
    if (ours) {
        (*s->vm)->DetachCurrentThread(s->vm);
    }
    return NULL;
}

/* ---- the JNI surface ---------------------------------------------------- */

static Session *handleOf(jlong h) {
    return (Session *) (intptr_t) h;
}

static char *dup_jstring(JNIEnv *env, jstring js) {
    if (!js) {
        return NULL;
    }
    const char *c = (*env)->GetStringUTFChars(env, js, NULL);
    char *out = c ? strdup(c) : NULL;
    if (c) {
        (*env)->ReleaseStringUTFChars(env, js, c);
    }
    return out;
}

/* `host::port` is the one unbracketed address with two colons in it that is not
   an IPv6 literal somebody left the brackets off: if the last colon is the one
   after the first, there is no third. */
static int bareLiteral(const char *address) {
    const char *first = strchr(address, ':');
    return first && strrchr(address, ':') > first + 1;
}

/* An address is `host`, `host:port`, `[literal]` or `[literal]:port`, and a bare
   IPv6 literal is refused in words rather than guessed at: a number under 100 is
   a display here, so `::1:1` cannot be an address and a display at once, and
   what the guess this replaced produced was `::` — the wildcard address, which
   connects to something. The host is left at the front of `address`, which stays
   the caller's buffer to free; `why` is empty unless there is nothing to dial. */
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
            /* `host::port` is how somebody says they meant the port. */
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

JNIEXPORT jstring JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeVersion(JNIEnv *env, jclass cls) {
    (void) cls;
    char buf[64];
    snprintf(buf, sizeof(buf), "LibVNCServer %s", LIBVNCSERVER_VERSION);
    return (*env)->NewStringUTF(env, buf);
}

JNIEXPORT jlong JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeCreate(
        JNIEnv *env, jclass cls, jobject listener, jstring address, jstring userName,
        jstring password, jboolean shared, jboolean anonymousTls, jstring encoding,
        jint compressLevel, jint qualityLevel, jint colorLevel, jint connectTimeoutMs) {
    (void) cls;
    if (!gCallbacksClass) {
        jclass k = (*env)->GetObjectClass(env, listener);
        mConnected = (*env)->GetMethodID(env, k, "onConnected", "(II)V");
        mDesktopSize = (*env)->GetMethodID(env, k, "onDesktopSize", "(II)V");
        mDamage = (*env)->GetMethodID(env, k, "onDamage", "(IIII)V");
        mFrameEnd = (*env)->GetMethodID(env, k, "onFrameEnd", "()V");
        mCursor = (*env)->GetMethodID(env, k, "onCursor", "([IIIIIJ)V");
        mBell = (*env)->GetMethodID(env, k, "onBell", "()V");
        mClipboard = (*env)->GetMethodID(env, k, "onClipboard", "(Ljava/lang/String;)V");
        mCredentialsNeeded = (*env)->GetMethodID(env, k, "onCredentialsNeeded", "(Z)V");
        mTrustNeeded = (*env)->GetMethodID(env, k, "onTrustNeeded", "(Ljava/lang/String;)V");
        mUnverified = (*env)->GetMethodID(env, k, "onUnverified", "(Ljava/lang/String;)V");
        mClosed = (*env)->GetMethodID(env, k, "onClosed", "(Ljava/lang/String;)V");
        /* Last, and that is the point: it is what every other caller tests, so
           publishing it first would let a second one past while the method ids
           above were still zero. */
        gCallbacksClass = (*env)->NewGlobalRef(env, k);
    }

    Session *s = calloc(1, sizeof(Session));
    if (!s) {
        return 0;
    }
    s->wake[0] = s->wake[1] = -1;
    (*env)->GetJavaVM(env, &s->vm);
    s->callbacks = (*env)->NewGlobalRef(env, listener);
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    pthread_rwlock_init(&s->fbLock, NULL);
    if (pipe(s->wake) != 0) {
        goto fail;
    }
    for (int i = 0; i < 2; i++) {
        const int fl = fcntl(s->wake[i], F_GETFL, 0);
        fcntl(s->wake[i], F_SETFL, fl | O_NONBLOCK);
    }

    char *addr = dup_jstring(env, address);
    s->port = 5900;
    if (addr) {
        s->host = addr;
        splitAddress(addr, &s->port, s->addressError, sizeof s->addressError);
    }
    s->storedUser = dup_jstring(env, userName);
    s->storedPass = dup_jstring(env, password);
    s->encodings = dup_jstring(env, encoding);
    s->compressLevel = compressLevel;
    s->qualityLevel = qualityLevel;
    s->colorLevel = colorLevel;
    s->shared = shared != JNI_FALSE;
    s->anonymousTls = anonymousTls != JNI_FALSE;
    s->connectTimeoutMs = connectTimeoutMs;

    rfbClientLog = logInfo;
    rfbClientErr = logErr;

    if (pthread_create(&s->thread, NULL, protocolThread, s) != 0) {
        goto fail;
    }
    return (jlong) (intptr_t) s;

fail:
    (*env)->DeleteGlobalRef(env, s->callbacks);
    for (int i = 0; i < 2; i++) {
        if (s->wake[i] >= 0) {
            close(s->wake[i]);
        }
    }
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
    free(s->host);
    free(s->storedUser);
    free(s->storedPass);
    free(s->encodings);
    free(s);
    return 0;
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeAnswerCredentials(
        JNIEnv *env, jclass cls, jlong handle, jstring userName, jstring password) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    char *u = dup_jstring(env, userName);
    char *p = dup_jstring(env, password);
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
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeAnswerTrust(
        JNIEnv *env, jclass cls, jlong handle, jboolean accept) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    pthread_mutex_lock(&s->lock);
    s->trustAccepted = accept != JNI_FALSE;
    s->trustAnswered = 1;
    if (!s->trustAccepted) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeDisconnect(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    pthread_mutex_lock(&s->lock);
    s->quit = 1;
    s->paused = 0;
    /* Nothing in the handshake watches the wake pipe: the protocol thread is
       inside somebody else's read, and the TLS one has no deadline of its own
       on purpose. Breaking the socket is what a flag cannot do. */
    if (s->handshaking && s->cl && s->cl->sock >= 0) {
        shutdown(s->cl->sock, SHUT_RDWR);
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeDestroy(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    pthread_mutex_lock(&s->lock);
    s->quit = 1;
    s->paused = 0;
    /* The same socket break `nativeDisconnect` does, and for a sharper reason:
       this one *joins*. Nothing in the handshake watches the wake pipe, so a
       session destroyed while it is still connecting — the panel's Reconnect,
       a second session started over the first — would hold the caller, which
       is the main thread, until the connect timeout ran out. */
    if (s->handshaking && s->cl && s->cl->sock >= 0) {
        shutdown(s->cl->sock, SHUT_RDWR);
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);

    pthread_join(s->thread, NULL);

    (*env)->DeleteGlobalRef(env, s->callbacks);
    close(s->wake[0]);
    close(s->wake[1]);
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        free(s->ring[i].text);
    }
    free(s->dirty);
    free(s->fb);
    free(s->screens);
    free(s->host);
    free(s->storedUser);
    free(s->storedPass);
    free(s->encodings);
    free(s->credUser);
    free(s->credPass);
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
    free(s);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativePointer(
        JNIEnv *env, jclass cls, jlong handle, jint x, jint y, jint buttonMask) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_POINTER, x, y, buttonMask, NULL};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeKeyDown(
        JNIEnv *env, jclass cls, jlong handle, jint keysym, jint keyId) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_KEY_DOWN, keysym, keyId, 0, NULL};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeKeyUp(
        JNIEnv *env, jclass cls, jlong handle, jint keyId) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_KEY_UP, keyId, 0, 0, NULL};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeReleaseAllKeys(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeFocus(
        JNIEnv *env, jclass cls, jlong handle, jboolean focused) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    pthread_mutex_lock(&s->lock);
    s->paused = !focused;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    const char one = 1;
    (void) !write(s->wake[1], &one, 1);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeViewOnly(
        JNIEnv *env, jclass cls, jlong handle, jboolean viewOnly) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        s->viewOnly = viewOnly ? 1 : 0;
    }
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeCanResize(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || !s->canResize || s->viewOnly) {
        return JNI_FALSE;
    }
    /* More than one screen says no, and that is the shim's limitation rather
       than the library's: SendExtDesktopSize sends a single screen covering
       the desktop, so granting one would merge somebody's monitors into one. */
    pthread_mutex_lock(&s->lock);
    const int screens = s->nscreens;
    pthread_mutex_unlock(&s->lock);
    return screens <= 1 ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jintArray JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeMonitors(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    jint *flat = NULL;
    int n = 0;
    if (s) {
        pthread_mutex_lock(&s->lock);
        n = s->nscreens;
        if (n > 0) {
            flat = malloc((size_t) n * 4 * sizeof(jint));
            if (flat) {
                memcpy(flat, s->screens, (size_t) n * 4 * sizeof(jint));
            } else {
                n = 0;
            }
        }
        pthread_mutex_unlock(&s->lock);
    }
    jintArray out = (*env)->NewIntArray(env, n * 4);
    if (out && flat) {
        (*env)->SetIntArrayRegion(env, out, 0, n * 4, flat);
    }
    free(flat);
    return out;
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeRequestDesktopSize(
        JNIEnv *env, jclass cls, jlong handle, jint width, jint height) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s && width > 0 && height > 0 && width <= 65535 && height <= 65535) {
        Cmd c = {CMD_RESIZE, width, height, 0, NULL};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeClipboard(
        JNIEnv *env, jclass cls, jlong handle, jstring text) {
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_CLIPBOARD, 0, 0, 0, dup_jstring(env, text)};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeSetEncodings(
        JNIEnv *env, jclass cls, jlong handle, jstring encoding, jint compressLevel,
        jint qualityLevel) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    char *e = dup_jstring(env, encoding);
    pthread_mutex_lock(&s->lock);
    char *old = s->encodings;
    s->encodings = e;
    s->compressLevel = compressLevel;
    s->qualityLevel = qualityLevel;
    pthread_mutex_unlock(&s->lock);
    /* The old string may still be the one libvncclient is holding in appData,
       so it is freed only after the command that replaces it has run — which
       is what the queue's ordering gives for nothing. */
    Cmd c = {CMD_ENCODINGS, 0, 0, 0, old};
    post(s, &c);
}

/* The wording TigerVNC's PixelFormat::print uses, because the panel shows one
   of these rows per backend and three of them are that library's or a
   deliberate copy of it. */
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

/* A VeNCrypt sub-type as the panel's Security row: which authentication ran,
   and, for the three with no certificate under them, that the encryption
   proved nothing about the server. A session that said only "TLS" would be
   claiming the identity check this family does not have. */
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

JNIEXPORT jobjectArray JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeInfo(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return NULL;
    }
    char desktop[256] = "", protocol[64] = "", security[96] = "", encoding[64] = "";
    char serverPixels[64] = "", viewerPixels[64] = "";

    pthread_rwlock_rdlock(&s->fbLock);
    rfbClient *cl = s->cl;
    if (cl) {
        snprintf(desktop, sizeof(desktop), "%s", cl->desktopName ? cl->desktopName : "");
        snprintf(protocol, sizeof(protocol), "RFB %d.%d", cl->major, cl->minor);
        /* VeNCrypt leaves authScheme at 19 and puts the sub-type beside it, and
           the sub-type is the whole of what the session is: which
           authentication ran, and whether the tunnel it ran inside proved
           anything about the server. */
        switch (cl->authScheme) {
            case rfbNoAuth: snprintf(security, sizeof(security), "None"); break;
            case rfbVncAuth: snprintf(security, sizeof(security), "VNC password"); break;
            case rfbVeNCrypt: describeVeNCrypt(security, sizeof(security), cl->subAuthScheme); break;
            default: snprintf(security, sizeof(security), "Type %u", cl->authScheme); break;
        }
        /* What was asked for, not what arrived: libvncclient tells nobody which
           encoding decoded a rectangle, so this row is the offer and its label
           says so. Every other backend here reports the encoding in use. */
        snprintf(encoding, sizeof(encoding), "%s",
                 cl->appData.encodingsString ? cl->appData.encodingsString : "");
        describeFormat(serverPixels, sizeof(serverPixels), &cl->si.format);
        describeFormat(viewerPixels, sizeof(viewerPixels), &cl->format);
    }
    pthread_rwlock_unlock(&s->fbLock);

    const char *values[] = {desktop, protocol, "TCP", security, encoding,
                            serverPixels, viewerPixels};
    const int n = (int) (sizeof(values) / sizeof(values[0]));
    jobjectArray out = (*env)->NewObjectArray(env, n,
            (*env)->FindClass(env, "java/lang/String"), NULL);
    if (!out) {
        return NULL;
    }
    for (int i = 0; i < n; i++) {
        /* The desktop name is the server's own bytes; the rest of these this
           file wrote. */
        jstring js = i == 0 ? toJava(env, values[i])
                            : (*env)->NewStringUTF(env, values[i]);
        (*env)->SetObjectArrayElement(env, out, i, js);
        (*env)->DeleteLocalRef(env, js);
    }
    return out;
}

/* The counters the patched sockets.c keeps, read under the same lock as
   `nativeInfo`: the client may be freed by a failed `rfbInitClient` under us. */
JNIEXPORT jlongArray JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeTraffic(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return NULL;
    }
    jlong both[2] = {-1, -1};
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->cl) {
        both[0] = (jlong) s->cl->bytesRcvd;
        both[1] = (jlong) s->cl->bytesSent;
    }
    pthread_rwlock_unlock(&s->fbLock);

    jlongArray out = (*env)->NewLongArray(env, 2);
    if (out) {
        (*env)->SetLongArrayRegion(env, out, 0, 2, both);
    }
    return out;
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeReadRegion(
        JNIEnv *env, jclass cls, jlong handle, jint x, jint y, jint w, jint h,
        jobject bitmap, jint dstX, jint dstY) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || w <= 0 || h <= 0) {
        return JNI_FALSE;
    }
    AndroidBitmapInfo info;
    if (AndroidBitmap_getInfo(env, bitmap, &info) != ANDROID_BITMAP_RESULT_SUCCESS
            || info.format != ANDROID_BITMAP_FORMAT_RGBA_8888) {
        return JNI_FALSE;
    }
    if (dstX < 0 || dstY < 0
            || (uint32_t) (dstX + w) > info.width || (uint32_t) (dstY + h) > info.height) {
        return JNI_FALSE;
    }

    jboolean ok = JNI_FALSE;
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->fb && x >= 0 && y >= 0 && x + w <= s->fbW && y + h <= s->fbH) {
        void *dst = NULL;
        if (AndroidBitmap_lockPixels(env, bitmap, &dst) == ANDROID_BITMAP_RESULT_SUCCESS) {
            for (int row = 0; row < h; row++) {
                const uint8_t *src = s->fb + ((size_t) (y + row) * (size_t) s->fbW + x) * 4u;
                uint8_t *out = (uint8_t *) dst + (size_t) (dstY + row) * info.stride
                        + (size_t) dstX * 4u;
                memcpy(out, src, (size_t) w * 4u);
            }
            AndroidBitmap_unlockPixels(env, bitmap);
            ok = JNI_TRUE;
        }
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeReadThumbnail(
        JNIEnv *env, jclass cls, jlong handle, jint step, jobject bitmap) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || step < 1) {
        return JNI_FALSE;
    }
    AndroidBitmapInfo info;
    if (AndroidBitmap_getInfo(env, bitmap, &info) != ANDROID_BITMAP_RESULT_SUCCESS
            || info.format != ANDROID_BITMAP_FORMAT_RGBA_8888) {
        return JNI_FALSE;
    }

    jboolean ok = JNI_FALSE;
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->fb && s->fbW > 0 && s->fbH > 0) {
        const int tw = (s->fbW + step - 1) / step;
        const int th = (s->fbH + step - 1) / step;
        if ((uint32_t) tw <= info.width && (uint32_t) th <= info.height) {
            void *dst = NULL;
            if (AndroidBitmap_lockPixels(env, bitmap, &dst) == ANDROID_BITMAP_RESULT_SUCCESS) {
                for (int row = 0; row < th; row++) {
                    const uint32_t *src = (const uint32_t *) (s->fb
                            + (size_t) (row * step) * (size_t) s->fbW * 4u);
                    uint32_t *out = (uint32_t *) ((uint8_t *) dst + (size_t) row * info.stride);
                    for (int col = 0; col < tw; col++) {
                        out[col] = src[col * step];
                    }
                }
                AndroidBitmap_unlockPixels(env, bitmap);
                ok = JNI_TRUE;
            }
        }
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}
