# Notepad, maximised, on a document long enough to scroll for a minute. The
# phone sends the Page keys, so all this does is put the window where they land.
$doc = 'C:\Users\user\scrolltext.txt'
if (-not (Test-Path $doc)) {
    $lines = 1..4000 | ForEach-Object { "{0,5}  The quick brown fox jumps over the lazy dog, and the {1} line of a document that exists to be scrolled." -f $_, $_ }
    $lines | Out-File -Encoding ascii $doc
}
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force
$p = Start-Process notepad.exe -ArgumentList $doc -PassThru
Start-Sleep -Seconds 2
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
"@
[Win]::ShowWindow($p.MainWindowHandle, 3) | Out-Null   # SW_MAXIMIZE
[Win]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
"notepad $($p.Id) maximised and focused"
