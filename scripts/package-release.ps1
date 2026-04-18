param(
    [string]$Version = "1.0.14-preview.6"
)

$ErrorActionPreference = "Stop"

$packageName = "ainput-$Version"
$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Join-Path $repoRoot "dist"
$packageDir = Join-Path $distRoot $packageName
$zipPath = Join-Path $distRoot "$packageName.zip"
$modelSource = Join-Path $repoRoot "models\sense-voice\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
$modelTarget = Join-Path $packageDir "models\sense-voice"
$streamingModelSource = Join-Path $repoRoot "models\streaming-zipformer-small-bilingual-zh-en"
$streamingModelTarget = Join-Path $packageDir "models\streaming-zipformer-small-bilingual-zh-en"
$packageExe = Join-Path $packageDir "ainput-desktop.exe"

function Get-PreferredConfigSource {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FileName,
        [Parameter(Mandatory = $true)]
        [string]$FallbackPath
    )

    $sameVersionPath = Join-Path $packageDir "config\$FileName"
    if (Test-Path $sameVersionPath) {
        return $sameVersionPath
    }

    $previousPackage = Get-ChildItem $distRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object {
            $_.Name -like 'ainput-*' -and
            $_.FullName -ne $packageDir -and
            (Test-Path (Join-Path $_.FullName "config\$FileName"))
        } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if ($previousPackage) {
        return (Join-Path $previousPackage.FullName "config\$FileName")
    }

    return $FallbackPath
}

function Copy-HudOverlayTemplateWithValues {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TemplatePath,
        [Parameter(Mandatory = $true)]
        [string]$SourcePath,
        [Parameter(Mandatory = $true)]
        [string]$DestinationPath
    )

    $content = Get-Content $TemplatePath -Raw
    if (Test-Path $SourcePath) {
        $sourceLines = Get-Content $SourcePath
        $keys = @(
            "anchor",
            "offset_x_px",
            "offset_y_px",
            "width_px",
            "min_width_px",
            "min_height_px",
            "min_text_width_px",
            "padding_x_px",
            "padding_y_px",
            "corner_radius_px",
            "display_hold_ms",
            "font_height_px",
            "font_weight",
            "font_family",
            "text_align",
            "text_color",
            "background_color",
            "background_alpha"
        )

        foreach ($key in $keys) {
            $pattern = '^\s*' + [regex]::Escape($key) + '\s*=.*$'
            $sourceLine = $sourceLines | Where-Object { $_ -match $pattern } | Select-Object -First 1
            if ($sourceLine) {
                $content = [regex]::Replace($content, "(?m)$pattern", $sourceLine, 1)
            }
        }
    }

    Set-Content -Path $DestinationPath -Value $content -Encoding Unicode
}

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

$mainConfigSource = Get-PreferredConfigSource `
    -FileName "ainput.toml" `
    -FallbackPath (Join-Path $repoRoot "config\ainput.toml")
$hudConfigSource = Get-PreferredConfigSource `
    -FileName "hud-overlay.toml" `
    -FallbackPath (Join-Path $repoRoot "config\hud-overlay.toml")

New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "config") | Out-Null
New-Item -ItemType Directory -Force -Path $modelTarget | Out-Null
New-Item -ItemType Directory -Force -Path $streamingModelTarget | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "assets") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "data\terms") | Out-Null

Copy-Item (Join-Path $repoRoot "target\release\ainput-desktop.exe") (Join-Path $packageDir "ainput-desktop.exe") -Force
Copy-Item $mainConfigSource (Join-Path $packageDir "config\ainput.toml") -Force
Copy-HudOverlayTemplateWithValues `
    -TemplatePath (Join-Path $repoRoot "config\hud-overlay.toml") `
    -SourcePath $hudConfigSource `
    -DestinationPath (Join-Path $packageDir "config\hud-overlay.toml")
Copy-Item (Join-Path $repoRoot "README.md") (Join-Path $packageDir "README.md") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon.ico") (Join-Path $packageDir "assets\app-icon.ico") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon-256.png") (Join-Path $packageDir "assets\app-icon-256.png") -Force
Copy-Item (Join-Path $repoRoot "data\terms\base_terms.json") (Join-Path $packageDir "data\terms\base_terms.json") -Force
Copy-Item $modelSource $modelTarget -Recurse -Force
Copy-Item $streamingModelSource $streamingModelTarget -Recurse -Force

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
    "3. Hold Alt+Z to talk; press Alt+X to capture; press F1/F2 to record video; press F8/F9/F10 for automation, F7 to pause or resume playback, Esc to stop the current automation flow",
    "",
    "Files:",
    "- ainput-desktop.exe: main app",
    "- run-ainput.bat: launcher",
    "- README.md: full guide",
    "- config\ainput.toml: main config",
    "- config\hud-overlay.toml: HUD parameter document",
    "- models\\sense-voice\\: fast voice recognition model",
    "- models\\streaming-zipformer-small-bilingual-zh-en\\: streaming voice recognition model",
    "- assets\app-icon.ico: tray icon resource",
    "- data\terms\base_terms.json: built-in AI terms",
    "- logs\: runtime logs",
    "",
    "Notes:",
    "- Launch at login is enabled by default and can be toggled from the tray menu",
    "- Release build does not show a console window",
    "- data\terms\user_terms.json and learned_terms.json will be created on first use",
    "- Streaming voice shows a bottom-center HUD above the taskbar while the hotkey is held, and submits the rewritten full text only after release",
    "- You can open config\hud-overlay.toml directly from the tray menu to adjust font size, color, width, and position",
    "- Saving config\hud-overlay.toml hot-reloads the HUD immediately",
    "- New preview packages reuse the latest dist config files when available so HUD settings are kept",
    "- Clipboard fallback is used when direct paste fails",
    "- Recording options are available from the tray: audio, mouse, watermark, FPS, and quality",
    "- During automation recording and playback, the tray icon, HUD, and click feedback show the current state",
    "- During automation playback, keyboard input, mouse clicks, wheel input, and clear mouse movement will auto-pause playback",
    "- Automation repeat count supports presets and custom values, and the last used value is written to config\\ainput.toml"
)

New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
Set-Content -Path (Join-Path $packageDir "logs\README.txt") -Encoding UTF8 -Value @(
    "Logs are written to this directory.",
    "The latest transcription is stored in last_result.txt."
)

Compress-Archive -Path (Join-Path $packageDir "*") -DestinationPath $zipPath -Force

Write-Output $packageDir
Write-Output $zipPath
