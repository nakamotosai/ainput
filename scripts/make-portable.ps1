param(
  # Empty => derive from Cargo.toml (recommended; avoids drift). Pass an explicit
  # value only to double-check it matches Cargo.toml.
  [string]$Version = "",
  [switch]$Overwrite
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$CargoToml = Get-Content (Join-Path $Root "Cargo.toml") -Raw
$CargoVersionMatch = [regex]::Match($CargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $CargoVersionMatch.Success) { throw "Cannot find package version in Cargo.toml" }
$CargoVersion = $CargoVersionMatch.Groups[1].Value
if ([string]::IsNullOrWhiteSpace($Version)) {
  $Version = $CargoVersion
  Write-Host "Version derived from Cargo.toml: $Version"
} elseif ($CargoVersion -ne $Version) {
  throw "Version mismatch: Cargo.toml is $CargoVersion but -Version is $Version"
}

$Dist = Join-Path $Root "dist\ainput-$Version-win64"
if (Test-Path $Dist) {
  if (-not $Overwrite) { throw "Dist exists: $Dist (pass -Overwrite)" }
  Remove-Item -Recurse -Force $Dist
}

# cargo is often at ~\.cargo\bin but not on PATH; add it so this script works
# when invoked directly (README documents that).
$CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ((Test-Path $CargoBin) -and ($env:PATH -notlike "*$CargoBin*")) {
  $env:PATH = "$CargoBin;$env:PATH"
}
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }

New-Item -ItemType Directory -Force $Dist | Out-Null
Copy-Item "$Root\target\release\ainput.exe" $Dist
# Allow-list only the DLLs the CPU ASR path needs. A wildcard sweep would drag
# in onnxruntime_providers_cuda.dll (~275MB) / _tensorrt.dll, which the shipped
# config (provider="cpu") never loads — a ~166MB (compressed) download bloat.
$RuntimeDlls = @(
  "onnxruntime.dll",
  "onnxruntime_providers_shared.dll",
  "sherpa-onnx-c-api.dll",
  "sherpa-onnx-cxx-api.dll",
  "cargs.dll"
)
foreach ($dll in $RuntimeDlls) {
  $src = Join-Path "$Root\target\release" $dll
  if (Test-Path $src) { Copy-Item $src $Dist -Force }
  else { Write-Warning "expected runtime DLL not found in target/release: $dll" }
}
# The MSVC C runtime is a hard import (VCRUNTIME140.dll). A green "unpack and
# run" zip must carry it, or it fails on machines without the VC++ redist.
foreach ($crt in @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll")) {
  $src = Join-Path $env:WINDIR "System32\$crt"
  if (Test-Path $src) { Copy-Item $src $Dist -Force }
  else { Write-Warning "C runtime DLL not found (users may need the VC++ redistributable): $crt" }
}
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
