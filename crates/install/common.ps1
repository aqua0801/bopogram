# Shared by install.ps1 and uninstall.ps1 (dot-sourced). Defines functions only.

$Clsid = "{B6B6D901-0BBF-4287-A302-F265DBE9A46C}"
$ProfileGuid = "{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"
$Tip = "0404:$Clsid$ProfileGuid"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
# One shortcut for every user: the Start menu finds the settings window by
# name ("Bopogram" and the word for settings, spelled in code points, as Windows PowerShell reads
# scripts without a byte order mark in the ANSI code page).
$Shortcut = Join-Path $env:ProgramData ("Microsoft\Windows\Start Menu\Programs\Bopogram " +
    [char]0x8A2D + [char]0x5B9A + ".lnk")

function Assert-Elevated {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
            [Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Run this from an elevated PowerShell (Run as administrator)."
    }
}

# Applications that have the input method loaded (64- and 32-bit).
# A loaded DLL keeps the name it was loaded under, even after a rename.
function Get-ImeUsers {
    $rows = & tasklist.exe /m bopogram_ime.dll /fo csv /nh 2>$null
    @($rows | Where-Object { $_ -match '^"' } | ForEach-Object { ($_ -split '","')[0].Trim('"') } |
        Sort-Object -Unique)
}

# Stop the host and the settings window, and wait for the engine: when the
# host goes away the engine sees the end of its input, saves the personal
# model and exits. Starting a new one before that could read a stale model.
function Stop-Bopogram {
    Get-Process bopogram-host, bopogram-settings -ErrorAction SilentlyContinue | Stop-Process -Force
    $deadline = (Get-Date).AddSeconds(10)
    do {
        $engines = @(Get-CimInstance Win32_Process -Filter "Name LIKE 'python%'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -match 'bopogram\.cli' -and $_.CommandLine -match '\bserve\b' })
        if ($engines.Count -eq 0) { return }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $deadline)
    Write-Output "The engine did not stop within 10 seconds; stopping it (unsaved learning since its last save is lost)."
    $engines | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

Add-Type -Namespace Bopogram -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool MoveFileEx(string existing, string replacement, int flags);
'@

# Delete a file or an (empty by then) folder at the next restart. Used for
# DLLs that running applications still have loaded.
function Remove-AtRestart([string]$Path) {
    $MOVEFILE_DELAY_UNTIL_REBOOT = 4
    if (-not [Bopogram.Native]::MoveFileEx($Path, $null, $MOVEFILE_DELAY_UNTIL_REBOOT)) {
        Write-Output "Could not schedule $Path for removal at restart."
    }
}

# Delete now, or at the next restart if the file is in use.
function Remove-OrSchedule([string]$Path) {
    try { Remove-Item -LiteralPath $Path -Force -ErrorAction Stop; $true }
    catch { Remove-AtRestart $Path; $false }
}

# Start a program as the signed-in user, not elevated: Explorer starts it.
# The host must not run elevated, or ordinary applications could not use it.
function Start-Unelevated([string]$Exe) {
    Start-Process explorer.exe -ArgumentList "`"$Exe`""
}
