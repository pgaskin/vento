# Test desktop, over RDP

`testvnc`'s twin: the same Openbox session and the same four X clients, with
xrdp in front of it instead of Xvnc. That is the whole point of it — "RDP is a
different protocol, not a different server" is a claim about the control
stack, and it is only tested by driving the *same desktop*
through both.

```sh
./run.sh                 # build if needed, run, print the address for the phone
./run.sh logs            # xrdp's and sesman's own logs
./run.sh shot [out.png]  # what the far end looks like now, without a viewer
./run.sh stop
```

Defaults: port `3389`, user `proto`, password `protopass`, `security_layer=tls`.
Override with `PORT=`, `RDP_USER=`, `RDP_PASSWORD=`, `SECURITY_LAYER=` in the
environment. The desktop's **size comes from the client**, which is RDP's model
and the reverse of RFB's — so the geometry is a setting on the connection, not
on the container.

The certificate is generated on first start and its SHA-256 printed, so what the
phone shows in the trust prompt is checkable against something that is not the
phone:

```
certificate: 91:BD:B4:1E:74:09:63:F0:34:7C:96:8C:08:B8:3D:94:…
```

## What it exercises that a Windows host would not

xrdp is the server anybody can run, and it differs from Windows in the two
places a client is most likely to be wrong:

| | |
|---|---|
| **No NLA** | it authenticates through PAM behind its own login window, so `security_layer=tls` is the only thing it will negotiate. A client that offers *only* CredSSP gets `SSL_REQUIRED_BY_SERVER` and nothing else — which is exactly what happened first, and why `Nla::Prefer` offers both |
| **Bitmaps padded to four** | RDP allows a bitmap update to be wider than the rectangle it lands in, and xrdp uses that. A decoder that chunks the decoded rows by the *rectangle's* width shears the picture one row at a time, which is a bug this rig found in the published IronRDP |
| **RemoteFX** | it negotiates RFX, so the tiled codec path is the one under test rather than plain bitmap updates |

`SECURITY_LAYER=rdp` turns TLS off entirely, which is how to check that the
client refuses rather than quietly downgrading; `SECURITY_LAYER=negotiate` is
xrdp's own default.

## Auto-login

The credentials go in the Client Info PDU with `INFO_AUTOLOGON` set, so xrdp
does not draw its login window. Without that flag it draws it, prefilled, and
waits — which is worth knowing, because a session that hangs on a grey dialog
looks exactly like a client bug.
