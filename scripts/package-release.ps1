param(
    [string]$Version = "1.0.1"
)

$ErrorActionPreference = "Stop"

$packageName = "ainput-$Version"
$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Join-Path $repoRoot "dist"
$packageDir = Join-Path $distRoot $packageName
$zipPath = Join-Path $distRoot "$packageName.zip"
$modelSource = Join-Path $repoRoot "models\sense-voice\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
$modelTarget = Join-Path $packageDir "models\sense-voice"

if (Test-Path $packageDir) {
    Remove-Item $packageDir -Recurse -Force
}

if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}

New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "config") | Out-Null
New-Item -ItemType Directory -Force -Path $modelTarget | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "assets") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "data\terms") | Out-Null

Copy-Item (Join-Path $repoRoot "target\release\ainput-desktop.exe") (Join-Path $packageDir "ainput-desktop.exe") -Force
Copy-Item (Join-Path $repoRoot "config\ainput.config.json") (Join-Path $packageDir "config\ainput.config.json") -Force
Copy-Item (Join-Path $repoRoot "README.md") (Join-Path $packageDir "README.md") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon.ico") (Join-Path $packageDir "assets\app-icon.ico") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon-256.png") (Join-Path $packageDir "assets\app-icon-256.png") -Force
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
    "3. Hold Ctrl+Win or hold mouse middle button, then release to transcribe and paste",
    "",
    "Files:",
    "- ainput-desktop.exe: main app",
    "- run-ainput.bat: launcher",
    "- README.md: full guide",
    "- config\ainput.config.json: default config",
    "- models\: offline speech model",
    "- assets\app-icon.ico: tray icon resource",
    "- logs\: runtime logs",
    "",
    "Notes:",
    "- Release build does not show a console window",
    "- data\terms\user_terms.txt will be created on first use",
    "- Clipboard fallback is used when direct paste fails"
)

Set-Content -Path (Join-Path $packageDir "logs\README.txt") -Encoding UTF8 -Value @(
    "Logs are written to this directory.",
    "The latest transcription is stored in last_result.txt."
)

Compress-Archive -Path (Join-Path $packageDir "*") -DestinationPath $zipPath -Force

Write-Output $packageDir
Write-Output $zipPath
