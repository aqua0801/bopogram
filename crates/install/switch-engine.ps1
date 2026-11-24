# Switches the engine the host runs, between the Rust engine and the Python
# prototype, without installing again. Run it from an elevated PowerShell
# (bopogram-host.json is in Program Files):
#
#   powershell -ExecutionPolicy Bypass -File "C:\Program Files\Bopogram\switch-engine.ps1" python
#   powershell -ExecutionPolicy Bypass -File "C:\Program Files\Bopogram\switch-engine.ps1" rust
#
# It checks that the chosen engine converts a sentence, then stops the host
# (the running engine saves the personal model first), changes the "engine"
# entry of bopogram-host.json and starts the host again. Both engines read the
# same config.json and the same personal files. Nothing else is changed.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("rust", "python")]
    [string]$To,
    [string]$Install = (Join-Path $env:ProgramFiles "Bopogram")
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
Assert-Elevated

$hostJson = Join-Path $Install "bopogram-host.json"
if (-not (Test-Path $hostJson)) { throw "missing: $hostJson (install first)" }
$settings = Get-Content $hostJson -Raw -Encoding UTF8 | ConvertFrom-Json
$engineExe = Join-Path $Install "bopogram.exe"
$models = if ($settings.models) { $settings.models } else { "D:\bopogram-data\arpa" }
if ($To -eq "rust" -and -not (Test-Path $engineExe)) { throw "missing: $engineExe (install again)" }
if ($To -eq "python" -and -not ($settings.python -and (Test-Path $settings.python))) {
    throw "the Python of the prototype is not found: '$($settings.python)'"
}

Test-Engine $To $engineExe $models $settings.python $settings.config
Write-Output "Engine check passed ($To)."

$hostConfig = @{}
foreach ($property in $settings.PSObject.Properties) { $hostConfig[$property.Name] = $property.Value }
$hostConfig.models = $models
if ($To -eq "rust") { $hostConfig.engine = $engineExe } else { $hostConfig.Remove("engine") }

Stop-Bopogram
[IO.File]::WriteAllText($hostJson, ($hostConfig | ConvertTo-Json))
Start-Unelevated (Join-Path $Install "bopogram-host.exe")
Write-Output "The host now runs the $To engine (see %LOCALAPPDATA%\Bopogram\host.log)."
