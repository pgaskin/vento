#!/bin/bash
# Put this directory on the Windows machine and register what it holds.
#
#   ./install.sh [user@host]        # default user@10.33.0.208
#
# The machine itself is not reproducible and is not meant to be: it is the one
# real far end here, with three VNC servers and an RDP one on it. What this
# makes reproducible is the *instrumentation* — the six things a measurement
# needs to be able to do to a desktop it cannot reach over ssh.
set -euo pipefail
cd "$(dirname "$0")"

HOST="${1:-user@10.33.0.208}"

scp -q ./*.ps1 "$HOST:C:/Users/user/"
ssh "$HOST" 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\user\setup.ps1'
