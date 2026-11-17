# Baut die exe (Release) und daraus den MSI-Installer nach dist\.
# Benötigt WiX 3.14 (Binaries unter %LOCALAPPDATA%\wix314 oder installiertes WiX Toolset).
Set-Location $PSScriptRoot

& "$PSScriptRoot\build.ps1"
if ($LASTEXITCODE) { throw "cargo build fehlgeschlagen" }
$ErrorActionPreference = 'Stop'

$wix = @("$env:LOCALAPPDATA\wix314", "${env:ProgramFiles(x86)}\WiX Toolset v3.14\bin") |
    Where-Object { Test-Path "$_\candle.exe" } | Select-Object -First 1
if (-not $wix) { throw "WiX 3.14 nicht gefunden" }

# Version aus Cargo.toml (z. B. 0.1.0)
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$exe = "$PSScriptRoot\target\release\rdp_manager.exe"
$obj = "target\wix"   # relativ: "...\Claude Code\...\" mit Schluss-\ würde das Argument zerlegen
New-Item -ItemType Directory -Force $obj, "$PSScriptRoot\dist" | Out-Null
$msi = "$PSScriptRoot\dist\RdpManager-$version-x64.msi"

& "$wix\candle.exe" -nologo -arch x64 "-dVersion=$version" "-dExePath=$exe" "-dIconPath=$PSScriptRoot\assets\icon.ico"-out "$obj\" "installer\rdp_manager.wxs"
if ($LASTEXITCODE) { throw "candle fehlgeschlagen" }
& "$wix\light.exe" -nologo -ext WixUIExtension -cultures:de-de -spdb -out $msi "$obj\rdp_manager.wixobj"
if ($LASTEXITCODE) { throw "light fehlgeschlagen" }
# Zusätzlich die exe zum direkten Testen ohne Installation
$portable = "$PSScriptRoot\dist\RdpManager-$version.exe"
Copy-Item $exe $portable -Force
Write-Host "Fertig: $msi"
Write-Host "        $portable"
