// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The session's connections for a C backend: a table of calls over
// `runtime::net::Network`, filled in by `api::c::net` and passed in a
// backend's config, so that a C library's connections are made where the
// Rust backends' are (the route, the name lookup, a hang-up) and only the
// connected socket is handed to the library. Function pointers rather than
// symbols, for the reason `vento/video.h` gives. `runtime::net`'s
// documentation has the whole contract; what a C backend must do:
//
// - Make every connection through `connect`, and every UDP flow through
//   `connect_datagram`. A library path that would open a socket or look a
//   name up by itself (a listening socket, a second connection, UDP outside
//   `connect_datagram`, a repeater) is refused by the backend's glue when the
//   route is not direct (`direct` below is 0), with an error the user sees.
//   A library that looks a name up before it connects asks `resolve`, which
//   opens nothing. Only a backend with no table (the C probes) dials by
//   itself.
// - The fd is a TCP socket on a direct route and one end of an AF_UNIX
//   socketpair where vento carries the bytes (a tunnel, or the socketpair
//   test mode). Both are SOCK_STREAM. On the latter, TCP socket options fail
//   (EOPNOTSUPP) and are to be ignored, and getpeername and getsockname name
//   no IP address: `vento_net_peer` says what is known of the two ends, and
//   either address may be missing.
// - Ask for liveness (keep-alive, TCP_USER_TIMEOUT) with the connect
//   (`vento_net_liveness`): on a relayed route the socket the library holds
//   is not the one that reaches the server.
// - When the stream ends or fails, ask `ended` why: a socketpair reads every
//   ending (a reset, a timeout, the tunnel's own failure) as the end of the
//   stream, so the route records the real cause, and the message the user
//   sees is worded from it.

#ifndef VENTO_NET_H
#define VENTO_NET_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* How a connection reaches its server: `runtime::net::Route`. */
#define VENTO_NET_DIRECT 0 /* this end's own TCP connection: the fd is a TCP socket */
#define VENTO_NET_TUNNEL 1 /* bytes carried by vento: the fd is a socketpair's end */

/* Why `connect` failed, negative. `why` has the route's own words. */
#define VENTO_NET_EFAILED (-1)      /* anything below does not cover */
#define VENTO_NET_ERESOLVE (-2)     /* the name has no address, or its lookup took too long */
#define VENTO_NET_EREFUSED (-3)     /* nothing listens there */
#define VENTO_NET_ETIMEDOUT (-4)    /* no address answered in time */
#define VENTO_NET_EUNREACHABLE (-5) /* no route to the host or its network */
#define VENTO_NET_ECANCELLED (-6)   /* `cancel` was called: the session is ending */
#define VENTO_NET_ENOTSUP (-7)      /* the route does not carry it (datagrams on a route of streams) */

/* How a connection ended, from `ended`: `runtime::net::End`. */
#define VENTO_NET_END_UNKNOWN 0  /* nothing recorded: the fd reports its own end (a direct route), or it has not ended */
#define VENTO_NET_END_CLOSED 1   /* the server closed its side (FIN) */
#define VENTO_NET_END_RESET 2    /* the server reset the connection */
#define VENTO_NET_END_TIMEDOUT 3 /* the connection to the server stopped answering */
#define VENTO_NET_END_ERROR 4    /* another error on the connection to the server */
#define VENTO_NET_END_TUNNEL 5   /* the route itself failed; `why` has its words */
#define VENTO_NET_END_LOCAL 6    /* this end closed or shut down its fd */

/* What is known of a connection's two ends: `runtime::net::Peer`. */
typedef struct vento_net_peer {
    /* The server's address as the connection reached it, numeric, and its
       port; "" and 0 where the route does not know it (a tunnel that
       resolves the name at its far end). */
    char address[64];
    int port;
    /* The address and port the server sees the connection come from; "" and
       0 where the route does not know them. */
    char local_address[64];
    int local_port;
    int route; /* VENTO_NET_DIRECT or VENTO_NET_TUNNEL */
    /* The connection, for `ended`: a number above 0, unique within the
       table. */
    int connection;
} vento_net_peer;

/* What the backend would have set on its own TCP socket to notice a server
   that has gone away: `runtime::net::Liveness`. The network sets it where it
   reaches the server (on a direct route, on the fd it returns). */
typedef struct vento_net_liveness {
    int keepalive;            /* nonzero: SO_KEEPALIVE with the timing below */
    int keepalive_idle_s;     /* TCP_KEEPIDLE */
    int keepalive_interval_s; /* TCP_KEEPINTVL */
    int keepalive_count;      /* TCP_KEEPCNT */
    int user_timeout_ms;      /* TCP_USER_TIMEOUT; 0 for the system's */
} vento_net_liveness;

typedef struct vento_net {
    /* Passed to every call. */
    void *user;
    /* Nonzero when every fd is this end's own TCP socket to the server
       (`runtime::net::Network::direct`). Where it is 0, nothing of the
       session's may reach the server except through `connect`. */
    int direct;
    /* A connected stream to `host`:`port`, as a blocking, close-on-exec fd
       the caller owns (`shutdown` it to end a read in progress, `close` it
       when done), or a VENTO_NET_E* code: any value of 0 or more is an fd (0
       included), any negative one a failure. Where vento carries the bytes,
       its relay for the connection runs until the fd is closed or shut down
       both ways, also after `release`. `host` is the name as the user typed
       it, without an IPv6 literal's brackets: resolving it is the route's
       business. The lookup, and then each address in turn, may take up to
       `timeout_ms` (0 or less for 20 seconds), as the libraries' own dials
       gave each address (`runtime::net::Timeout::PerAddress`); `cancel` ends
       either at once.
       `liveness` may be NULL for none. `peer` is filled in on success and may
       be NULL; `why` gets the reason on failure, NUL-terminated and truncated
       to `why_len`, and may be NULL. Any thread, several at once. */
    int (*connect)(void *user, const char *host, int port, int timeout_ms,
                   const vento_net_liveness *liveness, vento_net_peer *peer, char *why,
                   size_t why_len);
    /* A connected datagram socket to `host`:`port`, as a blocking,
       close-on-exec fd the caller owns, or a VENTO_NET_E* code:
       VENTO_NET_ENOTSUP where the route carries streams only, which a backend
       with a stream to fall back to takes as a server that does not answer
       over UDP. The fd is a UDP socket connected to one of the server's
       addresses on a direct route, and one end of an AF_UNIX SOCK_SEQPACKET
       socketpair where vento carries the datagrams: one datagram per send and
       per recv either way, boundaries kept, but no IP address, no IP-level
       options, no ancillary data, and no ICMP error from the server as such:
       where vento carries the datagrams, an error from the server's side (an
       ICMP port unreachable) closes the socketpair's other end, which reads
       as its end of file (recv 0 with POLLHUP, send failing EPIPE), and
       `ended` says what it was (VENTO_NET_END_ERROR with ECONNREFUSED for a
       port unreachable). A datagram it cannot pass on at once is lost, as UDP
       loses it. Its relay runs until the fd is closed or shut down both ways;
       a shutdown(SHUT_WR) ends only what the backend sends. Nothing is sent
       by the call. `host`, `timeout_ms`, `peer` (its `connection` for
       `ended`) and `why` as `connect`'s. Any thread. */
    int (*connect_datagram)(void *user, const char *host, int port, int timeout_ms,
                            vento_net_peer *peer, char *why, size_t why_len);
    /* Whether `host` has an address where the route resolves names, without
       connecting to it: 0 where it has; VENTO_NET_ERESOLVE where it has none
       or its lookup took longer than `timeout_ms` (0 or less for 20
       seconds); VENTO_NET_ECANCELLED after `cancel`; VENTO_NET_ENOTSUP where
       the route resolves names only as it connects (a tunnel whose far end
       takes a name and answers with a connection or a refusal), which the
       backend takes as resolved and learns otherwise at the connect. For a
       library that looks a name up before it connects, so that a name with
       no address fails at its lookup, as it does on a direct route. The
       addresses themselves are not given: the backend never dials them
       (`connect` resolves the name again, and may find something else by
       then), and a route that resolves at its far end has none this end
       could use. A literal address is its own answer on every route. `host`
       and `why` as `connect`'s. Any thread. */
    int (*resolve)(void *user, const char *host, int timeout_ms, char *why, size_t why_len);
    /* How connection `connection` (from its `vento_net_peer`) ended, as a
       VENTO_NET_END_* code. `error` (may be NULL) gets the errno a read on a
       direct TCP socket would have failed with (ECONNRESET for a reset,
       ETIMEDOUT, the error itself; ECONNABORTED for a tunnel's failure; 0
       where it would have read the end of the stream, or nothing is known).
       `why` (may be NULL) gets the reason in words: strerror's, or the
       tunnel's own. Recorded before the backend's fd can see the end, so it
       is there once a read has returned 0 or failed. Any thread, also after
       the fd is closed, until `release`; not after it, when the table is
       gone. */
    int (*ended)(void *user, int connection, int *error, char *why, size_t why_len);
    /* Ends every `connect`, `connect_datagram` and `resolve` in progress (a
       lookup or a dial) with VENTO_NET_ECANCELLED, and makes every later one
       fail the same way: the session's disconnect. It does nothing to fds already
       returned, which the backend shuts down or closes itself. Any thread,
       any number of times, until `release`. */
    void (*cancel)(void *user);
    /* The backend owns the table from the moment it is handed over. Called
       exactly once (a second call frees it twice), after the last
       `connect`, `connect_datagram`, `resolve`, `ended` and `cancel` have
       returned, and
       also when the backend fails to start. No call may follow it. */
    void (*release)(void *user);
} vento_net;

#ifdef __cplusplus
}
#endif

#endif
