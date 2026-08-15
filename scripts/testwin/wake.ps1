# Turn the display back on. A blanked display is served as a black desktop, so
# a run against a machine that has blanked measures nothing and says so in
# exactly the way a broken client would.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Wake {
    [DllImport("user32.dll")]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, int dx, int dy, uint data, IntPtr extra);
    [DllImport("kernel32.dll")]
    public static extern uint SetThreadExecutionState(uint flags);
}
"@
# HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, -1 = on
[Wake]::SendMessage([IntPtr]0xFFFF, 0x0112, [IntPtr]0xF170, [IntPtr](-1)) | Out-Null
# ES_CONTINUOUS | ES_DISPLAY_REQUIRED, so it stays on for as long as this
# process lives — which is not long, but the power setting is 0 anyway.
[Wake]::SetThreadExecutionState(0x80000002) | Out-Null
for ($i = 0; $i -lt 6; $i++) {
    [Wake]::mouse_event(0x0001, 8, 6, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [Wake]::mouse_event(0x0001, -8, -6, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 80
}
"woken"
