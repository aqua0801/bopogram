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

$install = Join-Path $env:ProgramFiles "Bopogram"
# A virtual environment on top of the read-only Python, pointed at the
# read-only engine source. It lives outside the install folder, as the
# prototype's does on the main system.
$venv = "C:\bopogram-venv"
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

# 1. The engine on its own.
$converted = & $python -X utf8 -m bopogram.cli --config $config convert --keys "ji3y94ru8 " 2>&1
[IO.File]::WriteAllLines("C:\out\engine.txt", [string[]]@($converted), (New-Object Text.UTF8Encoding($false)))
Log "engine exit code: $LASTEXITCODE (output in engine.txt)"

# 2. Traditional Chinese (Taiwan) in the language list, as on the main system.
try {
    $languages = Get-WinUserLanguageList
    if (-not ($languages | Where-Object { $_.LanguageTag -eq "zh-Hant-TW" })) {
        $languages.Add("zh-Hant-TW")
        Set-WinUserLanguageList $languages -Force
    }
} catch {
    Log "language list failed: $_"
}

# 3. The real install script, and shortcuts on the desktop to run it again
#    (an update) and to run the uninstall script. Each writes its output to C:\out.
$arguments = "-Python `"$python`" -Config `"$config`" -ReviewTool `"C:\bopogram-prototype\tools\review_log.py`""
$installScript = "C:\stage\crates\install\install.ps1"
try {
    & $installScript -Python $python -Config $config -ReviewTool "C:\bopogram-prototype\tools\review_log.py" *>&1 |
        Tee-Object C:\out\install.log
    Log "install finished (output in install.log)"
} catch {
    Log "install failed: $_"
}
foreach ($key in "HKLM:\SOFTWARE\Classes\CLSID\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}\InprocServer32",
        "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}\InprocServer32",
        "HKLM:\SOFTWARE\Microsoft\CTF\TIP\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}") {
    Log "$key present: $(Test-Path $key)"
}
$desktop = [Environment]::GetFolderPath("Desktop")
$update = "powershell -ExecutionPolicy Bypass -Command `"& '$installScript' $($arguments -replace '"', "'") *>&1 | Tee-Object C:\out\update.log`"`r`npause"
$remove = "powershell -ExecutionPolicy Bypass -Command `"& '$install\uninstall.ps1' *>&1 | Tee-Object C:\out\uninstall.log`"`r`npause"
[IO.File]::WriteAllText((Join-Path $desktop "update.cmd"), "@echo off`r`n$update`r`n")
[IO.File]::WriteAllText((Join-Path $desktop "uninstall.cmd"), "@echo off`r`n$remove`r`n")

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
