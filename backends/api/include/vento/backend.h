// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The C interface of the platform-neutral layer: a wrapped C or C++ client
// library is written against it, and the platform adapter calls it, with
// neither knowing the other.
//
// It is `backends/runtime` (`Callbacks` outward, `Runner` inward) restated
// in C, because several of the clients here are C and C++ libraries, each
// with a shim of ours around it. The protocol-specific parts stay in the
// shims; the shared parts are written once against this header, in Rust:
// `api::c` in `backends/api` wraps every C backend as an `api::Backend`,
// which the Linux client uses directly and the phone through the JNI layer
// in `backends/platform/android`, linked into each C backend's own library.
// This header lives in `backends/api/include/vento/` with the other shared
// headers: the `api` crate generates Rust bindings for them, and each C
// backend's crate generates bindings for its own header on top. The only C
// a platform supplies is `vento_log`: logcat on the phone, stderr on a
// desktop.
//
// The shape follows `runtime::Callbacks` method for method. `vento_callbacks`
// is the outward half, fire-and-forget: every question a protocol can ask is
// an event saying one is pending, answered later through the session. The
// inward half is `vento_session`, an opaque object reached through a vtable,
// so that several backends can live in one process without clashing symbols
// and the adapter can be written once. How a library is driven (the thread
// that owns a connection, the command ring it drains, the second
// framebuffer under a writer lock) is up to the session files and dictated
// by the libraries; this header only defines how they look from outside.
//
// ## Threads
//
// Every `vento_session_*` call may come from any thread. `read_region` and
// `read_thumbnail` may run concurrently with everything else, including each
// other, and are meant to be called from whichever thread is drawing: they
// take the read side of a lock the protocol thread takes the write side of
// only once per update, so a pixel fetch never queues behind a socket read.
//
// Every callback arrives on one of the session's own threads: the protocol
// thread (the one that owns the connection) or a thread its library runs
// alongside it for part of the protocol. FreeRDP's dynamic channels have
// their own thread, and the graphics pipeline's resizes and paints
// (`desktop_size`, `damaged`, `frame_end`) arrive on it, so two callbacks of
// one session can run at once and each has to be safe for that. Picture
// updates never overlap, since the library makes them all under one lock.
// A callback must not block on anything those threads are waiting for:
// calling back into the session is fine, waiting for a frame deadlocks. The
// six questions (`credentials_needed`, `trust_needed`, `question`,
// `unverified`, `security_needed`, `message`) only appear to be an
// exception: the asking thread parks after asking, and the answer arrives
// later through `answer_credentials` or `answer_trust`, from any thread.
// `closed` is the last callback a session makes, and `destroy` returns only
// after the session's threads have ended, so once it returns no callback is
// running and whatever `user` pointed at may be freed.
//
// ## Strings
//
// Every `const char *` is UTF-8, NUL-terminated, in both directions. On the
// way out the session files guarantee this, not the libraries: a desktop
// name may be Latin-1 or UTF-8, and a refused connection is reported in the
// library's words, which quote the server's. So a session file passes a
// server's bytes through `vento_utf8_from_bytes` (vento/text.h) before they
// reach a callback, and a platform may treat the string as UTF-8 without
// checking. Strings passed to a session are owned by the caller and copied
// before the call returns. Strings passed to a callback are owned by the
// session and are freed when the callback returns.
//
// ## Pixels
//
// The framebuffer's byte order is `R, G, B, A` in memory (a little-endian
// `uint32_t` of `0xAABBGGRR`), which matches Android's `ARGB_8888` in memory
// and a 32-bit RFB format with red at shift 0. The alpha byte is always
// `0xFF`: the libraries leave it undefined, and the session files fill it in
// during the copy they already make. The cursor uses the same order; a
// platform that wants `Color`'s `0xAARRGGBB` swaps on its side of the call,
// where it already has to copy, and hashes there too (see `runtime::cursor`
// for what the phone's JNI layer does with a cursor).

#ifndef VENTO_BACKEND_H
#define VENTO_BACKEND_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The states of RealVNC's security prompt: `Prompt.Trust`'s two enums in
   Java, restated in C in the same order. Used only by `security_needed`
   below. */
enum {
    VENTO_IDENTITY_OK = 0,              /* known, and matches */
    VENTO_IDENTITY_NEW = 1,             /* never seen before */
    VENTO_IDENTITY_MATCHES_ANOTHER = 2, /* seen, but under a different address */
    VENTO_IDENTITY_CHANGED = 3,         /* seen under this address, and different now */
    VENTO_IDENTITY_MISSING = 4,         /* the server offered none */
    VENTO_IDENTITY_PRESHARED = 5,       /* configured out of band */
    VENTO_IDENTITY_ARD = 6,             /* Apple Remote Desktop, which has none of this kind */
};

enum {
    VENTO_ENCRYPTED = 0,
    VENTO_UNENCRYPTED_WARN = 1,  /* plain text, and the user should be warned */
    VENTO_UNENCRYPTED_QUIET = 2, /* plain text, deliberately: a local or tunnelled link */
};

/* How a `message` should look: `Prompt.Message.Severity` in C. */
enum {
    VENTO_SEVERITY_INFORMATION = 0,
    VENTO_SEVERITY_QUESTION = 1,
    VENTO_SEVERITY_WARNING = 2,
    VENTO_SEVERITY_ERROR = 3,
};

/* What a session tells the platform. Every member is required: a platform
   implements the whole interface or none of it, as the Rust `Callbacks`
   trait requires, and the session files call through these without a NULL
   check, which keeps each call cheap (`damaged` runs hundreds of times a
   frame). A platform can implement an unwanted one as an empty function.
   `api::c` fills every member, forwarding into the mailbox both clients
   drain. */
typedef struct vento_callbacks {
    void *user;

    void (*connected)(void *user, int width, int height);
    void (*desktop_size)(void *user, int width, int height);
    void (*damaged)(void *user, int x, int y, int width, int height);
    void (*frame_end)(void *user);
    /* `pixels` is `width * height` words in the framebuffer's own order, or
       NULL with a size of zero when the server has hidden the pointer or
       asked for the default one, both of which mean "draw your own". Alpha
       comes from the cursor's mask. The pointer is owned by the session and
       invalid after this returns. */
    void (*cursor)(void *user, const uint32_t *pixels, int width, int height, int hot_x,
                   int hot_y);
    void (*pointer_mode)(void *user, bool relative);
    void (*bell)(void *user);
    void (*clipboard)(void *user, const char *utf8);
    /* Credentials are needed, and the protocol thread is stopped until
       `answer_credentials`. `needs_user` is false for VncAuth, which has no
       user name. */
    void (*credentials_needed)(void *user, bool needs_user);
    /* A certificate the library could not verify. VNC and RDP servers use
       self-signed certificates, so this is every certificate: the
       fingerprint is the identity, and the answer, through `answer_trust`,
       comes from a pin store or from the user. `fingerprint` is SHA-256 as
       `openssl x509 -fingerprint -sha256` prints it, so one host reached
       through two backends is one entry. */
    void (*trust_needed)(void *user, const char *fingerprint);
    /* The far end could not be verified at all (a TLS tunnel with no
       certificate), and the session asks whether to continue. Answered
       through the same `answer_trust`, since yes and no mean the same to the
       protocol thread and only one of these questions can be outstanding. */
    void (*unverified)(void *user, const char *why);
    /* The first of three extensions over `runtime::Callbacks`, called only
       by TigerVNC: for an RSA-AES server key, which TigerVNC's viewer shows
       and asks about every time without storing it, so this passes that
       question (a title and a text, in TigerVNC's words) rather than a
       fingerprint for the platform to decide about. (Its certificates go to
       `trust_needed` like any other backend's.) Answered through
       `answer_trust` like the other two, because it is the same question to
       the protocol thread. A platform without a dialog answers no. */
    void (*question)(void *user, const char *title, const char *text);
    /* The second extension, called only by RealVNC: its core's security
       prompt, combining `trust_needed` and `unverified` into one question
       with both states given (`identity` is a `VENTO_IDENTITY_*` and
       `encryption` a `VENTO_UNENCRYPTED_*` or `VENTO_ENCRYPTED`), because
       the core keeps its own identity store and decides whether a server is
       new, known or changed, leaving only a yes or no to this side.
       `signature` is the identity in the core's own form (a short hex
       string, not comparable with a certificate pin), `catchphrase` is its
       human-readable form, and `matching_address` is the address the
       identity is already known under, for `VENTO_IDENTITY_MATCHES_ANOTHER`;
       any of the three may be empty. Answered through `answer_trust` like
       the other three; a yes that should also be remembered goes through
       RealVNC's own `vento_realvnc_answer_security`, since no other backend
       has a remember option. A platform without a dialog answers no. */
    void (*security_needed)(void *user, int identity, int encryption, const char *signature,
                            const char *catchphrase, const char *matching_address);
    /* The third extension, also RealVNC's: a message its core wants shown,
       with a `VENTO_SEVERITY_*` and whether it is a yes-or-no `question` or
       an OK box. Most non-fatal errors arrive this way, in the core's words,
       and a refused connection is one of these followed by `closed`.
       `confirm_label` is the affirmative button's text when the core
       supplied one, and empty otherwise. Answered through `answer_trust`:
       yes for OK or yes, no for cancel or no. A platform without a dialog
       answers no, which for a plain OK box is equivalent. */
    void (*message)(void *user, const char *text, int severity, bool question,
                    const char *confirm_label);
    /* Terminal, and the last callback this session makes. `detail` is empty
       for a requested disconnect, `VENTO_LOGON_FAILED` when the server
       refused the user name or password, and otherwise gives the reason in
       the library's words. */
    void (*closed)(void *user, const char *detail);
    /* What the connection is waiting for while it connects, as a short
       sentence for the connecting screen: a server asking its user to accept
       the connection, a retry. NULL or "" clears it. Only for a wait long
       enough to read; the platform drops it once the session connects or
       closes. After `closed` in the table only because members are appended
       at its end; like every other callback it comes before `closed`. */
    void (*status)(void *user, const char *text);
} vento_callbacks;

/* The `closed` detail for a user name or password the server refused. The
   adapter shows it with advice for the domain that was tried
   (`api::credentials::logon_failed`). */
#define VENTO_LOGON_FAILED "Logon failed."

/* The strings `info` returns: a row per fact, in the order that backend's
   Rust adapter labels them. Owned by the caller, and freed with
   `vento_strings_free`. Chosen over an emit callback because the caller
   (`api::c`, turning them into a panel's rows for either client) wants the
   whole list at once, and over a fixed array because the number of facts
   differs between backends. */
typedef struct vento_strings {
    char **items;
    size_t count;
} vento_strings;

static inline void vento_strings_free(vento_strings *strings) {
    for (size_t i = 0; i < strings->count; i++) {
        free(strings->items[i]);
    }
    free(strings->items);
    strings->items = NULL;
    strings->count = 0;
}

/* For a session filling one in: copies the `count` strings in `values`.
   False, with nothing allocated, on an allocation failure. */
static inline bool vento_strings_set(vento_strings *out, const char *const *values, size_t count) {
    out->items = (char **) calloc(count, sizeof(char *));
    out->count = 0;
    if (!out->items) {
        return false;
    }
    for (size_t i = 0; i < count; i++) {
        const char *value = values[i] ? values[i] : "";
        const size_t bytes = strlen(value) + 1;
        out->items[i] = (char *) malloc(bytes);
        if (!out->items[i]) {
            out->count = i;
            vento_strings_free(out);
            return false;
        }
        memcpy(out->items[i], value, bytes);
    }
    out->count = count;
    return true;
}

/* The most screens `monitors` can report. A fixed buffer keeps the call
   allocation-free on both sides, and sixteen is more than any real
   setup. */
#define VENTO_MAX_MONITORS 16

typedef struct vento_session vento_session;

/* The calls a session takes. Each backend fills one of these in statically
   and points every session it creates at it. Callers use the inline
   wrappers below rather than the table directly.

   The table only grows at its end, and `size` is its length in the header
   the backend was built against: backend libraries are loaded at run time,
   so a table can be older than the caller's header, and a member past its
   `size` must not be read at all. Every member up to `destroy` is in every
   table; later ones are optional, read only when `size` covers them, with
   NULL meaning "not supported". */
typedef struct vento_session_ops {
    /* sizeof(vento_session_ops), as the backend saw it. */
    size_t size;
    void (*pointer)(vento_session *s, int x, int y, int button_mask);
    void (*key_down)(vento_session *s, uint32_t keysym, int64_t key_id);
    void (*key_up)(vento_session *s, int64_t key_id);
    void (*release_all_keys)(vento_session *s);
    void (*focus)(vento_session *s, bool focused);
    void (*view_only)(vento_session *s, bool view_only);
    /* Whether the far end accepts a new desktop size now. A session with
       `request_layout` answers for a layout of any number of screens; one
       without it answers for one screen covering the desktop, and returns
       false for a multi-screen desktop, which a resize would merge into
       one. */
    bool (*can_resize)(vento_session *s);
    /* The far end's screen layout, four ints per screen (x, y, width,
       height), into a buffer of `4 * VENTO_MAX_MONITORS`. Returns the number
       of screens, zero until the server has sent a layout. */
    int (*monitors)(vento_session *s, int32_t *out);
    void (*request_desktop_size)(vento_session *s, int width, int height);
    void (*clipboard)(vento_session *s, const char *utf8);
    /* The facts. False, with nothing allocated, when there are none yet. */
    bool (*info)(vento_session *s, vento_strings *out);
    /* Bytes received and sent, counted between the protocol and its
       transport (inside TLS, outside TCP), as every backend here counts
       them. False while there is nothing to count yet. */
    bool (*traffic)(vento_session *s, uint64_t out[2]);
    /* Copy the desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, where
       `dst` is rows of `R, G, B, A` bytes `dst_stride_bytes` apart. The
       caller guarantees the destination holds the region; the session checks
       only that the source contains it (for any int, without overflowing)
       and that `dst_x` and `dst_y` are not negative, and returns false when
       either check fails or there is no framebuffer yet. */
    bool (*read_region)(vento_session *s, int x, int y, int width, int height, void *dst,
                        size_t dst_stride_bytes, int dst_x, int dst_y);
    /* The whole desktop at `1/step`, nearest neighbour, into the top left of
       a destination `dst_w × dst_h` pixels, which the caller has sized to
       `ceil(w/step) × ceil(h/step)`. False, with nothing written, if it does
       not fit (the desktop was resized between the sizing and the read) or
       there is no framebuffer yet. */
    bool (*read_thumbnail)(vento_session *s, int step, void *dst, size_t dst_stride_bytes,
                           int dst_w, int dst_h);
    /* The answer to `credentials_needed`. `user` and `domain` may be NULL for
       empty; a NULL `password` cancels, which ends the session. The domain is
       RDP's and the RFB backends ignore it. */
    void (*answer_credentials)(vento_session *s, const char *user, const char *domain,
                               const char *password);
    /* The answer to `trust_needed`, `unverified`, `question`, `security_needed`
       and `message`. A no ends the session, except for a `message` that was
       not a question, where it only dismisses the message. */
    void (*answer_trust)(vento_session *s, bool accept);
    /* Ends the session without waiting for it. Idempotent. */
    void (*disconnect)(vento_session *s);
    /* Ends the session, waits for its thread, and frees it. Nothing may use
       the session afterwards. */
    void (*destroy)(vento_session *s);

    /* Optional from here: read only when `size` covers the member (see
       VENTO_SESSION_HAS), and NULL when the backend does not support it. */

    /* The far end's layout as `monitors` gives it, in the same order, with
       the far end's own id for each screen first: five ints per screen (id,
       x, y, width, height, the id's 32 bits as an int) into a buffer of
       `5 * VENTO_MAX_MONITORS`. Returns the number of screens. NULL when the
       protocol does not number its screens; a screen's id is then its
       index. */
    int (*screens)(vento_session *s, int32_t *out);
    /* Ask for a desktop `width × height` divided into `n` screens, five ints
       each as `screens` gives them. A known id moves or resizes that screen,
       keeping its flags; an unknown id, or one already used earlier in the
       list, is a new screen, which the session numbers itself. The far end
       may answer with `desktop_size` and a new layout, or not at all; a
       layout the session can tell is invalid is not sent. NULL when the
       protocol cannot place screens. */
    void (*request_layout)(vento_session *s, int width, int height, const int32_t *screens,
                           int n);
    /* How the picture is being decoded, for a debug overlay, when the
       session decodes video through the platform's decoders (vento/video.h):
       the decoder's status line as of roughly the last picture, into `out`
       with its NUL, truncated to fit. False, with nothing written, before
       the first picture and for a picture that is not video. */
    bool (*decoder_status)(vento_session *s, char *out, size_t len);
} vento_session_ops;

/* Whether `s`'s table has a non-NULL `member`: the table's `size` covers
   it, so it was in the header the backend was built against, and the
   backend filled it in. */
#define VENTO_SESSION_HAS(s, member)                                                         \
    ((s)->ops->size >= offsetof(vento_session_ops, member) + sizeof((s)->ops->member)        \
     && (s)->ops->member != NULL)

/* The first member of every backend's own session struct, so that a pointer
   to one is a pointer to this. */
struct vento_session {
    const vento_session_ops *ops;
};

static inline void vento_session_pointer(vento_session *s, int x, int y, int button_mask) {
    s->ops->pointer(s, x, y, button_mask);
}

static inline void vento_session_key_down(vento_session *s, uint32_t keysym, int64_t key_id) {
    s->ops->key_down(s, keysym, key_id);
}

static inline void vento_session_key_up(vento_session *s, int64_t key_id) {
    s->ops->key_up(s, key_id);
}

static inline void vento_session_release_all_keys(vento_session *s) {
    s->ops->release_all_keys(s);
}

static inline void vento_session_focus(vento_session *s, bool focused) {
    s->ops->focus(s, focused);
}

static inline void vento_session_view_only(vento_session *s, bool view_only) {
    s->ops->view_only(s, view_only);
}

static inline bool vento_session_can_resize(vento_session *s) {
    return s->ops->can_resize(s);
}

static inline int vento_session_monitors(vento_session *s, int32_t *out) {
    return s->ops->monitors(s, out);
}

static inline void vento_session_request_desktop_size(vento_session *s, int width, int height) {
    s->ops->request_desktop_size(s, width, height);
}

static inline void vento_session_clipboard(vento_session *s, const char *utf8) {
    s->ops->clipboard(s, utf8);
}

static inline bool vento_session_info(vento_session *s, vento_strings *out) {
    return s->ops->info(s, out);
}

static inline bool vento_session_traffic(vento_session *s, uint64_t out[2]) {
    return s->ops->traffic(s, out);
}

static inline bool vento_session_read_region(vento_session *s, int x, int y, int width,
                                             int height, void *dst, size_t dst_stride_bytes,
                                             int dst_x, int dst_y) {
    return s->ops->read_region(s, x, y, width, height, dst, dst_stride_bytes, dst_x, dst_y);
}

static inline bool vento_session_read_thumbnail(vento_session *s, int step, void *dst,
                                                size_t dst_stride_bytes, int dst_w, int dst_h) {
    return s->ops->read_thumbnail(s, step, dst, dst_stride_bytes, dst_w, dst_h);
}

static inline void vento_session_answer_credentials(vento_session *s, const char *user,
                                                    const char *domain, const char *password) {
    s->ops->answer_credentials(s, user, domain, password);
}

static inline void vento_session_answer_trust(vento_session *s, bool accept) {
    s->ops->answer_trust(s, accept);
}

static inline void vento_session_disconnect(vento_session *s) {
    s->ops->disconnect(s);
}

static inline void vento_session_destroy(vento_session *s) {
    s->ops->destroy(s);
}

/* -1 when the session has no screen ids, and `monitors` is the layout. */
static inline int vento_session_screens(vento_session *s, int32_t *out) {
    return VENTO_SESSION_HAS(s, screens) ? s->ops->screens(s, out) : -1;
}

/* False, with nothing sent, when the session cannot send a layout. */
static inline bool vento_session_request_layout(vento_session *s, int width, int height,
                                                const int32_t *screens, int n) {
    if (!VENTO_SESSION_HAS(s, request_layout)) {
        return false;
    }
    s->ops->request_layout(s, width, height, screens, n);
    return true;
}

#ifdef __cplusplus
}
#endif

#endif
