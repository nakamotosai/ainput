# Build the Windows installer from the portable folder.
#   .\scripts\build-installer.ps1
# Derives the version from Cargo.toml, requires the portable folder to exist.
param(
  [string]$Version = ""
)
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$CargoToml = Get-Content (Join-Path $Root "Cargo.toml") -Raw
$m = [regex]::Match($CargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $m.Success) { throw "Cannot find version in Cargo.toml" }
$CargoVersion = $m.Groups[1].Value
if ([string]::IsNullOrWhiteSpace($Version)) { $Version = $CargoVersion }
elseif ($CargoVersion -ne $Version) { throw "Version mismatch: Cargo.toml $CargoVersion vs -Version $Version" }

$SourceDir = Join-Path $Root "dist\ainput-$Version-win64"
if (-not (Test-Path $SourceDir)) {
  throw "Portable folder missing: $SourceDir (run scripts\make-portable.ps1 first)"
}

$Iscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $Iscc)) { throw "Inno Setup 6 not found at $Iscc" }

& $Iscc "/DAppVersion=$Version" "/DSourceDir=$SourceDir" (Join-Path $Root "scripts\ainput.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

$out = Join-Path $Root "dist\ainput-$Version-setup.exe"
if (Test-Path $out) {
  # Authenticode-sign the installer if a cert is configured (no-op otherwise).
  & (Join-Path $PSScriptRoot "sign-if-available.ps1") -File $out
  Write-Host ("Installer: {0} ({1:N1} MB)" -f $out, ((Get-Item $out).Length / 1MB))
} else {
  throw "Expected installer not produced: $out"
}
