# Installs or updates the input method on THIS system. Run it yourself, from
# an elevated PowerShell, only after reading crates\docs\install.md:
#
#   powershell -ExecutionPolicy Bypass -File crates\install\install.ps1
#
# Updating is running it again; no sign-out is needed. Applications that are
# running keep the version they loaded until they restart (the script lists
# them); everything started afterwards gets the new one.
#
# What it changes:
#   - copies bopogram_ime.dll, bopogram-host.exe, bopogram-settings.exe,
#     bopogram.exe (the Rust engine), bopogram.ico, uninstall.ps1 and
#     switch-engine.ps1 (with common.ps1) to $Install, the 32-bit
#     bopogram_ime.dll to $Install\x86, and writes bopogram-host.json there
#     (which engine runs: -Engine rust, the default, or python).
#     Files in use are renamed (*.old-<time>) and deleted at the next restart
#     (PendingFileRenameOperations) or by the next install.
#   - regsvr32 (64- and 32-bit): HKLM\SOFTWARE\Classes\CLSID\{B6B6D901-...},
#     its 32-bit view under WOW6432Node, and HKLM\SOFTWARE\Microsoft\CTF\TIP\{B6B6D901-...}
#   - adds the input method to the current user's Traditional Chinese
#     (Taiwan) keyboards (skip with -NoKeyboard)
#   - a Start menu shortcut to the settings window, for all users
#   - starts the host at sign-in: HKCU\Software\Microsoft\Windows\CurrentVersion\Run,
#     value "Bopogram" (skip with -NoAutostart)
# uninstall.ps1 reverses all of it.
param(
    # Which engine the host runs: "rust" (bopogram.exe) or "python" (the
    # prototype). Both are installed; switch-engine.ps1 changes it later.
    [ValidateSet("rust", "python")]
    [string]$Engine = "rust",
    # The Rust engine's model files (*.bglm, from `bopogram build-lm`).
    [string]$Models = "D:\bopogram-data\arpa",
    # The prototype's Python: runs the Python engine, and the typing log
    # review tool of the settings window.
    [string]$Python = "D:\bopogram\prototype\.venv\Scripts\python.exe",
    # config.json of the engine runtime bundle.
    [string]$Config = "D:\bopogram-data\bopogram-runtime\config.json",
    # Must be a folder only administrators can write to: the DLL is loaded
    # into every application, elevated ones included.
    [string]$Install = (Join-Path $env:ProgramFiles "Bopogram"),
    # The typing log review tool opened by the settings window.
    [string]$ReviewTool = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) "prototype\tools\review_log.py"),
    # Debugging only: the engine writes every request and response (so,
    # everything typed) to this file. Install again without it to turn it off.
    [string]$Trace = "",
    [switch]$NoKeyboard,
    [switch]$NoAutostart
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
Assert-Elevated

$crates = Split-Path -Parent $PSScriptRoot
$release = Join-Path $crates "target\release"
$dll = Join-Path $release "bopogram_ime.dll"
$hostExe = Join-Path $release "bopogram-host.exe"
$settingsExe = Join-Path $release "bopogram-settings.exe"
$engineExe = Join-Path $release "bopogram.exe"
$dll32 = Join-Path $crates "target\i686-pc-windows-msvc\release\bopogram_ime.dll"
$icon = Join-Path $crates "bopogram-ime\res\bopogram.ico"
$scripts = (Join-Path $PSScriptRoot "uninstall.ps1"), (Join-Path $PSScriptRoot "common.ps1"),
    (Join-Path $PSScriptRoot "switch-engine.ps1")
foreach ($path in @($Config, $dll, $hostExe, $settingsExe, $engineExe, $icon) + $scripts) {
    if (-not (Test-Path $path)) { throw "missing: $path (build with 'cargo build --release' in crates\)" }
}
if (-not (Test-Path $dll32)) {
    throw "missing: $dll32 (build with 'cargo build --release -p bopogram-ime --target i686-pc-windows-msvc' in crates\)"
}
if ($Engine -eq "python" -and -not (Test-Path $Python)) { throw "missing: $Python" }

# 1. The engine must work before anything is registered.
Test-Engine $Engine $engineExe $Models $Python $Config
Write-Output "Engine check passed ($Engine)."

# 2. Files. A DLL that applications have loaded cannot be overwritten, but it
#    can be renamed; the renamed copy goes at the next restart.
$running = Get-ImeUsers
Stop-Bopogram
$install32 = Join-Path $Install "x86"
New-Item -ItemType Directory -Force $Install, $install32 | Out-Null
Get-ChildItem $Install, $install32 -Filter "*.old-*" | ForEach-Object { [void](Remove-OrSchedule $_.FullName) }
$stamp = Get-Date -Format "yyyyMMddHHmmss"
$pairs = @(@($dll, $Install), @($hostExe, $Install), @($settingsExe, $Install), @($engineExe, $Install),
    @($icon, $Install), @($dll32, $install32)) + @($scripts | ForEach-Object { , @($_, $Install) })
foreach ($pair in $pairs) {
    $file, $folder = $pair
    $name = Split-Path -Leaf $file
    $target = Join-Path $folder $name
    if (Test-Path $target) {
        try { Remove-Item $target -Force }
        catch {
            $old = Join-Path $folder "$name.old-$stamp"
            Rename-Item $target $old
            Remove-AtRestart $old
        }
    }
    Copy-Item $file $target
}
$hostConfig = @{ python = $Python; config = $Config; review_tool = $ReviewTool; models = $Models }
if ($Engine -eq "rust") { $hostConfig.engine = Join-Path $Install "bopogram.exe" }
if ($Trace) { $hostConfig.trace = $Trace }
[IO.File]::WriteAllText((Join-Path $Install "bopogram-host.json"), ($hostConfig | ConvertTo-Json))
Write-Output "Files installed in $Install (engine: $Engine)."
if ($Trace) { Write-Output "TRACE IS ON: everything typed is written to $Trace. Install again without -Trace to turn it off." }

# 3. Register the text service.
$registration = Start-Process regsvr32.exe -ArgumentList "/s", "`"$(Join-Path $Install 'bopogram_ime.dll')`"" -Wait -PassThru
if ($registration.ExitCode -ne 0) { throw "regsvr32 failed with exit code $($registration.ExitCode)" }
$registration = Start-Process "$env:SystemRoot\SysWOW64\regsvr32.exe" -ArgumentList "/s", "`"$(Join-Path $install32 'bopogram_ime.dll')`"" -Wait -PassThru
if ($registration.ExitCode -ne 0) { throw "regsvr32 (x86) failed with exit code $($registration.ExitCode)" }
Write-Output "Text service registered (64- and 32-bit)."

# 4. Add it to the user's Traditional Chinese (Taiwan) keyboards.
if (-not $NoKeyboard) {
    $languages = Get-WinUserLanguageList
    $taiwan = $languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
    if (-not $taiwan) {
        Write-Output "Traditional Chinese (Taiwan) is not in your language list; add the keyboard in Settings."
    } elseif (-not $taiwan.InputMethodTips.Contains($Tip)) {
        $taiwan.InputMethodTips.Add($Tip)
        Set-WinUserLanguageList $languages -Force
        Write-Output "Keyboard added. Switch to it with Win+Space."
    } else {
        Write-Output "Keyboard already in the list."
    }
}

# 5. The settings window in the Start menu.
$link = (New-Object -ComObject WScript.Shell).CreateShortcut($Shortcut)
$link.TargetPath = Join-Path $Install "bopogram-settings.exe"
$link.WorkingDirectory = $Install
$link.IconLocation = "$(Join-Path $Install 'bopogram.ico'),0"
$link.Description = "Bopogram"
$link.Save()
Write-Output "Start menu shortcut: $Shortcut"

# 6. Start the host at sign-in, so that the engine is ready before the first
#    key, and so that applications that may not start it themselves
#    (elevated ones, Store apps) find it running. And start it now.
if (-not $NoAutostart) {
    Set-ItemProperty $RunKey -Name "Bopogram" -Value "`"$(Join-Path $Install 'bopogram-host.exe')`""
    Write-Output "The engine host starts at sign-in."
} elseif (Get-ItemProperty $RunKey -Name "Bopogram" -ErrorAction SilentlyContinue) {
    Remove-ItemProperty $RunKey -Name "Bopogram"
    Write-Output "The engine host no longer starts at sign-in."
}
Start-Unelevated (Join-Path $Install "bopogram-host.exe")
Write-Output "Engine host started."

if ($running.Count -gt 0) {
    Write-Output "Done. These programs keep the previous version until they restart:"
    Write-Output "  $($running -join ', ')"
    Write-Output "  (explorer: restart Windows Explorer from Task Manager; Start menu search: SearchHost restarts by itself when ended in Task Manager.)"
} else {
    Write-Output "Done."
}
