param(
    [Parameter(Mandatory = $true)]
    [string]$PayloadZip,
    [string]$InstallDir = "",
    [switch]$NoLaunch
)

$ErrorActionPreference = "Stop"
$appVersion = "1.0.10"

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\ainput"
}

$uninstallRegistryKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ainput"
$startMenuFolder = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\ainput"
$tempRoot = Join-Path $env:TEMP ("ainput-install-" + [guid]::NewGuid().ToString("N"))
$stagingDir = Join-Path $tempRoot "payload"
$installedExe = Join-Path $InstallDir "ainput-desktop.exe"
$installedConfig = Join-Path $InstallDir "config\ainput.toml"
$legacyInstalledConfig = Join-Path $InstallDir "config\ainput.config.json"
$backupConfig = Join-Path $tempRoot "ainput.toml"
$installedUserTerms = Join-Path $InstallDir "data\terms\user_terms.json"
$installedLearnedTerms = Join-Path $InstallDir "data\terms\learned_terms.json"
$backupUserTerms = Join-Path $tempRoot "user_terms.json"
$backupLearnedTerms = Join-Path $tempRoot "learned_terms.json"
$scriptSourceDir = Split-Path -Parent $PSCommandPath
$installedScriptsDir = Join-Path $InstallDir "scripts"
$installedUninstallScript = Join-Path $installedScriptsDir "uninstall-ainput.ps1"
$installedUninstallCmd = Join-Path $InstallDir "uninstall-ainput.cmd"

function Stop-InstalledProcess {
    param([string]$ExecutablePath)

    Get-Process ainput-desktop -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $ExecutablePath } |
        Stop-Process -Force
}

function New-Shortcut {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ShortcutPath,
        [Parameter(Mandatory = $true)]
        [string]$TargetPath,
        [string]$Arguments = "",
        [string]$WorkingDirectory = "",
        [string]$IconLocation = ""
    )

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetPath
    if ($Arguments) {
        $shortcut.Arguments = $Arguments
    }
    if ($WorkingDirectory) {
        $shortcut.WorkingDirectory = $WorkingDirectory
    }
    if ($IconLocation) {
        $shortcut.IconLocation = $IconLocation
    }
    $shortcut.Save()
}

function Set-UninstallMetadata {
    param(
        [Parameter(Mandatory = $true)]
        [string]$UninstallCmdPath,
        [Parameter(Mandatory = $true)]
        [string]$DisplayIconPath
    )

    New-Item -Path $uninstallRegistryKey -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "DisplayName" -Value "ainput" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "DisplayVersion" -Value $appVersion -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "Publisher" -Value "sai" -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "InstallLocation" -Value $InstallDir -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "DisplayIcon" -Value $DisplayIconPath -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "UninstallString" -Value ('"' + $UninstallCmdPath + '"') -PropertyType String -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "NoModify" -Value 1 -PropertyType DWord -Force | Out-Null
    New-ItemProperty -Path $uninstallRegistryKey -Name "NoRepair" -Value 1 -PropertyType DWord -Force | Out-Null
}

try {
    if (!(Test-Path $PayloadZip)) {
        throw "payload zip not found: $PayloadZip"
    }

    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    if (Test-Path $installedConfig) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $backupConfig) | Out-Null
        Copy-Item $installedConfig $backupConfig -Force
    } elseif (Test-Path $legacyInstalledConfig) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $backupConfig) | Out-Null
        Copy-Item $legacyInstalledConfig $backupConfig -Force
    }

    if (Test-Path $installedUserTerms) {
        Copy-Item $installedUserTerms $backupUserTerms -Force
    }

    if (Test-Path $installedLearnedTerms) {
        Copy-Item $installedLearnedTerms $backupLearnedTerms -Force
    }

    Stop-InstalledProcess -ExecutablePath $installedExe

    if (Test-Path $stagingDir) {
        Remove-Item $stagingDir -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
    Expand-Archive -Path $PayloadZip -DestinationPath $stagingDir -Force

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item (Join-Path $stagingDir "*") $InstallDir -Recurse -Force

    if (Test-Path $backupConfig) {
        Copy-Item $backupConfig $installedConfig -Force
    }

    if (Test-Path $backupUserTerms) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $installedUserTerms) | Out-Null
        Copy-Item $backupUserTerms $installedUserTerms -Force
    }

    if (Test-Path $backupLearnedTerms) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $installedLearnedTerms) | Out-Null
        Copy-Item $backupLearnedTerms $installedLearnedTerms -Force
    }

    New-Item -ItemType Directory -Force -Path $installedScriptsDir | Out-Null
    Copy-Item (Join-Path $scriptSourceDir "uninstall-ainput.ps1") $installedUninstallScript -Force

    Set-Content -Path $installedUninstallCmd -Encoding ASCII -Value @(
        "@echo off",
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""%~dp0scripts\uninstall-ainput.ps1"" -InstallDir ""%~dp0"""
    )

    New-Item -ItemType Directory -Force -Path $startMenuFolder | Out-Null
    New-Shortcut `
        -ShortcutPath (Join-Path $startMenuFolder "ainput.lnk") `
        -TargetPath $installedExe `
        -WorkingDirectory $InstallDir `
        -IconLocation (Join-Path $InstallDir "assets\app-icon.ico")
    New-Shortcut `
        -ShortcutPath (Join-Path $startMenuFolder "卸载 ainput.lnk") `
        -TargetPath $installedUninstallCmd `
        -WorkingDirectory $InstallDir `
        -IconLocation (Join-Path $InstallDir "assets\app-icon.ico")

    Set-UninstallMetadata -UninstallCmdPath $installedUninstallCmd -DisplayIconPath $installedExe

    if (-not $NoLaunch) {
        Start-Process -FilePath $installedExe -WorkingDirectory $InstallDir | Out-Null
    }
}
finally {
    if (Test-Path $tempRoot) {
        Remove-Item $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
