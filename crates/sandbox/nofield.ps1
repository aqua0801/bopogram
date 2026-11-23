# A window with no text field, like a game: it draws the characters it
# receives (WM_CHAR) and has no edit control or text store. Unlike a plain
# Windows Forms form it keeps input methods enabled (CanEnableIme), as games
# do: Microsoft's input methods can type into it blind. For testing the
# "type in games" setting. Test tool only; not part of the input method.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Windows.Forms;

public class NoFieldWindow : Form {
    string typed = "";

    public NoFieldWindow() {
        Text = "Bopogram no-field window";
        Size = new Size(640, 360);
        Font = new Font("Microsoft JhengHei UI", 14);
        ImeMode = ImeMode.On;
        DoubleBuffered = true;
    }

    // A form with no text control disables input methods by default.
    protected override bool CanEnableIme { get { return true; } }

    protected override void OnKeyPress(KeyPressEventArgs e) {
        if (e.KeyChar == '\b') {
            if (typed.Length > 0) typed = typed.Substring(0, typed.Length - 1);
        } else {
            typed += e.KeyChar;
        }
        e.Handled = true;
        Invalidate();
    }

    protected override void OnPaint(PaintEventArgs e) {
        e.Graphics.DrawString("No text field here. Type; received characters appear below.",
            Font, Brushes.Gray, 12, 12);
        e.Graphics.DrawString(typed, Font, Brushes.Black, new RectangleF(12, 60, ClientSize.Width - 24, ClientSize.Height - 72));
    }
}
'@
[System.Windows.Forms.Application]::EnableVisualStyles()
[System.Windows.Forms.Application]::Run((New-Object NoFieldWindow))
