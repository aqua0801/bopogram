# Removes the input method from this system. Run it yourself, from an
# elevated PowerShell (install.ps1 also leaves a copy in the install folder):
#
#   powershell -ExecutionPolicy Bypass -File crates\install\uninstall.ps1
#
# Removed: the registration (HKLM COM class in both views, the TSF profile),
# the keyboard in every language of the current user, the sign-in start, the
# Start menu shortcut, the install folder, the debugging trace if one was
# set, and %LOCALAPPDATA%\Bopogram (settings and logs; keep with -KeepSettings).
# Files that running applications still have loaded are deleted at the next
# restart. Nothing needs a sign-out.
#
# Personal data (the engine's personal model, typing log and user dictionary,
# in the runtime bundle) is kept unless -RemovePersonalData is given.
param(
    [string]$Install = (Join-Path $env:ProgramFiles "Bopogram"),
    [switch]$KeepSettings,
    [switch]$RemovePersonalData
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
Assert-Elevated

# What the install points at, read before the folder goes.
$hostConfig = $null
$hostJson = Join-Path $Install "bopogram-host.json"
if (Test-Path $hostJson) {
    $hostConfig = Get-Content -Raw -Encoding UTF8 $hostJson | ConvertFrom-Json
}

# 1. Take it out of the user's keyboards first, so nothing new loads it.
$languages = Get-WinUserLanguageList
$changed = $false
foreach ($language in $languages) {
    if ($language.InputMethodTips.Contains($Tip)) {
        [void]$language.InputMethodTips.Remove($Tip)
        $changed = $true
    }
}
if ($changed) {
    Set-WinUserLanguageList $languages -Force
    Write-Output "Keyboard removed from the language list."
}

# 2. No more starting the host at sign-in; no Start menu shortcut.
if (Get-ItemProperty $RunKey -Name "Bopogram" -ErrorAction SilentlyContinue) {
    Remove-ItemProperty $RunKey -Name "Bopogram"
    Write-Output "Sign-in start removed."
}
if (Test-Path -LiteralPath $Shortcut) {
    Remove-Item -LiteralPath $Shortcut -Force
    Write-Output "Start menu shortcut removed."
}

# 3. Unregister the text service: the 32-bit DLL first, then the 64-bit one
#    (each removes the language profile; the second finds it already gone).
$running = Get-ImeUsers
$dll32 = Join-Path $Install "x86\bopogram_ime.dll"
if (Test-Path $dll32) {
    $registration = Start-Process "$env:SystemRoot\SysWOW64\regsvr32.exe" -ArgumentList "/u", "/s", "`"$dll32`"" -Wait -PassThru
    Write-Output "regsvr32 /u (x86) exit code: $($registration.ExitCode)"
}
$dll = Join-Path $Install "bopogram_ime.dll"
if (Test-Path $dll) {
    $registration = Start-Process regsvr32.exe -ArgumentList "/u", "/s", "`"$dll`"" -Wait -PassThru
    Write-Output "regsvr32 /u exit code: $($registration.ExitCode)"
}
# Whatever unregistering left (for example, when the DLL files are already gone).
foreach ($key in "HKLM:\SOFTWARE\Classes\CLSID\$Clsid", "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\$Clsid",
        "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$Clsid", "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$Clsid") {
    if (Test-Path $key) {
        Remove-Item $key -Recurse -Force
        Write-Output "Removed leftover key $key."
    }
}

# 4. Files. Applications that loaded the DLL keep it open until they exit;
#    those files go at the next restart.
Stop-Bopogram
$later = 0
if (Test-Path $Install) {
    $files = Get-ChildItem $Install -Recurse -File
    foreach ($file in $files) {
        if (-not (Remove-OrSchedule $file.FullName)) { $later++ }
    }
    # Deepest folders first; a folder still holding scheduled files goes after them.
    $folders = @(Get-ChildItem $Install -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending) +
        @(Get-Item $Install)
    foreach ($folder in $folders) {
        try { Remove-Item -LiteralPath $folder.FullName -Force -ErrorAction Stop }
        catch { Remove-AtRestart $folder.FullName }
    }
}
if ($later -gt 0) {
    Write-Output "$later file(s) in $Install are in use and will be deleted at the next restart."
} else {
    Write-Output "Removed $Install."
}

# 5. The debugging trace records everything typed: it never outlives the input method.
if ($hostConfig -and $hostConfig.trace -and (Test-Path -LiteralPath $hostConfig.trace)) {
    Remove-Item -LiteralPath $hostConfig.trace -Force
    Write-Output "Removed the trace file $($hostConfig.trace)."
}

# 6. The input method's own settings and logs.
$local = Join-Path $env:LOCALAPPDATA "Bopogram"
if ((Test-Path $local) -and -not $KeepSettings) {
    Remove-Item $local -Recurse -Force
    Write-Output "Removed $local (settings and logs)."
}

# 7. Personal data, only when asked for.
if ($RemovePersonalData) {
    if (-not ($hostConfig -and $hostConfig.config -and (Test-Path -LiteralPath $hostConfig.config))) {
        Write-Output "Personal data not removed: the engine's config.json is unknown or missing."
    } else {
        $bundle = Split-Path -Parent $hostConfig.config
        $engine = Get-Content -Raw -Encoding UTF8 $hostConfig.config | ConvertFrom-Json
        # The user dictionary files have default paths when config.json does not name them.
        $defaults = @{ user_dict = "userdata/userdict.tsv"; excluded_words = "userdata/excluded.tsv" }
        foreach ($name in "user_model", "typing_log", "user_dict", "excluded_words") {
            $relative = $engine.$name
            if (-not $relative -and $defaults.ContainsKey($name) -and
                    -not ($engine.PSObject.Properties.Name -contains $name)) {
                $relative = $defaults[$name]
            }
            if (-not $relative) { continue }
            $path = if ([IO.Path]::IsPathRooted($relative)) { $relative } else { Join-Path $bundle $relative }
            foreach ($candidate in $path, [IO.Path]::ChangeExtension($path, ".tmp")) {
                if (Test-Path -LiteralPath $candidate) {
                    Remove-Item -LiteralPath $candidate -Force
                    Write-Output "Removed $candidate."
                }
            }
        }
    }
}

# 8. Check that nothing of it is left in the registry.
#    (The COM class keys were checked in step 3.)
$left = @()
foreach ($root in "HKLM\SOFTWARE\Microsoft\CTF", "HKLM\SOFTWARE\WOW6432Node\Microsoft\CTF", "HKCU\Software\Microsoft\CTF",
        "HKCU\Control Panel\International\User Profile") {
    $found = & reg.exe query $root /s /f ($Clsid.Trim('{}')) 2>$null | Where-Object { $_ -like "HKEY_*" }
    $left += @($found)
}
if ($left.Count -gt 0) {
    Write-Output "Still in the registry (Windows may tidy these itself after a sign-out):"
    $left | ForEach-Object { Write-Output "  $_" }
} else {
    Write-Output "Nothing left in the registry."
}
if ($running.Count -gt 0) {
    Write-Output "These programs still have the input method loaded until they restart:"
    Write-Output "  $($running -join ', ')"
}
Write-Output "Done."
