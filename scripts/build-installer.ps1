param(
    [string]$Version = "1.0.6"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Join-Path $repoRoot "dist"
$tmpRoot = Join-Path $repoRoot "tmp"
$setupPath = Join-Path $distRoot ("ainput-setup-" + $Version + ".exe")
$tempCabPath = Join-Path $distRoot ("~ainput-setup-" + $Version + ".CAB")
$tempDdfPath = Join-Path $distRoot ("~ainput-setup-" + $Version + ".DDF")
$portableZip = Join-Path $distRoot ("ainput-" + $Version + ".zip")
$stagingDir = Join-Path $tmpRoot ("ainput-installer-" + $Version)
$sedPath = Join-Path $stagingDir "ainput-installer.sed"
$payloadZipPath = Join-Path $stagingDir "payload.zip"
$installCmdPath = Join-Path $stagingDir "install.cmd"

& (Join-Path $PSScriptRoot "package-release.ps1") -Version $Version | Out-Null

if (Test-Path $stagingDir) {
    Remove-Item $stagingDir -Recurse -Force
}

if (Test-Path $setupPath) {
    Remove-Item $setupPath -Force
}

New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
Copy-Item $portableZip $payloadZipPath -Force
Copy-Item (Join-Path $PSScriptRoot "install-ainput.ps1") (Join-Path $stagingDir "install-ainput.ps1") -Force
Copy-Item (Join-Path $PSScriptRoot "uninstall-ainput.ps1") (Join-Path $stagingDir "uninstall-ainput.ps1") -Force

Set-Content -Path $installCmdPath -Encoding ASCII -Value @(
    "@echo off",
    "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""%~dp0install-ainput.ps1"" -PayloadZip ""%~dp0payload.zip""",
    "exit /b %errorlevel%"
)

$sedBody = @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgramWindow=1
HideExtractAnimation=1
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=%InstallPrompt%
DisplayLicense=%DisplayLicense%
FinishMessage=%FinishMessage%
TargetName=%TargetName%
FriendlyName=%FriendlyName%
AppLaunched=%AppLaunched%
PostInstallCmd=%PostInstallCmd%
AdminQuietInstCmd=%AdminQuietInstCmd%
UserQuietInstCmd=%UserQuietInstCmd%
SourceFiles=SourceFiles
[Strings]
InstallPrompt=
DisplayLicense=
FinishMessage=ainput installation completed.
TargetName=$setupPath
FriendlyName=ainput Setup $Version
AppLaunched=cmd.exe /d /s /c ""install.cmd""
PostInstallCmd=<None>
AdminQuietInstCmd=cmd.exe /d /s /c ""install.cmd""
UserQuietInstCmd=cmd.exe /d /s /c ""install.cmd""
FILE0="payload.zip"
FILE1="install.cmd"
FILE2="install-ainput.ps1"
FILE3="uninstall-ainput.ps1"
[SourceFiles]
SourceFiles0=$stagingDir\
[SourceFiles0]
%FILE0%=
%FILE1%=
%FILE2%=
%FILE3%=
"@

Set-Content -Path $sedPath -Encoding ASCII -Value $sedBody

& "$env:SystemRoot\System32\iexpress.exe" /N $sedPath | Out-Null

if (!(Test-Path $setupPath)) {
    throw "installer was not generated: $setupPath"
}

Remove-Item $tempCabPath -Force -ErrorAction SilentlyContinue
Remove-Item $tempDdfPath -Force -ErrorAction SilentlyContinue

Write-Output $setupPath
