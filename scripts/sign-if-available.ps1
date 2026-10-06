# Sign a file with Authenticode IF a code-signing certificate is available.
# Configure ONE of:
#   - env AINPUT_SIGN_PFX      = path to a .pfx
#     env AINPUT_SIGN_PFX_PASS = its password
#   - a cert already in Cert:\CurrentUser\My with -CodeSigningCert (auto-picked)
# If nothing is configured, prints a notice and skips (release ships unsigned).
# Timestamps via DigiCert so the signature outlives the cert.
param(
  [Parameter(Mandatory = $true)][string]$File
)
$ErrorActionPreference = "Continue"

function Find-Signtool {
  $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $kit = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1
  if ($kit) {
    $st = Join-Path $kit.FullName "x64\signtool.exe"
    if (Test-Path $st) { return $st }
  }
  return $null
}

$signtool = Find-Signtool
if (-not $signtool) { Write-Host "[sign] signtool not found; skipping"; return }

$ts = "http://timestamp.digicert.com"
$pfx = $env:AINPUT_SIGN_PFX
$pfxPass = $env:AINPUT_SIGN_PFX_PASS

if ($pfx -and (Test-Path $pfx)) {
  Write-Host "[sign] signing $File with PFX"
  & $signtool sign /fd SHA256 /f $pfx /p $pfxPass /tr $ts /td SHA256 $File
  if ($LASTEXITCODE -ne 0) { Write-Warning "[sign] signtool failed ($LASTEXITCODE)"; return }
} else {
  $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $cert) {
    Write-Host "[sign] no certificate configured; shipping unsigned (SmartScreen may warn)."
    return
  }
  Write-Host "[sign] signing $File with cert $($cert.Thumbprint)"
  & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /tr $ts /td SHA256 $File
  if ($LASTEXITCODE -ne 0) { Write-Warning "[sign] signtool failed ($LASTEXITCODE)"; return }
}
& $signtool verify /pa $File | Out-Null
Write-Host "[sign] verified: $File"
