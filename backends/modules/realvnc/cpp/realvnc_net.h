// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The RealVNC core's connections through the session's network. The core
// dials the server itself, through its own imports of libc's socket calls,
// so the session's `vento_net` is put under it by replacing some of those
// imports (`realvnc_net_hooks`): the Linux hosting layer puts the hooks in
// the symbol map it loads the core with (`host/Loader.cxx`), and the phone
// writes them into the core's GOT once it is loaded (`realvnc_got.c`). The
// hooks are the same code on both. `vento/net.h` has the contract of the
// network itself; this file is what the hooks add for the core.
//
// What the core does on its own (read off strace and its log, Linux and the
// phone, viewer 4.9.3.60175):
//
//   - It is one thread for every session in the process (the core thread);
//     each session's connection is a non-blocking `connect` that the core
//     thread polls on its looper, so a slow dial never holds the others up.
//     The core thread looks the host up with AI_NUMERICHOST first; anything
//     that fails that is looked up again on a thread of its own
//     (IpResolveThread), and the core thread connects to the result.
//   - With EnableUdpRfb on, it first tries its UDP transport (SCTP over UDP,
//     with RTP for the picture, on the server's TCP port number, one
//     connected socket for the whole session, no addresses in the payload)
//     and falls back to TCP after 750 ms without an answer (100 ms on a
//     reconnect), or at once on an error from the UDP socket (an ICMP port
//     unreachable is ECONNREFUSED). Each transport looks the host up for
//     itself, with the same hints (SOCK_STREAM for both). Its UDP socket is
//     bound to the wildcard address and asked IP_PKTINFO and its own address
//     before `connect`; after it, only SOL_SOCKET options, `sendmsg` without
//     an address and `recvmsg`, whose address and ancillary data it does not
//     read.
//   - With AutoReconnect on, a connection that fails with an error (a reset,
//     ECONNRESET from `read`) is dialled again about a second later; one that
//     reads the end of the stream ends the session ("The connection closed
//     unexpectedly.").
//   - It keys what it remembers of a server (its identities file, the
//     unencrypted-connection consent) on the address string as given, case
//     and all ("LOCALHOST::5901"), so the address handed to the core must
//     stay the one the user typed: a placeholder there would change which
//     server the core thinks it is talking to.
//   - Nothing it calls says which session a lookup or connect is for, and
//     its callbacks do not either: with two sessions reset at once, it showed
//     the reconnect banner for only one of them and dialled for both.
//
// So sessions are told apart by host alone. Each session attaches a route
// (`realvnc_net_attach`) with the address the core is given and what
// carries it; two sessions to one host at once must be on the same route
// (both direct, or through the same tunnel), and a second one on another
// route is refused at attach, with the reason.
//
// While no session goes through a network (every one direct), each hook is
// one atomic load in front of the call it replaced: the core dials as it
// always has, with its own sockets. Once one does, the rule is that a
// session on a direct route keeps the core's own calls and native sockets,
// a session through a network has every lookup and connect of its own go
// through that network, and neither ever reaches the other's route; where
// the hooks cannot tell which a call is for, it is refused (lookups with
// EAI_FAIL, connects and sends with ENETUNREACH, each logged), never made
// directly. In detail:
//
//   - `getaddrinfo` of a host a session through a network was attached
//     with (a name or a numeric address) answers a placeholder address
//     (198.18.0.0/15, or 2001:2::/48 where only IPv6 is asked for) standing
//     for the host, since the route resolves it, not this end, and dials
//     nothing: the connect does. A lookup the core does not follow with a
//     connect leaves nothing open, and the core's UDP and TCP lookups (which
//     look alike) need not be told apart. A name is first resolved through
//     the route without connecting (`vento_net.resolve`, on the core's
//     resolver thread, as its own lookup would be), so that a name with no
//     address fails the lookup with EAI_NONAME, as it does on a direct
//     route, and the core says the name could not be resolved; where the
//     route resolves names only as it connects, the connect finds out. The
//     connect resolves the name again, through the route, so a connect
//     costs two lookups (four with the UDP transport on, which looks up and
//     connects for itself), and a name that goes away between the two fails
//     the connect. A lookup ended by the session's disconnect fails with
//     EAI_NONAME too, which the core does not get to report. With
//     AI_NUMERICHOST it answers as a direct lookup would: the placeholder
//     for a numeric host, EAI_NONAME for a name, which the core then looks
//     up again on its resolver thread.
//     Of a direct session's host, it is the core's own lookup. Of any other
//     name, while only sessions through one tunnel are open, a placeholder
//     dialled through that tunnel: the core's own business goes where the
//     sessions go. With a direct session also open, or sessions through more
//     than one tunnel, it is refused.
//   - `connect` to a placeholder is dialled through the route of the lookup
//     that gave it (where that session has closed, the latest open session
//     to its host), on a thread of its own, so that the core's thread, which
//     every session shares, never waits for a dial. The core's socket is
//     replaced at once, before `connect` returns (the core's looper
//     registers it then: an epoll registration, which belongs to the open
//     file, not the number), with one end of a socketpair the thread later
//     relays to what the network dialled (`dial_later` in realvnc_net.c),
//     keeping the socket's status and close-on-exec flags. The relay passes
//     on a half-close either way; once the core closes its socket, what the
//     relay still holds goes to the server for at most 10 s without
//     progress, as `runtime::net`'s relay does. A TCP connect
//     answers EINPROGRESS, as the core's non-blocking connect does: the
//     socket does not poll writable until the dial is done, whatever the core
//     asks of it meanwhile, and SO_ERROR then answers the dial's error (a
//     name the route cannot resolve at the connect, having resolved it at
//     the lookup or being unable to resolve without connecting, is
//     EHOSTUNREACH: the core says it could not reach the computer rather
//     than that the name is unknown) or 0. A
//     UDP connect returns 0 at once, as UDP's does, and the datagrams the
//     core sends wait for the dial; where the route carries streams only, or
//     the dial fails, the socket reads its end of file at once, which
//     `recvmsg` answers ECONNREFUSED, and the core falls back to TCP. A dial
//     ended by the session's disconnect leaves the connect in progress (or
//     the UDP socket silent), as a direct one is left when the core closes
//     the session during it. A blocking TCP socket, or a socket of any other
//     kind, is refused. The dial asks for no keep-alive: the core sets none
//     on its TCP socket (it has its own, in RFB), and options it sets after
//     `connect` land on the socketpair's end.
//   - `connect` to an address that is not a placeholder is the core's own
//     where a direct session is open (its own lookup gave the address, which
//     a session through a network never is given), and refused otherwise.
//   - On such a socket, which is a socketpair's end, `getpeername` answers
//     with the placeholder the core connected to, `getsockname` with this
//     end's address on the route where it has one and the unspecified
//     address otherwise, SO_DOMAIN, SO_TYPE and SO_PROTOCOL as a TCP or UDP
//     socket's, and TCP and IP options are accepted and answered with
//     plausible values. Other SOL_SOCKET options go to the socket.
//   - On such a UDP socket, `recvmsg` returning the socketpair's end of file
//     and `sendmsg` failing EPIPE (sent with MSG_NOSIGNAL) mean the network
//     has stopped carrying the datagrams, and fail with the error the route
//     recorded (`vento_net.ended`): ECONNREFUSED for the server's port
//     unreachable, as on a direct UDP socket, and where the route says
//     nothing, so that the core falls back to TCP or reconnects as it does
//     there. What the network cannot pass on of UDP: its ancillary data and
//     IP-level options, which the core does not use once connected.
//   - `read`, `__read_chk` and `recvmsg` on such a TCP socket that would
//     return the end of the stream ask the route how the connection ended
//     (`vento_net.ended`) and fail as a direct socket would have: ECONNRESET
//     for a server's reset (so that the core reconnects, as it does on a
//     direct route), ETIMEDOUT, the error itself, ECONNABORTED for the
//     route's own failure (its words logged).
//   - `getnameinfo` of a placeholder gives the host back.
//   - `sendmsg` with an address of its own (a UDP socket not connected
//     through the hooks, which the core has not been seen to use for a
//     session) is the core's own where a direct session is open and the
//     address is not a placeholder, and refused otherwise.
//
// The calls are the core's, which is a bionic binary: `addrinfo` is in
// bionic's layout and the EAI_, AI_ and NI_ numbers are bionic's, also on
// Linux, where the hooks sit in front of the hosting layer's conversions.

#ifndef VENTO_REALVNC_NET_H
#define VENTO_REALVNC_NET_H

#include <stddef.h>
#include <sys/socket.h>

#include <vento/net.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One replaced import: the hook, and where the hook keeps the function it
   replaced, which the installer fills in before the hook can be called. */
typedef struct realvnc_net_hook {
    const char *name;
    void *hook;
    void **next;
} realvnc_net_hook;

/* The hooks, by import name: connect, getaddrinfo, freeaddrinfo,
   getnameinfo, getpeername, getsockname, getsockopt, setsockopt, read,
   __read_chk, recvmsg, sendmsg. Every one is to be installed, or none. */
#define REALVNC_NET_HOOKS 12
extern const realvnc_net_hook realvnc_net_hooks[REALVNC_NET_HOOKS];

/* Called by the installer once it has put every hook in place (`ok`
   nonzero), or has found it cannot (`why` says why). Until it is called
   with `ok`, no route that is not direct can be attached. */
void realvnc_net_installed(int ok, const char *why);

/* Installs `notice`, called with one line of text (the core's resolver
   thread, or whichever thread the core looks a name up on) when the hooks
   refuse a lookup they cannot assign to a session: a direct session and a
   tunnelled one open at once, or sessions through more than one tunnel.
   The text is "bug: unhandled realvnc host lookup <host>". The platform
   shows it (a toast on the phone). NULL for none. */
void realvnc_net_set_notice(void (*notice)(const char *text));

/* Routes the connections of a session to `address` (as the core is given
   it: `host`, `host:display`, `host::port`, `[literal]::port`) through
   `net`, which is copied and owned from here on, also on failure; NULL for a
   direct route, which the core dials itself. `key` names what carries the
   route (`Network::tunnel`, e.g. "dev-socketpair"; ignored for a direct
   one): sessions with the same key are on the same route. `timeout_ms`
   bounds each dial and each lookup as `vento_net.connect`'s and
   `resolve`'s do. Returns the route's
   number, or 0 with the reason in `why` (may be NULL) where there are too
   many sessions, where another session to the same host is open on another
   route, or where `net` is given and the hooks are not in place: such a
   session must not start, direct or not, since the core would dial
   directly or for the wrong session. */
int realvnc_net_attach(const vento_net *net, const char *key, const char *address, int timeout_ms,
                       char *why, size_t why_len);

/* The session has closed (the core reported it closed, or it was closed
   before it connected): from here on nothing of the core's is for it, and
   another session to its host may attach on another route. Any thread. */
void realvnc_net_closed(int route);

/* Ends a dial or a lookup in progress on the route, and fails every later
   one: the session's disconnect. Any thread. */
void realvnc_net_cancel(int route);

/* Removes the route: ends a dial or a lookup in progress on it as
   `realvnc_net_cancel` does, waits for it and any `ended` call on it to
   return, ends the relays
   of its connections (the core's sockets then read the end of the stream),
   and releases its `net`. */
void realvnc_net_detach(int route);

/* Whether the route goes through a `vento_net`. */
int realvnc_net_relayed(int route);

#ifdef __cplusplus
}
#endif

#endif
