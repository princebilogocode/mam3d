# Construit l'installateur Windows de mam3d : dist\mam3d-setup.exe
# Utilisation (depuis le dossier app) :  powershell -ExecutionPolicy Bypass -File make_installer.ps1
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "1/3  Construction de mam3d (optimisé)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "la construction de mam3d a échoué" }

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$env:MAM3D_EXE = (Resolve-Path "target\release\mam3d.exe").Path
$env:MAM3D_VERSION = $version

Write-Host "2/3  Construction de l'installateur (version $version)..."
cargo build --release --manifest-path installer\Cargo.toml --target-dir target
if ($LASTEXITCODE -ne 0) { throw "la construction de l'installateur a échoué" }

Write-Host "3/3  Copie dans dist\"
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item "target\release\mam3d-setup.exe" "dist\mam3d-setup.exe" -Force
Copy-Item "target\release\mam3d-setup.exe" "dist\mam3d-$version-setup.exe" -Force
Copy-Item "target\release\mam3d.exe" "dist\mam3d-portable.exe" -Force
Get-ChildItem dist | Format-Table Name, @{n="Taille (Mo)";e={[math]::Round($_.Length/1MB,1)}}
Write-Host "Installateur prêt : $(Resolve-Path dist\mam3d-setup.exe)"
