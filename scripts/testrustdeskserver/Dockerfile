# A rendezvous server and a relay of somebody's own, which is the half of the
# id path the public network cannot show: a server whose key came from the
# person connecting rather than from a constant in every build.
#
# Their released binaries rather than a build from source, pinned by SHA-256 —
# `scripts/testrustdesk`'s rule and `third_party/`'s. Two programs come out of
# the zip: hbbs introduces two peers and hbbr copies bytes between them.
FROM debian:trixie-slim

ARG SERVER_VERSION=1.1.16
ARG SERVER_SHA256=0565c41affe6c3f409b0bddfaf5a24ccbf3f64f5f8e3fec250b69a0d5f6bdbcf

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        unzip \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl -fsSLo /tmp/server.zip \
        "https://github.com/rustdesk/rustdesk-server/releases/download/${SERVER_VERSION}/rustdesk-server-linux-amd64.zip"; \
    echo "${SERVER_SHA256}  /tmp/server.zip" | sha256sum -c -; \
    unzip -j /tmp/server.zip 'amd64/hbbs' 'amd64/hbbr' -d /usr/local/bin; \
    chmod +x /usr/local/bin/hbbs /usr/local/bin/hbbr; \
    rm /tmp/server.zip

COPY start.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

WORKDIR /root
# 21115 is their NAT test, 21116 the rendezvous itself over both TCP and UDP,
# 21117 the relay.
EXPOSE 21115 21116 21116/udp 21117
ENTRYPOINT ["/usr/local/bin/start.sh"]
