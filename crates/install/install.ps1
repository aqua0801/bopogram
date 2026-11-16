# Installs (or updates) the trial input method on THIS system. Run it yourself,
# from an elevated PowerShell, only after reading crates\docs\install.md:
#
#   powershell -ExecutionPolicy Bypass -File crates\install\install.ps1
#
# What it changes:
#   - copies bopogram_ime.dll, bopogram-host.exe and bopogram-settings.exe
#     to $Install and writes
#     bopogram-host.json there
#   - regsvr32: HKLM\SOFTWARE\Classes\CLSID\{B6B6D901-...} and
#     HKLM\SOFTWARE\Microsoft\CTF\TIP\{B6B6D901-...}
#   - adds the input method to the current user's Traditional Chinese
#     (Taiwan) keyboards (skip with -NoKeyboard)
# uninstall.ps1 reverses all of it.
param(
    # The Python that runs the engine (the prototype's virtual environment).
    [string]$Python = "D:\bopogram\prototype\.venv\Scripts\python.exe",
    # config.json of the engine runtime bundle.
    [string]$Config = "D:\bopogram-data\bopogram-runtime\config.json",
    # Must be a folder only administrators can write to: the DLL is loaded
    # into every application, elevated ones included.
    [string]$Install = (Join-Path $env:ProgramFiles "Bopogram"),
    [switch]$NoKeyboard
)
$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this from an elevated PowerShell (Run as administrator)."
}

$clsid = "{B6B6D901-0BBF-4287-A302-F265DBE9A46C}"
$profile = "{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"
$release = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release"
$dll = Join-Path $release "bopogram_ime.dll"
$hostExe = Join-Path $release "bopogram-host.exe"
$settingsExe = Join-Path $release "bopogram-settings.exe"
$reviewTool = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) "prototype\tools\review_log.py"
foreach ($path in $Python, $Config, $dll, $hostExe, $settingsExe) {
    if (-not (Test-Path $path)) { throw "missing: $path (build with 'cargo build --release' in crates\)" }
}

# 1. The engine must work before anything is registered.
$converted = & $Python -X utf8 -m bopogram.cli --config $Config convert --keys "ji3y94ru8 " 2>&1
if ($LASTEXITCODE -ne 0) { throw "the engine does not start:`n$($converted | Out-String)" }
Write-Output "Engine check passed."

# 2. Files. A DLL that applications have loaded cannot be overwritten, but it
#    can be renamed; the old copy is removed by a later install or uninstall.
Get-Process bopogram-host, bopogram-settings -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $Install | Out-Null
Get-ChildItem $Install -Filter "*.old-*" | Remove-Item -Force -ErrorAction SilentlyContinue
$stamp = Get-Date -Format "yyyyMMddHHmmss"
foreach ($file in $dll, $hostExe, $settingsExe) {
    $target = Join-Path $Install (Split-Path -Leaf $file)
    if (Test-Path $target) {
        try { Remove-Item $target -Force }
        catch { Rename-Item $target "$(Split-Path -Leaf $file).old-$stamp" }
    }
    Copy-Item $file $target
}
$hostConfig = @{ python = $Python; config = $Config; review_tool = $reviewTool } | ConvertTo-Json
[IO.File]::WriteAllText((Join-Path $Install "bopogram-host.json"), $hostConfig)
Write-Output "Files installed in $Install."

# 3. Register the text service.
$registration = Start-Process regsvr32.exe -ArgumentList "/s", "`"$(Join-Path $Install 'bopogram_ime.dll')`"" -Wait -PassThru
if ($registration.ExitCode -ne 0) { throw "regsvr32 failed with exit code $($registration.ExitCode)" }
Write-Output "Text service registered."

# 4. Add it to the user's Traditional Chinese (Taiwan) keyboards.
if (-not $NoKeyboard) {
    $languages = Get-WinUserLanguageList
    $taiwan = $languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
    $tip = "0404:$clsid$profile"
    if (-not $taiwan) {
        Write-Output "Traditional Chinese (Taiwan) is not in your language list; add the keyboard in Settings."
    } elseif (-not $taiwan.InputMethodTips.Contains($tip)) {
        $taiwan.InputMethodTips.Add($tip)
        Set-WinUserLanguageList $languages -Force
        Write-Output "Keyboard added. Switch to it with Win+Space."
    } else {
        Write-Output "Keyboard already in the list."
    }
}
Write-Output "Done. Applications that were already running use the new version after a restart."
