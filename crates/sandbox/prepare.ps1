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
} finally {
    Pop-Location
}

$stage = Join-Path $Work "stage"
$out = Join-Path $Work "out"
$runtimeCopy = Join-Path $Work "runtime"
New-Item -ItemType Directory -Force $stage, $out | Out-Null
Copy-Item (Join-Path $release "bopogram_ime.dll"), (Join-Path $release "bopogram-host.exe") $stage -Force
Copy-Item (Join-Path $PSScriptRoot "setup.ps1"), (Join-Path $PSScriptRoot "test.html") $stage -Force
Remove-Item (Join-Path $out "*") -Force -ErrorAction SilentlyContinue

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
