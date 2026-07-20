param(
  [string]$Version = "0.1.0",
  [switch]$Overwrite
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$CargoToml = Get-Content (Join-Path $Root "Cargo.toml") -Raw
$CargoVersionMatch = [regex]::Match($CargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $CargoVersionMatch.Success) { throw "Cannot find package version in Cargo.toml" }
$CargoVersion = $CargoVersionMatch.Groups[1].Value
if ($CargoVersion -ne $Version) {
  throw "Version mismatch: Cargo.toml is $CargoVersion but -Version is $Version"
}

$Dist = Join-Path $Root "dist\ainput-$Version-win64"
if (Test-Path $Dist) {
  if (-not $Overwrite) { throw "Dist exists: $Dist (pass -Overwrite)" }
  Remove-Item -Recurse -Force $Dist
}

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }

New-Item -ItemType Directory -Force $Dist | Out-Null
Copy-Item "$Root\target\release\ainput.exe" $Dist
Get-ChildItem "$Root\target\release" -Filter "*.dll" -File -ErrorAction SilentlyContinue |
  ForEach-Object { Copy-Item $_.FullName $Dist -Force }
Copy-Item "$Root\run-ainput.bat" $Dist -ErrorAction SilentlyContinue
Copy-Item -Recurse "$Root\config" $Dist
Copy-Item "$Root\README.md" $Dist -ErrorAction SilentlyContinue
Copy-Item "$Root\LICENSE" $Dist -ErrorAction SilentlyContinue
Copy-Item "$Root\THIRD_PARTY_NOTICES" $Dist -ErrorAction SilentlyContinue

$SenseVoiceSource = Join-Path $Root "models\sense-voice"
if (-not (Test-Path $SenseVoiceSource)) {
  throw "Missing models\sense-voice — place SenseVoice int8 bundle before packaging"
}
$SenseVoiceTarget = Join-Path $Dist "models\sense-voice"
New-Item -ItemType Directory -Force $SenseVoiceTarget | Out-Null
Copy-Item "$SenseVoiceSource\*" $SenseVoiceTarget -Recurse -Force
# drop archives/tests if any slipped in
Get-ChildItem $SenseVoiceTarget -Recurse -Include "*.tar.bz2","*.tar.gz" -File -ErrorAction SilentlyContinue |
  Remove-Item -Force
Get-ChildItem $SenseVoiceTarget -Recurse -Directory -Filter "test_wavs" -ErrorAction SilentlyContinue |
  Remove-Item -Recurse -Force

$ModelFile = Get-ChildItem $SenseVoiceTarget -Recurse -File -Filter "model*.onnx" | Select-Object -First 1
$TokensFile = Get-ChildItem $SenseVoiceTarget -Recurse -File -Filter "tokens.txt" | Select-Object -First 1
if (-not $ModelFile -or -not $TokensFile) {
  throw "Packaged SenseVoice model incomplete under $SenseVoiceTarget"
}

$AssetsDist = Join-Path $Dist "assets"
New-Item -ItemType Directory -Force $AssetsDist | Out-Null
Copy-Item "$Root\assets\*" $AssetsDist -Recurse -Force

# Never ship runtime state (logs/keys/history) inside the green package.
$StateDist = Join-Path $Dist "state"
if (Test-Path $StateDist) {
  Remove-Item -Recurse -Force $StateDist
}

$Zip = Join-Path $Root "dist\ainput-$Version-win64.zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
# Wrap as ainput-<ver>-win64\... so unpack keeps one folder.
Compress-Archive -Path $Dist -DestinationPath $Zip -Force

Write-Host "Packaged folder: $Dist"
Write-Host "Packaged zip:    $Zip"
Write-Host "SenseVoice model: $($ModelFile.FullName) ($([math]::Round($ModelFile.Length/1MB,1)) MB)"
