# Adds Bopogram to, or removes it from, the current user's keyboards.
# Run by the installer as the signed-in user:
#   keyboard.ps1 add      exit code 2: Traditional Chinese (Taiwan) is not in the language list
#   keyboard.ps1 remove
param([ValidateSet("add", "remove")][string]$Action = "add")
$ErrorActionPreference = "Stop"
$Tip = "0404:{B6B6D901-0BBF-4287-A302-F265DBE9A46C}{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"
$languages = Get-WinUserLanguageList
if ($Action -eq "add") {
    $taiwan = $languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
    if (-not $taiwan) { exit 2 }
    if (-not $taiwan.InputMethodTips.Contains($Tip)) {
        $taiwan.InputMethodTips.Add($Tip)
        Set-WinUserLanguageList $languages -Force
    }
} else {
    $changed = $false
    foreach ($language in $languages) {
        if ($language.InputMethodTips.Contains($Tip)) {
            [void]$language.InputMethodTips.Remove($Tip)
            $changed = $true
        }
    }
    if ($changed) { Set-WinUserLanguageList $languages -Force }
}
exit 0
