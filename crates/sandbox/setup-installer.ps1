# Runs INSIDE Windows Sandbox at logon (see prepare-installer.ps1): installs
# Bopogram with the real installer, silently, and checks what it did. Results
# in C:\out\result.txt. Do not run this on a real system.
$ErrorActionPreference = "Continue"
if ($env:USERNAME -ne "WDAGUtilityAccount") { Write-Output "This script is for Windows Sandbox only."; exit 1 }
$result = "C:\out\result.txt"
function Log($message) { Add-Content -Path $result -Value $message -Encoding UTF8 }
function Check($name, $ok, $detail = "") { Log ("{0}  {1}{2}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $name, $(if ($detail) { "  ($detail)" } else { "" })) }
Set-Content -Path $result -Value "installer test $(Get-Date -Format s)" -Encoding UTF8

# Unsigned code: Smart App Control off, in this disposable sandbox only.
Set-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy" -Name VerifiedAndReputablePolicyState -Value 0 -Type DWord -ErrorAction SilentlyContinue
& CiTool.exe --refresh *> $null

# Traditional Chinese (Taiwan) in the language list, as on the main system.
$languages = Get-WinUserLanguageList
if (-not ($languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" })) {
    $languages.Add("zh-Hant-TW")
    Set-WinUserLanguageList $languages -Force
}

# An older script install: bopogram-host.json pointing at a runtime bundle
# whose userdata holds a personal model, which the installer must carry over.
$install = Join-Path $env:ProgramFiles "Bopogram"
$bundle = "C:\old-bundle"
New-Item -ItemType Directory -Force $install, "$bundle\userdata" | Out-Null
Set-Content "$bundle\config.json" "{}" -Encoding ASCII
Set-Content "$bundle\userdata\user.json" '{"old": true}' -Encoding ASCII
Set-Content "$bundle\userdata\userdict.tsv" "測試詞`tㄘㄜˋ ㄕˋ ㄘˊ`n" -Encoding UTF8
@{ engine = "rust"; config = "$bundle\config.json" } | ConvertTo-Json | Set-Content (Join-Path $install "bopogram-host.json") -Encoding UTF8

# Install, silently.
$setup = Get-ChildItem "C:\stage\bopogram-setup-*-full.exe" | Select-Object -First 1
$started = Get-Date
$p = Start-Process $setup.FullName -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=C:\out\install.log" -Wait -PassThru
Check "installer exit code 0" ($p.ExitCode -eq 0) "exit $($p.ExitCode), $([int]((Get-Date) - $started).TotalSeconds) s"

foreach ($f in "bopogram_ime.dll", "x86\bopogram_ime.dll", "bopogram-host.exe", "bopogram-settings.exe", "bopogram.exe",
        "bopogram.ico", "keyboard.ps1", "LICENSE.txt", "THIRD-PARTY-LICENSES.html", "bopogram-host.json", "unins000.exe") {
    Check "file $f" (Test-Path (Join-Path $install $f))
}
$clsid = "{B6B6D901-0BBF-4287-A302-F265DBE9A46C}"
Check "registered (64-bit COM)" (Test-Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32")
Check "registered (32-bit COM)" (Test-Path "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\$clsid\InprocServer32")
Check "registered (TSF profile)" (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid")
$tip = "0404:$clsid{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"
$taiwan = Get-WinUserLanguageList | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
Check "keyboard in zh-Hant-TW" ($taiwan.InputMethodTips -contains $tip)
$run = (Get-ItemProperty "HKLM:\Software\Microsoft\Windows\CurrentVersion\Run" -ErrorAction SilentlyContinue).Bopogram
Check "starts at sign-in (HKLM Run)" ($run -like "*bopogram-host.exe*") $run
Check "Start menu shortcut" (Test-Path "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Bopogram 設定.lnk")
$hostJson = Get-Content (Join-Path $install "bopogram-host.json") -Raw
Check "host.json names the user's model pack" ($hostJson -match 'LOCALAPPDATA') $hostJson.Trim()

$data = Join-Path $env:LOCALAPPDATA "Bopogram\data"
Check "model pack installed" (Test-Path "$data\models\engine.json")
Check "dictionary built" ((Test-Path "$data\models\dict.tsv") -and (Get-Item "$data\models\dict.tsv").Length -gt 7000000)
Check "personal model carried over" ((Get-Content "$data\userdata\user.json" -Raw -ErrorAction SilentlyContinue) -match 'old')
Check "user dictionary carried over" (Test-Path "$data\userdata\userdict.tsv")
$status = & (Join-Path $install "bopogram.exe") pack status 2>$null | Select-Object -Last 1
Log "     pack status: $status"
$converted = & (Join-Path $install "bopogram.exe") --config "$data\models\engine.json" convert --keys "ji3y94d04d04" 2>$null
Check "engine converts" ($converted -eq "我再看看") $converted

Start-Sleep -Seconds 3
$hosts = @(Get-Process bopogram-host -ErrorAction SilentlyContinue)
Check "host running after install" ($hosts.Count -eq 1) "$($hosts.Count) running"
$stop = Start-Process (Join-Path $install "bopogram-host.exe") -ArgumentList "--stop" -Wait -PassThru
Start-Sleep -Milliseconds 500
Check "bopogram-host --stop" (($stop.ExitCode -eq 0) -and -not (Get-Process bopogram-host -ErrorAction SilentlyContinue)) "exit $($stop.ExitCode)"
Start-Process (Join-Path $install "bopogram-host.exe")

# Installing again (an update) over the running host and loaded DLL.
Start-Sleep -Seconds 3
$p = Start-Process $setup.FullName -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=C:\out\update.log" -Wait -PassThru
Check "update over a running install" ($p.ExitCode -eq 0) "exit $($p.ExitCode)"
Check "previous models kept after update" (Test-Path "$data\models-previous\engine.json")

# For trying by hand: Notepad, the settings window, and uninstalling.
$desktop = [Environment]::GetFolderPath("Desktop")
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut("$desktop\Uninstall Bopogram.lnk"); $link.TargetPath = Join-Path $install "unins000.exe"; $link.Save()
$link = $shell.CreateShortcut("$desktop\Bopogram settings.lnk"); $link.TargetPath = Join-Path $install "bopogram-settings.exe"; $link.Save()
Log "done; try typing in Notepad (Win+Space to Bopogram), the settings window, then uninstall from the desktop"
