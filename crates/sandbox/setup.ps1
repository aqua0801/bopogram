# Runs INSIDE Windows Sandbox at logon (see prepare.ps1). Installs the trial
# input method into the sandbox and writes a log to C:\out, which the main
# system can read. Do not run this on a real system: it registers a COM
# class and a TSF text service machine-wide.
$ErrorActionPreference = "Continue"
$log = "C:\out\setup.log"
function Log($message) { Add-Content -Path $log -Value $message -Encoding UTF8 }

if ($env:USERNAME -ne "WDAGUtilityAccount") {
    Write-Output "This script is for Windows Sandbox only."
    exit 1
}
Set-Content -Path $log -Value "setup started $(Get-Date -Format s)" -Encoding UTF8

# Smart App Control blocks unsigned code, which is all of ours. Turn it off
# for this disposable sandbox only.
$ciPolicy = "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy"
$sacBefore = (Get-ItemProperty $ciPolicy -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
Log "Smart App Control state before: $sacBefore (0 off, 1 on, 2 evaluation)"
try {
    Set-ItemProperty $ciPolicy -Name VerifiedAndReputablePolicyState -Value 0 -Type DWord -ErrorAction Stop
    $refresh = & CiTool.exe --refresh 2>&1 | Out-String
    Log "CiTool --refresh exit code: $LASTEXITCODE $($refresh.Trim() -replace '\s+', ' ')"
} catch {
    Log "turning Smart App Control off failed: $_"
}
Log "Smart App Control state after: $((Get-ItemProperty $ciPolicy -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState)"

$clsid = "{B6B6D901-0BBF-4287-A302-F265DBE9A46C}"
$profile = "{79EFFB1B-3CE8-41DB-9CA2-3318EC0AED82}"
$install = "C:\Bopogram"
New-Item -ItemType Directory -Force $install | Out-Null
Copy-Item "C:\stage\bopogram_ime.dll", "C:\stage\bopogram-host.exe", "C:\stage\bopogram-settings.exe", "C:\stage\bopogram.ico" $install -Force
New-Item -ItemType Directory -Force (Join-Path $install "x86") | Out-Null
Copy-Item "C:\stage\x86\bopogram_ime.dll" (Join-Path $install "x86") -Force

# A virtual environment on top of the read-only Python, pointed at the
# read-only engine source.
$venv = Join-Path $install "venv"
& "C:\Python312\python.exe" -m venv --without-pip $venv
# KenLM goes into the environment itself, with the VC++ runtime DLLs it needs
# beside it (extension modules find their dependencies in their own folder).
$sitePackages = Join-Path $venv "Lib\site-packages"
Copy-Item "C:\stage\kenlm\*" $sitePackages -Force
Copy-Item "C:\stage\packages\*" $sitePackages -Recurse -Force
# ONNX Runtime needs the same VC++ runtime; extension modules find it in their own folder.
Copy-Item "C:\stage\kenlm\*.dll" (Join-Path $sitePackages "onnxruntime\capi") -Force
Set-Content -Path (Join-Path $sitePackages "bopogram.pth") -Encoding ASCII -Value "C:\bopogram-prototype"
$python = Join-Path $venv "Scripts\python.exe"
$config = "C:\bopogram-runtime\config.json"
$hostConfig = @{
    python = $python; config = $config; review_tool = "C:\bopogram-prototype\tools\review_log.py"
} | ConvertTo-Json
[IO.File]::WriteAllText((Join-Path $install "bopogram-host.json"), $hostConfig)

# 1. The engine on its own.
$converted = & $python -X utf8 -m bopogram.cli --config $config convert --keys "ji3y94ru8 " 2>&1
[IO.File]::WriteAllLines("C:\out\engine.txt", [string[]]@($converted), (New-Object Text.UTF8Encoding($false)))
Log "engine exit code: $LASTEXITCODE (output in engine.txt)"

# 2. Register the text service.
$registration = Start-Process regsvr32.exe -ArgumentList "/s", (Join-Path $install "bopogram_ime.dll") -Wait -PassThru
Log "regsvr32 exit code: $($registration.ExitCode)"
Log "CLSID key present: $(Test-Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32")"
Log "TSF profile key present: $(Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid")"

# 2b. The 32-bit DLL, registered by the 32-bit regsvr32 (its COM class goes
#     to the 32-bit view of the registry).
$registration = Start-Process "$env:SystemRoot\SysWOW64\regsvr32.exe" -ArgumentList "/s", (Join-Path $install "x86\bopogram_ime.dll") -Wait -PassThru
Log "regsvr32 (x86) exit code: $($registration.ExitCode)"
Log "CLSID key present (32-bit view): $(Test-Path "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\$clsid\InprocServer32")"
Log "TSF profile key present (32-bit view): $(Test-Path "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$clsid")"

# 3. Add Traditional Chinese (Taiwan) with this input method to the user's languages.
try {
    $languages = Get-WinUserLanguageList
    $taiwan = $languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
    if (-not $taiwan) {
        $languages.Add("zh-Hant-TW")
        $taiwan = $languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" }
    }
    $tip = "0404:$clsid$profile"
    if (-not $taiwan.InputMethodTips.Contains($tip)) { $taiwan.InputMethodTips.Add($tip) }
    Set-WinUserLanguageList $languages -Force
    Log "input methods for zh-Hant-TW: $($taiwan.InputMethodTips -join ', ')"
} catch {
    Log "language list failed: $_"
}

# What Code Integrity blocked, if anything.
try {
    $blocked = Get-WinEvent -LogName "Microsoft-Windows-CodeIntegrity/Operational" -MaxEvents 200 -ErrorAction Stop |
        Where-Object { $_.TimeCreated -gt (Get-Date).AddMinutes(-10) -and $_.LevelDisplayName -ne "Information" }
    Log "Code Integrity warnings/errors in the last 10 minutes: $(@($blocked).Count)"
    foreach ($event in @($blocked) | Select-Object -First 12) {
        Log "  [$($event.Id)] $(($event.Message -split "`n")[0].Trim())"
    }
} catch {
    Log "Code Integrity log unavailable: $_"
}

# 4. Things to type into.
Start-Process powershell.exe -ArgumentList "-STA", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File", "C:\stage\testform.ps1"
Start-Process "C:\stage\test.html"
# A 32-bit application.
Start-Process "$env:SystemRoot\SysWOW64\notepad.exe"
Log "setup finished $(Get-Date -Format s)"
