param(
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\ainput"
}

$installDir = $InstallDir.TrimEnd("\")
$installedExe = Join-Path $installDir "ainput-desktop.exe"
$startMenuFolder = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\ainput"
$runRegistryKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$uninstallRegistryKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ainput"
$cleanupScript = Join-Path $env:TEMP ("ainput-uninstall-" + [guid]::NewGuid().ToString("N") + ".cmd")

Get-Process ainput-desktop -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $installedExe } |
    Stop-Process -Force

Remove-ItemProperty -Path $runRegistryKey -Name "ainput" -ErrorAction SilentlyContinue
Remove-Item -Path $uninstallRegistryKey -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path $startMenuFolder -Recurse -Force -ErrorAction SilentlyContinue

$cleanupBody = @"
@echo off
set TARGET=$installDir
:retry
rmdir /s /q "%TARGET%" >nul 2>nul
if exist "%TARGET%" (
  ping 127.0.0.1 -n 2 >nul
  goto retry
)
del /f /q "%~f0"
"@

Set-Content -Path $cleanupScript -Encoding ASCII -Value $cleanupBody
Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$cleanupScript`"" -WindowStyle Hidden | Out-Null
