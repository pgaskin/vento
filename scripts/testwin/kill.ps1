Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force
# The animation window, and whatever an earlier stage left on the desktop. By
# window title rather than by process name: both are powershell.exe, and one of
# them is this script.
Get-Process | Where-Object { $_.MainWindowTitle -match 'animation|key layout debugger' } | Stop-Process -Force
"killed"
