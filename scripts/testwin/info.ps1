Add-Type -AssemblyName System.Windows.Forms
$out = @()
$out += [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "screen $($_.DeviceName) $($_.Bounds) primary=$($_.Primary)" }
# In the same coordinates the screens above are in, which is what says which of
# them a client's pointer actually reached. A screenshot cannot answer it: the
# capture APIs here do not draw the cursor.
$out += "cursor $([System.Windows.Forms.Cursor]::Position)"
$out += Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { "window $($_.ProcessName): $($_.MainWindowTitle)" }
$out | Out-File -Encoding ascii C:\Users\user\info.txt
