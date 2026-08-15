# A key-layout debugger for a Windows RDP far end - the analogue of `xev` for
# the X test desktops. RDP carries a POSITION (a set-1 scancode), and the server
# turns it into a virtual key using its own layout; this shows which one, so our
# client's scancode table can be checked against what Windows actually resolves.
#
# Run it in the RDP session and give its window focus (like xev, it only sees
# keys while focused). Each key that arrives is shown and appended to a file.
#
#   powershell -ExecutionPolicy Bypass -File winkeys.ps1
#   powershell -ExecutionPolicy Bypass -File winkeys.ps1 -Out C:\Users\Public\farkeys.txt
#
# Then drive the keyboard from the phone (scripts/hidkbd + scripts/hidkbd/farwalk
# style) and read the file back. The window shows the last keys live so a person
# watching the RDP session can see each one land.
#
# ASCII only, unlike everything else here, because Windows PowerShell reads a
# .ps1 as ANSI unless it has a byte-order mark: a UTF-8 em dash arrives as three
# CP1252 characters of which the last is a curly closing quote, which PowerShell
# honours as a string delimiter, and the file stops parsing mid-line. Whether a
# copy of this file keeps its BOM is up to whatever carried it here, so it is
# simpler for the file to have nothing that needs one.
param([string]$Out = "$env:USERPROFILE\farkeys.txt")

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

# A window message carries the generic virtual key for a modifier - VK_SHIFT for
# both shifts - and the side only in the scan code, which WinForms does not hand
# over. The key state does distinguish them, and is current while the key is
# still down, which is when this runs.
Add-Type -Namespace Win -Name Kbd -MemberDefinition @'
[DllImport("user32.dll")] public static extern short GetKeyState(int vk);
'@
$sides = @{ 16 = @(160, 161); 17 = @(162, 163); 18 = @(164, 165) }   # Shift, Control, Alt

"# started $(Get-Date -Format o)" | Out-File $Out -Encoding ascii

$form = New-Object System.Windows.Forms.Form
$form.Text = "RDP key layout debugger - give me focus, then type"
$form.Width = 640; $form.Height = 480
$form.KeyPreview = $true
# It only sees keys while focused, and the console it was started from is a
# window of its own that opens over it.
$form.TopMost = $true
$form.Add_Shown({ $form.Activate() })

$box = New-Object System.Windows.Forms.TextBox
$box.Multiline = $true; $box.Dock = "Fill"; $box.ReadOnly = $true
$box.ScrollBars = "Vertical"
$box.Font = New-Object System.Drawing.Font("Consolas", 12)
# An empty one, because the default context menu is what the Apps key opens -
# a menu takes the keyboard, and every key after it in a walk is then a blank
# that reads exactly like a key the server dropped.
$box.ContextMenuStrip = New-Object System.Windows.Forms.ContextMenuStrip
$form.ContextMenuStrip = $box.ContextMenuStrip
$form.Controls.Add($box)

$down = @{}

function Show-Key($e, $suffix) {
    # Keys names the virtual keys a .NET application expects and not the ones a
    # layout can resolve a scancode to, and it is a [Flags] enum, so everything
    # it has no member for prints as "None" whatever its value - which is three
    # different keys under one name in a walk. The number is the answer there.
    $name = if ([enum]::IsDefined([System.Windows.Forms.Keys], [int]$e.KeyValue)) {
        $e.KeyCode.ToString()
    } else {
        "VK_0x{0:X2}" -f $e.KeyValue
    }
    $line = "{0,-18} 0x{1:X2}{2}" -f $name, $e.KeyValue, $suffix
    $box.AppendText($line + "`r`n")
    $line | Out-File $Out -Append -Encoding ascii
}

# KeyCode is the resolved virtual key (Keys.F13, Keys.NumPad7, Keys.Pause, ...);
# KeyValue is its number, and the side comes off the key state above.
$onDown = {
    param($s, $e)
    $side = ""
    if ($sides.ContainsKey([int]$e.KeyValue)) {
        $l, $r = $sides[[int]$e.KeyValue]
        if ([Win.Kbd]::GetKeyState($r) -lt 0) { $side = " right" }
        elseif ([Win.Kbd]::GetKeyState($l) -lt 0) { $side = " left" }
    }
    Show-Key $e $side
    $down[[int]$e.KeyValue] = $true
    $e.SuppressKeyPress = $true   # keep the box read-only and swallow the key
}
# Print Screen is delivered as a key-up alone, so a down-only log would call the
# one key on this walk that Windows treats specially a key that never arrived.
$onUp = {
    param($s, $e)
    $k = [int]$e.KeyValue
    # Hashtable.Remove is void here, not the bool Dictionary.Remove returns, so
    # the test has to come first or every key is logged twice.
    if ($down.ContainsKey($k)) { $down.Remove($k); return }
    Show-Key $e " up-only"
}
$form.Add_KeyDown($onDown)
$form.Add_KeyUp($onUp)

[System.Windows.Forms.Application]::Run($form)
