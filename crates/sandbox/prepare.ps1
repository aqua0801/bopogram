# Runs on the main system. Stages everything a Windows Sandbox session needs
# and writes the .wsb file. Changes nothing outside the work folder: no
# registry, no registration. Registration happens inside the sandbox only
# (see setup.ps1).
#
#   powershell -ExecutionPolicy Bypass -File crates\sandbox\prepare.ps1
#   then open <Work>\bopogram.wsb
param(
    # A Python 3.12 installation (not a virtual environment).
    [string]$Python = (& py -3.12 -c "import sys; print(sys.base_prefix)"),
    # The engine runtime bundle (dictionary and models).
    [string]$Runtime = "D:\bopogram-data\bopogram-runtime",
    # Where the sandbox files go. Must be outside the repository.
    [string]$Work = "D:\bopogram-data\sandbox"
)
$ErrorActionPreference = "Stop"

$crates = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $crates
$prototype = Join-Path $repo "prototype"
$release = Join-Path $crates "target\release"

foreach ($path in $Python, $Runtime, $prototype, (Join-Path $prototype ".venv\Lib\site-packages")) {
    if (-not (Test-Path $path)) { throw "missing: $path" }
}

Push-Location $crates
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    # The 32-bit DLL, for 32-bit applications.
    & cargo build --release -p bopogram-ime --target i686-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "cargo build (x86) failed" }
} finally {
    Pop-Location
}

$stage = Join-Path $Work "stage"
$out = Join-Path $Work "out"
$runtimeCopy = Join-Path $Work "runtime"
New-Item -ItemType Directory -Force $stage, $out | Out-Null
# The install scripts and what they install, laid out as in crates\, so that
# the sandbox runs the real install.ps1 and uninstall.ps1.
Remove-Item (Join-Path $stage "crates"), (Join-Path $stage "x86") -Recurse -Force -ErrorAction SilentlyContinue
Get-ChildItem $stage -File | Where-Object { $_.Extension -in ".dll", ".exe", ".ico" } | Remove-Item -Force
foreach ($file in "target\release\bopogram_ime.dll", "target\release\bopogram-host.exe",
        "target\release\bopogram-settings.exe", "target\i686-pc-windows-msvc\release\bopogram_ime.dll",
        "bopogram-ime\res\bopogram.ico", "install\install.ps1", "install\uninstall.ps1", "install\common.ps1") {
    $target = Join-Path $stage "crates\$file"
    New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
    Copy-Item (Join-Path $crates $file) $target -Force
}
foreach ($file in "setup.ps1", "testform.ps1", "nofield.ps1", "test.html") {
    Copy-Item (Join-Path $PSScriptRoot $file) $stage -Force
}
Remove-Item (Join-Path $out "*") -Force -ErrorAction SilentlyContinue

# The compiled KenLM module needs the VC++ runtime (msvcp140.dll), which a
# fresh sandbox does not have. Stage the module with this system's copies of
# the runtime DLLs beside it.
$kenlm = Join-Path $stage "kenlm"
New-Item -ItemType Directory -Force $kenlm | Out-Null
Copy-Item (Join-Path $prototype ".venv\Lib\site-packages\kenlm*.pyd") $kenlm -Force
foreach ($dll in "msvcp140.dll", "msvcp140_1.dll", "msvcp140_2.dll", "vcruntime140.dll", "vcruntime140_1.dll") {
    Copy-Item (Join-Path $env:SystemRoot "System32\$dll") $kenlm -Force
}
# The other packages the engine imports (the character model needs NumPy
# and ONNX Runtime), copied as installed.
$packages = Join-Path $stage "packages"
New-Item -ItemType Directory -Force $packages | Out-Null
$sitePackages = Join-Path $prototype ".venv\Lib\site-packages"
Get-ChildItem $sitePackages -Directory |
    Where-Object { $_.Name -notmatch '^(pip|bopogram|kenlm|__pycache__)' } |
    ForEach-Object { Copy-Item $_.FullName $packages -Recurse -Force }

# The sandbox gets its own copy of the runtime bundle, so that what is typed
# there never reaches the real personal data (userdata\user.json).
if (-not (Test-Path (Join-Path $runtimeCopy "config.json"))) {
    Copy-Item $Runtime $runtimeCopy -Recurse
    Remove-Item (Join-Path $runtimeCopy "userdata") -Recurse -Force -ErrorAction SilentlyContinue
}

function Folder($hostPath, $sandboxPath, $readOnly) {
    "    <MappedFolder>`n      <HostFolder>$hostPath</HostFolder>`n      <SandboxFolder>$sandboxPath</SandboxFolder>`n      <ReadOnly>$readOnly</ReadOnly>`n    </MappedFolder>"
}
$folders = @(
    (Folder $stage "C:\stage" "true"),
    (Folder $Python "C:\Python312" "true"),
    (Folder $prototype "C:\bopogram-prototype" "true"),
    (Folder $runtimeCopy "C:\bopogram-runtime" "false"),
    (Folder $out "C:\out" "false")
) -join "`n"
$wsb = @"
<Configuration>
  <Networking>Disable</Networking>
  <MappedFolders>
$folders
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -ExecutionPolicy Bypass -File C:\stage\setup.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbPath = Join-Path $Work "bopogram.wsb"
[IO.File]::WriteAllText($wsbPath, $wsb)
Write-Output "Sandbox configuration: $wsbPath"
Write-Output "Results will appear in: $out"
