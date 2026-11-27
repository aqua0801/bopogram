# Builds the Windows installer: the programs (64- and 32-bit), the licence
# texts of the Rust crates, then Inno Setup.
#
#   powershell -ExecutionPolicy Bypass -File crates\installer\build.ps1 [-Pack <bopogram-models-*.zip>] [-Out <folder>]
#
# Without -Pack: bopogram-setup-<version>.exe (the programs; the user adds a
# model pack in the settings window). With -Pack: bopogram-setup-<version>-full.exe,
# which installs that pack too. Nothing is installed on this machine.
param(
    [string]$Pack = "",
    [string]$Out = "D:\bopogram-data\dist"
)
$ErrorActionPreference = "Stop"
$crates = Split-Path -Parent $PSScriptRoot

$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 is not installed (ISCC.exe not found)" }
if ($Pack -and -not (Test-Path $Pack)) { throw "missing: $Pack" }

# cargo writes its progress to stderr, which Windows PowerShell would treat as errors.
function Cargo([string[]]$Arguments) {
    $p = Start-Process cargo -ArgumentList $Arguments -WorkingDirectory $crates -NoNewWindow -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "cargo $($Arguments -join ' ') failed" }
}
Cargo @("build", "--release", "--workspace")
Cargo @("build", "--release", "-p", "bopogram-ime", "--target", "i686-pc-windows-msvc")
Cargo @("deny", "check", "licenses")
Cargo @("about", "generate", "--workspace", "--offline", "--fail", "about.hbs", "-o", "target\THIRD-PARTY-LICENSES.html")

New-Item -ItemType Directory -Force $Out | Out-Null
$defines = @("/DOut=$Out")
if ($Pack) { $defines += "/DPack=$((Resolve-Path $Pack).Path)" }
& $iscc @defines (Join-Path $PSScriptRoot "bopogram.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Get-ChildItem $Out -Filter "bopogram-setup-*.exe" | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "{0}  {1:N0} bytes  sha256 {2}" -f $_.Name, $_.Length, $hash
}
