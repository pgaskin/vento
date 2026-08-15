# Register the tasks the measurement drives this machine with.
#
# An ssh session on Windows is in session 0 and the desktop the VNC servers
# serve is the interactive one, so nothing an ssh command runs can appear on it,
# take a screenshot of it, or give a window focus in it. A scheduled task with
# an Interactive principal can do all three, which is why every one of these is a
# task rather than a command.
$p = New-ScheduledTaskPrincipal -UserId "$env:COMPUTERNAME\user" -LogonType Interactive -RunLevel Highest

function Reg($name, $script) {
    $a = New-ScheduledTaskAction -Execute 'powershell.exe' `
        -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\Users\user\$script"
    Register-ScheduledTask -TaskName $name -Action $a -Principal $p -Force | Out-Null
    "registered $name"
}

Reg 'vento-wake'    'wake.ps1'       # the display, which Windows turns off by itself
Reg 'vento-notepad' 'notepad.ps1'    # the scroll workload's window, maximised and focused
Reg 'vento-mystify' 'animate.ps1'    # the animation workload
Reg 'vento-kill'    'kill.ps1'       # both of those, away again
Reg 'vento-shot'    'shot.ps1'       # what the desktop looks like, for comparing against the phone
Reg 'vento-info'    'info.ps1'       # the screen layout and the windows on it

# A machine that blanks its display serves a black desktop, and a measurement
# against it reads as a client that has stopped drawing.
powercfg /change monitor-timeout-ac 0
powercfg /change monitor-timeout-dc 0
powercfg /change standby-timeout-ac 0
"display timeouts off"
