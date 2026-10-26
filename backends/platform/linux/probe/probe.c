// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "probe.h"

#include <errno.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <vento/backend.h>

/* One session's end of the probe: the session, once `create` has returned
   it, and what the callbacks have said so far. Everything in it is written
   under `lock`, because the callbacks arrive on the protocol thread and the
   body waits on the main one. */
typedef struct Probe {
    const vento_probe *opts;
    pthread_mutex_t lock;
    pthread_cond_t cond;
    struct timespec started;
    vento_session *session; /* NULL until create has returned */
    bool created;           /* whether it has, with or without one */
    bool connected;
    bool closed;
    char *closed_detail;
    int width, height;
    unsigned damaged, frames; /* counted as well as printed */
    /* Whether --password has been given to a credentials prompt, and whether
       a second prompt came. It is given once: a second prompt is the far end
       saying it was wrong, and the same password again is a loop that ends
       as "no connection" rather than as the refusal it was. */
    bool answered, refused;
} Probe;

static double elapsed(const Probe *p) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (double) (now.tv_sec - p->started.tv_sec)
           + (double) (now.tv_nsec - p->started.tv_nsec) / 1e9;
}

/* Every callback is one line, stamped with seconds since the session was
   asked for; the ones that carry a decision say what was answered. Written
   whole, so that a protocol thread's lines and the body's do not interleave. */
static void say(const Probe *p, const char *fmt, ...)
#if defined(__GNUC__) || defined(__clang__)
        __attribute__((format(printf, 2, 3)))
#endif
        ;

static void say(const Probe *p, const char *fmt, ...) {
    char line[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof(line), fmt, ap);
    va_end(ap);
    printf("[%8.3f] %s\n", elapsed(p), line);
    fflush(stdout);
}

/* The session, for a callback that answers: `create` starts the protocol
   thread before it returns, so a question can arrive before the body has
   the handle to answer it through. */
static vento_session *sessionForAnswer(Probe *p) {
    pthread_mutex_lock(&p->lock);
    while (!p->created) {
        pthread_cond_wait(&p->cond, &p->lock);
    }
    vento_session *s = p->session;
    pthread_mutex_unlock(&p->lock);
    return s;
}

/* ---- the callbacks ------------------------------------------------------ */

static void cbConnected(void *user, int width, int height) {
    Probe *p = user;
    say(p, "connected %dx%d", width, height);
    pthread_mutex_lock(&p->lock);
    p->connected = true;
    p->width = width;
    p->height = height;
    pthread_cond_broadcast(&p->cond);
    pthread_mutex_unlock(&p->lock);
}

static void cbDesktopSize(void *user, int width, int height) {
    Probe *p = user;
    say(p, "desktop_size %dx%d", width, height);
    pthread_mutex_lock(&p->lock);
    p->width = width;
    p->height = height;
    pthread_mutex_unlock(&p->lock);
}

static void cbDamaged(void *user, int x, int y, int width, int height) {
    Probe *p = user;
    say(p, "damaged %d,%d %dx%d", x, y, width, height);
    pthread_mutex_lock(&p->lock);
    p->damaged++;
    pthread_mutex_unlock(&p->lock);
}

static void cbFrameEnd(void *user) {
    Probe *p = user;
    say(p, "frame_end");
    pthread_mutex_lock(&p->lock);
    p->frames++;
    pthread_mutex_unlock(&p->lock);
}

static void cbCursor(void *user, const uint32_t *pixels, int width, int height, int hotX,
                     int hotY) {
    Probe *p = user;
    if (!pixels) {
        say(p, "cursor default");
        return;
    }
    /* How much of it is visible, which is the one fact a line can carry. */
    unsigned opaque = 0;
    for (int i = 0; i < width * height; i++) {
        opaque += (pixels[i] >> 24) != 0;
    }
    say(p, "cursor %dx%d hot %d,%d, %u of %d pixels visible", width, height, hotX, hotY,
        opaque, width * height);
}

static void cbPointerMode(void *user, bool relative) {
    say(user, "pointer_mode %s", relative ? "relative" : "absolute");
}

static void cbBell(void *user) {
    say(user, "bell");
}

static void cbStatus(void *user, const char *text) {
    if (text && text[0]) {
        say(user, "status: %s", text);
    } else {
        say(user, "status cleared");
    }
}

static void cbClipboard(void *user, const char *utf8) {
    say(user, "clipboard %zu bytes: \"%.60s\"%s", strlen(utf8), utf8,
        strlen(utf8) > 60 ? "..." : "");
}

static void cbCredentialsNeeded(void *user, bool needsUser) {
    Probe *p = user;
    const vento_probe *o = p->opts;
    vento_session *s = sessionForAnswer(p);
    if (!s) {
        return;
    }
    pthread_mutex_lock(&p->lock);
    const bool again = p->answered;
    if (o->password) {
        p->answered = true;
        p->refused = again;
    }
    pthread_mutex_unlock(&p->lock);
    if (o->password && again) {
        say(p, "credentials_needed (user %s) -> asked again, so the password was refused; "
               "cancelling",
            needsUser ? "wanted" : "not wanted");
        vento_session_answer_credentials(s, NULL, NULL, NULL);
    } else if (o->password) {
        say(p, "credentials_needed (user %s) -> answering user \"%s\"%s%s%s and the password",
            needsUser ? "wanted" : "not wanted", o->user ? o->user : "",
            o->domain ? " domain \"" : "", o->domain ? o->domain : "", o->domain ? "\"" : "");
        vento_session_answer_credentials(s, o->user, o->domain, o->password);
    } else {
        say(p, "credentials_needed (user %s) -> no --password, cancelling",
            needsUser ? "wanted" : "not wanted");
        vento_session_answer_credentials(s, NULL, NULL, NULL);
    }
}

static void answerTrust(Probe *p, const char *what) {
    vento_session *s = sessionForAnswer(p);
    if (!s) {
        return;
    }
    say(p, "%s -> %s", what, p->opts->trust ? "yes (--trust)" : "no (no --trust)");
    vento_session_answer_trust(s, p->opts->trust);
}

static void cbTrustNeeded(void *user, const char *fingerprint) {
    Probe *p = user;
    char what[256];
    snprintf(what, sizeof(what), "trust_needed %s", fingerprint);
    answerTrust(p, what);
}

static void cbUnverified(void *user, const char *why) {
    Probe *p = user;
    char what[512];
    snprintf(what, sizeof(what), "unverified: %s", why);
    answerTrust(p, what);
}

static void cbQuestion(void *user, const char *title, const char *text) {
    Probe *p = user;
    char what[1024];
    snprintf(what, sizeof(what), "question \"%s\": %s", title, text);
    answerTrust(p, what);
}

static void cbSecurityNeeded(void *user, int identity, int encryption, const char *signature,
                             const char *catchphrase, const char *matchingAddress) {
    Probe *p = user;
    static const char *const identities[] = {"ok", "new", "matches another", "changed",
                                             "missing", "preshared", "ard"};
    static const char *const encryptions[] = {"encrypted", "unencrypted (warn)",
                                              "unencrypted (quiet)"};
    char what[1024];
    snprintf(what, sizeof(what), "security_needed identity %s, %s, signature \"%s\" (%s)%s%s",
             identity >= 0 && identity <= VENTO_IDENTITY_ARD ? identities[identity] : "?",
             encryption >= 0 && encryption <= VENTO_UNENCRYPTED_QUIET ? encryptions[encryption]
                                                                      : "?",
             signature, catchphrase, matchingAddress[0] ? ", known as " : "", matchingAddress);
    answerTrust(p, what);
}

static void cbMessage(void *user, const char *text, int severity, bool question,
                      const char *confirmLabel) {
    Probe *p = user;
    static const char *const severities[] = {"information", "question", "warning", "error"};
    vento_session *s = sessionForAnswer(p);
    if (!s) {
        return;
    }
    const char *kind = severity >= 0 && severity <= VENTO_SEVERITY_ERROR ? severities[severity]
                                                                         : "?";
    /* A box with one button has one answer; a question gets --trust's. */
    const bool yes = question ? p->opts->trust : true;
    say(p, "message (%s) \"%s\"%s%s%s -> %s", kind, text, confirmLabel[0] ? " [" : "",
        confirmLabel, confirmLabel[0] ? "]" : "",
        question ? (yes ? "yes (--trust)" : "no (no --trust)") : "ok");
    vento_session_answer_trust(s, yes);
}

static void cbClosed(void *user, const char *detail) {
    Probe *p = user;
    say(p, "closed%s%s", detail[0] ? ": " : "", detail);
    pthread_mutex_lock(&p->lock);
    p->closed = true;
    free(p->closed_detail);
    p->closed_detail = strdup(detail);
    pthread_cond_broadcast(&p->cond);
    pthread_mutex_unlock(&p->lock);
}

/* ---- the command line --------------------------------------------------- */

void vento_probe_init(vento_probe *p) {
    memset(p, 0, sizeof(*p));
    p->seconds = 5;
    p->connect_timeout_s = 30;
    p->keysym = -1;
    p->click_x = p->click_y = -1;
    p->thumb_step = 8;
}

const char vento_probe_usage[] =
        "  --seconds N          how long to watch after connecting (5)\n"
        "  --connect-timeout N  how long to wait for the connection (30)\n"
        "  --trust              answer yes to trust_needed, unverified, question,\n"
        "                       security_needed and a message that is a question\n"
        "  --user NAME          the answer to credentials_needed, with\n"
        "  --domain NAME          --domain for RDP and\n"
        "  --password TEXT        --password; without one the answer is a cancel\n"
        "  --clipboard TEXT     send this as the clipboard a second after connecting\n"
        "  --key KEYSYM         press and release this X keysym then (0x61 is 'a')\n"
        "  --click X,Y          left-click there first, to give something the focus\n"
        "  --out FILE.ppm       write the framebuffer there before leaving, and a\n"
        "                       thumbnail to FILE-thumb.ppm\n"
        "  --thumb-step N       the thumbnail's scale, 1/N (8)\n";

static void die(const char *fmt, const char *arg) {
    fprintf(stderr, fmt, arg);
    fputc('\n', stderr);
    exit(2);
}

const char *vento_probe_value(int argc, char **argv, int *i) {
    if (*i + 1 >= argc) {
        die("%s needs a value", argv[*i]);
    }
    const char *value = argv[*i + 1];
    *i += 2;
    return value;
}

static long number(const char *option, const char *value) {
    char *end;
    errno = 0;
    const long n = strtol(value, &end, 0);
    if (errno || end == value || *end) {
        die("%s: not a number", option);
    }
    return n;
}

bool vento_probe_option(vento_probe *p, int argc, char **argv, int *i) {
    const char *a = argv[*i];
    if (strcmp(a, "--trust") == 0) {
        p->trust = true;
        (*i)++;
    } else if (strcmp(a, "--seconds") == 0) {
        p->seconds = (int) number(a, vento_probe_value(argc, argv, i));
    } else if (strcmp(a, "--connect-timeout") == 0) {
        p->connect_timeout_s = (int) number(a, vento_probe_value(argc, argv, i));
    } else if (strcmp(a, "--user") == 0) {
        p->user = vento_probe_value(argc, argv, i);
    } else if (strcmp(a, "--domain") == 0) {
        p->domain = vento_probe_value(argc, argv, i);
    } else if (strcmp(a, "--password") == 0) {
        p->password = vento_probe_value(argc, argv, i);
    } else if (strcmp(a, "--clipboard") == 0) {
        p->clipboard = vento_probe_value(argc, argv, i);
    } else if (strcmp(a, "--key") == 0) {
        p->keysym = number(a, vento_probe_value(argc, argv, i));
    } else if (strcmp(a, "--click") == 0) {
        const char *v = vento_probe_value(argc, argv, i);
        if (sscanf(v, "%d,%d", &p->click_x, &p->click_y) != 2) {
            die("--click: not X,Y", v);
        }
    } else if (strcmp(a, "--out") == 0) {
        p->out = vento_probe_value(argc, argv, i);
    } else if (strcmp(a, "--thumb-step") == 0) {
        p->thumb_step = (int) number(a, vento_probe_value(argc, argv, i));
        if (p->thumb_step < 1) {
            die("--thumb-step: must be at least 1", a);
        }
    } else {
        return false;
    }
    return true;
}

/* ---- the body ----------------------------------------------------------- */

/* Waits until `*flag` or `*closed`, or `seconds` have passed; whether the
   flag was set. */
static bool waitFor(Probe *p, const bool *flag, int seconds) {
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += seconds;
    pthread_mutex_lock(&p->lock);
    while (!*flag && !p->closed) {
        if (pthread_cond_timedwait(&p->cond, &p->lock, &deadline) == ETIMEDOUT) {
            break;
        }
    }
    const bool set = *flag;
    pthread_mutex_unlock(&p->lock);
    return set;
}

/* Writes rows of R, G, B, A as a binary PPM, a picture format that needs no
   library and that every viewer and `magick` reads. */
static bool writePpm(const char *path, const uint8_t *rgba, int width, int height) {
    FILE *f = fopen(path, "wb");
    if (!f) {
        perror(path);
        return false;
    }
    fprintf(f, "P6\n%d %d\n255\n", width, height);
    for (int i = 0; i < width * height; i++) {
        fwrite(rgba + (size_t) i * 4, 1, 3, f);
    }
    const bool ok = fclose(f) == 0;
    if (!ok) {
        perror(path);
    }
    return ok;
}

static bool writePictures(Probe *p, vento_session *s) {
    const vento_probe *o = p->opts;
    pthread_mutex_lock(&p->lock);
    const int w = p->width, h = p->height;
    pthread_mutex_unlock(&p->lock);
    if (w <= 0 || h <= 0) {
        say(p, "no framebuffer size to read");
        return false;
    }

    bool ok = true;
    uint8_t *pixels = malloc((size_t) w * h * 4);
    if (pixels && vento_session_read_region(s, 0, 0, w, h, pixels, (size_t) w * 4, 0, 0)) {
        ok = writePpm(o->out, pixels, w, h) && ok;
        say(p, "read_region %dx%d -> %s", w, h, o->out);
    } else {
        say(p, "read_region %dx%d refused", w, h);
        ok = false;
    }
    free(pixels);

    const int step = o->thumb_step;
    const int tw = (w + step - 1) / step, th = (h + step - 1) / step;
    uint8_t *thumb = malloc((size_t) tw * th * 4);
    char thumbPath[4096];
    const char *dot = strrchr(o->out, '.');
    const char *slash = strrchr(o->out, '/');
    if (!dot || (slash && dot < slash)) {
        snprintf(thumbPath, sizeof(thumbPath), "%s-thumb", o->out);
    } else {
        snprintf(thumbPath, sizeof(thumbPath), "%.*s-thumb%s", (int) (dot - o->out), o->out, dot);
    }
    if (thumb && vento_session_read_thumbnail(s, step, thumb, (size_t) tw * 4, tw, th)) {
        ok = writePpm(thumbPath, thumb, tw, th) && ok;
        say(p, "read_thumbnail 1/%d %dx%d -> %s", step, tw, th, thumbPath);
    } else {
        say(p, "read_thumbnail 1/%d refused", step);
        ok = false;
    }
    free(thumb);
    return ok;
}

static void poke(Probe *p, vento_session *s) {
    const vento_probe *o = p->opts;
    if (o->click_x >= 0) {
        say(p, "pointer -> click at %d,%d", o->click_x, o->click_y);
        vento_session_pointer(s, o->click_x, o->click_y, 0);
        vento_session_pointer(s, o->click_x, o->click_y, 1);
        vento_session_pointer(s, o->click_x, o->click_y, 0);
    }
    if (o->clipboard) {
        say(p, "clipboard -> \"%s\"", o->clipboard);
        vento_session_clipboard(s, o->clipboard);
    }
    if (o->keysym >= 0) {
        say(p, "key_down/key_up -> keysym 0x%lx", o->keysym);
        vento_session_key_down(s, (uint32_t) o->keysym, 1);
        vento_session_key_up(s, 1);
    }
}

static void facts(Probe *p, vento_session *s) {
    vento_strings info;
    if (vento_session_info(s, &info)) {
        for (size_t i = 0; i < info.count; i++) {
            say(p, "info[%zu] %s", i, info.items[i]);
        }
        vento_strings_free(&info);
    } else {
        say(p, "info: none yet");
    }
    uint64_t traffic[2];
    if (vento_session_traffic(s, traffic)) {
        say(p, "traffic %llu in, %llu out", (unsigned long long) traffic[0],
            (unsigned long long) traffic[1]);
    }
    int32_t screens[4 * VENTO_MAX_MONITORS];
    const int n = vento_session_monitors(s, screens);
    say(p, "monitors %d%s", n, vento_session_can_resize(s) ? ", resizable" : ", not resizable");
    for (int i = 0; i < n; i++) {
        say(p, "  %dx%d+%d+%d", screens[i * 4 + 2], screens[i * 4 + 3], screens[i * 4],
            screens[i * 4 + 1]);
    }
}

int vento_probe_run(const vento_probe *o, vento_probe_create create, void *config) {
    Probe p = {.opts = o};
    pthread_mutex_init(&p.lock, NULL);
    pthread_cond_init(&p.cond, NULL);
    clock_gettime(CLOCK_MONOTONIC, &p.started);

    const vento_callbacks callbacks = {
            .user = &p,
            .connected = cbConnected,
            .desktop_size = cbDesktopSize,
            .damaged = cbDamaged,
            .frame_end = cbFrameEnd,
            .cursor = cbCursor,
            .pointer_mode = cbPointerMode,
            .bell = cbBell,
            .clipboard = cbClipboard,
            .credentials_needed = cbCredentialsNeeded,
            .trust_needed = cbTrustNeeded,
            .unverified = cbUnverified,
            .question = cbQuestion,
            .security_needed = cbSecurityNeeded,
            .message = cbMessage,
            .closed = cbClosed,
            .status = cbStatus,
    };

    say(&p, "create");
    vento_session *s = create(config, &callbacks);
    pthread_mutex_lock(&p.lock);
    p.session = s;
    p.created = true;
    pthread_cond_broadcast(&p.cond);
    pthread_mutex_unlock(&p.lock);
    if (!s) {
        say(&p, "create returned NULL");
        return 1;
    }

    int status = 0;
    if (!waitFor(&p, &p.connected, o->connect_timeout_s)) {
        pthread_mutex_lock(&p.lock);
        const bool closed = p.closed, refused = p.refused;
        pthread_mutex_unlock(&p.lock);
        if (refused) {
            say(&p, "authentication failed: the password was refused");
        } else {
            say(&p, closed ? "ended before connecting" : "no connection in %d s",
                o->connect_timeout_s);
        }
        status = 1;
    } else {
        /* A second for the first frame to land, so that a click or a key
           goes to a desktop that is drawn rather than one that is loading;
           then the rest of the time asked for. Both wait on `closed` with a
           deadline, by waiting on a flag that is never set. */
        const bool never = false;
        waitFor(&p, &never, 1);
        if (!p.closed) {
            poke(&p, s);
        }
        const int rest = o->seconds > 1 ? o->seconds - 1 : 0;
        waitFor(&p, &never, rest);

        pthread_mutex_lock(&p.lock);
        const bool closed = p.closed, refused = p.refused;
        pthread_mutex_unlock(&p.lock);
        if (closed) {
            say(&p, "ended before the %d s were up%s", o->seconds,
                refused ? ": the password was refused" : "");
            status = 1;
        } else {
            say(&p, "%u damaged rectangles in %u frames", p.damaged, p.frames);
            facts(&p, s);
            if (o->out && !writePictures(&p, s)) {
                status = 1;
            }
        }
    }

    say(&p, "disconnect");
    vento_session_disconnect(s);
    if (!waitFor(&p, &p.closed, 10)) {
        say(&p, "no closed within 10 s of disconnect");
        status = 1;
    }
    say(&p, "destroy");
    vento_session_destroy(s);
    say(&p, "done");

    free(p.closed_detail);
    pthread_cond_destroy(&p.cond);
    pthread_mutex_destroy(&p.lock);
    return status;
}
