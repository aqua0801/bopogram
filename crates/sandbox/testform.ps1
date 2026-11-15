# A small window with classic Win32 edit controls to type into. The sandbox
# has no Notepad. Test tool only; not part of the input method.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = "Bopogram test window"
$form.Size = New-Object System.Drawing.Size(640, 480)
$form.Font = New-Object System.Drawing.Font("Microsoft JhengHei UI", 12)

function Add-Field($label, $top, $height, $configure) {
    $caption = New-Object System.Windows.Forms.Label
    $caption.Text = $label
    $caption.SetBounds(12, $top, 600, 24)
    $box = New-Object System.Windows.Forms.TextBox
    $box.SetBounds(12, $top + 26, 600, $height)
    & $configure $box
    $form.Controls.Add($caption)
    $form.Controls.Add($box)
}

Add-Field "Multi-line edit control" 10 200 { param($box) $box.Multiline = $true; $box.ScrollBars = "Vertical" }
Add-Field "Single-line edit control" 250 30 { param($box) }
Add-Field "Password (ES_PASSWORD): the input method must stay out of the way" 320 30 {
    param($box) $box.UseSystemPasswordChar = $true
}

[System.Windows.Forms.Application]::Run($form)
