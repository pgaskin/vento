# A Mystify-style animation in an ordinary maximised window: bouncing lines on
# black, redrawn 30 times a second.
#
# Windows' own screen saver cannot be the workload. It runs on the screen-saver
# desktop, which the VNC servers here do not follow — the phone is handed one
# frozen frame of it and nothing after, and a capture taken in the interactive
# session comes back white. So the animation is a window on the normal desktop,
# which is what a client would have to carry anyway.
#
# Drawn with DrawLine rather than DrawPolygon: an exception inside a WinForms
# paint handler is swallowed, so a bad argument is a window that paints its
# background and nothing else — which reads as a client with nothing to draw.
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$script:form = New-Object System.Windows.Forms.Form
$form.Text = 'animation'
$form.BackColor = 'Black'
$form.FormBorderStyle = 'None'
$form.WindowState = 'Maximized'
$form.TopMost = $true

# Double buffering through the protected property, since the form is not a
# subclass here.
$form.GetType().GetProperty('DoubleBuffered', 'Instance,NonPublic').SetValue($form, $true)

$script:n = 8
$script:x1 = New-Object 'double[]' $n; $script:y1 = New-Object 'double[]' $n
$script:x2 = New-Object 'double[]' $n; $script:y2 = New-Object 'double[]' $n
$script:d1x = New-Object 'double[]' $n; $script:d1y = New-Object 'double[]' $n
$script:d2x = New-Object 'double[]' $n; $script:d2y = New-Object 'double[]' $n
$rand = New-Object System.Random 12345          # the same animation every run
for ($i = 0; $i -lt $n; $i++) {
    $x1[$i] = $rand.Next(0, 1900); $y1[$i] = $rand.Next(0, 1400)
    $x2[$i] = $rand.Next(0, 1900); $y2[$i] = $rand.Next(0, 1400)
    $d1x[$i] = $rand.Next(8, 22); $d1y[$i] = $rand.Next(8, 22)
    $d2x[$i] = -$rand.Next(8, 22); $d2y[$i] = $rand.Next(8, 22)
}
$script:pens = @(
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::Cyan), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::Magenta), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::Yellow), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::LimeGreen), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::Orange), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::White), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::DeepSkyBlue), 4),
    (New-Object System.Drawing.Pen ([System.Drawing.Color]::Red), 4))

$form.Add_Paint({
    param($s, $e)
    for ($i = 0; $i -lt $script:n; $i++) {
        $e.Graphics.DrawLine($script:pens[$i],
            [int]$script:x1[$i], [int]$script:y1[$i],
            [int]$script:x2[$i], [int]$script:y2[$i])
    }
})

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 33
$timer.Add_Tick({
    $w = $script:form.ClientSize.Width; $h = $script:form.ClientSize.Height
    for ($i = 0; $i -lt $script:n; $i++) {
        $script:x1[$i] += $script:d1x[$i]; $script:y1[$i] += $script:d1y[$i]
        $script:x2[$i] += $script:d2x[$i]; $script:y2[$i] += $script:d2y[$i]
        if ($script:x1[$i] -lt 0 -or $script:x1[$i] -gt $w) { $script:d1x[$i] = -$script:d1x[$i] }
        if ($script:y1[$i] -lt 0 -or $script:y1[$i] -gt $h) { $script:d1y[$i] = -$script:d1y[$i] }
        if ($script:x2[$i] -lt 0 -or $script:x2[$i] -gt $w) { $script:d2x[$i] = -$script:d2x[$i] }
        if ($script:y2[$i] -lt 0 -or $script:y2[$i] -gt $h) { $script:d2y[$i] = -$script:d2y[$i] }
    }
    $script:form.Invalidate()
})
$timer.Start()
[System.Windows.Forms.Application]::Run($form)
