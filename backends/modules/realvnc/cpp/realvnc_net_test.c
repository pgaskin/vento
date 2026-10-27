// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Tests of the hooks under the RealVNC core's socket calls (realvnc_net.c),
// built with the probes (`-DVENTO_PROBES=ON`) and run by `ctest` in that
// build tree or as `realvnc/vento_realvnc_net_test`. No core is loaded: each
// test calls the hooks as the core calls its imports (a lookup, a
// non-blocking connect polled for writable, SO_ERROR, getpeername, reads
// and writes), with the functions the hooks replaced standing in as libc's
// own, counted, and a fake `vento_net` whose dials go to a listener here.
// Nothing leaves this machine. VENTO_TEST_VERBOSE=1 prints the hooks' log.

#define _GNU_SOURCE

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#include <vento/log.h>
#include <vento/net.h>

#include "realvnc_net.h"

static int failures;

#define CHECK(cond)                                                                                \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            fprintf(stderr, "%s:%d: %s: CHECK failed: %s\n", __FILE__, __LINE__, __func__, #cond); \
            failures++;                                                                            \
        }                                                                                          \
    } while (0)

void vento_log(int level, const char *tag, const char *fmt, ...) {
    static int verbose = -1;
    if (verbose < 0) {
        const char *v = getenv("VENTO_TEST_VERBOSE");
        verbose = v && strcmp(v, "1") == 0;
    }
    if (!verbose) {
        return;
    }
    va_list ap;
    va_start(ap, fmt);
    fprintf(stderr, "  [%d %s] ", level, tag);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
}

static long now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000L + t.tv_nsec / 1000000L;
}

static void sleep_ms(int ms) {
    struct timespec t = {ms / 1000, (long) (ms % 1000) * 1000000L};
    nanosleep(&t, NULL);
}

// ---- the core's view: bionic's numbers and addrinfo --------------------------

enum {
    BIONIC_AI_NUMERICHOST = 0x4,
    BIONIC_EAI_FAIL = 4,
    BIONIC_EAI_NONAME = 8,
};

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

// ---- what the hooks replaced, counted ----------------------------------------

static atomic_int real_connects;
static atomic_int real_lookups;
static atomic_int real_sendmsgs;

static int rec_connect(int fd, const struct sockaddr *addr, socklen_t len) {
    atomic_fetch_add(&real_connects, 1);
    return connect(fd, addr, len);
}

static int rec_getaddrinfo(const char *node, const char *service, const void *hints, void **res) {
    (void) node;
    (void) service;
    (void) hints;
    atomic_fetch_add(&real_lookups, 1);
    *res = NULL;
    return BIONIC_EAI_NONAME;
}

static void rec_freeaddrinfo(void *res) {
    (void) res;
}

static int rec_getnameinfo(const struct sockaddr *sa, socklen_t sa_len, char *host, socklen_t host_len,
                           char *serv, socklen_t serv_len, int flags) {
    (void) sa_len;
    (void) flags;
    if (host && host_len) {
        snprintf(host, host_len, "real");
    }
    if (serv && serv_len) {
        snprintf(serv, serv_len, "%d", ntohs(((const struct sockaddr_in *) sa)->sin_port));
    }
    return 0;
}

static ssize_t rec_read_chk(int fd, void *buf, size_t count, size_t size) {
    (void) size;
    return read(fd, buf, count);
}

static ssize_t rec_sendmsg(int fd, const struct msghdr *msg, int flags) {
    atomic_fetch_add(&real_sendmsgs, 1);
    return sendmsg(fd, msg, flags);
}

static void *hook(const char *name) {
    for (int i = 0; i < REALVNC_NET_HOOKS; i++) {
        if (strcmp(realvnc_net_hooks[i].name, name) == 0) {
            return realvnc_net_hooks[i].hook;
        }
    }
    fprintf(stderr, "no hook %s\n", name);
    exit(2);
}

static void install(void) {
    const struct {
        const char *name;
        void *fn;
    } nexts[] = {
            {"connect", (void *) rec_connect},
            {"getaddrinfo", (void *) rec_getaddrinfo},
            {"freeaddrinfo", (void *) rec_freeaddrinfo},
            {"getnameinfo", (void *) rec_getnameinfo},
            {"getpeername", (void *) getpeername},
            {"getsockname", (void *) getsockname},
            {"getsockopt", (void *) getsockopt},
            {"setsockopt", (void *) setsockopt},
            {"read", (void *) read},
            {"__read_chk", (void *) rec_read_chk},
            {"recvmsg", (void *) recvmsg},
            {"sendmsg", (void *) rec_sendmsg},
    };
    for (int i = 0; i < REALVNC_NET_HOOKS; i++) {
        bool found = false;
        for (size_t k = 0; k < sizeof nexts / sizeof nexts[0]; k++) {
            if (strcmp(realvnc_net_hooks[i].name, nexts[k].name) == 0) {
                *realvnc_net_hooks[i].next = nexts[k].fn;
                found = true;
            }
        }
        if (!found) {
            fprintf(stderr, "no stand-in for %s\n", realvnc_net_hooks[i].name);
            exit(2);
        }
    }
    realvnc_net_installed(1, NULL);
}

/* The hooks, as the core calls them. */
static int h_connect(int fd, const struct sockaddr *addr, socklen_t len) {
    return ((int (*)(int, const struct sockaddr *, socklen_t)) hook("connect"))(fd, addr, len);
}

static int h_getaddrinfo(const char *node, const char *service, const struct bionic_addrinfo *hints,
                         struct bionic_addrinfo **res) {
    return ((int (*)(const char *, const char *, const void *, void **)) hook("getaddrinfo"))(
            node, service, hints, (void **) res);
}

static void h_freeaddrinfo(struct bionic_addrinfo *res) {
    ((void (*)(void *)) hook("freeaddrinfo"))(res);
}

static int h_getpeername(int fd, struct sockaddr *addr, socklen_t *len) {
    return ((int (*)(int, struct sockaddr *, socklen_t *)) hook("getpeername"))(fd, addr, len);
}

static int h_getsockopt(int fd, int level, int name, void *value, socklen_t *len) {
    return ((int (*)(int, int, int, void *, socklen_t *)) hook("getsockopt"))(fd, level, name, value, len);
}

static int h_setsockopt(int fd, int level, int name, const void *value, socklen_t len) {
    return ((int (*)(int, int, int, const void *, socklen_t)) hook("setsockopt"))(fd, level, name, value, len);
}

static ssize_t h_read(int fd, void *buf, size_t count) {
    return ((ssize_t (*)(int, void *, size_t)) hook("read"))(fd, buf, count);
}

static ssize_t h_recvmsg(int fd, struct msghdr *msg, int flags) {
    return ((ssize_t (*)(int, struct msghdr *, int)) hook("recvmsg"))(fd, msg, flags);
}

static ssize_t h_sendmsg(int fd, const struct msghdr *msg, int flags) {
    return ((ssize_t (*)(int, const struct msghdr *, int)) hook("sendmsg"))(fd, msg, flags);
}

// ---- a fake vento_net --------------------------------------------------------

struct fake {
    int tcp_port;         // `connect` dials 127.0.0.1 here
    int udp_port;         // `connect_datagram` connects a UDP socket here; 0: streams only
    int delay_ms;         // each dial takes this long, unless cancelled
    int fail;             // a VENTO_NET_E* code each dial fails with, or 0
    int resolve_delay_ms; // each `resolve` takes this long, unless cancelled
    int resolve_fail;     // a VENTO_NET_E* code each `resolve` fails with, or 0
    int end_code;         // `ended`'s answer
    int end_errno;
    atomic_int cancelled;
    atomic_int connects;
    atomic_int datagrams;
    atomic_int resolves;
    atomic_int releases;
    pthread_mutex_t lock;
    char host[256]; // the last dial's
    int port;
    char resolved[256]; // the last `resolve`'s
    int connection;
};

static void fake_init(struct fake *f) {
    memset(f, 0, sizeof *f);
    pthread_mutex_init(&f->lock, NULL);
}

/* Waits out a delay; false where the fake was cancelled meanwhile. */
static bool fake_wait(struct fake *f, int delay_ms) {
    for (long until = now_ms() + delay_ms;;) {
        if (atomic_load(&f->cancelled)) {
            return false;
        }
        if (now_ms() >= until) {
            return true;
        }
        sleep_ms(5);
    }
}

static int fake_fail(struct fake *f, char *why, size_t why_len) {
    const int code = atomic_load(&f->cancelled) ? VENTO_NET_ECANCELLED : f->fail;
    if (why && why_len) {
        snprintf(why, why_len, "%s", code == VENTO_NET_ECANCELLED ? "cancelled" : "the fake says no");
    }
    return code;
}

static void fake_peer(struct fake *f, int fd, int port, vento_net_peer *peer) {
    if (!peer) {
        return;
    }
    memset(peer, 0, sizeof *peer);
    snprintf(peer->address, sizeof peer->address, "127.0.0.1");
    peer->port = port;
    struct sockaddr_in local;
    socklen_t len = sizeof local;
    if (getsockname(fd, (struct sockaddr *) &local, &len) == 0) {
        snprintf(peer->local_address, sizeof peer->local_address, "127.0.0.1");
        peer->local_port = ntohs(local.sin_port);
    }
    peer->route = VENTO_NET_TUNNEL;
    pthread_mutex_lock(&f->lock);
    peer->connection = ++f->connection;
    pthread_mutex_unlock(&f->lock);
}

static int fake_dial(struct fake *f, int type, int port, const char *host, int to, vento_net_peer *peer,
                     char *why, size_t why_len) {
    pthread_mutex_lock(&f->lock);
    snprintf(f->host, sizeof f->host, "%s", host);
    f->port = port;
    pthread_mutex_unlock(&f->lock);
    if (!fake_wait(f, f->delay_ms) || f->fail) {
        return fake_fail(f, why, why_len);
    }
    const int fd = socket(AF_INET, type | SOCK_CLOEXEC, 0);
    struct sockaddr_in a = {.sin_family = AF_INET, .sin_port = htons((uint16_t) to)};
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (fd < 0 || connect(fd, (struct sockaddr *) &a, sizeof a) != 0) {
        if (fd >= 0) {
            close(fd);
        }
        snprintf(why, why_len, "%s", strerror(errno));
        return VENTO_NET_EREFUSED;
    }
    fake_peer(f, fd, port, peer);
    return fd;
}

static int fake_connect(void *user, const char *host, int port, int timeout_ms, const vento_net_liveness *liveness,
                        vento_net_peer *peer, char *why, size_t why_len) {
    (void) timeout_ms;
    (void) liveness;
    struct fake *f = user;
    atomic_fetch_add(&f->connects, 1);
    return fake_dial(f, SOCK_STREAM, port, host, f->tcp_port, peer, why, why_len);
}

static int fake_connect_datagram(void *user, const char *host, int port, int timeout_ms, vento_net_peer *peer,
                                 char *why, size_t why_len) {
    (void) timeout_ms;
    struct fake *f = user;
    atomic_fetch_add(&f->datagrams, 1);
    if (!f->udp_port) {
        snprintf(why, why_len, "this tunnel carries streams only");
        return VENTO_NET_ENOTSUP;
    }
    return fake_dial(f, SOCK_DGRAM, port, host, f->udp_port, peer, why, why_len);
}

static int fake_resolve(void *user, const char *host, int timeout_ms, char *why, size_t why_len) {
    (void) timeout_ms;
    struct fake *f = user;
    atomic_fetch_add(&f->resolves, 1);
    pthread_mutex_lock(&f->lock);
    snprintf(f->resolved, sizeof f->resolved, "%s", host);
    pthread_mutex_unlock(&f->lock);
    if (!fake_wait(f, f->resolve_delay_ms) || f->resolve_fail) {
        const int code = atomic_load(&f->cancelled) ? VENTO_NET_ECANCELLED : f->resolve_fail;
        if (why && why_len) {
            snprintf(why, why_len, "%s", code == VENTO_NET_ECANCELLED ? "cancelled" : "Name or service not known");
        }
        return code;
    }
    return 0;
}

static int fake_ended(void *user, int connection, int *error, char *why, size_t why_len) {
    (void) connection;
    struct fake *f = user;
    if (error) {
        *error = f->end_errno;
    }
    if (why && why_len) {
        snprintf(why, why_len, "%s", f->end_errno ? strerror(f->end_errno) : "");
    }
    return f->end_code;
}

static void fake_cancel(void *user) {
    atomic_store(&((struct fake *) user)->cancelled, 1);
}

static void fake_release(void *user) {
    atomic_fetch_add(&((struct fake *) user)->releases, 1);
}

static vento_net table(struct fake *f) {
    vento_net n = {
            .user = f,
            .direct = 0,
            .connect = fake_connect,
            .connect_datagram = fake_connect_datagram,
            .resolve = fake_resolve,
            .ended = fake_ended,
            .cancel = fake_cancel,
            .release = fake_release,
    };
    return n;
}

// ---- helpers -----------------------------------------------------------------

/* A TCP listener on 127.0.0.1 and its port. */
static int listener(int *port) {
    const int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    struct sockaddr_in a = {.sin_family = AF_INET};
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    socklen_t len = sizeof a;
    if (fd < 0 || bind(fd, (struct sockaddr *) &a, sizeof a) != 0 || listen(fd, 8) != 0
        || getsockname(fd, (struct sockaddr *) &a, &len) != 0) {
        perror("listener");
        exit(2);
    }
    *port = ntohs(a.sin_port);
    return fd;
}

/* A UDP socket on 127.0.0.1 and its port. */
static int udp_server(int *port) {
    const int fd = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    struct sockaddr_in a = {.sin_family = AF_INET};
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    socklen_t len = sizeof a;
    if (fd < 0 || bind(fd, (struct sockaddr *) &a, sizeof a) != 0
        || getsockname(fd, (struct sockaddr *) &a, &len) != 0) {
        perror("udp");
        exit(2);
    }
    *port = ntohs(a.sin_port);
    return fd;
}

/* Accepts one connection, waiting up to two seconds; -1 for none. */
static int accept_within(int fd, int ms) {
    struct pollfd p = {fd, POLLIN, 0};
    if (poll(&p, 1, ms) != 1) {
        return -1;
    }
    return accept4(fd, NULL, NULL, SOCK_CLOEXEC);
}

/* The core's socket: non-blocking and close-on-exec, as it makes them. */
static int core_socket(int type) {
    const int fd = socket(AF_INET, type | SOCK_CLOEXEC, 0);
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
    return fd;
}

/* Polls `fd` for `events` up to `ms`; the events seen, 0 for none. */
static int poll_for(int fd, short events, int ms) {
    struct pollfd p = {fd, events, 0};
    return poll(&p, 1, ms) == 1 ? p.revents : 0;
}

static int so_error(int fd) {
    int e = -1;
    socklen_t len = sizeof e;
    return h_getsockopt(fd, SOL_SOCKET, SO_ERROR, &e, &len) == 0 ? e : -1;
}

/* The core's resolver-thread lookup of `host`: the address it gives. */
static bool lookup(const char *host, int port, struct sockaddr_storage *out, socklen_t *len) {
    char service[16];
    snprintf(service, sizeof service, "%d", port);
    struct bionic_addrinfo hints = {.ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    if (h_getaddrinfo(host, service, &hints, &res) != 0 || !res) {
        return false;
    }
    memcpy(out, res->ai_addr, res->ai_addrlen);
    *len = res->ai_addrlen;
    h_freeaddrinfo(res);
    return true;
}

static bool is_placeholder(const struct sockaddr_storage *a) {
    return a->ss_family == AF_INET
           && (ntohl(((const struct sockaddr_in *) a)->sin_addr.s_addr) & 0xfffe0000u) == 0xc6120000u;
}

static bool write_all(int fd, const char *text) {
    return write(fd, text, strlen(text)) == (ssize_t) strlen(text);
}

/* Reads `want` bytes from `fd` (blocking or not) within two seconds,
   through the read hook. */
static bool read_text(int fd, const char *want) {
    char got[256] = "";
    size_t have = 0;
    const size_t n = strlen(want);
    for (long until = now_ms() + 2000; have < n && now_ms() < until;) {
        poll_for(fd, POLLIN, 100);
        const ssize_t r = h_read(fd, got + have, n - have);
        if (r > 0) {
            have += (size_t) r;
        } else if (r == 0 || (errno != EAGAIN && errno != EWOULDBLOCK)) {
            break;
        }
    }
    return have == n && memcmp(got, want, n) == 0;
}

static char last_notice[320];

static void record_notice(const char *text) {
    snprintf(last_notice, sizeof last_notice, "%s", text);
}

static void reset_counts(void) {
    atomic_store(&real_connects, 0);
    atomic_store(&real_lookups, 0);
    atomic_store(&real_sendmsgs, 0);
}

// ---- tests -------------------------------------------------------------------

/* A TCP connect, dialled on its own thread, is not writable until its dial
   is done, even where the core asks getpeername or sets SO_SNDBUF first;
   then it is, SO_ERROR is 0, and its send buffer is what the core set. */
static void testUnwritableUntilDialled(void) {
    reset_counts();
    int tcp_port, udp_port;
    const int l = listener(&tcp_port);
    const int u = udp_server(&udp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    f.udp_port = udp_port;
    vento_net n = table(&f);
    char why[200] = "";
    const int route = realvnc_net_attach(&n, "test", "early.example::5901", 5000, why, sizeof why);
    CHECK(route > 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("early.example", 5901, &a, &len));
    CHECK(is_placeholder(&a));
    f.delay_ms = 400;
    const int fd = core_socket(SOCK_STREAM);
    const long started = now_ms();
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    struct sockaddr_storage peer;
    socklen_t peer_len = sizeof peer;
    CHECK(h_getpeername(fd, (struct sockaddr *) &peer, &peer_len) == 0);
    const int asked = 256 * 1024;
    CHECK(h_setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &asked, sizeof asked) == 0);
    CHECK(poll_for(fd, POLLOUT, 100) == 0);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(now_ms() - started >= 350);
    CHECK(so_error(fd) == 0);
    int sndbuf = 0;
    socklen_t sndbuf_len = sizeof sndbuf;
    CHECK(getsockopt(fd, SOL_SOCKET, SO_SNDBUF, &sndbuf, &sndbuf_len) == 0 && sndbuf >= asked);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    CHECK(write_all(fd, "hello"));
    char got[8] = "";
    CHECK(read(s, got, 5) == 5 && memcmp(got, "hello", 5) == 0);
    CHECK(atomic_load(&real_connects) == 0);
    close(fd);
    close(s);
    realvnc_net_detach(route);
    CHECK(atomic_load(&f.releases) == 1);
    close(l);
    close(u);
}

/* The core's whole TCP flow through a network: the lookups answer a
   placeholder and dial nothing, the connect dials (for the host as typed)
   and answers EINPROGRESS, the socket polls writable once connected with
   SO_ERROR 0, getpeername is the placeholder, the core's flags are kept,
   bytes go both ways, and the server's reset reads as ECONNRESET. */
static void testStreamFlow(void) {
    reset_counts();
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "flow.example::5901", 5000, NULL, 0);
    CHECK(route > 0);
    CHECK(realvnc_net_relayed(route));
    // The core thread's first look: not numeric, as for a name on a direct
    // route.
    struct bionic_addrinfo numeric_hints = {.ai_flags = BIONIC_AI_NUMERICHOST, .ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    CHECK(h_getaddrinfo("flow.example", "5901", &numeric_hints, &res) == BIONIC_EAI_NONAME);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("flow.example", 5901, &a, &len));
    CHECK(is_placeholder(&a));
    CHECK(atomic_load(&f.connects) == 0 && atomic_load(&f.datagrams) == 0);
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0);
    CHECK(atomic_load(&f.connects) == 1);
    CHECK(strcmp(f.host, "flow.example") == 0 && f.port == 5901);
    struct sockaddr_storage peer;
    socklen_t peer_len = sizeof peer;
    CHECK(h_getpeername(fd, (struct sockaddr *) &peer, &peer_len) == 0);
    CHECK(peer_len == len && memcmp(&peer, &a, len) == 0);
    CHECK(fcntl(fd, F_GETFL) & O_NONBLOCK);
    CHECK(fcntl(fd, F_GETFD) & FD_CLOEXEC);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    CHECK(write_all(fd, "ping"));
    char got[8] = "";
    CHECK(read(s, got, 4) == 4 && memcmp(got, "ping", 4) == 0);
    CHECK(write_all(s, "pong"));
    CHECK(read_text(fd, "pong"));
    // The server's reset, which the route records.
    f.end_code = VENTO_NET_END_RESET;
    f.end_errno = ECONNRESET;
    const struct linger hard = {1, 0};
    setsockopt(s, SOL_SOCKET, SO_LINGER, &hard, sizeof hard);
    close(s);
    CHECK(poll_for(fd, POLLIN, 2000) & (POLLIN | POLLHUP));
    char byte;
    CHECK(h_read(fd, &byte, 1) == -1 && errno == ECONNRESET);
    CHECK(atomic_load(&real_connects) == 0 && atomic_load(&real_lookups) == 0);
    close(fd);
    realvnc_net_detach(route);
    CHECK(atomic_load(&f.releases) == 1);
    close(l);
}

/* A lookup dials nothing, so a lookup the core does not follow with a
   connect leaves nothing open. */
static void testLookupsDialNothing(void) {
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    f.udp_port = tcp_port;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "idle.example::5901", 5000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    for (int i = 0; i < 3; i++) {
        CHECK(lookup("idle.example", 5901, &a, &len));
    }
    sleep_ms(50);
    CHECK(atomic_load(&f.connects) == 0 && atomic_load(&f.datagrams) == 0);
    CHECK(accept_within(l, 0) < 0);
    realvnc_net_detach(route);
    close(l);
}

/* UDP on: the UDP connect returns at once and its datagrams go through the
   network both ways; the TCP fallback then dials a stream of its own, and
   nothing else is dialled. */
static void testDatagramsThenStream(void) {
    reset_counts();
    int tcp_port, udp_port;
    const int l = listener(&tcp_port);
    const int u = udp_server(&udp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    f.udp_port = udp_port;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "udp.example::5901", 5000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("udp.example", 5901, &a, &len));
    const int d = core_socket(SOCK_DGRAM);
    CHECK(h_connect(d, (struct sockaddr *) &a, len) == 0);
    char ping[] = "ping";
    struct iovec iov = {ping, 4};
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};
    CHECK(h_sendmsg(d, &msg, 0) == 4);
    char got[16] = "";
    struct sockaddr_in from;
    socklen_t from_len = sizeof from;
    CHECK(poll_for(u, POLLIN, 2000) & POLLIN);
    CHECK(recvfrom(u, got, sizeof got, 0, (struct sockaddr *) &from, &from_len) == 4
          && memcmp(got, "ping", 4) == 0);
    CHECK(sendto(u, "pong", 4, 0, (struct sockaddr *) &from, from_len) == 4);
    CHECK(poll_for(d, POLLIN, 2000) & POLLIN);
    char back[16] = "";
    struct iovec biov = {back, sizeof back};
    struct msghdr bmsg = {.msg_iov = &biov, .msg_iovlen = 1};
    CHECK(h_recvmsg(d, &bmsg, MSG_DONTWAIT) == 4 && memcmp(back, "pong", 4) == 0);
    CHECK(strcmp(f.host, "udp.example") == 0);
    // The TCP fallback.
    CHECK(lookup("udp.example", 5901, &a, &len));
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    CHECK(atomic_load(&f.datagrams) == 1 && atomic_load(&f.connects) == 1);
    CHECK(atomic_load(&real_connects) == 0 && atomic_load(&real_sendmsgs) == 1);
    close(fd);
    close(s);
    close(d);
    realvnc_net_detach(route);
    close(l);
    close(u);
}

/* A route of streams: the UDP connect returns, its socket reads the end of
   file at once, which recvmsg answers ECONNREFUSED (the core falls back to
   TCP), and TCP goes through. */
static void testStreamsOnlyRoute(void) {
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "streams.example::5901", 5000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("streams.example", 5901, &a, &len));
    const int d = core_socket(SOCK_DGRAM);
    CHECK(h_connect(d, (struct sockaddr *) &a, len) == 0);
    CHECK(poll_for(d, POLLIN, 2000) & (POLLIN | POLLHUP));
    char buf[16];
    struct iovec iov = {buf, sizeof buf};
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};
    CHECK(h_recvmsg(d, &msg, MSG_DONTWAIT) == -1 && errno == ECONNREFUSED);
    CHECK(atomic_load(&f.datagrams) == 1);
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    close(fd);
    close(d);
    realvnc_net_detach(route);
    close(l);
}

/* The process's CPU time so far, in ms. */
static long cpu_ms(void) {
    struct rusage u;
    getrusage(RUSAGE_SELF, &u);
    return (u.ru_utime.tv_sec + u.ru_stime.tv_sec) * 1000L + (u.ru_utime.tv_usec + u.ru_stime.tv_usec) / 1000L;
}

/* How many threads the process has. */
static int threads(void) {
    FILE *f = fopen("/proc/self/stat", "r");
    int n = -1;
    if (f) {
        // Field 20, after the command's parentheses.
        char line[1024];
        if (fgets(line, sizeof line, f)) {
            const char *p = strrchr(line, ')');
            for (int field = 2; p && field < 20; field++) {
                p = strchr(p + 1, ' ');
            }
            n = p ? atoi(p + 1) : -1;
        }
        fclose(f);
    }
    return n;
}

/* The core closes its socket while the relay holds bytes for a server that
   is not reading: the relay waits for the server without spinning, and
   detaching the route ends it. */
static void testCloseWhileHoldingBytes(void) {
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "stalled.example::5901", 5000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("stalled.example", 5901, &a, &len));
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    // Until every buffer on the way is full: the core's socket stays
    // unwritable for a while.
    static const char chunk[16384];
    for (long last = now_ms(); now_ms() - last < 300;) {
        if (write(fd, chunk, sizeof chunk) > 0) {
            last = now_ms();
        } else {
            sleep_ms(10);
        }
    }
    const int relaying = threads();
    close(fd);
    sleep_ms(100);
    const long before = cpu_ms();
    sleep_ms(1000);
    const long spent = cpu_ms() - before;
    CHECK(spent < 300);
    if (spent >= 300) {
        fprintf(stderr, "  %ld ms of CPU in 1 s after the close\n", spent);
    }
    realvnc_net_detach(route);
    bool gone = false;
    for (long until = now_ms() + 1500; !gone && now_ms() < until; sleep_ms(10)) {
        gone = threads() < relaying;
    }
    CHECK(gone);
    // What the relay had passed on is still there, then the end.
    bool ended = false;
    char buf[65536];
    for (long until = now_ms() + 3000; !ended && now_ms() < until;) {
        if (!(poll_for(s, POLLIN, 100) & (POLLIN | POLLHUP))) {
            continue;
        }
        const ssize_t got = read(s, buf, sizeof buf);
        ended = got <= 0;
    }
    CHECK(ended);
    close(s);
    close(l);
}

/* A failed dial answers its error once SO_ERROR is asked, once; a dial
   ended by the disconnect leaves the connect in progress (never writable,
   no hang-up), and the route then detaches at once. */
static void testFailedAndCancelledDials(void) {
    struct fake f;
    fake_init(&f);
    f.fail = VENTO_NET_EREFUSED;
    vento_net n = table(&f);
    int route = realvnc_net_attach(&n, "test", "refused.example::5901", 5000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("refused.example", 5901, &a, &len));
    int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == ECONNREFUSED);
    CHECK(so_error(fd) == 0);
    close(fd);
    realvnc_net_detach(route);

    struct fake g;
    fake_init(&g);
    g.delay_ms = 10000;
    vento_net m = table(&g);
    route = realvnc_net_attach(&m, "test", "slow.example::5901", 20000, NULL, 0);
    CHECK(lookup("slow.example", 5901, &a, &len));
    fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    sleep_ms(100);
    realvnc_net_cancel(route);
    CHECK(poll_for(fd, POLLOUT | POLLIN, 500) == 0);
    const long started = now_ms();
    realvnc_net_detach(route);
    CHECK(now_ms() - started < 1000);
    CHECK(atomic_load(&g.releases) == 1);
    close(fd);
}

/* Detaching a route with a dial in progress ends the dial rather than
   waiting it out. */
static void testDetachEndsADial(void) {
    struct fake f;
    fake_init(&f);
    f.delay_ms = 10000;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "hanging.example::5901", 20000, NULL, 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("hanging.example", 5901, &a, &len));
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    sleep_ms(100);
    const long started = now_ms();
    realvnc_net_detach(route);
    CHECK(now_ms() - started < 1000);
    CHECK(atomic_load(&f.cancelled) == 1);
    close(fd);
}

/* A direct session and one through a network at once: each keeps its own
   calls, a name of neither's is refused, an address that is not a
   placeholder is the direct session's, the same host with another tunnel
   setting is refused at attach, and a refused lookup gives the notice. With the network's session alone, a name of neither's
   goes through its tunnel and a plain address is refused; with two tunnels,
   such a name is refused. */
static void testRouting(void) {
    reset_counts();
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    vento_net n = table(&f);
    const int direct = realvnc_net_attach(NULL, NULL, "direct.example::1", 5000, NULL, 0);
    const int relayed = realvnc_net_attach(&n, "test", "relayed.example::2", 5000, NULL, 0);
    CHECK(direct > 0 && relayed > 0);
    CHECK(!realvnc_net_relayed(direct));
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(!lookup("direct.example", 1, &a, &len));
    CHECK(atomic_load(&real_lookups) == 1);
    CHECK(lookup("relayed.example", 2, &a, &len) && is_placeholder(&a));
    struct bionic_addrinfo hints = {.ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    realvnc_net_set_notice(record_notice);
    last_notice[0] = 0;
    CHECK(h_getaddrinfo("third.example", "3", &hints, &res) == BIONIC_EAI_FAIL);
    CHECK(atomic_load(&real_lookups) == 1);
    CHECK(strcmp(last_notice, "bug: unhandled realvnc host lookup third.example") == 0);
    // A plain address: the direct session's.
    struct sockaddr_in plain = {.sin_family = AF_INET, .sin_port = htons((uint16_t) tcp_port)};
    plain.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    int fd = core_socket(SOCK_STREAM);
    const int rc = h_connect(fd, (struct sockaddr *) &plain, sizeof plain);
    CHECK(rc == 0 || errno == EINPROGRESS);
    CHECK(atomic_load(&real_connects) == 1 && atomic_load(&f.connects) == 0);
    close(fd);
    int s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    // A datagram to an address of its own: the direct session's too.
    const int u = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    char x[] = "x";
    struct iovec iov = {x, 1};
    struct msghdr msg = {.msg_name = &plain, .msg_namelen = sizeof plain, .msg_iov = &iov, .msg_iovlen = 1};
    CHECK(h_sendmsg(u, &msg, 0) == 1);
    // The same host on another route.
    char why[200] = "";
    CHECK(realvnc_net_attach(NULL, NULL, "relayed.example::2", 5000, why, sizeof why) == 0);
    CHECK(strstr(why, "another tunnel setting") != NULL);
    struct fake other;
    fake_init(&other);
    vento_net o = table(&other);
    CHECK(realvnc_net_attach(&o, "other", "direct.example::1", 5000, why, sizeof why) == 0);
    CHECK(atomic_load(&other.releases) == 1);

    // The network's session alone.
    realvnc_net_detach(direct);
    CHECK(lookup("third.example", 3, &a, &len) && is_placeholder(&a));
    fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0 && strcmp(f.host, "third.example") == 0);
    s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    close(fd);
    fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &plain, sizeof plain) == -1 && errno == ENETUNREACH);
    close(fd);
    CHECK(atomic_load(&real_connects) == 1 && atomic_load(&f.connects) == 1);
    CHECK(h_sendmsg(u, &msg, 0) == -1 && errno == ENETUNREACH);
    CHECK(atomic_load(&real_sendmsgs) == 1);
    close(u);
    // Two tunnels.
    struct fake second;
    fake_init(&second);
    vento_net t = table(&second);
    const int other_tunnel = realvnc_net_attach(&t, "other", "x.example::4", 5000, NULL, 0);
    CHECK(other_tunnel > 0);
    last_notice[0] = 0;
    CHECK(h_getaddrinfo("fourth.example", "4", &hints, &res) == BIONIC_EAI_FAIL);
    CHECK(strcmp(last_notice, "bug: unhandled realvnc host lookup fourth.example") == 0);
    // A lookup that is assigned gives no notice.
    last_notice[0] = 0;
    CHECK(lookup("x.example", 4, &a, &len) && is_placeholder(&a));
    CHECK(last_notice[0] == 0);
    realvnc_net_set_notice(NULL);
    realvnc_net_detach(other_tunnel);
    realvnc_net_detach(relayed);
    // Nothing through a network: every call is the core's own again.
    reset_counts();
    CHECK(!lookup("anything.example", 5, &a, &len));
    CHECK(atomic_load(&real_lookups) == 1);
    close(l);
}

/* A session through a network typed with an address, and a direct one
   typed with a name for the same machine: the direct one's lookup and
   connect stay its own, and the other's go through its network. */
static void testAliasesStayApart(void) {
    reset_counts();
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    vento_net n = table(&f);
    char address[64];
    snprintf(address, sizeof address, "127.0.0.1::%d", tcp_port);
    const int relayed = realvnc_net_attach(&n, "test", address, 5000, NULL, 0);
    snprintf(address, sizeof address, "localhost::%d", tcp_port);
    const int direct = realvnc_net_attach(NULL, NULL, address, 5000, NULL, 0);
    CHECK(relayed > 0 && direct > 0);
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(!lookup("localhost", tcp_port, &a, &len));
    CHECK(atomic_load(&real_lookups) == 1);
    struct sockaddr_in plain = {.sin_family = AF_INET, .sin_port = htons((uint16_t) tcp_port)};
    plain.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    int fd = core_socket(SOCK_STREAM);
    const int rc = h_connect(fd, (struct sockaddr *) &plain, sizeof plain);
    CHECK(rc == 0 || errno == EINPROGRESS);
    CHECK(atomic_load(&real_connects) == 1 && atomic_load(&f.connects) == 0);
    int s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    close(fd);
    // The other session's numeric host, as the core thread looks it up.
    char service[16];
    snprintf(service, sizeof service, "%d", tcp_port);
    struct bionic_addrinfo hints = {.ai_flags = BIONIC_AI_NUMERICHOST, .ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    CHECK(h_getaddrinfo("127.0.0.1", service, &hints, &res) == 0 && res);
    if (res) {
        memcpy(&a, res->ai_addr, res->ai_addrlen);
        len = res->ai_addrlen;
        h_freeaddrinfo(res);
    }
    CHECK(is_placeholder(&a));
    fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0);
    CHECK(atomic_load(&f.connects) == 1 && strcmp(f.host, "127.0.0.1") == 0);
    CHECK(atomic_load(&real_connects) == 1);
    s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    close(fd);
    realvnc_net_detach(direct);
    realvnc_net_detach(relayed);
    close(l);
}

/* A name is resolved through the route at the core's lookup, without a
   connection: one with no address fails the lookup with EAI_NONAME, as on
   a direct route, and one with an address gets a placeholder and no dial. A
   literal is not resolved. A route that resolves only as it connects, or
   has no `resolve`, gives the placeholder and leaves it to the connect. */
static void testUnresolvableNameFailsAtLookup(void) {
    reset_counts();
    int tcp_port;
    const int l = listener(&tcp_port);
    struct fake f;
    fake_init(&f);
    f.tcp_port = tcp_port;
    f.udp_port = tcp_port;
    f.resolve_fail = VENTO_NET_ERESOLVE;
    vento_net n = table(&f);
    const int route = realvnc_net_attach(&n, "test", "nowhere.example::5901", 5000, NULL, 0);
    CHECK(route > 0);
    struct bionic_addrinfo hints = {.ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    CHECK(h_getaddrinfo("nowhere.example", "5901", &hints, &res) == BIONIC_EAI_NONAME);
    CHECK(atomic_load(&f.resolves) == 1 && strcmp(f.resolved, "nowhere.example") == 0);
    CHECK(atomic_load(&f.connects) == 0 && atomic_load(&f.datagrams) == 0);
    CHECK(accept_within(l, 0) < 0);
    // The core thread's numeric-only look at a name, which is not resolved.
    struct bionic_addrinfo numeric_hints = {.ai_flags = BIONIC_AI_NUMERICHOST, .ai_socktype = SOCK_STREAM};
    CHECK(h_getaddrinfo("nowhere.example", "5901", &numeric_hints, &res) == BIONIC_EAI_NONAME);
    CHECK(atomic_load(&f.resolves) == 1);
    // With an address: the placeholder, and still no dial.
    f.resolve_fail = 0;
    struct sockaddr_storage a;
    socklen_t len;
    CHECK(lookup("nowhere.example", 5901, &a, &len) && is_placeholder(&a));
    CHECK(atomic_load(&f.resolves) == 2);
    sleep_ms(50);
    CHECK(atomic_load(&f.connects) == 0 && atomic_load(&f.datagrams) == 0);
    // A failure of another kind.
    f.resolve_fail = VENTO_NET_EFAILED;
    CHECK(h_getaddrinfo("nowhere.example", "5901", &hints, &res) == BIONIC_EAI_FAIL);
    // A route that resolves only as it connects: the connect finds out.
    f.resolve_fail = VENTO_NET_ENOTSUP;
    CHECK(lookup("nowhere.example", 5901, &a, &len) && is_placeholder(&a));
    CHECK(atomic_load(&f.resolves) == 4);
    realvnc_net_detach(route);
    CHECK(atomic_load(&real_lookups) == 0);

    // A literal host: its own answer, with no lookup through the route.
    struct fake g;
    fake_init(&g);
    g.tcp_port = tcp_port;
    g.resolve_fail = VENTO_NET_ERESOLVE;
    vento_net m = table(&g);
    char address[64];
    snprintf(address, sizeof address, "127.0.0.1::%d", tcp_port);
    const int literal = realvnc_net_attach(&m, "test", address, 5000, NULL, 0);
    CHECK(lookup("127.0.0.1", tcp_port, &a, &len) && is_placeholder(&a));
    char service[16];
    snprintf(service, sizeof service, "%d", tcp_port);
    CHECK(h_getaddrinfo("127.0.0.1", service, &numeric_hints, &res) == 0 && res);
    if (res) {
        h_freeaddrinfo(res);
    }
    CHECK(atomic_load(&g.resolves) == 0);
    realvnc_net_detach(literal);

    // A table without `resolve`: the placeholder, and the connect finds out.
    struct fake h;
    fake_init(&h);
    h.tcp_port = tcp_port;
    vento_net o = table(&h);
    o.resolve = NULL;
    const int old = realvnc_net_attach(&o, "test", "old.example::5901", 5000, NULL, 0);
    CHECK(lookup("old.example", 5901, &a, &len) && is_placeholder(&a));
    const int fd = core_socket(SOCK_STREAM);
    CHECK(h_connect(fd, (struct sockaddr *) &a, len) == -1 && errno == EINPROGRESS);
    CHECK(poll_for(fd, POLLOUT, 2000) & POLLOUT);
    CHECK(so_error(fd) == 0 && atomic_load(&h.connects) == 1);
    const int s = accept_within(l, 2000);
    CHECK(s >= 0);
    close(s);
    close(fd);
    realvnc_net_detach(old);
    CHECK(atomic_load(&real_lookups) == 0 && atomic_load(&real_connects) == 0);
    close(l);
}

struct lookup_run {
    int rc;
    long took_ms;
};

static void *run_lookup(void *arg) {
    struct lookup_run *r = arg;
    const long started = now_ms();
    struct bionic_addrinfo hints = {.ai_socktype = SOCK_STREAM};
    struct bionic_addrinfo *res = NULL;
    r->rc = h_getaddrinfo("slow-name.example", "5901", &hints, &res);
    if (r->rc == 0 && res) {
        h_freeaddrinfo(res);
    }
    r->took_ms = now_ms() - started;
    return NULL;
}

/* A lookup through the route in progress ends at the session's cancel,
   with EAI_NONAME, and a detach ends it too rather than waiting it out. */
static void testCancelEndsALookup(void) {
    struct fake f;
    fake_init(&f);
    f.resolve_delay_ms = 10000;
    vento_net n = table(&f);
    int route = realvnc_net_attach(&n, "test", "slow-name.example::5901", 20000, NULL, 0);
    struct lookup_run run = {0};
    pthread_t thread;
    CHECK(pthread_create(&thread, NULL, run_lookup, &run) == 0);
    sleep_ms(100);
    realvnc_net_cancel(route);
    pthread_join(thread, NULL);
    CHECK(run.rc == BIONIC_EAI_NONAME);
    CHECK(run.took_ms < 1000);
    realvnc_net_detach(route);
    CHECK(atomic_load(&f.releases) == 1);

    struct fake g;
    fake_init(&g);
    g.resolve_delay_ms = 10000;
    vento_net m = table(&g);
    route = realvnc_net_attach(&m, "test", "slow-name.example::5901", 20000, NULL, 0);
    CHECK(pthread_create(&thread, NULL, run_lookup, &run) == 0);
    sleep_ms(100);
    const long started = now_ms();
    realvnc_net_detach(route);
    CHECK(now_ms() - started < 1000);
    pthread_join(thread, NULL);
    CHECK(run.rc == BIONIC_EAI_NONAME);
    CHECK(run.took_ms < 1000);
    CHECK(atomic_load(&g.releases) == 1);
}

/* With hooks missing, a session through a network is refused, and a
   direct one is not. */
static void testRefusedWhenUnhooked(void) {
    struct fake f;
    fake_init(&f);
    vento_net n = table(&f);
    realvnc_net_installed(0, "a test");
    char why[200] = "";
    CHECK(realvnc_net_attach(&n, "test", "unhooked.example::1", 5000, why, sizeof why) == 0);
    CHECK(strstr(why, "not hooked (a test)") != NULL);
    CHECK(atomic_load(&f.releases) == 1);
    const int direct = realvnc_net_attach(NULL, NULL, "unhooked.example::1", 5000, NULL, 0);
    CHECK(direct > 0);
    realvnc_net_detach(direct);
    realvnc_net_installed(1, NULL);
}

int main(void) {
    install();
    testUnwritableUntilDialled();
    testStreamFlow();
    testLookupsDialNothing();
    testDatagramsThenStream();
    testStreamsOnlyRoute();
    testFailedAndCancelledDials();
    testCloseWhileHoldingBytes();
    testDetachEndsADial();
    testRouting();
    testAliasesStayApart();
    testUnresolvableNameFailsAtLookup();
    testCancelEndsALookup();
    testRefusedWhenUnhooked();
    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    printf("ok\n");
    return 0;
}
