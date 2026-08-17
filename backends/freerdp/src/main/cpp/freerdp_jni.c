// FreeRDP behind the same JNI surface the IronRDP backend has, so the two are
// interchangeable above this file and a disagreement between them is a
// disagreement between the clients rather than between two ways of binding one.
// That is the whole point of the second RDP client: every RFB question here has
// been answerable by asking the other two clients, and every RDP one has had a
// single answer.
//
// Four things are the library's decisions rather than ours, and each cost a
// session to find out (they were found by driving the library on the phone with
// no backend around it, before this file existed):
//
//   1. **This file must not define `JNI_OnLoad`.** WinPR exports one of its own
//      and takes its `JavaVM` from it, because it reads the timezone through
//      Java; without it, the first `freerdp_settings_new` asserts. A shim that
//      defines one silently replaces WinPR's and the library stops working in a
//      way that looks nothing like the cause.
//   2. **`freerdp_client_start` replaces the callbacks you set.** With
//      `UseCommonStdioCallbacks` on, which is the default, it installs the
//      command-line prompts over the certificate and credential ones — and on a
//      phone there is no terminal, so they refuse, and the refusal is reported
//      as a TLS failure over a handshake that in fact succeeded.
//   3. **`update->DesktopResize` is not optional.** `gdi_ResetGraphics` asserts
//      on it, so no callback is `abort()` rather than a missing feature.
//   4. **The graphics pipeline paints nothing unless it is joined to the GDI**
//      when its channel connects. It negotiates perfectly either way, which is
//      a black desktop with a healthy-looking connection behind it.
//
// And two are ours, both forced by the seam rather than chosen:
//
//   * One thread owns the connection and every call from Java is a queued
//     command it drains, woken by an event of ours in the library's own handle
//     set — because setting a flag does not wake a thread inside
//     `WaitForMultipleObjects`.
//   * There are two framebuffers. The GDI decodes into its own between
//     `BeginPaint` and `EndPaint`, and the seam promises that pixels may be read
//     from the drawing thread at any moment; so the damage is copied under a
//     writer lock when the batch ends, and reads are served from the copy. The
//     alternative is holding that lock across a decode, which puts the drawing
//     thread behind an H.264 frame.

#include <android/bitmap.h>
#include <android/log.h>
#include <jni.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <freerdp/freerdp.h>
#include <freerdp/client.h>
#include <freerdp/client/channels.h>
#include <freerdp/client/cliprdr.h>
#include <freerdp/client/cmdline.h>
#include <freerdp/client/disp.h>
#include <freerdp/channels/rdpgfx.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/gdi/gfx.h>
#include <freerdp/graphics.h>
#include <freerdp/autodetect.h>
#include <freerdp/settings.h>
#include <winpr/synch.h>

#define TAG "FreeRdp"
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
    CMD_FOCUS,
    CMD_RESIZE,
    CMD_DISCONNECT,
};

typedef struct {
    int type;
    int a, b;
    INT64 c;
    WCHAR *text; /* owned by the queue */
    size_t textLen;
} Cmd;

typedef struct Session Session;

/* The context FreeRDP allocates for us: its own, and a pointer back to ours.
   `rdpClientContext` is what the client layer writes into the allocation, so
   `ContextSize` has to name a struct that starts with one — leave it zero and
   everything above `rdpContext` lands off the end of the allocation, which
   nothing asserts on. */
typedef struct {
    rdpClientContext client;
    Session *session;
} ShimContext;

struct Session {
    JavaVM *vm;
    jobject callbacks; /* global ref */

    ShimContext *ctx; /* protocol thread; published under both locks */
    freerdp *instance;

    pthread_t thread;
    HANDLE wake; /* the doorbell in the library's handle set */

    pthread_mutex_t lock;
    pthread_cond_t cond;

    Cmd ring[CMD_RING];
    int head, tail;

    int quit;
    int started;
    int connected;
    int closedReported;
    int sawPointerPosition; /* protocol thread only: log the first, not the stream */
    _Atomic int viewOnly;
    _Atomic int canResize;

    /* the credential prompt, answered from Java */
    int credAnswered;
    char *credUser, *credDomain, *credPass;

    /* and the certificate one, which is the same shape */
    int trustAnswered, trustAccepted;

    /* the copy readRegion is served from */
    pthread_rwlock_t fbLock;
    uint8_t *fb;
    int fbW, fbH;

    /* The channels, kept because they arrive as an event rather than from a
       call: a channel's interface is handed over once, when it connects. */
    CliprdrClientContext *cliprdr;
    DispClientContext *disp;
    _Atomic int gfxOpen; /* the graphics pipeline's channel is up */

    /* The codec of the last surface the server sent, and the library's own
       handler it is read on the way to. Asking the settings instead answers
       what was *requested*: nothing on a client clears RemoteFxCodec when the
       server declines it, and a server may decline for reasons two rows away —
       xrdp offers the codec only to a client that calls its link a LAN. */
    _Atomic UINT32 lastCodec;
    pSurfaceBits gdiSurfaceBits;

    /* What this phone has copied, as the wire wants it — UTF-16 with its
       terminator — held until something over there pastes. RDP's clipboard is a
       delayed rendering: the format list says there is text and the bytes go
       only when they are asked for. */
    WCHAR *clipOut;
    size_t clipOutLen;

    struct {
        INT64 id;
        UINT16 code;  /* RDP scancode, with 0xE000 for an extended one */
        int shift;    /* the layout needed a Shift for it, and we pressed one */
        WCHAR unicode; /* or it had no position at all */
    } keys[MAX_HELD_KEYS];
    int nkeys;
    int shiftHeld; /* by the person, not by us */

    UINT16 buttons; /* the RFB mask as last seen, because RDP wants the edges */

    char *host;
    int port;
    /* Why the address was not one, empty if it was: a session is created and
       started in one call, so the protocol thread is the first place there is
       to say it. */
    char addressError[160];
    char *user, *domain, *password;
    char *nla;
    char *experience;
    char *graphics;
    char *sound;
    char *clientName;
    char *configPath;
    int width, height, monitors, keyboardLayout;
    int scale; /* percent; what the far end is asked to draw its own interface at */
    int compression;
    int connectTimeoutMs;
};

static jclass gCallbacksClass;
static jmethodID mConnected, mDesktopSize, mDamage, mFrameEnd, mCursor, mBell,
        mClipboard, mCredentialsNeeded, mTrustNeeded, mClosed;

/* ---- JNI plumbing ------------------------------------------------------- */

/* Anything a callback throws is pending on this thread when it returns, and
   every JNI call made while an exception is pending is undefined — including
   the detach below, which runs the thread's uncaught-exception handler and
   takes the process with it. */
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

/* Every string the *server* chose crosses here. `NewStringUTF` takes modified
   UTF-8, so a four-byte sequence is not a string it can be given at all, and a
   build with CheckJNI on aborts rather than producing mojibake — which is the
   bug both other C shims here had. Only a string this file wrote itself may go
   the short way. */
static jstring toJava(JNIEnv *env, const char *bytes) {
    if (!bytes) {
        return NULL;
    }
    const size_t n = strlen(bytes);
    jchar *out = malloc((n + 1) * sizeof(jchar));
    if (!out) {
        return NULL;
    }
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
        /* Latin-1, which is one code point a byte. A server's own bytes are
           whatever it decided, and a name that is not UTF-8 is still a name. */
        for (w = 0; w < n; w++) {
            out[w] = (jchar) (uint8_t) bytes[w];
        }
    }
    jstring js = (*env)->NewString(env, out, (jsize) w);
    free(out);
    return js;
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

/* An address is `host`, `host:port`, `[literal]` or `[literal]:port`, and a bare
   IPv6 literal is refused in words rather than guessed at: unbracketed there is
   no telling it from a host with a port after it, and what the guess this
   replaced produced was the address truncated at its last colon. No display
   number here, which is a VNC habit RDP has never had, so a second colon has
   nothing to mean. The host is left at the front of `address`, which stays the
   caller's buffer to free; `why` is empty unless there is nothing to dial. */
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

static void post(Session *s, const Cmd *c) {
    pthread_mutex_lock(&s->lock);
    if (s->quit) {
        pthread_mutex_unlock(&s->lock);
        free(c->text);
        return;
    }
    /* Motion coalesces onto the tail: the protocol thread can be inside a
       decode, and by the time it drains, every position but the last is
       somewhere the pointer no longer is. A button change is a different event
       and is never merged away. */
    if (c->type == CMD_POINTER && s->head != s->tail) {
        const int last = (s->head + CMD_RING - 1) % CMD_RING;
        if (s->ring[last].type == CMD_POINTER && s->ring[last].c == c->c) {
            s->ring[last] = *c;
            pthread_mutex_unlock(&s->lock);
            SetEvent(s->wake);
            return;
        }
    }
    const int next = (s->head + 1) % CMD_RING;
    if (next == s->tail) {
        free(s->ring[s->tail].text);
        s->tail = (s->tail + 1) % CMD_RING;
    }
    s->ring[s->head] = *c;
    s->head = next;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

/* ---- callbacks into Java ------------------------------------------------ */

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
        jstring d = toJava(env, detail ? detail : "");
        (*env)->CallVoidMethod(env, s->callbacks, mClosed, d);
        cleared(env);
        (*env)->DeleteLocalRef(env, d);
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

/* ---- the framebuffer ---------------------------------------------------- */

static Session *sessionOf(rdpContext *context) {
    return ((ShimContext *) context)->session;
}

/* One allocation per desktop size, zeroed opaque so that a region read before
   anything has been decoded is black rather than transparent. */
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

/* The whole of the second framebuffer's cost, and the only place it is paid:
   once per damaged rectangle per paint, under the writer lock. The GDI's own
   buffer is BGRX and Android's ARGB_8888 is RGBX, so red and blue trade places
   here and the alpha the codecs leave undefined is filled in — one pass over
   the damage rather than the library's converting copy and then ours. */
static BOOL onEndPaint(rdpContext *context) {
    Session *s = sessionOf(context);
    rdpGdi *gdi = context->gdi;
    HGDI_WND hwnd = gdi->primary->hdc->hwnd;
    if (hwnd->invalid->null || hwnd->ninvalid <= 0) {
        return TRUE;
    }

    const int fw = s->fbW, fh = s->fbH;
    pthread_rwlock_wrlock(&s->fbLock);
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
                    + (size_t) (y + row) * (size_t) fw * 4u) + x;
            for (int col = 0; col < w; col++) {
                const uint32_t p = src[col];
                dst[col] = 0xFF000000u | ((p & 0x00FF0000u) >> 16)
                        | (p & 0x0000FF00u) | ((p & 0x000000FFu) << 16);
            }
        }
    }
    pthread_rwlock_unlock(&s->fbLock);

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        for (INT32 i = 0; i < hwnd->ninvalid; i++) {
            const HGDI_RGN r = &hwnd->cinvalid[i];
            if (r->w > 0) {
                (*env)->CallVoidMethod(env, s->callbacks, mDamage, r->x, r->y, r->w, r->h);
                cleared(env);
            }
        }
        (*env)->CallVoidMethod(env, s->callbacks, mFrameEnd);
        cleared(env);
        detach(s, attached);
    }

    hwnd->invalid->null = TRUE;
    hwnd->ninvalid = 0;
    return TRUE;
}

/* Only to see what the picture arrived as; the drawing is the library's. */
static BOOL onSurfaceBits(rdpContext *context, const SURFACE_BITS_COMMAND *cmd) {
    Session *s = sessionOf(context);
    if (cmd) {
        s->lastCodec = cmd->bmp.codecID;
    }
    return s->gdiSurfaceBits ? s->gdiSurfaceBits(context, cmd) : TRUE;
}

/* Not optional: gdi_ResetGraphics asserts on this callback, so a client without
   one does not fail to resize, it aborts on the channel thread. */
static BOOL onDesktopResize(rdpContext *context) {
    Session *s = sessionOf(context);
    const UINT32 w = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopHeight);
    if (!allocFramebuffer(s, (int) w, (int) h) || !gdi_resize(context->gdi, w, h)) {
        return FALSE;
    }
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, mDesktopSize, (jint) w, (jint) h);
        cleared(env);
        detach(s, attached);
    }
    return TRUE;
}

/* ---- the remote cursor -------------------------------------------------- */

/* FNV-1a over a cursor's pixels, which is its identity in `CursorCache` — so
   this, the other shims' copies of it and the Java one all have to agree. */
static jlong cursorHash(const jint *argb, int width, int height) {
    uint64_t h = 0xcbf29ce484222325ull;
    h = (h ^ (uint32_t) width) * 0x100000001b3ull;
    h = (h ^ (uint32_t) height) * 0x100000001b3ull;
    for (int i = 0; i < width * height; i++) {
        h = (h ^ (uint32_t) argb[i]) * 0x100000001b3ull;
    }
    return (jlong) h;
}

static void fireCursor(Session *s, jint *argb, int w, int h, int hotX, int hotY) {
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (!env) {
        return;
    }
    jintArray array = NULL;
    jlong hash = 0;
    if (argb && w > 0 && h > 0) {
        array = (*env)->NewIntArray(env, w * h);
        if (array) {
            (*env)->SetIntArrayRegion(env, array, 0, w * h, argb);
            hash = cursorHash(argb, w, h);
        }
    }
    (*env)->CallVoidMethod(env, s->callbacks, mCursor, array, w, h, hotX, hotY, hash);
    cleared(env);
    if (array) {
        (*env)->DeleteLocalRef(env, array);
    }
    detach(s, attached);
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

/* Converted when the pointer becomes the current one rather than when it
   arrives: a server sends shapes it then never uses, and an unchanged shape
   costs nothing on the other side anyway — `CursorCache` keys on the hash. */
static BOOL onPointerSet(rdpContext *context, rdpPointer *pointer) {
    Session *s = sessionOf(context);
    const UINT32 w = pointer->width, h = pointer->height;
    if (w == 0 || h == 0 || w > 384 || h > 384) {
        return TRUE;
    }
    jint *argb = calloc((size_t) w * h, sizeof(jint));
    if (!argb) {
        return TRUE;
    }
    /* BGRA32 rather than the framebuffer's own order, because what goes to Java
       is Color-packed — A,R,G,B from the top — which is BGRA in memory on a
       little-endian machine. */
    if (freerdp_image_copy_from_pointer_data((BYTE *) argb, PIXEL_FORMAT_BGRA32, 0, 0, 0, w, h,
                                             pointer->xorMaskData, pointer->lengthXorMask,
                                             pointer->andMaskData, pointer->lengthAndMask,
                                             pointer->xorBpp, &context->gdi->palette)) {
        fireCursor(s, argb, (int) w, (int) h, (int) pointer->xPos, (int) pointer->yPos);
    }
    free(argb);
    return TRUE;
}

/* An empty shape is the seam's "the server has hidden the pointer". The default
   arrow is the same answer: this end draws its own cursor, and a server's idea
   of an arrow is not more informative than ours. */
static BOOL onPointerSetNull(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

static BOOL onPointerSetDefault(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

/* The far end saying where it has put the cursor, which this app's cursor does
   not take instruction from: it is owned here and the viewport follows it.

   On `update->pointer` rather than on the graphics pointer below, which has a
   SetPosition of the same shape that the library never calls: the position
   update is dispatched in core/update.c and the graphics module only ever sees
   shapes. */
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

/* Text and nothing else, in both directions. CF_UNICODETEXT is UTF-16 with a
   terminator, which is what every Windows clipboard holds; the phone's is a
   String, and a file list or a bitmap has nowhere to go on this side. */
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
    /* The empty list matters as much as the full one: a channel that never
       sends a format list is one neither direction works on. */
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

/* The far end has something. Ask for it only if it is text — the request is
   what makes the bytes arrive, since RDP renders a clipboard on demand. */
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

/* Something over there is pasting what this phone copied. */
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

/* And the answer to our own request: the far end's text. */
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
        /* A WCHAR is UTF-16 on the wire and a jchar is UTF-16 in Java, so this
           is the one string that needs no conversion at all. */
        fireClipboard(s, (const jchar *) text, units);
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

/* A channel's interface arrives on an event rather than from a call, so the
   pointer has to be kept: this is the only place either of them is handed
   over. */
static void onChannelConnected(void *context, const ChannelConnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        /* Without this the pipeline negotiates, the channel opens and nothing
           is ever painted. */
        gdi_graphics_pipeline_init(ctx->gdi, (RdpgfxClientContext *) e->pInterface);
        s->gfxOpen = 1;
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        wireCliprdr(s, (CliprdrClientContext *) e->pInterface);
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = (DispClientContext *) e->pInterface;
        /* The one thing that says a desktop can be reshaped at all: the channel
           being open is the capability, and it is a fact about this session
           rather than about the backend. */
        s->canResize = 1;
    }
}

static void onChannelDisconnected(void *context, const ChannelDisconnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        gdi_graphics_pipeline_uninit(ctx->gdi, (RdpgfxClientContext *) e->pInterface);
        s->gfxOpen = 0;
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        s->cliprdr = NULL;
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = NULL;
        s->canResize = 0;
    }
}

/* ---- what the connection asks a person ---------------------------------- */

/* Blocks the protocol thread until Java answers, which is the design: on the
   other side of it is a dialog and a person. */
static BOOL onAuthenticate(freerdp *instance, char **username, char **password, char **domain,
                           rdp_auth_reason reason) {
    Session *s = sessionOf(instance->context);
    /* Asked *unconditionally* on the TLS and RDP paths — the library's own
       prompt there is "confirm what you are about to send", and only the NLA one
       is asked because something is missing. So a shim that forwards every ask
       to a person turns a saved password into a dialog on every connect, which
       is what this answers instead: what the record already holds is the
       answer, and the prompt is for when there is nothing to give. */
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

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, mCredentialsNeeded, JNI_TRUE);
        cleared(env);
        detach(s, attached);
    }

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

/* The same wait, for the other question a handshake asks. Nothing vouches for
   an RDP server's certificate — a Windows host signs its own — so the identity
   is the fingerprint, and the answer comes from the pin store or from a person.

   1 rather than 2 deliberately: 2 is "remember this", which would put the
   decision in a file of FreeRDP's beside the one this app already keeps. */
static DWORD askTrust(Session *s, const char *fingerprint) {
    pthread_mutex_lock(&s->lock);
    s->trustAnswered = 0;
    s->trustAccepted = 0;
    pthread_mutex_unlock(&s->lock);

    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        jstring js = (*env)->NewStringUTF(env, fingerprint ? fingerprint : "");
        (*env)->CallVoidMethod(env, s->callbacks, mTrustNeeded, js);
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
    return accepted ? 1 : 0;
}

static DWORD onVerifyCertificate(freerdp *instance, const char *host, UINT16 port,
                                 const char *common_name, const char *subject, const char *issuer,
                                 const char *fingerprint, DWORD flags) {
    (void) host; (void) port; (void) common_name; (void) subject; (void) issuer; (void) flags;
    return askTrust(sessionOf(instance->context), fingerprint);
}

/* Not the same question, and leaving it unset is a refusal: this is the one
   asked when the library's own store disagrees with what arrived. The store is
   not what decides here — the app's pin store is — so both ask the same thing. */
static DWORD onVerifyChangedCertificate(freerdp *instance, const char *host, UINT16 port,
                                        const char *common_name, const char *subject,
                                        const char *issuer, const char *fingerprint,
                                        const char *old_subject, const char *old_issuer,
                                        const char *old_fingerprint, DWORD flags) {
    (void) host; (void) port; (void) common_name; (void) subject; (void) issuer;
    (void) old_subject; (void) old_issuer; (void) old_fingerprint; (void) flags;
    return askTrust(sessionOf(instance->context), fingerprint);
}

/* ---- keysyms to scancodes ----------------------------------------------- */

/*
 * The one place the protocol does not match the stack. Everything above
 * `CursorController` speaks X11 keysyms, because that is RFB's vocabulary;
 * RDP's keyboard is scancodes — set 1, with an E0 prefix for the keys the
 * original PC keyboard did not have.
 *
 * Three things follow, each a decision rather than a detail:
 *
 *   1. A scancode is a position, not a character: 0x1E is "the key left of S",
 *      which is `a` on a US layout and `q` on a French one. So this table is a
 *      US layout, the connection asks for a US layout, and the server's own
 *      layout is what turns a position back into a character.
 *   2. What the layout cannot place goes as a Unicode keyboard event, which is
 *      how a phone's IME gets `é` and `→` across without a layout in between.
 *      It carries no modifier state, so it is the fallback and not the rule.
 *   3. Shift is part of the answer: `!` is not a key, it is `1` with Shift. The
 *      caller synthesises the modifier and has to know not to synthesise one
 *      that the person is already holding.
 */

#define KEY_EXT 0xE000u
#define KEY_NONE 0xFFFFu
/* Pause, which is not a position: the key never had a scancode of its own, and
   the library has a call of its own for the sequence mstsc sends instead. */
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

        /* The keypad. Each key appears twice, once under each Num Lock state,
           and both spellings are the same position — which is the whole of what
           RDP can say. Which of the two the far end produces is decided by the
           far end's own Num Lock and not by ours, so this is the one place
           where the phone owning the lock does not carry: with the lock off
           here and on there, KP_Home arrives as 7. */
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

        /* The unambiguous three of a Japanese board's conversion keys, which
           are the three the control layer sends. */
        case 0xff27: return 0x70;             /* Hiragana_Katakana */
        case 0xff23: return 0x79;             /* Henkan_Mode */
        case 0xff22: return 0x7b;             /* Muhenkan */
        default:
            /* F1–F10 are consecutive; the keypad took the codes after them. */
            if (keysym >= 0xffbe && keysym <= 0xffc7) {
                return (UINT16) (0x3b + (keysym - 0xffbe));
            }
            /* F13–F24, which no PC keyboard has and every layout still carries
               a position for. F13–F23 run from 0x64; F24 does not follow them,
               and a Windows far end resolves the position that would be F24 by
               counting to VK_OEM_PA3 — KBD4_T6F in winpr's own table, and
               KBD4_T76 is the one that is F24. */
            if (keysym >= 0xffca && keysym <= 0xffd4) {
                return (UINT16) (0x64 + (keysym - 0xffca));
            }
            if (keysym == 0xffd5) {
                return 0x76;   /* F24 */
            }
            return KEY_NONE;
    }
}

/* The printable half of a US layout, written as the rows of the keyboard
   rather than sorted, because that is what it is: a wrong entry is visible as
   a wrong neighbour. */
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

/* Fills in one of the three answers: a position, a position with Shift, or a
   character with no position at all. */
static void mapKeysym(UINT32 keysym, UINT16 *code, int *shift, WCHAR *unicode) {
    *code = KEY_NONE;
    *shift = 0;
    *unicode = 0;
    const UINT16 named = namedScancode(keysym);
    if (named != KEY_NONE) {
        *code = named;
        return;
    }
    /* The one keypad key a PC has no position for — FreeRDP's own xkb table has
       KP_Equal as unknown — and it names a character, so it goes as one. */
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
        /* The whole sequence goes on the press, so the release sends nothing.
           What the library puts on the wire is what mstsc does — a Ctrl marked
           E1 around a Num Lock — rather than the E1 1D 45 a keyboard sends. */
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

    /* A second press of a key already down is auto-repeat, which RDP carries as
       another down and nothing else — and re-recording it would fill the table
       and then silently drop every later key, which is the fault stage 23f
       found in two other backends. */
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
        /* Synthesised only when the person is not already holding one: a
           capital typed while Shift is down would otherwise be released by us
           on the way up, and the far end would see Shift let go mid-word. */
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
    if (!s->keys[index].unicode) {
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

/* An absolute position and an RFB button mask, which is what the whole control
   stack speaks. Three of those bits are buttons and four are wheel notches, and
   RDP has nothing pseudo about its wheel — so the buttons are held state and
   the wheel is an edge. */
static void doPointer(Session *s, int x, int y, UINT16 mask) {
    if (s->viewOnly || !s->connected) {
        return;
    }
    rdpInput *input = s->instance->context->input;
    const UINT16 previous = s->buttons;
    s->buttons = mask;

    /* The move goes first, so that a click lands where the finger is. */
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
    /* Buttons 8 and 9, which RDP calls X1 and X2 and Windows means browser back
       and forward by. */
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

/* The reverse of RFB's pause: an RDP server sends what it likes, so this is a
   Suppress Output PDU telling it not to, and the redraw on the way back is the
   server's own doing when output is allowed again. */
static void doFocus(Session *s, int focused) {
    if (!s->connected || !s->instance->context->gdi) {
        return;
    }
    gdi_send_suppress_output(s->instance->context->gdi, focused ? FALSE : TRUE);
}

static void doResize(Session *s, int width, int height) {
    if (!s->disp || !s->disp->SendMonitorLayout) {
        return;
    }
    DISPLAY_CONTROL_MONITOR_LAYOUT layout[16] = {0};
    const int count = s->monitors < 1 ? 1 : (s->monitors > 16 ? 16 : s->monitors);
    for (int i = 0; i < count; i++) {
        layout[i].Flags = i == 0 ? DISPLAY_CONTROL_MONITOR_PRIMARY : 0;
        layout[i].Left = i * width;
        layout[i].Top = 0;
        layout[i].Width = (UINT32) width;
        layout[i].Height = (UINT32) height;
        layout[i].PhysicalWidth = 0;
        layout[i].PhysicalHeight = 0;
        layout[i].Orientation = 0;
        /* The connection's, not 100: a resize re-states the whole layout, so
           leaving these at the default would undo the interface size the
           session was opened with. */
        layout[i].DesktopScaleFactor = (UINT32) s->scale;
        layout[i].DeviceScaleFactor = (UINT32) s->scale;
    }
    s->disp->SendMonitorLayout(s->disp, (UINT32) count, layout);
}

static void drainCommands(Session *s) {
    for (;;) {
        Cmd c;
        pthread_mutex_lock(&s->lock);
        if (s->head == s->tail) {
            pthread_mutex_unlock(&s->lock);
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
            case CMD_DISCONNECT:
                doReleaseAll(s);
                freerdp_abort_connect_context(s->instance->context);
                break;
            default: break;
        }
        free(c.text);
    }
}

/* ---- the connection ----------------------------------------------------- */

static BOOL onPreConnect(freerdp *instance) {
    rdpContext *ctx = instance->context;
    PubSub_SubscribeChannelConnected(ctx->pubSub, onChannelConnected);
    PubSub_SubscribeChannelDisconnected(ctx->pubSub, onChannelDisconnected);
    return TRUE;
}

/* Idempotent on purpose: upstream has two call sites for this callback — the
   activation sequence and the end of freerdp_connect — and which of them runs is
   the server's business rather than ours. A second one that got through would
   re-initialise the GDI under a framebuffer the first had already published. */
static BOOL onPostConnect(freerdp *instance) {
    Session *s = sessionOf(instance->context);
    if (s->connected) {
        return TRUE;
    }
    rdpSettings *settings = instance->context->settings;
    const UINT32 w = freerdp_settings_get_uint32(settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(settings, FreeRDP_DesktopHeight);

    /* BGRX because that is what every codec here decodes to, so the library's
       copy into this buffer is a memcpy per row. Android's ARGB_8888 wants RGBX
       and the difference is two bytes swapped — which onEndPaint does in a pass
       it makes anyway. Asking the GDI for RGBX instead moves that swap into
       FreeRDP's generic converter, which reads and writes a pixel at a time and
       cost a fifth of the decode thread when it was measured. */
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
    /* Chained rather than replaced: the GDI's own handler is what draws. */
    s->gdiSurfaceBits = update->SurfaceBits;
    update->SurfaceBits = onSurfaceBits;

    /* What the server will take from us. Both of these are the client's wish
       ANDed with the server's offer during capability exchange, so after
       PostConnect they are facts about this session rather than about us. The
       relative one is not acted on yet; whether a server offers it at all is
       the question a relative pointer over RDP turns on. */
    LOGI("server input: relative pointer %s, horizontal wheel %s, unicode %s",
         freerdp_settings_get_bool(settings, FreeRDP_HasRelativeMouseEvent) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_HasHorizontalWheel) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_UnicodeInput) ? "yes" : "no");

    s->connected = 1;
    int attached;
    JNIEnv *env = attach(s, &attached);
    if (env) {
        (*env)->CallVoidMethod(env, s->callbacks, mConnected, (jint) w, (jint) h);
        cleared(env);
        detach(s, attached);
    }
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
    freerdp_settings_set_uint32(settings, FreeRDP_KeyboardLayout, (UINT32) s->keyboardLayout);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth, (UINT32) s->width);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopHeight, (UINT32) s->height);
    freerdp_settings_set_uint32(settings, FreeRDP_ColorDepth, 32);
    freerdp_settings_set_bool(settings, FreeRDP_SoftwareGdi, TRUE);
    freerdp_settings_set_uint32(settings, FreeRDP_TcpConnectTimeout,
                                (UINT32) (s->connectTimeoutMs > 0 ? s->connectTimeoutMs : 20000));

    /* Where the library keeps its own certificate store. It is not what decides
       trust here — the app's pin store is — but a directory it cannot write is
       reported as a TLS failure rather than as itself. */
    if (s->configPath) {
        freerdp_settings_set_string(settings, FreeRDP_ConfigPath, s->configPath);
    }
    /* Or freerdp_client_start replaces every callback with a command-line one,
       and there is no terminal on a phone for them to prompt at. */
    freerdp_settings_set_bool(settings, FreeRDP_UseCommonStdioCallbacks, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_IgnoreCertificate, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_AutoAcceptCertificate, FALSE);

    /* Network Level Authentication: prefer it, insist on it, or refuse it.
       Nothing here turns off TLS — a server that offers only the RDP security
       layer is one whose password crosses in something a 1990s cipher calls
       encryption, and the connection is worth failing. */
    const int nlaOff = s->nla && strcmp(s->nla, "off") == 0;
    const int nlaRequired = s->nla && strcmp(s->nla, "require") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_NlaSecurity, !nlaOff);
    freerdp_settings_set_bool(settings, FreeRDP_TlsSecurity, !nlaRequired);
    freerdp_settings_set_bool(settings, FreeRDP_RdpSecurity, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_UseRdpSecurityLayer, FALSE);

    freerdp_settings_set_bool(settings, FreeRDP_CompressionEnabled, s->compression != 0);
    freerdp_settings_set_bool(settings, FreeRDP_RedirectClipboard, TRUE);
    freerdp_settings_set_bool(settings, FreeRDP_SupportDisplayControl, TRUE);

    /* The picture. The graphics pipeline is what a machine since Windows 8
       would rather send a desktop over, and the only path that carries H.264 —
       which this build decodes on the phone's own hardware. RemoteFX is the
       older codec, and bitmaps are what is left when a server has neither. */
    const char *g = s->graphics ? s->graphics : "gfx";
    const int gfx = strcmp(g, "gfx") == 0 || strcmp(g, "gfx-novideo") == 0;
    const int h264 = strcmp(g, "gfx") == 0;
    const int rfx = gfx || strcmp(g, "rfx") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteFxCodec, rfx);
    /* One switch and not two: with H264 on and AVC444 off, the client advertises
       capability version 8.1 and stops — every version from 10 up is inside a
       branch these two select together — so half of "H.264 but only AVC420" is
       also "no progressive v2, no scaling, no error-reporting caps". The choice
       worth offering is video or no video, and it is this pair. */
    freerdp_settings_set_bool(settings, FreeRDP_GfxH264, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxAVC444, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxProgressive, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxPlanar, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxSmallCache, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_GfxThinClient, FALSE);
    /* Off, which is the library's own default and is stated here because the
       obvious change is to turn it on: the legacy path is the only one that
       uses cache orders, and on the damage workload the cache misses every
       time and costs 0.97 MiB against 0.77. */
    freerdp_settings_set_bool(settings, FreeRDP_BitmapCacheEnabled, FALSE);

    /* Sound, which is a channel rather than a flag: the two settings decide
       what the client info tells the server to do — play it here, play it over
       there, or play nothing — and the channel that carries it has to be asked
       for separately.

       Asked for here rather than left to the library, because the library's own
       "load the channels the settings ask for" is guarded on
       CHANNEL_RPDSND_CLIENT, which is a misspelling of a macro nothing else
       uses: rdpsnd is never loaded for AudioPlayback in 3.19.1. What loads
       instead is the fallback further down that function, which adds rdpsnd
       against a device that accepts every format and discards every sample —
       so an app that never asked for sound gets the channel, and one that does
       gets the same silent device. Only the command line escapes it, by adding
       the channel itself, which is what this does. */
    const int soundHere = s->sound && strcmp(s->sound, "local") == 0;
    const int soundThere = s->sound && strcmp(s->sound, "remote") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_AudioPlayback, soundHere);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteConsoleAudio, soundThere);
    if (soundHere) {
        const char *const rdpsnd[] = {"rdpsnd"};
        freerdp_client_add_dynamic_channel(settings, 1, rdpsnd);
        freerdp_client_add_static_channel(settings, 1, rdpsnd);
    }

    /* How big the far end draws its own interface. The protocol allows 100–500
       for the desktop factor and exactly 100, 140 or 180 for the device one,
       and a value outside either range makes a server ignore both — which is
       why one row sets them to the same three values rather than two rows.
       These two are the copy in the client core data, which Windows reads and
       does nothing with; the copy that works is in the monitor block below. */
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopScaleFactor, (UINT32) s->scale);
    freerdp_settings_set_uint32(settings, FreeRDP_DeviceScaleFactor, (UINT32) s->scale);

    /* What the remote machine may spend the link on drawing. The flags are a
       hint it is free to ignore, and it decides some of the rest for itself
       from the connection type. */
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
        /* LAN, on a row that only asks about effects, because a server reads
           this as what it may spend: xrdp offers RemoteFX to a client that says
           LAN and plain bitmaps to one that says broadband, so anything less
           here silently costs the codec. The other RDP client says the same
           thing on the same row, which is what makes the two comparable. */
        connectionType = CONNECTION_TYPE_LAN;
    } else {
        performance = PERF_ENABLE_FONT_SMOOTHING | PERF_ENABLE_DESKTOP_COMPOSITION;
        connectionType = CONNECTION_TYPE_LAN;
    }
    freerdp_settings_set_uint32(settings, FreeRDP_PerformanceFlags, performance);
    freerdp_settings_set_uint32(settings, FreeRDP_ConnectionType, connectionType);
    /* On whatever the connection type says, because a Windows host sends RTT
       measure requests of its own accord and a client that has not enabled
       autodetect treats one as a protocol error and drops the connection —
       which is a session that ends half a second after it starts. What it buys
       as well is the only line speed this protocol ever states. */
    freerdp_settings_set_bool(settings, FreeRDP_NetworkAutoDetect, TRUE);

    /* The monitor layout, which a scale other than 100 needs as much as a second
       screen does. Two library conditions gate the block that carries each
       monitor's scale factors: the layout is only written under UseMultimon,
       and the attributes inside it only under HasMonitorAttributes, which
       nothing on the client path ever sets — so a client of this library sends
       its scale in the core data alone, and Windows acts on neither number
       until the monitor block arrives with them. Both are therefore set for one
       monitor as well, which costs a layout block naming a single screen. */
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
    /* Two callbacks rather than one: the library keeps a store of its own and
       asks a different question when what is on file disagrees. Leaving the
       second unset is a refusal. */
    ctx->instance->VerifyCertificateEx = onVerifyCertificate;
    ctx->instance->VerifyChangedCertificateEx = onVerifyChangedCertificate;

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
        pthread_mutex_lock(&s->lock);
        const int asked = s->quit;
        pthread_mutex_unlock(&s->lock);
        fireClosed(s, asked ? "" : (detail ? detail : "The connection failed"));
        return NULL;
    }
    LOGI("handshake done: %s", freerdp_settings_get_string(ctx->settings, FreeRDP_ServerHostname));

    /* The library's own handles plus one of ours. Setting a flag does not wake
       a thread inside WaitForMultipleObjects, so the doorbell has to be in the
       set the wait is over. */
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
        /* Before the drain, not after: a command posted in between only costs a
           wakeup that finds nothing, where the other order loses it until the
           timeout. The reset is ours to do — a WinPR event handle has no
           cleanup hook, so no wait resets one whatever it was created as, and
           an auto-reset event left signalled turns this loop into a spin. */
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

/* ---- the JNI surface ---------------------------------------------------- */

static Session *handleOf(jlong h) {
    return (Session *) (intptr_t) h;
}

JNIEXPORT jstring JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeVersion(JNIEnv *env,
                                                                          jclass cls) {
    (void) cls;
    char buf[64];
    snprintf(buf, sizeof(buf), "FreeRDP %s", freerdp_get_version_string());
    return (*env)->NewStringUTF(env, buf);
}

JNIEXPORT jlong JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeCreate(
        JNIEnv *env, jclass cls, jobject listener, jstring address, jstring userName,
        jstring domain, jstring password, jstring nla, jboolean compression, jstring graphics,
        jstring experience, jstring sound, jint scale, jint width, jint height, jint monitors,
        jint keyboardLayout, jstring clientName, jstring configPath, jint connectTimeoutMs) {
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
    (*env)->GetJavaVM(env, &s->vm);
    s->callbacks = (*env)->NewGlobalRef(env, listener);
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    pthread_rwlock_init(&s->fbLock, NULL);
    s->wake = CreateEvent(NULL, FALSE, FALSE, NULL);
    if (!s->wake) {
        LOGW("no wake event");
        goto fail;
    }

    char *addr = dup_jstring(env, address);
    s->port = 3389;
    if (addr) {
        s->host = addr;
        splitAddress(addr, &s->port, s->addressError, sizeof s->addressError);
    }
    s->user = dup_jstring(env, userName);
    s->domain = dup_jstring(env, domain);
    s->password = dup_jstring(env, password);
    s->nla = dup_jstring(env, nla);
    s->graphics = dup_jstring(env, graphics);
    s->experience = dup_jstring(env, experience);
    s->sound = dup_jstring(env, sound);
    s->clientName = dup_jstring(env, clientName);
    s->configPath = dup_jstring(env, configPath);
    s->compression = compression != JNI_FALSE;
    /* Out of the protocol's range is a scale both ends ignore, so a bad value
       is 100 rather than something a server has to argue with. */
    s->scale = scale == 140 || scale == 180 ? scale : 100;
    s->width = width;
    s->height = height;
    s->monitors = monitors;
    s->keyboardLayout = keyboardLayout;
    s->connectTimeoutMs = connectTimeoutMs;

    /* WinPR reads $HOME through GetKnownPath, and an app's process has no HOME
       at all — which is not a missing feature but a session that cannot start:
       freerdp_settings_new fails outright when the home path comes back null,
       so the context below is never allocated. Process-global, as every session
       here wants the same answer, and set before anything of FreeRDP's runs. */
    if (s->configPath) {
        setenv("HOME", s->configPath, 1);
    }

    /* ContextSize is what the client layer allocates and then writes
       rdpClientContext into; zero means everything above rdpContext lands off
       the end of it, and nothing asserts. */
    RDP_CLIENT_ENTRY_POINTS entry = {0};
    entry.Size = sizeof(RDP_CLIENT_ENTRY_POINTS_V1);
    entry.Version = RDP_CLIENT_INTERFACE_VERSION;
    entry.ContextSize = sizeof(ShimContext);
    rdpContext *ctx = freerdp_client_context_new(&entry);
    if (!ctx) {
        LOGW("freerdp_client_context_new failed");
        goto fail;
    }
    s->ctx = (ShimContext *) ctx;
    s->ctx->session = s;
    s->instance = ctx->instance;

    if (pthread_create(&s->thread, NULL, protocolThread, s) != 0) {
        freerdp_client_context_free(ctx);
        s->ctx = NULL;
        goto fail;
    }
    return (jlong) (intptr_t) s;

fail:
    (*env)->DeleteGlobalRef(env, s->callbacks);
    if (s->wake) {
        CloseHandle(s->wake);
    }
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
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
    free(s);
    return 0;
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeAnswerCredentials(
        JNIEnv *env, jclass cls, jlong handle, jstring userName, jstring domain,
        jstring password) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    char *u = dup_jstring(env, userName);
    char *d = dup_jstring(env, domain);
    char *p = dup_jstring(env, password);
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

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeAnswerTrust(
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
    SetEvent(s->wake);
}

/* Both of the ways a session ends set the flag, wake the wait *and* abort the
   connect: during a handshake the thread is inside somebody else's socket read,
   which no event of ours is in the set of. */
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

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeDisconnect(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        stop(s);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeDestroy(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    stop(s);
    pthread_join(s->thread, NULL);

    if (s->ctx) {
        if (s->started) {
            freerdp_client_stop(&s->ctx->client.context);
        }
        freerdp_client_context_free(&s->ctx->client.context);
        s->ctx = NULL;
    }
    (*env)->DeleteGlobalRef(env, s->callbacks);
    CloseHandle(s->wake);
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
    pthread_rwlock_destroy(&s->fbLock);
    free(s);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativePointer(
        JNIEnv *env, jclass cls, jlong handle, jint x, jint y, jint buttonMask) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_POINTER, x, y, buttonMask, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeKeyDown(
        JNIEnv *env, jclass cls, jlong handle, jint keysym, jlong keyId) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_KEY_DOWN, keysym, 0, keyId, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeKeyUp(
        JNIEnv *env, jclass cls, jlong handle, jlong keyId) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_KEY_UP, 0, 0, keyId, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeReleaseAllKeys(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeFocus(
        JNIEnv *env, jclass cls, jlong handle, jboolean focused) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_FOCUS, focused != JNI_FALSE, 0, 0, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeViewOnly(
        JNIEnv *env, jclass cls, jlong handle, jboolean viewOnly) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (!s) {
        return;
    }
    s->viewOnly = viewOnly != JNI_FALSE;
    if (s->viewOnly) {
        /* Whatever is held at the far end is held by this session, so it is
           this session's to let go of when it stops driving. */
        Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeCanResize(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    return s && s->canResize ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeRequestDesktopSize(
        JNIEnv *env, jclass cls, jlong handle, jint width, jint height) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        Cmd c = {CMD_RESIZE, width, height, 0, NULL, 0};
        post(s, &c);
    }
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeSetMonitorCount(
        JNIEnv *env, jclass cls, jlong handle, jint count) {
    (void) env;
    (void) cls;
    Session *s = handleOf(handle);
    if (s) {
        /* Remembered rather than acted on: the count is what the *next* layout
           asks for, and a desktop somebody is working on does not get
           rearranged as a side effect of a settings row moving. */
        s->monitors = count < 1 ? 1 : (count > 16 ? 16 : count);
    }
}

JNIEXPORT jintArray JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeMonitors(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || !s->connected || !s->ctx) {
        return NULL;
    }
    rdpSettings *settings = s->ctx->client.context.settings;
    const UINT32 count = freerdp_settings_get_uint32(settings, FreeRDP_MonitorCount);
    if (count < 2 || count > 16) {
        return NULL;
    }
    jint flat[4 * 16];
    for (UINT32 i = 0; i < count; i++) {
        const rdpMonitor *m = freerdp_settings_get_pointer_array(settings,
                                                                 FreeRDP_MonitorDefArray, i);
        if (!m) {
            return NULL;
        }
        flat[4 * i + 0] = m->x;
        flat[4 * i + 1] = m->y;
        flat[4 * i + 2] = m->width;
        flat[4 * i + 3] = m->height;
    }
    jintArray array = (*env)->NewIntArray(env, (jsize) (4 * count));
    if (array) {
        (*env)->SetIntArrayRegion(env, array, 0, (jsize) (4 * count), flat);
    }
    return array;
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeClipboard(
        JNIEnv *env, jclass cls, jlong handle, jstring text) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || !text) {
        return;
    }
    const jsize n = (*env)->GetStringLength(env, text);
    WCHAR *copy = calloc((size_t) n + 1, sizeof(WCHAR));
    if (!copy) {
        return;
    }
    (*env)->GetStringRegion(env, text, 0, n, (jchar *) copy);
    Cmd c = {CMD_CLIPBOARD, 0, 0, 0, copy, (size_t) n};
    post(s, &c);
}

/* The facts. What the protocol has no concept of is omitted rather than printed
   empty, which is why there is no desktop name here: RDP has none. */
JNIEXPORT jobjectArray JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeInfo(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || !s->connected || !s->ctx) {
        return NULL;
    }
    rdpSettings *settings = s->ctx->client.context.settings;
    char protocol[64], connection[128], security[64], lineSpeed[64], server[64], viewer[64];

    snprintf(protocol, sizeof(protocol), "RDP (FreeRDP %s)", freerdp_get_version_string());
    /* Written back the way it would have to be typed, brackets and all. */
    const char *host = s->host ? s->host : "";
    if (strchr(host, ':')) {
        snprintf(connection, sizeof(connection), "[%s]:%d", host, s->port);
    } else {
        snprintf(connection, sizeof(connection), "%s:%d", host, s->port);
    }
    const UINT32 selected = freerdp_settings_get_uint32(settings, FreeRDP_SelectedProtocol);
    const char *layer = selected == 0 ? "RDP" : selected == 1 ? "TLS" : "NLA (CredSSP)";
    snprintf(security, sizeof(security), "%s", layer);

    /* The autodetect exchange's own number, in kilobits, and empty until the
       server has run one — which it does when it feels like it rather than at
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

    /* What the picture is arriving as rather than what was asked for: the
       pipeline's channel opening is the server agreeing to it, and the library
       rewrites its own H.264 flags from the capability set the server
       confirmed. The codec below it is read off the surfaces themselves, since
       a declined RemoteFX leaves the setting that asked for it standing. */
    char encoding[64];
    if (s->gfxOpen) {
        snprintf(encoding, sizeof(encoding), "EGFX%s",
                 freerdp_settings_get_bool(settings, FreeRDP_GfxH264) ? ", H.264" : "");
    } else if (s->lastCodec == RDP_CODEC_ID_REMOTEFX) {
        snprintf(encoding, sizeof(encoding), "RemoteFX");
    } else if (s->lastCodec == RDP_CODEC_ID_NSCODEC) {
        snprintf(encoding, sizeof(encoding), "NSCodec");
    } else {
        snprintf(encoding, sizeof(encoding), "Bitmaps");
    }

    const char *values[] = {protocol, connection, security, encoding, lineSpeed,
                            server, viewer};
    const jsize n = (jsize) (sizeof(values) / sizeof(values[0]));
    jobjectArray array = (*env)->NewObjectArray(env, n,
                                                (*env)->FindClass(env, "java/lang/String"), NULL);
    if (!array) {
        return NULL;
    }
    for (jsize i = 0; i < n; i++) {
        jstring js = toJava(env, values[i]);
        (*env)->SetObjectArrayElement(env, array, i, js);
        (*env)->DeleteLocalRef(env, js);
    }
    return array;
}

/* Both directions out of the library's own statistics, which are counted where
   the transport reads and writes its front BIO — so they are the protocol,
   inside TLS and outside TCP, which is where every other backend here counts.
   Nothing is patched for this and no private header is reached into: the pair
   is published. */
JNIEXPORT jlongArray JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeTraffic(
        JNIEnv *env, jclass cls, jlong handle) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || !s->connected || !s->ctx) {
        return NULL;
    }
    UINT64 in = 0, out = 0;
    if (!freerdp_get_stats(s->ctx->client.context.rdp, &in, &out, NULL, NULL)) {
        return NULL;
    }
    const jlong both[2] = {(jlong) in, (jlong) out};
    jlongArray array = (*env)->NewLongArray(env, 2);
    if (array) {
        (*env)->SetLongArrayRegion(env, array, 0, 2, both);
    }
    return array;
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeReadRegion(
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
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeReadThumbnail(
        JNIEnv *env, jclass cls, jlong handle, jint step, jobject bitmap) {
    (void) cls;
    Session *s = handleOf(handle);
    if (!s || step <= 0) {
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
        void *dst = NULL;
        if (AndroidBitmap_lockPixels(env, bitmap, &dst) == ANDROID_BITMAP_RESULT_SUCCESS) {
            for (uint32_t row = 0; row < info.height; row++) {
                const int sy = (int) row * step;
                if (sy >= s->fbH) {
                    break;
                }
                uint32_t *out = (uint32_t *) ((uint8_t *) dst + (size_t) row * info.stride);
                const uint32_t *src = (const uint32_t *) (s->fb
                        + (size_t) sy * (size_t) s->fbW * 4u);
                for (uint32_t col = 0; col < info.width; col++) {
                    const int sx = (int) col * step;
                    out[col] = sx < s->fbW ? src[sx] : 0xFF000000u;
                }
            }
            AndroidBitmap_unlockPixels(env, bitmap);
            ok = JNI_TRUE;
        }
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}
