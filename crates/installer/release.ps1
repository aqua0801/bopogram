# Builds everything that goes into a GitHub release, into ONE folder:
#
#   powershell -ExecutionPolicy Bypass -File crates\installer\release.ps1 -ModelsVersion 2026.12
#
#   <Out>\bopogram-<program version>\
#     bopogram-setup-<v>.exe              Windows, programs only
#     bopogram-setup-<v>-full.exe         Windows, programs and the model pack
#     bopogram-<v>-linux-x86_64.tar.gz    Linux, install.sh
#     bopogram_<v>_amd64.deb              Debian / Ubuntu
#     bopogram-models-<m>.zip             the model pack (prototype/tools/make_model_pack.py)
#     bopogram-charlm-lite-<m>.zip        extra charlm tiers (make_tier_pack.py)
#     bopogram-charlm-advanced-<m>.zip
#     SHA256SUMS.txt
#
# Upload exactly the files of that folder. Every pack is checked with
# `bopogram pack verify` before the folder is written. Linux packages are made
# in WSL (make-packages.sh). Nothing is installed on this machine.
param(
    [Parameter(Mandatory = $true)][string]$ModelsVersion,
    # The lowest program version that reads these packs.
    [string]$Engine = ">=0.2",
    [string]$Out = "D:\bopogram-data\release",
    [string]$Runtime = "D:\bopogram-data\bopogram-runtime",
    [string]$Bglm = "D:\bopogram-data\arpa",
    # The MOE archives as downloaded, their usage notes, and dict_extra.tsv.
    [string]$DictSource = "D:\bopogram-data\dict-source"
)
$ErrorActionPreference = "Stop"
$crates = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $crates
$python = Join-Path $repo "prototype\.venv\Scripts\python.exe"
$version = (Select-String -Path (Join-Path $crates "Cargo.toml") -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value
$work = Join-Path $Out "work"
$final = Join-Path $Out "bopogram-$version"

function Wsl-Path([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    "/mnt/" + $full.Substring(0, 1).ToLower() + ($full.Substring(2) -replace '\\', '/')
}
function Cargo([string[]]$Arguments, [hashtable]$Environment = @{}) {
    foreach ($k in $Environment.Keys) { Set-Item "env:$k" $Environment[$k] }
    $p = Start-Process cargo -ArgumentList $Arguments -WorkingDirectory $crates -NoNewWindow -Wait -PassThru
    foreach ($k in $Environment.Keys) { Remove-Item "env:$k" }
    if ($p.ExitCode -ne 0) { throw "cargo $($Arguments -join ' ') failed" }
}
function Run([string]$Exe, [string[]]$Arguments) {
    $p = Start-Process $Exe -ArgumentList $Arguments -NoNewWindow -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "$Exe $($Arguments -join ' ') failed" }
}

if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory -Force $work | Out-Null

# 1. Packs, with the current packer, checked by the program that reads them.
Run $python @("-I", (Join-Path $repo "prototype\tools\make_model_pack.py"), "--version", $ModelsVersion, "--engine", "`"$Engine`"",
    "--runtime", $Runtime, "--bglm", $Bglm, "--moe", $DictSource, "--extra", (Join-Path $DictSource "dict_extra.tsv"), $work)
foreach ($tier in "lite", "advanced") {
    Run $python @("-I", (Join-Path $PSScriptRoot "make_tier_pack.py"), "--tier", $tier, "--version", $ModelsVersion,
        "--engine", "`"$Engine`"", "--charlm", (Join-Path $Runtime "charlm"), "--release", (Join-Path $repo "prototype\release"), $work)
}
Cargo @("build", "--release", "-p", "bopogram")
$modelsZip = Join-Path $work "bopogram-models-$ModelsVersion.zip"
foreach ($zip in @($modelsZip) + @(Get-ChildItem $work -Filter "bopogram-charlm-*.zip" | ForEach-Object FullName)) {
    $check = & (Join-Path $crates "target\release\bopogram.exe") pack verify $zip 2>$null | Select-Object -Last 1
    if ($check -notmatch '"ok":true') { throw "pack verify failed for $zip`: $check" }
    Write-Output "verified $(Split-Path -Leaf $zip): $check"
}

# 2. Windows installers (build.ps1 builds the programs and the licence texts).
$build = Join-Path $PSScriptRoot "build.ps1"
Run powershell.exe @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $build, "-Out", $work)
Run powershell.exe @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $build, "-Out", $work, "-Pack", $modelsZip)

# 3. Linux packages.
Cargo @("build", "--release", "-p", "bopogram", "-p", "bopogram-ibus", "--target", "x86_64-unknown-linux-musl") @{ CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "rust-lld" }
Run wsl.exe @("-e", "sh", (Wsl-Path (Join-Path $crates "bopogram-ibus\dist\make-packages.sh")), (Wsl-Path $work))

# 4. The release folder: exactly the files to upload, and their checksums.
$names = @("bopogram-setup-$version.exe", "bopogram-setup-$version-full.exe", "bopogram-$version-linux-x86_64.tar.gz",
    "bopogram_${version}_amd64.deb", "bopogram-models-$ModelsVersion.zip",
    "bopogram-charlm-lite-$ModelsVersion.zip", "bopogram-charlm-advanced-$ModelsVersion.zip")
foreach ($name in $names) { if (-not (Test-Path (Join-Path $work $name))) { throw "missing: $name" } }
if (Test-Path $final) { Remove-Item $final -Recurse -Force }
New-Item -ItemType Directory -Force $final | Out-Null
$sums = foreach ($name in $names) {
    Move-Item (Join-Path $work $name) $final
    "{0}  {1}" -f (Get-FileHash (Join-Path $final $name) -Algorithm SHA256).Hash.ToLower(), $name
}
[IO.File]::WriteAllText((Join-Path $final "SHA256SUMS.txt"), ($sums -join "`n") + "`n")
Remove-Item $work -Recurse -Force
Write-Output ""
Write-Output "Release folder: $final"
Get-ChildItem $final | ForEach-Object { "  {0,-45} {1,14:N0} bytes" -f $_.Name, $_.Length }
