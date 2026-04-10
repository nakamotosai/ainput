param(
    [string]$Version = "1.0.6"
)

$ErrorActionPreference = "Stop"

$packageName = "ainput-$Version"
$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Join-Path $repoRoot "dist"
$packageDir = Join-Path $distRoot $packageName
$zipPath = Join-Path $distRoot "$packageName.zip"
$modelSource = Join-Path $repoRoot "models\sense-voice\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
$modelTarget = Join-Path $packageDir "models\sense-voice"
$packageExe = Join-Path $packageDir "ainput-desktop.exe"

function Remove-ItemWithRetry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (!(Test-Path $Path)) {
        return
    }

    $lastError = $null
    for ($i = 0; $i -lt 10; $i++) {
        try {
            Remove-Item $Path -Recurse -Force -ErrorAction Stop
            return
        } catch {
            if (!(Test-Path $Path)) {
                return
            }
            $lastError = $_
            Start-Sleep -Milliseconds 500
        }
    }

    throw $lastError
}

Get-Process ainput-desktop -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $packageExe } |
    Stop-Process -Force

for ($i = 0; $i -lt 20; $i++) {
    $running = Get-Process ainput-desktop -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $packageExe }
    if (-not $running) {
        break
    }
    Start-Sleep -Milliseconds 250
}

if (Test-Path $packageDir) {
    Remove-ItemWithRetry -Path $packageDir
}

if (Test-Path $zipPath) {
    Remove-ItemWithRetry -Path $zipPath
}

New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "config") | Out-Null
New-Item -ItemType Directory -Force -Path $modelTarget | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "assets") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "data\terms") | Out-Null

Copy-Item (Join-Path $repoRoot "target\release\ainput-desktop.exe") (Join-Path $packageDir "ainput-desktop.exe") -Force
Copy-Item (Join-Path $repoRoot "config\ainput.toml") (Join-Path $packageDir "config\ainput.toml") -Force
Copy-Item (Join-Path $repoRoot "README.md") (Join-Path $packageDir "README.md") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon.ico") (Join-Path $packageDir "assets\app-icon.ico") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon-256.png") (Join-Path $packageDir "assets\app-icon-256.png") -Force
Copy-Item (Join-Path $repoRoot "data\terms\base_terms.json") (Join-Path $packageDir "data\terms\base_terms.json") -Force
Copy-Item $modelSource $modelTarget -Recurse -Force

Set-Content -Path (Join-Path $packageDir "run-ainput.bat") -Encoding ASCII -Value @(
    "@echo off",
    "setlocal",
    "cd /d ""%~dp0""",
    "start """" ""%~dp0ainput-desktop.exe""",
    "exit /b 0"
)

Set-Content -Path (Join-Path $packageDir "README.txt") -Encoding UTF8 -Value @(
    "ainput $Version",
    "",
    "Start:",
    "1. Double-click run-ainput.bat",
    "2. The app will stay in the system tray",
    "3. Hold Alt+Z to talk; press Alt+X to capture; press F1/F2 to record video; press F8/F9/F10 for automation, F7 to pause or resume, Esc to stop the current automation flow",
    "",
    "Files:",
    "- ainput-desktop.exe: main app",
    "- run-ainput.bat: launcher",
    "- README.md: full guide",
    "- config\ainput.toml: default config",
    "- models\: offline speech model",
    "- assets\app-icon.ico: tray icon resource",
    "- data\terms\base_terms.json: built-in AI terms",
    "- logs\: runtime logs",
    "",
    "Notes:",
    "- Launch at login is enabled by default and can be toggled from the tray menu",
    "- Release build does not show a console window",
    "- data\terms\user_terms.json and learned_terms.json will be created on first use",
    "- Clipboard fallback is used when direct paste fails",
    "- Recording options are available from the tray: audio, mouse, watermark, FPS, and quality",
    "- During automation playback, any manual keyboard or mouse input will auto-pause playback"
)

New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
Set-Content -Path (Join-Path $packageDir "logs\README.txt") -Encoding UTF8 -Value @(
    "Logs are written to this directory.",
    "The latest transcription is stored in last_result.txt."
)

Compress-Archive -Path (Join-Path $packageDir "*") -DestinationPath $zipPath -Force

Write-Output $packageDir
Write-Output $zipPath
