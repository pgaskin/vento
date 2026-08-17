#!/bin/sh
# What a session is: the agent's session half, a window manager with a taskbar,
# and two windows to drag about.
xset s off -dpms          # a screen that blanks during a measurement is a lost run
xsetroot -solid '#1e3a5f'

spice-vdagent &
/usr/local/bin/apply-preferred-mode >/var/log/modes.log 2>&1 &   # PID 1 has no PATH worth the name

xterm -geometry 80x24+40+60 -bg black -fg '#00ff88' -title guest &
xterm -geometry 80x10+40+420 -bg '#202020' -fg '#dddddd' -e /bin/sh -c \
    'while true; do date; sleep 1; done' &

exec jwm
