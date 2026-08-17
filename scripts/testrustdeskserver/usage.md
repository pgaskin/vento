# scripts/testrustdeskserver

A rendezvous server and a relay of our own — their released `hbbs` and `hbbr`
1.1.16, pinned by SHA-256 — which is the half of the id path the public network
cannot show: a server whose key came from the person connecting rather than
from a constant compiled into every build of theirs.

```sh
scripts/testrustdeskserver/run.sh        # build if needed, run, print the address and the key
scripts/testrustdeskserver/run.sh relay  # the same, telling every client to relay
scripts/testrustdeskserver/run.sh key    # the key on its own
scripts/testrustdeskserver/run.sh logs
scripts/testrustdeskserver/run.sh stop
scripts/testrustdeskserver/run.sh forget # and its identity with it
```

Its key lives in a volume, so a restart is the same server: a client that has
pinned a machine reached through it would otherwise see every restart as that
machine having changed.

A peer and a client to point at it, which `run.sh` prints:

```sh
RENDEZVOUS=<host> KEY=<key> scripts/testrustdesk/run.sh
cargo run --example rustdesk-probe -- id <peer-id> --server <host> --key <key>
scripts/session.sh -b rustdesk -o ConnectBy=id,RendezvousServer=<host>,RendezvousKey=<key> <peer-id>
```

Two things it is here to reach, and both were found with it:

- **A self-hosted server refuses a client that does not name its key.** The
  field is called `licence_key` and the refusal is `LICENSE_MISMATCH`, which
  reads like a licensing feature and is a shared secret standing in for one.
  The public network wants it empty.
- **The relay path**, which on a network where every punch lands is otherwise
  unreachable. `run.sh relay` sets their `ALWAYS_USE_RELAY`, and a peer inside
  Docker reaches it without being asked, since the address the server hands out
  for it is one nothing outside that bridge can dial.
