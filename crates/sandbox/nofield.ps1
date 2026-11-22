# A window with no text field, like a game: it shows the characters it
# receives (WM_CHAR) and nothing else. With "no input context" ticked it
# also takes the input context away (ImmAssociateContext NULL), as some
# games do outside their chat line. For testing the "type in games" setting.
# Test tool only; not part of the input method.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Native -Name Imm -MemberDefinition @'
[DllImport("imm32.dll")] public static extern System.IntPtr ImmAssociateContext(System.IntPtr window, System.IntPtr context);
'@
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = "Bopogram no-field window"
$form.Size = New-Object System.Drawing.Size(640, 360)
$form.Font = New-Object System.Drawing.Font("Microsoft JhengHei UI", 14)
$form.KeyPreview = $true

$typed = New-Object System.Windows.Forms.Label
$typed.SetBounds(12, 12, 600, 200)
$typed.Text = "Click here and type. Received characters appear here."
$form.Controls.Add($typed)

$detach = New-Object System.Windows.Forms.CheckBox
$detach.Text = "No input context (ImmAssociateContext NULL)"
$detach.SetBounds(12, 230, 600, 30)
$detach.TabStop = $false
$form.Controls.Add($detach)

$script:saved = [System.IntPtr]::Zero
$script:text = ""
$detach.Add_CheckedChanged({
    if ($detach.Checked) {
        $script:saved = [Native.Imm]::ImmAssociateContext($form.Handle, [System.IntPtr]::Zero)
    } else {
        [void][Native.Imm]::ImmAssociateContext($form.Handle, $script:saved)
    }
    $form.ActiveControl = $null
})
$form.Add_KeyPress({
    param($sender, $e)
    if ($e.KeyChar -eq [char]8) {
        if ($script:text.Length -gt 0) { $script:text = $script:text.Substring(0, $script:text.Length - 1) }
    } else {
        $script:text += $e.KeyChar
    }
    $typed.Text = $script:text
    $e.Handled = $true
})
$form.Add_Shown({ $form.ActiveControl = $null })

[System.Windows.Forms.Application]::Run($form)
