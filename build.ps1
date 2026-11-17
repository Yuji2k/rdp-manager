# Baut den RDP Manager (Release). Setzt PATH für Rust + WinLibs-MinGW (GNU-Toolchain).
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -ErrorAction SilentlyContinue |
    Where-Object Name -match 'WinLibs' |
    ForEach-Object { Get-ChildItem $_.FullName -Recurse -Filter dlltool.exe | Select-Object -First 1 } |
    Select-Object -First 1
if ($mingw) { $env:Path = "$($mingw.DirectoryName);$env:Path" }
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
Set-Location $PSScriptRoot
cargo build --release
if ($?) { Write-Host "Fertig: $PSScriptRoot\target\release\rdp_manager.exe" }
