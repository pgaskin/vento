Add-Type -AssemblyName System.Windows.Forms
$out = @()
$out += [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "screen $($_.DeviceName) $($_.Bounds) primary=$($_.Primary)" }
$out += Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { "window $($_.ProcessName): $($_.MainWindowTitle)" }
$out | Out-File -Encoding ascii C:\Users\user\info.txt
