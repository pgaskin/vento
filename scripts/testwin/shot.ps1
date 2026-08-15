Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
$bmp.Save('C:\Users\user\shot.png', [System.Drawing.Imaging.ImageFormat]::Png)
"$($b.Width)x$($b.Height) at $($b.X),$($b.Y)" | Out-File -Encoding ascii C:\Users\user\shot.txt
