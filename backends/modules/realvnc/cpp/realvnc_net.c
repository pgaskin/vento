// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The hooks under the RealVNC core's socket calls: `realvnc_net.h` says what
// each answers, and why. One lock guards the routes, the placeholder names,
// the addrinfo lists handed out and the sockets bridged by `connect`; it is
// never held across a dial or a call into a `vento_net`. Each hook first
// checks a counter without it, so that while nothing is routed (every
// session direct) a call costs one atomic load on top of what it replaced
// and goes on to it: the core's own calls, unchanged. The read hooks look
// further only at the end of a stream.

#define _GNU_SOURCE // dup3, pipe2

#include "realvnc_net.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <poll.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <vento/log.h>

#define TAG "RealVnc"

/* Bionic's numbers, from the NDK's <netdb.h>: the core's, on both platforms.
   Several differ from glibc's. */
enum {
    BIONIC_AI_CANONNAME = 0x2,
    BIONIC_AI_NUMERICHOST = 0x4,
    BIONIC_EAI_FAIL = 4,
    BIONIC_EAI_FAMILY = 5,
    BIONIC_EAI_MEMORY = 6,
    BIONIC_EAI_NONAME = 8,
    BIONIC_EAI_SERVICE = 9,
    BIONIC_EAI_OVERFLOW = 14,
    BIONIC_NI_NUMERICHOST = 0x2,
};

/* Bionic's `struct addrinfo`: `ai_canonname` comes before `ai_addr`, the
   reverse of glibc's. */
struct bionic_addrinfo {
    int ai_flags;
    int ai_family;
    int ai_socktype;
    int ai_protocol;
    socklen_t ai_addrlen;
    char *ai_canonname;
    struct sockaddr *ai_addr;
    struct bionic_addrinfo *ai_next;
};

#ifdef __BIONIC__
#include <netdb.h>
#include <stddef.h>
_Static_assert(offsetof(struct addrinfo, ai_canonname)
                       == offsetof(struct bionic_addrinfo, ai_canonname)
                       && offsetof(struct addrinfo, ai_addr)
                       == offsetof(struct bionic_addrinfo, ai_addr)
                       && sizeof(struct addrinfo) == sizeof(struct bionic_addrinfo),
               "bionic's addrinfo layout");
_Static_assert(AI_CANONNAME == BIONIC_AI_CANONNAME && AI_NUMERICHOST == BIONIC_AI_NUMERICHOST
                       && NI_NUMERICHOST == BIONIC_NI_NUMERICHOST && EAI_FAIL == BIONIC_EAI_FAIL
                       && EAI_FAMILY == BIONIC_EAI_FAMILY && EAI_MEMORY == BIONIC_EAI_MEMORY
                       && EAI_NONAME == BIONIC_EAI_NONAME && EAI_SERVICE == BIONIC_EAI_SERVICE
                       && EAI_OVERFLOW == BIONIC_EAI_OVERFLOW,
               "bionic's resolver numbers");
#endif

// ---- what the hooks replaced -------------------------------------------------

typedef int (*connect_fn)(int, const struct sockaddr *, socklen_t);
typedef int (*getaddrinfo_fn)(const char *, const char *, const void *, void **);
typedef void (*freeaddrinfo_fn)(void *);
typedef int (*getnameinfo_fn)(const struct sockaddr *, socklen_t, char *, socklen_t, char *,
                              socklen_t, int);
typedef int (*sockname_fn)(int, struct sockaddr *, socklen_t *);
typedef int (*getsockopt_fn)(int, int, int, void *, socklen_t *);
typedef int (*setsockopt_fn)(int, int, int, const void *, socklen_t);
typedef ssize_t (*read_fn)(int, void *, size_t);
typedef ssize_t (*read_chk_fn)(int, void *, size_t, size_t);
typedef ssize_t (*recvmsg_fn)(int, struct msghdr *, int);
typedef ssize_t (*sendmsg_fn)(int, const struct msghdr *, int);

static connect_fn next_connect;
static getaddrinfo_fn next_getaddrinfo;
static freeaddrinfo_fn next_freeaddrinfo;
static getnameinfo_fn next_getnameinfo;
static sockname_fn next_getpeername;
static sockname_fn next_getsockname;
static getsockopt_fn next_getsockopt;
static setsockopt_fn next_setsockopt;
static read_fn next_read;
static read_chk_fn next_read_chk;
static recvmsg_fn next_recvmsg;
static sendmsg_fn next_sendmsg;

// ---- state -------------------------------------------------------------------

#define MAX_ROUTES 16
#define MAX_NAMES 1024
#define MAX_RESULTS 64
#define MAX_BRIDGED 32
#define MAX_NOTED 64
#define MAX_PARKED 8

struct route {
    int id; // 0: free
    bool has_net;
    vento_net net;
    char host[256];
    int timeout_ms;
    char key[128]; // what carries it (`realvnc_net_attach`'s `key`); "" for direct
    int busy; // dials, cancels and `ended` calls in progress, which `detach` waits for
    bool detaching;
    bool closed; // the session has closed: nothing of the core's is for it any more
    unsigned long order;
    int parked[MAX_PARKED]; // the far ends of sockets whose dial a cancel ended, closed by `detach`
    int parked_count;
    // A pipe whose write end `detach` closes, ending every relay of the
    // route's (each polls a copy of the read end); -1 where there is none.
    int stop[2];
};

/* A socket `connect` put a route's fd under: identified by number and by
   inode, so that a number the core has closed and reused is recognised
   without hooking `close`. */
struct bridged {
    bool used;
    int fd;
    dev_t dev;
    ino_t ino;
    struct sockaddr_storage peer;
    socklen_t peer_len;
    struct sockaddr_storage local;
    socklen_t local_len;
    int nodelay;
    int route;      // the route it was dialled through
    int connection; // `vento_net_peer.connection`, for `ended`; 0 for none (yet)
    bool ended_logged;
    int kind;      // SOCK_STREAM or SOCK_DGRAM, as the core made it
    bool dialling; // its dial (`dial_later`) is not done: a stream is kept unwritable
    int so_error;  // a stream's SO_ERROR answer, once its dial is done
    int sndbuf;    // SO_SNDBUF the core set while it was dialling, for after; 0 for none
};

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t idle = PTHREAD_COND_INITIALIZER;
static struct route routes[MAX_ROUTES];
static int last_id;
static unsigned long last_order;
static char *names[MAX_NAMES]; // placeholder i + 1 stands for names[i]
static int name_route[MAX_NAMES]; // ...and is dialled through this route, its latest lookup's
static void *results[MAX_RESULTS];
static struct bridged bridged[MAX_BRIDGED];
static unsigned noted[MAX_NOTED];
static int installed; // 1: every hook in place; -1: could not be; 0: not yet known
static char install_failure[200];
static void (*_Atomic notice_fn)(const char *text);

/* Read without the lock, as the hooks' fast path. */
static atomic_int relayed_routes;
static atomic_int name_count;
static atomic_int bridged_count;

/* Logs the first use of each (call, level, option) on a bridged socket, so
   that what the core asks of one is in the log once per process. Locked. */
static void note(const char *call, int level, int name, const char *answer) {
    unsigned key = 5381;
    for (const char *c = call; *c; c++) {
        key = key * 33 + (unsigned char) *c;
    }
    key = (key ^ ((unsigned) level << 16) ^ (unsigned) name) | 1;
    for (int i = 0; i < MAX_NOTED; i++) {
        if (noted[i] == key) {
            return;
        }
        if (noted[i] == 0) {
            noted[i] = key;
            if (level || name) {
                vento_log(VENTO_LOG_INFO, TAG, "%s(level %d, option %d) on a socket from the network: %s",
                          call, level, name, answer);
            } else {
                vento_log(VENTO_LOG_INFO, TAG, "%s on a socket from the network: %s", call, answer);
            }
            return;
        }
    }
}

// ---- addresses ---------------------------------------------------------------

/* 198.18.0.0/15 (RFC 2544, benchmarking) and 2001:2::/48 (RFC 5180, the
   same for IPv6): ranges no server is on, so a placeholder is never a real
   address. Placeholder n is the range's base plus n. */
static const uint32_t PLACEHOLDER4 = 0xc6120000u;
static const uint8_t PLACEHOLDER6[6] = {0x20, 0x01, 0x00, 0x02, 0x00, 0x00};

static void placeholder_addr(int n, int family, int port, struct sockaddr_storage *out,
                             socklen_t *len) {
    memset(out, 0, sizeof *out);
    if (family == AF_INET6) {
        struct sockaddr_in6 *a = (struct sockaddr_in6 *) out;
        a->sin6_family = AF_INET6;
        a->sin6_port = htons((uint16_t) port);
        memcpy(a->sin6_addr.s6_addr, PLACEHOLDER6, sizeof PLACEHOLDER6);
        a->sin6_addr.s6_addr[14] = (uint8_t) (n >> 8);
        a->sin6_addr.s6_addr[15] = (uint8_t) n;
        *len = sizeof *a;
    } else {
        struct sockaddr_in *a = (struct sockaddr_in *) out;
        a->sin_family = AF_INET;
        a->sin_port = htons((uint16_t) port);
        a->sin_addr.s_addr = htonl(PLACEHOLDER4 + (uint32_t) n);
        *len = sizeof *a;
    }
}

/* The placeholder `sa` is, or 0. Locked. */
static int placeholder_index(const struct sockaddr *sa, socklen_t len) {
    uint32_t n = 0;
    if (sa->sa_family == AF_INET && len >= sizeof(struct sockaddr_in)) {
        const uint32_t a = ntohl(((const struct sockaddr_in *) sa)->sin_addr.s_addr);
        if ((a & 0xfffe0000u) != PLACEHOLDER4) {
            return 0;
        }
        n = a - PLACEHOLDER4;
    } else if (sa->sa_family == AF_INET6 && len >= sizeof(struct sockaddr_in6)) {
        const uint8_t *b = ((const struct sockaddr_in6 *) sa)->sin6_addr.s6_addr;
        static const uint8_t mapped[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};
        if (memcmp(b, mapped, sizeof mapped) == 0) {
            const uint32_t a = (uint32_t) b[12] << 24 | (uint32_t) b[13] << 16
                               | (uint32_t) b[14] << 8 | b[15];
            if ((a & 0xfffe0000u) != PLACEHOLDER4) {
                return 0;
            }
            n = a - PLACEHOLDER4;
        } else if (memcmp(b, PLACEHOLDER6, sizeof PLACEHOLDER6) == 0) {
            for (int i = 6; i < 14; i++) {
                if (b[i]) {
                    return 0;
                }
            }
            n = (uint32_t) b[14] << 8 | b[15];
        } else {
            return 0;
        }
    } else {
        return 0;
    }
    return n >= 1 && n <= (uint32_t) atomic_load(&name_count) ? (int) n : 0;
}

/* The placeholder for `name`, made the first time it is asked for; 0 when
   there is no room left. Locked. */
static int placeholder_for(const char *name) {
    const int count = atomic_load(&name_count);
    for (int i = 0; i < count; i++) {
        if (strcasecmp(names[i], name) == 0) {
            return i + 1;
        }
    }
    if (count == MAX_NAMES || !(names[count] = strdup(name))) {
        return 0;
    }
    atomic_store(&name_count, count + 1);
    return count + 1;
}

static bool same_host(const char *a, const char *b) {
    unsigned char x[16], y[16];
    if (inet_pton(AF_INET, a, x) == 1) {
        return inet_pton(AF_INET, b, y) == 1 && memcmp(x, y, 4) == 0;
    }
    if (inet_pton(AF_INET6, a, x) == 1) {
        return inet_pton(AF_INET6, b, y) == 1 && memcmp(x, y, 16) == 0;
    }
    return strcasecmp(a, b) == 0;
}

/* The host part of an address as the core takes it: `[literal]...`,
   `host::port`, `host:display` or `host`. Anything else with more than one
   colon is taken as an IPv6 literal. */
static void host_of(const char *address, char *out, size_t len) {
    const char *start = address;
    const char *end;
    if (*address == '[') {
        start = address + 1;
        end = strchr(start, ']');
        if (!end) {
            end = start + strlen(start);
        }
    } else {
        const char *port = NULL;
        for (const char *p = strstr(address, "::"); p; p = strstr(p + 1, "::")) {
            if (p[2] && strspn(p + 2, "0123456789") == strlen(p + 2)) {
                port = p;
            }
        }
        const char *colon = strchr(address, ':');
        end = port && port != address ? port
              : colon && !strchr(colon + 1, ':') ? colon
                                                 : address + strlen(address);
    }
    snprintf(out, len, "%.*s", (int) (end - start), start);
}

static int port_of(const struct sockaddr *sa) {
    return sa->sa_family == AF_INET6 ? ntohs(((const struct sockaddr_in6 *) sa)->sin6_port)
                                     : ntohs(((const struct sockaddr_in *) sa)->sin_port);
}

static void text_of(const struct sockaddr *sa, char *out, size_t len) {
    const void *a = sa->sa_family == AF_INET6
                            ? (const void *) &((const struct sockaddr_in6 *) sa)->sin6_addr
                            : (const void *) &((const struct sockaddr_in *) sa)->sin_addr;
    if (!inet_ntop(sa->sa_family, a, out, (socklen_t) len)) {
        snprintf(out, len, "?");
    }
}

/* `text` and `port` as a socket address of `text`'s family, or, where
   `text` is empty or not numeric, the unspecified address of `family`. */
static void sockaddr_of(const char *text, int port, int family, struct sockaddr_storage *out,
                        socklen_t *len) {
    memset(out, 0, sizeof *out);
    struct sockaddr_in *v4 = (struct sockaddr_in *) out;
    struct sockaddr_in6 *v6 = (struct sockaddr_in6 *) out;
    if (text[0] && inet_pton(AF_INET, text, &v4->sin_addr) == 1) {
        family = AF_INET;
    } else if (text[0] && inet_pton(AF_INET6, text, &v6->sin6_addr) == 1) {
        family = AF_INET6;
    } else {
        port = 0;
    }
    if (family == AF_INET6) {
        v6->sin6_family = AF_INET6;
        v6->sin6_port = htons((uint16_t) port);
        *len = sizeof *v6;
    } else {
        v4->sin_family = AF_INET;
        v4->sin_port = htons((uint16_t) port);
        *len = sizeof *v4;
    }
}


// ---- routes ------------------------------------------------------------------

static struct route *route_by_id(int id) {
    for (int i = 0; i < MAX_ROUTES; i++) {
        if (id && routes[i].id == id) {
            return &routes[i];
        }
    }
    return NULL;
}

/* Whether two routes carry a connection differently: one of them dials
   through a network and the other does not, or they go through different
   tunnels. Two sessions to the same host on such routes cannot be told
   apart by the hooks (`realvnc_net.h`), so they never run at once. */
static bool distinct(const struct route *a, const struct route *b) {
    return a->has_net != b->has_net || strcmp(a->key, b->key) != 0;
}

/* A route that can still be dialled for. Locked. */
static bool live(const struct route *r) {
    return r->id && !r->detaching && !r->closed;
}

/* Whether a session on a direct route is open, whose own lookups give the
   core addresses that are not placeholders. Locked. */
static bool any_direct(void) {
    for (int i = 0; i < MAX_ROUTES; i++) {
        if (live(&routes[i]) && !routes[i].has_net) {
            return true;
        }
    }
    return false;
}

enum decision {
    DECIDE_ROUTE,  // a placeholder, dialled through `*out`
    DECIDE_PASS,   // the core's own call: a direct session's
    DECIDE_REFUSE, // neither: `why` says why
};

/* Which session a lookup of `host` is for (`realvnc_net.h` gives the rules).
   Locked. */
static enum decision lookup_route(const char *host, struct route **out, char *why, size_t why_len) {
    struct route *same = NULL, *latest = NULL;
    bool own_direct = false, other_direct = false, one_tunnel = true;
    for (int i = 0; i < MAX_ROUTES; i++) {
        struct route *r = &routes[i];
        if (!live(r)) {
            continue;
        }
        const bool mine = same_host(r->host, host);
        if (!r->has_net) {
            own_direct |= mine;
            other_direct |= !mine;
            continue;
        }
        // Several sessions to one host are all on the same route
        // (`realvnc_net_attach` refuses the rest): the latest is as good as
        // any.
        if (mine && (!same || r->order > same->order)) {
            same = r;
        }
        one_tunnel &= !latest || strcmp(latest->key, r->key) == 0;
        if (!latest || r->order > latest->order) {
            latest = r;
        }
    }
    *out = same;
    if (same) {
        return DECIDE_ROUTE;
    }
    if (own_direct) {
        return DECIDE_PASS;
    }
    if (latest && !other_direct && one_tunnel) {
        // A name no session has, while every open session goes through the
        // same tunnel: the core's own, which that tunnel carries too.
        *out = latest;
        return DECIDE_ROUTE;
    }
    snprintf(why, why_len, "%s",
             !latest       ? "no tunnelled session is open"
             : other_direct ? "not a session's server, and a direct session is open too"
                            : "not a session's server, and sessions use more than one tunnel");
    return DECIDE_REFUSE;
}

/* The route a connect to placeholder `n` is dialled through: its latest
   lookup's, where that session is still open, else the latest open session
   whose host it stands for; NULL for none. Locked. */
static struct route *placeholder_route(int n) {
    struct route *r = route_by_id(name_route[n - 1]);
    if (r && live(r) && r->has_net) {
        return r;
    }
    r = NULL;
    for (int i = 0; i < MAX_ROUTES; i++) {
        struct route *o = &routes[i];
        if (live(o) && o->has_net && same_host(o->host, names[n - 1]) && (!r || o->order > r->order)) {
            r = o;
        }
    }
    return r;
}

void realvnc_net_set_notice(void (*notice)(const char *text)) {
    atomic_store(&notice_fn, notice);
}

void realvnc_net_installed(int ok, const char *why) {
    pthread_mutex_lock(&lock);
    installed = ok ? 1 : -1;
    snprintf(install_failure, sizeof install_failure, "%s", ok || !why ? "" : why);
    pthread_mutex_unlock(&lock);
}

int realvnc_net_attach(const vento_net *net, const char *key, const char *address, int timeout_ms,
                       char *why, size_t why_len) {
    char reason[600] = "";
    struct route want = {0};
    want.has_net = net != NULL;
    snprintf(want.key, sizeof want.key, "%s", net && key ? key : "");
    host_of(address ? address : "", want.host, sizeof want.host);
    if (!want.host[0]) {
        snprintf(want.host, sizeof want.host, "%s", address ? address : "");
    }
    pthread_mutex_lock(&lock);
    struct route *r = NULL;
    if (net && installed != 1) {
        snprintf(reason, sizeof reason, "the core's socket calls are not hooked (%s)",
                 installed < 0 && install_failure[0] ? install_failure : "not installed");
    } else {
        for (int i = 0; i < MAX_ROUTES && !reason[0]; i++) {
            const struct route *o = &routes[i];
            if (live(o) && distinct(o, &want) && same_host(o->host, want.host)) {
                snprintf(reason, sizeof reason,
                         "a session to %s is already open with another tunnel setting, and RealVNC cannot "
                         "differentiate connections to the same hostname internally to be routed to the "
                         "correct tunnel",
                         want.host);
            }
        }
        for (int i = 0; i < MAX_ROUTES && !r && !reason[0]; i++) {
            if (!routes[i].id) {
                r = &routes[i];
            }
        }
        if (!r && !reason[0]) {
            snprintf(reason, sizeof reason, "too many sessions");
        }
    }
    if (!r) {
        pthread_mutex_unlock(&lock);
        vento_log(VENTO_LOG_WARN, TAG, "%s: not attached: %s", address ? address : "", reason);
        if (why && why_len) {
            snprintf(why, why_len, "%s", reason);
        }
        if (net && net->release) {
            net->release(net->user);
        }
        return 0;
    }
    *r = want;
    r->id = ++last_id;
    r->order = ++last_order;
    r->stop[0] = r->stop[1] = -1;
    if (net) {
        r->net = *net;
        atomic_fetch_add(&relayed_routes, 1);
        if (pipe2(r->stop, O_CLOEXEC) != 0) {
            // Relays then end only as the core's socket or the server's side
            // ends.
            r->stop[0] = r->stop[1] = -1;
        }
    }
    r->timeout_ms = timeout_ms;
    const int id = r->id;
    pthread_mutex_unlock(&lock);
    return id;
}

void realvnc_net_closed(int id) {
    pthread_mutex_lock(&lock);
    struct route *r = route_by_id(id);
    if (r) {
        r->closed = true;
    }
    pthread_mutex_unlock(&lock);
}

/* `net.cancel` on route `r`, with the lock let go meanwhile. Locked. */
static void cancel_locked(struct route *r) {
    if (!r->has_net || !r->net.cancel) {
        return;
    }
    r->busy++;
    const vento_net net = r->net;
    pthread_mutex_unlock(&lock);
    net.cancel(net.user);
    pthread_mutex_lock(&lock);
    r->busy--;
    pthread_cond_broadcast(&idle);
}

void realvnc_net_cancel(int id) {
    pthread_mutex_lock(&lock);
    struct route *r = route_by_id(id);
    if (r) {
        cancel_locked(r);
    }
    pthread_mutex_unlock(&lock);
}

void realvnc_net_detach(int id) {
    pthread_mutex_lock(&lock);
    struct route *r = route_by_id(id);
    if (!r) {
        pthread_mutex_unlock(&lock);
        return;
    }
    r->detaching = true;
    // A dial still in progress ends now, rather than at its timeout.
    cancel_locked(r);
    while (r->busy) {
        pthread_cond_wait(&idle, &lock);
    }
    const bool has_net = r->has_net;
    const vento_net net = r->net;
    for (int i = 0; i < r->parked_count; i++) {
        close(r->parked[i]);
    }
    // Every relay of the route's sees its copy of the read end hang up and
    // ends: a connection the core has not closed (or whose last bytes the
    // server is not taking) is not left running for a session that is gone.
    for (int i = 0; i < 2; i++) {
        if (r->stop[i] >= 0) {
            close(r->stop[i]);
        }
    }
    memset(r, 0, sizeof *r);
    if (has_net) {
        atomic_fetch_sub(&relayed_routes, 1);
    }
    pthread_mutex_unlock(&lock);
    if (has_net && net.release) {
        net.release(net.user);
    }
}

int realvnc_net_relayed(int id) {
    pthread_mutex_lock(&lock);
    const struct route *r = route_by_id(id);
    const int relayed = r && r->has_net;
    pthread_mutex_unlock(&lock);
    return relayed;
}

// ---- bridged sockets ---------------------------------------------------------

/* The entry for `fd`, dropping one whose number has been reused. Locked. */
static struct bridged *bridged_find(int fd) {
    for (int i = 0; i < MAX_BRIDGED; i++) {
        struct bridged *b = &bridged[i];
        if (!b->used || b->fd != fd) {
            continue;
        }
        struct stat st;
        if (fstat(fd, &st) == 0 && st.st_dev == b->dev && st.st_ino == b->ino) {
            return b;
        }
        b->used = false;
        atomic_fetch_sub(&bridged_count, 1);
    }
    return NULL;
}

/* Records `fd` as bridged; false where there is no room, even after
   dropping the entries of closed sockets. Locked. */
static struct bridged *bridged_add(int fd, const struct sockaddr *peer, socklen_t peer_len,
                                   const vento_net_peer *route_peer, int route, int kind) {
    struct stat st;
    if (fstat(fd, &st) != 0) {
        return NULL;
    }
    struct bridged *slot = NULL;
    for (int i = 0; i < MAX_BRIDGED; i++) {
        struct bridged *b = &bridged[i];
        if (b->used) {
            struct stat now;
            if (b->fd == fd || fstat(b->fd, &now) != 0 || now.st_dev != b->dev
                || now.st_ino != b->ino) {
                b->used = false;
                atomic_fetch_sub(&bridged_count, 1);
            }
        }
        if (!b->used && !slot) {
            slot = b;
        }
    }
    if (!slot) {
        return NULL;
    }
    memset(slot, 0, sizeof *slot);
    slot->used = true;
    slot->fd = fd;
    slot->dev = st.st_dev;
    slot->ino = st.st_ino;
    if (peer_len > sizeof slot->peer) {
        peer_len = sizeof slot->peer;
    }
    memcpy(&slot->peer, peer, peer_len);
    slot->peer_len = peer_len;
    sockaddr_of(route_peer->local_address, route_peer->local_port, peer->sa_family, &slot->local,
                &slot->local_len);
    slot->route = route;
    slot->connection = route_peer->connection;
    slot->kind = kind;
    atomic_fetch_add(&bridged_count, 1);
    return slot;
}

static int copy_out(const struct sockaddr_storage *have, socklen_t have_len, struct sockaddr *out,
                    socklen_t *len) {
    if (!out || !len) {
        errno = EFAULT;
        return -1;
    }
    memcpy(out, have, *len < have_len ? *len : have_len);
    *len = have_len;
    return 0;
}

// ---- carrying a dialled connection to the core's socket -------------------------

/* How long a relay goes on passing the core's last bytes to a server that
   is not taking them, once the core has closed its socket: as
   `runtime::net::socketpair::FLUSH`. */
#define FLUSH_MS 10000

/* Copies between `near` (the socketpair end the core's socket is the other
   end of) and `far` (the network's fd) until both directions have ended, the
   core has closed its socket, or `stop` (the route's, which `detach` closes)
   hangs up, passing on a half-close either way. Once the core's end reads
   nothing more (closed, or shut for reading), what comes down is dropped,
   and what the relay still holds for the server is passed on for at most
   FLUSH_MS without progress. */
static void relay(int near, int far, int stop) {
    char up[16384], down[16384];
    size_t up_at = 0, up_len = 0, down_at = 0, down_len = 0;
    bool up_eof = false, down_eof = false, up_done = false, down_done = false;
    bool near_gone = false;
    while (!(up_done && down_done)) {
        short near_events = 0, far_events = 0;
        if (!up_eof && up_at == up_len) {
            near_events |= POLLIN;
        }
        if (down_at < down_len) {
            near_events |= POLLOUT;
        }
        if (!down_eof && down_at == down_len) {
            far_events |= POLLIN;
        }
        if (up_at < up_len) {
            far_events |= POLLOUT;
        }
        // The core's end is polled for its hang-up until it is known, also
        // with nothing else to wait for there, so that a close while the
        // relay holds bytes for a server not taking them starts the flush
        // time; once known it is left out, or its POLLHUP would wake the
        // loop on every turn.
        struct pollfd p[3] = {{near_events || !near_gone ? near : -1, near_events, 0},
                              {far_events ? far : -1, far_events, 0},
                              {stop, POLLIN, 0}};
        const int ready = poll(p, 3, near_gone && up_at < up_len ? FLUSH_MS : -1);
        if (ready < 0) {
            if (errno == EINTR) {
                continue;
            }
            return;
        }
        if (ready == 0) {
            return; // the server did not take the core's last bytes in time
        }
        if (p[2].revents) {
            return; // the route is detached
        }
        if ((p[0].revents | p[1].revents) & (POLLERR | POLLNVAL)) {
            // The server's side failed (the route records why), or the core
            // closed its socket with bytes unread.
            return;
        }
        if ((near_events & POLLIN) && (p[0].revents & (POLLIN | POLLHUP))) {
            const ssize_t n = recv(near, up, sizeof up, MSG_DONTWAIT);
            if (n == 0) {
                up_eof = true;
            } else if (n > 0) {
                up_at = 0;
                up_len = (size_t) n;
            } else if (errno != EAGAIN && errno != EINTR) {
                return;
            }
        }
        if ((far_events & POLLIN) && (p[1].revents & (POLLIN | POLLHUP))) {
            const ssize_t n = recv(far, down, sizeof down, MSG_DONTWAIT);
            if (n == 0) {
                down_eof = true;
            } else if (n > 0) {
                down_at = 0;
                down_len = (size_t) n;
            } else if (errno != EAGAIN && errno != EINTR) {
                return;
            }
        }
        if (down_at < down_len && (p[0].revents & (POLLOUT | POLLHUP))) {
            const ssize_t n = send(near, down + down_at, down_len - down_at, MSG_DONTWAIT | MSG_NOSIGNAL);
            if (n < 0 && errno != EAGAIN && errno != EINTR) {
                near_gone = true; // closed or shut for reading
            }
            down_at += n > 0 ? (size_t) n : 0;
        }
        if (up_at < up_len && (p[1].revents & (POLLOUT | POLLHUP))) {
            const ssize_t n = send(far, up + up_at, up_len - up_at, MSG_DONTWAIT | MSG_NOSIGNAL);
            if (n < 0 && errno != EAGAIN && errno != EINTR) {
                return;
            }
            up_at += n > 0 ? (size_t) n : 0;
        }
        if (p[0].revents & POLLHUP) {
            near_gone = true;
        }
        if (near_gone && !down_done) {
            down_at = down_len;
            down_eof = true;
            down_done = true;
        }
        if (up_eof && up_at == up_len && !up_done) {
            shutdown(far, SHUT_WR);
            up_done = true;
        }
        if (down_eof && down_at == down_len && !down_done) {
            shutdown(near, SHUT_WR);
            down_done = true;
        }
    }
}

// ---- dials -------------------------------------------------------------------

/* The errno a failed dial gives the core's `connect`. */
static int errno_for(int code) {
    switch (code) {
        case VENTO_NET_EREFUSED: return ECONNREFUSED;
        case VENTO_NET_ETIMEDOUT: return ETIMEDOUT;
        case VENTO_NET_EUNREACHABLE: return EHOSTUNREACH;
        // The route could not resolve the name: the core looked it up and
        // was given a placeholder, so this is where it learns of it.
        case VENTO_NET_ERESOLVE: return EHOSTUNREACH;
        case VENTO_NET_ECANCELLED: return ECANCELED;
        default: return ECONNABORTED;
    }
}

/* What `later_dial` needs. */
struct later {
    int route;
    vento_net net;
    int timeout_ms;
    int kind; // SOCK_STREAM or SOCK_DGRAM
    char host[256];
    int port;
    int fd; // the core's socket, and which file it is
    dev_t dev;
    ino_t ino;
    int near;      // the socketpair end the thread keeps; the core has the other
    int core_end;  // streams: a second fd for the core's end, kept until the dial is done
    size_t filled; // streams: what was written from the core's end to keep it from polling writable
    int sndbuf;    // streams: the core's end's SO_SNDBUF before it was shrunk, as set
    int stop;      // the route's `stop` read end, which hangs up once the route is detached
};

/* Copies datagrams between `near` (a SOCK_SEQPACKET end the core's UDP
   socket is the other end of) and `far` (the network's datagram fd), one
   per send, until either ends: the core closing its socket, or the network
   no longer carrying them, which closing `near` then tells the core (its
   `recvmsg` and `sendmsg` hooks ask the route why). A datagram that cannot
   be passed on at once, or is longer than the buffer, is dropped, as UDP
   drops it. The core shutting its socket for writing ends only what it
   sends. `stop` hanging up (the route detached) ends it too. */
static void relay_datagrams(int near, int far, int stop) {
    char buf[65536];
    bool near_done = false;
    for (;;) {
        struct pollfd p[3] = {{near, (short) (near_done ? 0 : POLLIN | POLLRDHUP), 0},
                              {far, POLLIN | POLLRDHUP, 0},
                              {stop, POLLIN, 0}};
        if (poll(p, 3, -1) < 0) {
            if (errno == EINTR) {
                continue;
            }
            return;
        }
        if (p[2].revents || ((p[0].revents | p[1].revents) & POLLNVAL)) {
            return;
        }
        const bool near_gone = p[0].revents & (POLLHUP | POLLERR);
        if (near_gone && near_done) {
            return;
        }
        const bool near_shut = near_gone || (p[0].revents & POLLRDHUP);
        if (!near_done && (p[0].revents & POLLIN || near_shut)) {
            const ssize_t n = recv(near, buf, sizeof buf, MSG_DONTWAIT | MSG_TRUNC);
            if ((n == 0 || (n < 0 && errno == EAGAIN)) && near_shut) {
                if (near_gone) {
                    return;
                }
                near_done = true;
            } else if (n < 0 && errno != EAGAIN && errno != EINTR) {
                return;
            } else if (n >= 0 && (size_t) n <= sizeof buf
                       && send(far, buf, (size_t) n, MSG_DONTWAIT | MSG_NOSIGNAL) < 0 && errno != EAGAIN
                       && errno != ENOBUFS && errno != EINTR && errno != EMSGSIZE) {
                return; // the network has stopped carrying them
            }
        }
        if (p[1].revents & (POLLIN | POLLRDHUP | POLLHUP | POLLERR)) {
            const ssize_t n = recv(far, buf, sizeof buf, MSG_DONTWAIT | MSG_TRUNC);
            if (n == 0 && (p[1].revents & (POLLHUP | POLLRDHUP))) {
                return;
            }
            if (n < 0 && errno != EAGAIN && errno != EINTR) {
                return; // an error from the server's side, which the route recorded
            }
            if (n >= 0 && (size_t) n <= sizeof buf
                && send(near, buf, (size_t) n, MSG_DONTWAIT | MSG_NOSIGNAL) < 0 && errno == EPIPE) {
                return; // the core has closed its socket
            }
        }
    }
}

/* `dial_later`'s thread: the dial, its answer to the core, and then the
   bytes or the datagrams. */
static void *later_dial(void *arg) {
    struct later *a = arg;
    vento_net_peer peer;
    memset(&peer, 0, sizeof peer);
    char why[200] = "";
    int got;
    if (a->kind == SOCK_DGRAM) {
        got = a->net.connect_datagram
                      ? a->net.connect_datagram(a->net.user, a->host, a->port, a->timeout_ms, &peer, why,
                                                sizeof why)
                      : VENTO_NET_ENOTSUP;
        if (got == VENTO_NET_ENOTSUP && !why[0]) {
            snprintf(why, sizeof why, "the session's network carries streams only");
        }
    } else {
        // No liveness: the core sets no TCP keep-alive on its socket (it has
        // its own, in RFB), and what it sets after `connect` lands on the
        // socketpair's end.
        got = a->net.connect(a->net.user, a->host, a->port, a->timeout_ms, NULL, &peer, why, sizeof why);
    }
    bool parked = false;
    pthread_mutex_lock(&lock);
    struct route *r = route_by_id(a->route);
    if (got == VENTO_NET_ECANCELLED && r && r->parked_count < MAX_PARKED) {
        // Ended by the disconnect: a stream's connect stays in progress (its
        // end stays unwritable) and a UDP socket stays open with nothing
        // coming, as a direct one is left when the core closes the session
        // during it, with this end kept until the route is detached.
        // Otherwise the core would report the cancelled dial as a failed
        // connection, with a message, after the disconnect.
        r->parked[r->parked_count++] = a->near;
        parked = true;
    }
    int sndbuf = a->sndbuf;
    for (int i = 0; i < MAX_BRIDGED; i++) {
        struct bridged *b = &bridged[i];
        if (b->used && b->fd == a->fd && b->dev == a->dev && b->ino == a->ino) {
            b->so_error = got >= 0 ? 0 : errno_for(got);
            b->dialling = false;
            sndbuf = b->sndbuf > 0 ? b->sndbuf : sndbuf;
            if (got >= 0) {
                b->connection = peer.connection;
                sockaddr_of(peer.local_address, peer.local_port, b->peer.ss_family, &b->local,
                            &b->local_len);
            }
        }
    }
    if (r) {
        r->busy--;
        pthread_cond_broadcast(&idle);
    }
    pthread_mutex_unlock(&lock);
    if (a->core_end >= 0) {
        if (!parked) {
            // The core's end gets its send buffer back, here rather than in
            // a hook, so that nothing the core asks before the dial is done
            // makes it writable early; SO_ERROR already has its answer.
            next_setsockopt(a->core_end, SOL_SOCKET, SO_SNDBUF, &sndbuf, sizeof sndbuf);
        }
        // Not kept past this: the core's close of its socket must reach
        // `near`.
        close(a->core_end);
    }
    const char *what = a->kind == SOCK_DGRAM ? "UDP" : "TCP";
    if (parked) {
        vento_log(VENTO_LOG_INFO, TAG, "%s:%d: %s connect ended by the disconnect", a->host, a->port, what);
    } else {
        if (got >= 0) {
            vento_log(VENTO_LOG_INFO, TAG, "%s:%d: %s connected through the session's network (%s)", a->host,
                      a->port, what, peer.route == VENTO_NET_TUNNEL ? "tunnel" : "direct");
        } else {
            vento_log(VENTO_LOG_INFO, TAG, "%s:%d: %s connect through the session's network failed: %s",
                      a->host, a->port, what, why);
        }
        // What was written to keep the core's socket from polling writable
        // is taken back, so that it does: its connect is done, and SO_ERROR
        // says how.
        char sink[4096];
        for (size_t left = a->filled; left > 0;) {
            const ssize_t n = read(a->near, sink, left < sizeof sink ? left : sizeof sink);
            if (n <= 0 && errno != EINTR) {
                break;
            }
            left -= n > 0 ? (size_t) n : 0;
        }
        if (got >= 0 && a->kind == SOCK_DGRAM) {
            relay_datagrams(a->near, got, a->stop);
        } else if (got >= 0) {
            relay(a->near, got, a->stop);
        }
        // A failed dial's datagrams: the core reads the end of file, which
        // its `recvmsg` hook answers as a refusal, and falls back to TCP.
        close(a->near);
    }
    if (got >= 0) {
        close(got);
    }
    if (a->stop >= 0) {
        close(a->stop);
    }
    free(a);
    return NULL;
}

/* The core's `connect` of `fd` (of `kind`, SOCK_STREAM or SOCK_DGRAM) to a
   placeholder for `host`, dialled through `r` on a thread of its own, so
   that the core's thread, which every session shares, never waits for a
   dial. `connect` returns at once, as the core's own does: EINPROGRESS for
   a stream (the core's sockets are non-blocking; a blocking one is
   refused), 0 for UDP, whose connect sends nothing.

   The core's socket must not be replaced once `connect` has returned (its
   looper registers it then, an epoll registration that belongs to the open
   file, not the number), so it is replaced now with one end of a socketpair
   whose other end the thread keeps (SOCK_SEQPACKET for UDP, which keeps a
   datagram's boundaries), keeping the socket's status and close-on-exec
   flags; the thread later copies between that end and what the network
   dialled. Datagrams the core sends meanwhile wait in the socketpair.

   A stream's end must not poll writable until the dial is done, which
   would tell the core it is connected: its send buffer is made as small as
   the system allows and filled from its own side, and the thread reads that
   back once the dial is done, which makes it writable; SO_ERROR
   (`hook_getsockopt`) then answers the dial's error or 0. The thread puts
   the buffer back itself, through a second fd for the core's end, once the
   dial is done: a hook answering the core (`getpeername`, say) must not
   make the socket writable before then, and an SO_SNDBUF the core sets
   meanwhile is kept for then.

   Locked on entry, unlocked on return; returns `connect`'s answer, with
   errno set. */
static int dial_later(struct route *r, int fd, int kind, const struct sockaddr *addr, socklen_t len,
                      const char *host, int port) {
    struct later *a = calloc(1, sizeof *a);
    if (a) {
        a->stop = -1;
    }
    int sv[2] = {-1, -1};
    int core_end = -1;
    const int status = fcntl(fd, F_GETFL);
    const int fd_flags = fcntl(fd, F_GETFD);
    int err = !a ? ENOMEM : status < 0 || fd_flags < 0 ? EBADF : 0;
    if (!err && kind == SOCK_STREAM && !(status & O_NONBLOCK)) {
        err = EOPNOTSUPP; // the core's are non-blocking; a blocking one would wait here
    }
    if (!err && socketpair(AF_UNIX, (kind == SOCK_DGRAM ? SOCK_SEQPACKET : SOCK_STREAM) | SOCK_CLOEXEC, 0, sv) != 0) {
        err = errno;
    }
    int sndbuf = 0;
    socklen_t sndbuf_len = sizeof sndbuf;
    size_t filled = 0;
    if (!err && kind == SOCK_STREAM) {
        const int smallest = 1;
        next_getsockopt(sv[0], SOL_SOCKET, SO_SNDBUF, &sndbuf, &sndbuf_len);
        next_setsockopt(sv[0], SOL_SOCKET, SO_SNDBUF, &smallest, sizeof smallest);
        static const char fill[256] = {0};
        for (;;) {
            const ssize_t n = send(sv[0], fill, sizeof fill, MSG_DONTWAIT | MSG_NOSIGNAL);
            if (n <= 0) {
                break;
            }
            filled += (size_t) n;
        }
        struct pollfd p = {sv[0], POLLOUT, 0};
        if (filled == 0 || poll(&p, 1, 0) != 0) {
            err = EIO; // still writable: the trick does not work here
        } else if ((core_end = fcntl(sv[0], F_DUPFD_CLOEXEC, 0)) < 0) {
            err = errno;
        }
    }
    if (!err && (fcntl(sv[0], F_SETFL, status) != 0
                 || dup3(sv[0], fd, (fd_flags & FD_CLOEXEC) ? O_CLOEXEC : 0) < 0)) {
        err = errno;
    }
    struct stat st;
    struct bridged *b = NULL;
    if (!err && fstat(fd, &st) == 0) {
        vento_net_peer none;
        memset(&none, 0, sizeof none);
        b = bridged_add(fd, addr, len, &none, r->id, kind);
    }
    if (!err && !b) {
        err = EMFILE;
    }
    if (!err) {
        b->dialling = true;
        a->route = r->id;
        a->net = r->net;
        a->timeout_ms = r->timeout_ms;
        a->kind = kind;
        snprintf(a->host, sizeof a->host, "%s", host);
        a->port = port;
        a->fd = fd;
        a->dev = st.st_dev;
        a->ino = st.st_ino;
        a->near = sv[1];
        a->core_end = core_end;
        a->filled = filled;
        a->sndbuf = sndbuf / 2; // as SO_SNDBUF reads it: twice what was set
        a->stop = r->stop[0] >= 0 ? fcntl(r->stop[0], F_DUPFD_CLOEXEC, 0) : -1;
        r->busy++;
        pthread_attr_t attr;
        pthread_t thread;
        pthread_attr_init(&attr);
        pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
        err = pthread_create(&thread, &attr, later_dial, a);
        pthread_attr_destroy(&attr);
        if (err) {
            r->busy--;
            pthread_cond_broadcast(&idle);
            b->used = false;
            atomic_fetch_sub(&bridged_count, 1);
        }
    }
    pthread_mutex_unlock(&lock);
    if (sv[0] >= 0) {
        close(sv[0]);
    }
    if (err) {
        // The thread was not started: what it would have owned is closed here.
        if (sv[1] >= 0) {
            close(sv[1]);
        }
        if (core_end >= 0) {
            close(core_end);
        }
        if (a && a->stop >= 0) {
            close(a->stop);
        }
        free(a);
        vento_log(VENTO_LOG_WARN, TAG, "%s:%d: could not start a dial: %s", host, port, strerror(err));
        errno = err == EOPNOTSUPP ? ENETUNREACH : err;
        return -1;
    }
    vento_log(VENTO_LOG_INFO, TAG, "%s:%d: %s connect through the session's network", host, port,
              kind == SOCK_DGRAM ? "UDP" : "TCP");
    if (kind == SOCK_DGRAM) {
        return 0;
    }
    errno = EINPROGRESS;
    return -1;
}

// ---- the hooks ---------------------------------------------------------------

static int hook_connect(int fd, const struct sockaddr *addr, socklen_t len) {
    if (atomic_load(&relayed_routes) == 0 || !addr
        || (addr->sa_family != AF_INET && addr->sa_family != AF_INET6)) {
        return next_connect(fd, addr, len);
    }
    char host[256];
    const int port = port_of(addr);
    pthread_mutex_lock(&lock);
    const int n = placeholder_index(addr, len);
    if (!n) {
        // An address of the core's own: a direct session's (its own lookup
        // gave it), never one that goes through a network, whose lookups
        // give placeholders.
        const bool direct = any_direct();
        pthread_mutex_unlock(&lock);
        if (direct) {
            return next_connect(fd, addr, len);
        }
        text_of(addr, host, sizeof host);
        vento_log(VENTO_LOG_WARN, TAG, "%s:%d: connect refused: not a placeholder, and no direct session is open",
                  host, port);
        errno = ENETUNREACH;
        return -1;
    }
    snprintf(host, sizeof host, "%s", names[n - 1]);
    struct route *r = placeholder_route(n);
    pthread_mutex_unlock(&lock);
    int type = 0;
    socklen_t type_len = sizeof type;
    const char *why = !r ? "no tunnelled session for it"
                      : next_getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &type_len) != 0 ? strerror(errno)
                      : type != SOCK_STREAM && type != SOCK_DGRAM ? "not a TCP or UDP socket"
                                                                   : NULL;
    if (why) {
        vento_log(VENTO_LOG_WARN, TAG, "%s:%d: connect refused: %s", host, port, why);
        errno = ENETUNREACH;
        return -1;
    }
    pthread_mutex_lock(&lock);
    // Looked up again: the route may have gone while the lock was let go.
    r = placeholder_route(n);
    if (!r) {
        pthread_mutex_unlock(&lock);
        vento_log(VENTO_LOG_WARN, TAG, "%s:%d: connect refused: no tunnelled session for it", host, port);
        errno = ENETUNREACH;
        return -1;
    }
    return dial_later(r, fd, type, addr, len, host, port);
}

static bool numeric(const char *host) {
    unsigned char a[16];
    return inet_pton(AF_INET, host, a) == 1 || inet_pton(AF_INET6, host, a) == 1;
}

static int hook_getaddrinfo(const char *node, const char *service, const void *hints_,
                            void **res) {
    const struct bionic_addrinfo *hints = hints_;
    const int flags = hints ? hints->ai_flags : 0;
    if (atomic_load(&relayed_routes) == 0 || !node || !res) {
        return next_getaddrinfo(node, service, hints_, res);
    }
    char why[200];
    pthread_mutex_lock(&lock);
    struct route *r;
    const enum decision d = lookup_route(node, &r, why, sizeof why);
    if (d == DECIDE_REFUSE) {
        pthread_mutex_unlock(&lock);
        vento_log(VENTO_LOG_WARN, TAG, "%s: lookup refused: %s", node, why);
        // The hooks cannot tell which session the lookup is for, so it is
        // refused rather than guessed; the platform shows that it happened.
        char text[320];
        snprintf(text, sizeof text, "bug: unhandled realvnc host lookup %s", node);
        vento_log(VENTO_LOG_WARN, TAG, "%s", text);
        void (*notice)(const char *) = atomic_load(&notice_fn);
        if (notice) {
            notice(text);
        }
        return BIONIC_EAI_FAIL;
    }
    if (d == DECIDE_PASS) {
        pthread_mutex_unlock(&lock);
        return next_getaddrinfo(node, service, hints_, res);
    }
    if ((flags & BIONIC_AI_NUMERICHOST) && !numeric(node)) {
        // As a name fails it: the core then looks it up again on its
        // resolver thread.
        pthread_mutex_unlock(&lock);
        return BIONIC_EAI_NONAME;
    }
    long port = 0;
    if (service && service[0]) {
        char *end;
        port = strtol(service, &end, 10);
        if (*end || port < 0 || port > 65535) {
            pthread_mutex_unlock(&lock);
            return BIONIC_EAI_SERVICE;
        }
    }
    const int family = hints ? hints->ai_family : AF_UNSPEC;
    if (family != AF_UNSPEC && family != AF_INET && family != AF_INET6) {
        pthread_mutex_unlock(&lock);
        return BIONIC_EAI_FAMILY;
    }
    if (!(flags & BIONIC_AI_NUMERICHOST) && !numeric(node) && r->net.resolve) {
        // A name is resolved through the route now, without connecting (a
        // route that resolves only as it connects answers ENOTSUP, and the
        // connect finds out), so that a name with no address fails here, as
        // it does on a direct route: the core then says the name could not
        // be resolved rather than that the computer could not be contacted.
        // The connect resolves the name again (`vento_net.connect` takes
        // the name, not an address), so a connect costs two lookups, and a
        // name that goes away between the two fails the connect instead.
        // Held busy meanwhile, so that `detach` ends the lookup and waits
        // for it, and `r` stays.
        const vento_net net = r->net;
        const int timeout_ms = r->timeout_ms;
        r->busy++;
        pthread_mutex_unlock(&lock);
        const int rc = net.resolve(net.user, node, timeout_ms, why, sizeof why);
        pthread_mutex_lock(&lock);
        r->busy--;
        pthread_cond_broadcast(&idle);
        if (rc != 0 && rc != VENTO_NET_ENOTSUP) {
            pthread_mutex_unlock(&lock);
            vento_log(VENTO_LOG_INFO, TAG, "%s: lookup through the session's network failed: %s", node, why);
            // A lookup ended by the session's disconnect fails the same
            // way: the core closes the session before it can report it.
            return rc == VENTO_NET_ERESOLVE || rc == VENTO_NET_ECANCELLED ? BIONIC_EAI_NONAME
                                                                           : BIONIC_EAI_FAIL;
        }
    }
    // Nothing is dialled here: the connect to the placeholder dials, for
    // whichever transport it is (the core's UDP and TCP lookups are alike),
    // and a lookup the core does not follow with a connect leaves nothing
    // open.
    const int n = placeholder_for(node);
    if (!n) {
        pthread_mutex_unlock(&lock);
        return BIONIC_EAI_MEMORY;
    }
    name_route[n - 1] = r->id;
    const int socktype = hints ? hints->ai_socktype : 0;
    int slot = -1;
    for (int i = 0; i < MAX_RESULTS && slot < 0; i++) {
        if (!results[i]) {
            slot = i;
        }
    }
    // One allocation: the entry, its address, and the canonical name.
    struct result {
        struct bionic_addrinfo ai;
        struct sockaddr_storage addr;
        char name[];
    };
    const size_t name_len = (flags & BIONIC_AI_CANONNAME) ? strlen(node) + 1 : 0;
    struct result *out = slot >= 0 ? calloc(1, sizeof *out + name_len) : NULL;
    if (!out) {
        pthread_mutex_unlock(&lock);
        return BIONIC_EAI_MEMORY;
    }
    results[slot] = out;
    pthread_mutex_unlock(&lock);

    placeholder_addr(n, family == AF_INET6 ? AF_INET6 : AF_INET, (int) port, &out->addr,
                     &out->ai.ai_addrlen);
    out->ai.ai_family = out->addr.ss_family;
    out->ai.ai_socktype = socktype ? socktype : SOCK_STREAM;
    out->ai.ai_protocol = hints && hints->ai_protocol ? hints->ai_protocol
                          : out->ai.ai_socktype == SOCK_DGRAM ? IPPROTO_UDP
                                                              : IPPROTO_TCP;
    out->ai.ai_addr = (struct sockaddr *) &out->addr;
    if (name_len) {
        memcpy(out->name, node, name_len);
        out->ai.ai_canonname = out->name;
    }
    *res = &out->ai;
    char text[INET6_ADDRSTRLEN];
    text_of(out->ai.ai_addr, text, sizeof text);
    vento_log(VENTO_LOG_INFO, TAG, "%s: placeholder %s; the session's network resolves the name", node, text);
    return 0;
}

static void hook_freeaddrinfo(void *res) {
    if (res) {
        pthread_mutex_lock(&lock);
        for (int i = 0; i < MAX_RESULTS; i++) {
            if (results[i] == res) {
                results[i] = NULL;
                pthread_mutex_unlock(&lock);
                free(res);
                return;
            }
        }
        pthread_mutex_unlock(&lock);
    }
    next_freeaddrinfo(res);
}

static int hook_getnameinfo(const struct sockaddr *sa, socklen_t sa_len, char *host,
                            socklen_t host_len, char *serv, socklen_t serv_len, int flags) {
    if (atomic_load(&name_count) == 0 || !sa || !host || (flags & BIONIC_NI_NUMERICHOST)) {
        return next_getnameinfo(sa, sa_len, host, host_len, serv, serv_len, flags);
    }
    char name[256];
    pthread_mutex_lock(&lock);
    const int n = placeholder_index(sa, sa_len);
    if (n) {
        snprintf(name, sizeof name, "%s", names[n - 1]);
    }
    pthread_mutex_unlock(&lock);
    if (!n) {
        return next_getnameinfo(sa, sa_len, host, host_len, serv, serv_len, flags);
    }
    if (serv && serv_len) {
        const int rc = next_getnameinfo(sa, sa_len, NULL, 0, serv, serv_len,
                                        flags | BIONIC_NI_NUMERICHOST);
        if (rc) {
            return rc;
        }
    }
    if (strlen(name) >= host_len) {
        return BIONIC_EAI_OVERFLOW;
    }
    strcpy(host, name);
    return 0;
}

static int hook_getpeername(int fd, struct sockaddr *addr, socklen_t *len) {
    if (atomic_load(&bridged_count) > 0) {
        pthread_mutex_lock(&lock);
        struct bridged *b = bridged_find(fd);
        if (b) {
            note("getpeername", 0, 0, "answered with the address connected to");
            const int rc = copy_out(&b->peer, b->peer_len, addr, len);
            pthread_mutex_unlock(&lock);
            return rc;
        }
        pthread_mutex_unlock(&lock);
    }
    return next_getpeername(fd, addr, len);
}

static int hook_getsockname(int fd, struct sockaddr *addr, socklen_t *len) {
    if (atomic_load(&bridged_count) > 0) {
        pthread_mutex_lock(&lock);
        const struct bridged *b = bridged_find(fd);
        if (b) {
            note("getsockname", 0, 0,
                 b->local_len && ((const struct sockaddr_in *) &b->local)->sin_port
                         ? "answered with this end's address through the session's network"
                         : "answered with the unspecified address");
            const int rc = copy_out(&b->local, b->local_len, addr, len);
            pthread_mutex_unlock(&lock);
            return rc;
        }
        pthread_mutex_unlock(&lock);
    }
    return next_getsockname(fd, addr, len);
}

static int hook_setsockopt(int fd, int level, int name, const void *value, socklen_t len) {
    if (atomic_load(&bridged_count) > 0) {
        pthread_mutex_lock(&lock);
        struct bridged *b = bridged_find(fd);
        if (b && level != SOL_SOCKET) {
            if (level == IPPROTO_TCP && name == TCP_NODELAY && value && len >= sizeof(int)) {
                b->nodelay = *(const int *) value != 0;
            }
            note("setsockopt", level, name, "accepted");
            pthread_mutex_unlock(&lock);
            return 0;
        }
        if (b && b->dialling && b->kind == SOCK_STREAM && name == SO_SNDBUF && value && len >= sizeof(int)) {
            // Set once the dial is done: until then the small buffer is what
            // keeps the socket from polling writable.
            b->sndbuf = *(const int *) value;
            pthread_mutex_unlock(&lock);
            return 0;
        }
        if (b) {
            const int rc = next_setsockopt(fd, level, name, value, len);
            if (rc != 0) {
                note("setsockopt", level, name, strerror(errno));
            }
            pthread_mutex_unlock(&lock);
            // What the socket cannot do is the route's business; the core
            // asked it of a TCP socket, where it would have worked.
            return 0;
        }
        pthread_mutex_unlock(&lock);
    }
    return next_setsockopt(fd, level, name, value, len);
}

static int hook_getsockopt(int fd, int level, int name, void *value, socklen_t *len) {
    if (atomic_load(&bridged_count) > 0) {
        pthread_mutex_lock(&lock);
        struct bridged *b = bridged_find(fd);
        if (b && b->kind == SOCK_STREAM && level == SOL_SOCKET && name == SO_ERROR && value && len
            && *len >= sizeof(int)) {
            // How `dial_later`'s dial went, asked once the socket polls
            // writable, which it does only once the dial is done.
            *(int *) value = b->so_error;
            *len = sizeof(int);
            b->so_error = 0;
            pthread_mutex_unlock(&lock);
            return 0;
        }
        if (b && value && len && *len >= sizeof(int)
            && ((level == SOL_SOCKET && (name == SO_DOMAIN || name == SO_PROTOCOL || name == SO_TYPE))
                || level != SOL_SOCKET)) {
            // A TCP or UDP socket's answers: its family, type and protocol,
            // and for TCP and IP options zeroes, apart from Nagle as the core
            // set it and an established state in TCP_INFO.
            memset(value, 0, *len);
            if (level == SOL_SOCKET) {
                *(int *) value = name == SO_DOMAIN ? b->peer.ss_family
                                 : name == SO_TYPE ? b->kind
                                 : b->kind == SOCK_DGRAM ? IPPROTO_UDP
                                                         : IPPROTO_TCP;
                *len = sizeof(int);
            } else if (level == IPPROTO_TCP && name == TCP_NODELAY) {
                *(int *) value = b->nodelay;
                *len = sizeof(int);
            } else if (level == IPPROTO_TCP && name == TCP_INFO) {
                ((unsigned char *) value)[0] = 1; // TCP_ESTABLISHED
            }
            note("getsockopt", level, name, b->kind == SOCK_DGRAM ? "answered as UDP" : "answered as TCP");
            pthread_mutex_unlock(&lock);
            return 0;
        }
        pthread_mutex_unlock(&lock);
    }
    return next_getsockopt(fd, level, name, value, len);
}

/* The errno the route says the connection on bridged socket `fd`, of
   `kind`, ended with: `fallback` where it says nothing (or knows nothing of
   it), and -1 where `fd` is not a bridged socket of that kind. The route's
   reason is logged once per connection. */
static int route_end(int fd, int kind, int fallback) {
    pthread_mutex_lock(&lock);
    struct bridged *b = bridged_find(fd);
    if (!b || b->kind != kind) {
        pthread_mutex_unlock(&lock);
        return -1;
    }
    struct route *r = b->connection > 0 ? route_by_id(b->route) : NULL;
    if (!r || r->detaching || !r->has_net || !r->net.ended) {
        pthread_mutex_unlock(&lock);
        return fallback;
    }
    const bool logged = b->ended_logged;
    b->ended_logged = true;
    const int connection = b->connection;
    r->busy++;
    const vento_net net = r->net;
    pthread_mutex_unlock(&lock);
    int error = 0;
    char why[200] = "";
    const int end = net.ended(net.user, connection, &error, why, sizeof why);
    pthread_mutex_lock(&lock);
    r->busy--;
    pthread_cond_broadcast(&idle);
    pthread_mutex_unlock(&lock);
    if (error == 0 || (end != VENTO_NET_END_RESET && end != VENTO_NET_END_TIMEDOUT
                       && end != VENTO_NET_END_ERROR && end != VENTO_NET_END_TUNNEL)) {
        return fallback;
    }
    if (!logged) {
        vento_log(VENTO_LOG_INFO, TAG, "connection end reason from the session's network: %s", why);
    }
    return error;
}

/* What a read on `fd` that returned `got` answers: on a bridged TCP socket,
   the end of the stream (0) becomes the error a direct socket would have
   failed with, where the route says the connection ended with one. */
static ssize_t stream_end(int fd, ssize_t got) {
    if (got != 0 || atomic_load(&bridged_count) == 0) {
        return got;
    }
    const int error = route_end(fd, SOCK_STREAM, 0);
    if (error <= 0) {
        return 0;
    }
    errno = error;
    return -1;
}

/* On a bridged UDP socket, which is a SOCK_SEQPACKET end where vento carries
   the datagrams: its end of file (a receive of 0 with the other end gone,
   rather than an empty datagram) means the route has stopped carrying them,
   and becomes the error the route recorded, ECONNREFUSED (what a direct UDP
   socket says once the server's port is unreachable) where it has none. 0
   where `fd` is not such a socket or has not ended. */
static int datagrams_ended(int fd) {
    if (atomic_load(&bridged_count) == 0) {
        return 0;
    }
    struct pollfd p = {fd, POLLIN, 0};
    if (poll(&p, 1, 0) != 1 || !(p.revents & POLLHUP)) {
        return 0;
    }
    const int error = route_end(fd, SOCK_DGRAM, ECONNREFUSED);
    return error > 0 ? error : 0;
}

static ssize_t hook_read(int fd, void *buf, size_t count) {
    return stream_end(fd, next_read(fd, buf, count));
}

static ssize_t hook_read_chk(int fd, void *buf, size_t count, size_t buf_size) {
    return stream_end(fd, next_read_chk(fd, buf, count, buf_size));
}

static ssize_t hook_recvmsg(int fd, struct msghdr *msg, int flags) {
    const ssize_t got = next_recvmsg(fd, msg, flags);
    if (got == 0) {
        const int error = datagrams_ended(fd);
        if (error) {
            errno = error;
            return -1;
        }
    }
    return stream_end(fd, got);
}

static ssize_t hook_sendmsg(int fd, const struct msghdr *msg, int flags) {
    if (atomic_load(&relayed_routes) > 0 && msg && msg->msg_name
        && msg->msg_namelen >= sizeof(struct sockaddr_in)) {
        // A datagram sent to an address of its own, which the core has not
        // been seen to send on a session's socket: as a connect to it, the
        // core's own where a direct session is open and it is not a
        // placeholder, refused otherwise.
        const struct sockaddr *to = msg->msg_name;
        if (to->sa_family == AF_INET || to->sa_family == AF_INET6) {
            char host[INET6_ADDRSTRLEN];
            text_of(to, host, sizeof host);
            pthread_mutex_lock(&lock);
            const bool own = !placeholder_index(to, msg->msg_namelen) && any_direct();
            pthread_mutex_unlock(&lock);
            if (!own) {
                vento_log(VENTO_LOG_WARN, TAG, "%s:%d: datagram refused: not through the session's network",
                          host, port_of(to));
                errno = ENETUNREACH;
                return -1;
            }
        }
    }
    if (atomic_load(&bridged_count) == 0) {
        return next_sendmsg(fd, msg, flags);
    }
    pthread_mutex_lock(&lock);
    const struct bridged *b = bridged_find(fd);
    const bool datagrams = b && b->kind == SOCK_DGRAM;
    pthread_mutex_unlock(&lock);
    if (!datagrams) {
        return next_sendmsg(fd, msg, flags);
    }
    // A socketpair whose other end is gone answers EPIPE, and a SIGPIPE
    // without MSG_NOSIGNAL; a UDP socket would have said why.
    const ssize_t sent = next_sendmsg(fd, msg, flags | MSG_NOSIGNAL);
    if (sent < 0 && errno == EPIPE) {
        const int error = route_end(fd, SOCK_DGRAM, ECONNREFUSED);
        errno = error > 0 ? error : ECONNREFUSED;
    }
    return sent;
}

const realvnc_net_hook realvnc_net_hooks[REALVNC_NET_HOOKS] = {
        {"connect", (void *) hook_connect, (void **) &next_connect},
        {"getaddrinfo", (void *) hook_getaddrinfo, (void **) &next_getaddrinfo},
        {"freeaddrinfo", (void *) hook_freeaddrinfo, (void **) &next_freeaddrinfo},
        {"getnameinfo", (void *) hook_getnameinfo, (void **) &next_getnameinfo},
        {"getpeername", (void *) hook_getpeername, (void **) &next_getpeername},
        {"getsockname", (void *) hook_getsockname, (void **) &next_getsockname},
        {"getsockopt", (void *) hook_getsockopt, (void **) &next_getsockopt},
        {"setsockopt", (void *) hook_setsockopt, (void **) &next_setsockopt},
        {"read", (void *) hook_read, (void **) &next_read},
        {"__read_chk", (void *) hook_read_chk, (void **) &next_read_chk},
        {"recvmsg", (void *) hook_recvmsg, (void **) &next_recvmsg},
        {"sendmsg", (void *) hook_sendmsg, (void **) &next_sendmsg},
};
