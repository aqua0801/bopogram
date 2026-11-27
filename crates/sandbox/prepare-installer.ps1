# Runs on the main system. Builds the full installer (programs + model pack)
# and writes a Windows Sandbox configuration that installs it there and
# checks the result. Changes nothing outside the work folder.
#
#   powershell -ExecutionPolicy Bypass -File crates\sandbox\prepare-installer.ps1 -Pack <bopogram-models-*.zip>
#   then open <Work>\installer.wsb; results appear in <Work>\out-installer
param(
    [Parameter(Mandatory = $true)][string]$Pack,
    [string]$Dist = "D:\bopogram-data\dist",
    [string]$Work = "D:\bopogram-data\sandbox"
)
$ErrorActionPreference = "Stop"
$crates = Split-Path -Parent $PSScriptRoot
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $crates "installer\build.ps1") -Pack $Pack -Out $Dist
if ($LASTEXITCODE -ne 0) { throw "build.ps1 failed" }

$stage = Join-Path $Work "stage-installer"
$out = Join-Path $Work "out-installer"
New-Item -ItemType Directory -Force $stage, $out | Out-Null
Remove-Item (Join-Path $stage "*"), (Join-Path $out "*") -Recurse -Force -ErrorAction SilentlyContinue
Copy-Item (Join-Path $PSScriptRoot "setup-installer.ps1") $stage -Force
Copy-Item (Join-Path $Dist "bopogram-setup-*-full.exe") $stage -Force
Copy-Item $Pack (Join-Path $stage "models.zip") -Force

$wsb = @"
<Configuration>
  <Networking>Disable</Networking>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>$stage</HostFolder>
      <SandboxFolder>C:\stage</SandboxFolder>
      <ReadOnly>true</ReadOnly>
    </MappedFolder>
    <MappedFolder>
      <HostFolder>$out</HostFolder>
      <SandboxFolder>C:\out</SandboxFolder>
      <ReadOnly>false</ReadOnly>
    </MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -ExecutionPolicy Bypass -File C:\stage\setup-installer.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbPath = Join-Path $Work "installer.wsb"
[IO.File]::WriteAllText($wsbPath, $wsb)
Write-Output "Sandbox configuration: $wsbPath"
Write-Output "Results will appear in: $out"
