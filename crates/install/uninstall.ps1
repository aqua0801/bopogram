# Removes the trial input method from this system. Run it yourself, from an
# elevated PowerShell:
#
#   powershell -ExecutionPolicy Bypass -File crates\install\uninstall.ps1
#
# Personal data (the runtime bundle's userdata\ folder) and
# %LOCALAPPDATA%\Bopogram (host log, style setting) are left alone.
param(
    [string]$Install = (Join-Path $env:ProgramFiles "Bopogram")
)
$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this from an elevated PowerShell (Run as administrator)."
}

$clsid = "{B6B6D901-0BBF-4287-A302-F265DBE9A46C}"
$profile = "{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"

# 1. Take it out of the user's keyboards first, so nothing tries to load it.
$languages = Get-WinUserLanguageList
$tip = "0404:$clsid$profile"
$changed = $false
foreach ($language in $languages) {
    if ($language.InputMethodTips.Contains($tip)) {
        [void]$language.InputMethodTips.Remove($tip)
        $changed = $true
    }
}
if ($changed) {
    Set-WinUserLanguageList $languages -Force
    Write-Output "Keyboard removed from the language list."
}

# 2. Unregister the text service.
$dll = Join-Path $Install "bopogram_ime.dll"
if (Test-Path $dll) {
    $registration = Start-Process regsvr32.exe -ArgumentList "/u", "/s", "`"$dll`"" -Wait -PassThru
    Write-Output "regsvr32 /u exit code: $($registration.ExitCode)"
}
foreach ($key in "HKLM:\SOFTWARE\Classes\CLSID\$clsid", "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid") {
    Write-Output "$key still present: $(Test-Path $key)"
}

# 3. Files. Applications that loaded the DLL keep it open until they exit.
Get-Process bopogram-host -ErrorAction SilentlyContinue | Stop-Process -Force
try {
    if (Test-Path $Install) { Remove-Item $Install -Recurse -Force }
    Write-Output "Removed $Install."
} catch {
    Write-Output "Some files in $Install are still in use. Sign out and back in, then delete the folder."
}
