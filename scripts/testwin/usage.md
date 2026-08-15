# The Windows far end's instrumentation

The four containers beside this directory are rigs somebody can rebuild. The
Windows machine is not one: it is a real desktop with RealVNC Server, TightVNC,
UltraVNC and RDP on it, and it is where a finding that only appears against a
real server appears. What *is* reproducible is what a measurement has to be able
to do to it, and that is what this directory holds.

```sh
./install.sh                 # copy the scripts over and register the tasks
ssh user@10.33.0.208 "schtasks /run /tn vento-shot"
scp user@10.33.0.208:C:/Users/user/shot.png .
```

## Why every one of these is a scheduled task

An ssh session on Windows runs in session 0. The desktop the VNC servers serve
is the interactive session, and from session 0 you cannot put a window on it,
capture it, give anything focus in it, or synthesise input to it — a screenshot
taken from there comes back white or black, which reads exactly like a far end
that has stopped drawing. A task with an `Interactive` principal runs where the
desktop is. `vnc-compare.sh` drives these by name.

| | |
|---|---|
| `wake.ps1` | turns the display back on and moves the pointer with real input. Windows blanks its own display, and a blanked display is served as a black desktop |
| `notepad.ps1` | the scroll workload's document, maximised and focused. The keys themselves come from the phone |
| `animate.ps1` | the animation workload: bouncing lines on black, 30 times a second. Windows' own screen saver cannot be used — it runs on the screen-saver desktop, which the servers here do not follow, so the phone gets one frozen frame of it |
| `kill.ps1` | both of those away again, by window title rather than process name, since both are `powershell.exe` and one of them is this script |
| `shot.ps1` | the whole virtual screen to a PNG, which is this machine's `xwd` |
| `info.ps1` | the screen layout and the windows on it |

The machine's account is `user` throughout, which is what `setup.ps1` names in
the principal and what `install.sh` copies to.
