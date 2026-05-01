param(
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = if ((Split-Path -Leaf $PSScriptRoot) -eq "scripts") {
    Split-Path -Parent $PSScriptRoot
} else {
    $PSScriptRoot
}
$distRoot = Join-Path $repoRoot "dist"
$cargoTomlPath = Join-Path $repoRoot "Cargo.toml"

function Get-WorkspacePackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CargoTomlPath
    )

    $insideWorkspacePackage = $false
    foreach ($line in Get-Content $cargoTomlPath -Encoding UTF8) {
        if ($line -match '^\s*\[workspace\.package\]\s*$') {
            $insideWorkspacePackage = $true
            continue
        }
        if ($insideWorkspacePackage -and $line -match '^\s*\[') {
            break
        }
        if ($insideWorkspacePackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }

    throw "failed to derive workspace package version from $CargoTomlPath"
}

function Set-WorkspacePackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CargoTomlPath,
        [Parameter(Mandatory = $true)]
        [string]$NewVersion
    )

    $lines = [System.IO.File]::ReadAllLines($CargoTomlPath, [System.Text.Encoding]::UTF8)
    $insideWorkspacePackage = $false
    $updated = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i]
        if ($line -match '^\s*\[workspace\.package\]\s*$') {
            $insideWorkspacePackage = $true
            continue
        }
        if ($insideWorkspacePackage -and $line -match '^\s*\[') {
            break
        }
        if ($insideWorkspacePackage -and $line -match '^\s*version\s*=') {
            $lines[$i] = ('version = "' + $NewVersion + '"')
            $updated = $true
            break
        }
    }

    if (-not $updated) {
        throw "failed to update workspace package version in $CargoTomlPath"
    }

    [System.IO.File]::WriteAllLines($CargoTomlPath, $lines, (New-Object System.Text.UTF8Encoding($true)))
}

function Get-NextPackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CurrentVersion,
        [Parameter(Mandatory = $true)]
        [string]$DistRoot
    )

    if ($CurrentVersion -notmatch '^(?<prefix>.+-preview\.)(?<number>\d+)$') {
        $stamp = Get-Date -Format "yyyyMMddHHmmss"
        return ($CurrentVersion + "." + $stamp)
    }

    $prefix = $Matches["prefix"]
    $maxNumber = [int]$Matches["number"]
    $escapedPrefix = [regex]::Escape("ainput-" + $prefix)
    if (Test-Path $DistRoot) {
        $names = @()
        $names += Get-ChildItem $DistRoot -Directory -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
        $names += Get-ChildItem $DistRoot -File -Filter "ainput-*.zip" -ErrorAction SilentlyContinue | ForEach-Object { $_.BaseName }
        foreach ($name in $names) {
            if ($name -match ('^' + $escapedPrefix + '(?<number>\d+)$')) {
                $number = [int]$Matches["number"]
                if ($number -gt $maxNumber) {
                    $maxNumber = $number
                }
            }
        }
    }

    return ($prefix + ($maxNumber + 1))
}

$workspaceVersion = Get-WorkspacePackageVersion -CargoTomlPath $cargoTomlPath
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = Get-NextPackageVersion -CurrentVersion $workspaceVersion -DistRoot $distRoot
}
if ($Version -ne $workspaceVersion) {
    Set-WorkspacePackageVersion -CargoTomlPath $cargoTomlPath -NewVersion $Version
    Write-Host "Updated workspace package version: $workspaceVersion -> $Version"
}

$packageName = "ainput-$Version"
$packageDir = Join-Path $distRoot $packageName
$zipPath = Join-Path $distRoot "$packageName.zip"
if ((Test-Path $packageDir) -or (Test-Path $zipPath)) {
    throw "package version already exists, refusing to overwrite: $packageName"
}
$modelSource = Join-Path $repoRoot "models\sense-voice\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
$modelTarget = Join-Path $packageDir "models\sense-voice"
$streamingModelName = "sherpa-onnx-streaming-paraformer-bilingual-zh-en"
$streamingModelSource = Join-Path $repoRoot ("models\" + $streamingModelName)
$streamingModelTarget = Join-Path $packageDir ("models\" + $streamingModelName)
$streamingPunctuationModelName = "sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8"
$streamingPunctuationModelSource = Join-Path $repoRoot ("models\punctuation\" + $streamingPunctuationModelName)
$streamingPunctuationModelTarget = Join-Path $packageDir ("models\punctuation\" + $streamingPunctuationModelName)
$packageExe = Join-Path $packageDir "ainput-desktop.exe"
$releaseExe = Join-Path $repoRoot "target\release\ainput-desktop.exe"

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
            "display_hold_ms",
            "font_height_px",
            "font_weight",
            "font_family"
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

function Sync-MainConfigSectionKeys {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$CanonicalConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$SectionName,
        [Parameter(Mandatory = $true)]
        [string[]]$Keys,
        [hashtable]$ForcedValues = @{}
    )

    $lines = [System.IO.File]::ReadAllLines($ConfigPath, [System.Text.Encoding]::UTF8)
    $mutableLines = New-Object System.Collections.Generic.List[string]
    foreach ($line in $lines) {
        $mutableLines.Add($line) | Out-Null
    }
    $canonicalLines = [System.IO.File]::ReadAllLines($CanonicalConfigPath, [System.Text.Encoding]::UTF8)
    $canonicalSectionLines = New-Object System.Collections.Generic.List[string]
    $insideSection = $false
    $insideCanonicalSection = $false
    $updatedKeys = @{}
    $sectionStartIndex = -1
    $sectionEndIndex = -1

    foreach ($canonicalLine in $canonicalLines) {
        if ($canonicalLine -match ('^\s*\[' + [regex]::Escape($SectionName) + '\]\s*$')) {
            $insideCanonicalSection = $true
            continue
        }
        if ($insideCanonicalSection -and $canonicalLine -match '^\s*\[') {
            break
        }
        if ($insideCanonicalSection) {
            $canonicalSectionLines.Add($canonicalLine)
        }
    }

    if ($canonicalSectionLines.Count -eq 0) {
        throw "failed to find canonical section [$SectionName] in $CanonicalConfigPath"
    }

    for ($i = 0; $i -lt $mutableLines.Count; $i++) {
        $line = $mutableLines[$i]
        if ($line -match ('^\s*\[' + [regex]::Escape($SectionName) + '\]\s*$')) {
            $insideSection = $true
            $sectionStartIndex = $i
            continue
        }
        if ($insideSection -and $line -match '^\s*\[') {
            $insideSection = $false
            $sectionEndIndex = $i
        }
        if (-not $insideSection) {
            continue
        }

        foreach ($key in $Keys) {
            if ($line -notmatch ('^\s*' + [regex]::Escape($key) + '\s*=')) {
                continue
            }

            if ($ForcedValues.ContainsKey($key)) {
                $mutableLines[$i] = $ForcedValues[$key]
            } else {
                $canonicalLine = $canonicalSectionLines |
                    Where-Object { $_ -match ('^\s*' + [regex]::Escape($key) + '\s*=') } |
                    Select-Object -First 1
                if ([string]::IsNullOrWhiteSpace($canonicalLine)) {
                    throw "failed to find canonical $key in [$SectionName] of $CanonicalConfigPath"
                }
                $mutableLines[$i] = $canonicalLine
            }

            $updatedKeys[$key] = $true
            break
        }
    }

    if ($sectionStartIndex -lt 0) {
        throw "failed to find section [$SectionName] in $ConfigPath"
    }
    if ($sectionEndIndex -lt 0) {
        $sectionEndIndex = $mutableLines.Count
    }

    $insertLines = New-Object System.Collections.Generic.List[string]
    foreach ($key in $Keys) {
        if ($updatedKeys.ContainsKey($key)) {
            continue
        }

        if ($ForcedValues.ContainsKey($key)) {
            $insertLines.Add($ForcedValues[$key]) | Out-Null
            continue
        }

        $canonicalLine = $canonicalSectionLines |
            Where-Object { $_ -match ('^\s*' + [regex]::Escape($key) + '\s*=') } |
            Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($canonicalLine)) {
            throw "failed to find canonical $key in [$SectionName] of $CanonicalConfigPath"
        }
        $insertLines.Add($canonicalLine) | Out-Null
    }

    for ($offset = 0; $offset -lt $insertLines.Count; $offset++) {
        $mutableLines.Insert($sectionEndIndex + $offset, $insertLines[$offset])
    }

    [System.IO.File]::WriteAllLines($ConfigPath, $mutableLines, (New-Object System.Text.UTF8Encoding($true)))
}

function Ensure-MainConfigSection {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$CanonicalConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$SectionName
    )

    $configLines = [System.IO.File]::ReadAllLines($ConfigPath, [System.Text.Encoding]::UTF8)
    foreach ($line in $configLines) {
        if ($line -match ('^\s*\[' + [regex]::Escape($SectionName) + '\]\s*$')) {
            return
        }
    }

    $canonicalLines = [System.IO.File]::ReadAllLines($CanonicalConfigPath, [System.Text.Encoding]::UTF8)
    $sectionLines = New-Object System.Collections.Generic.List[string]
    $insideSection = $false
    foreach ($canonicalLine in $canonicalLines) {
        if ($canonicalLine -match ('^\s*\[' + [regex]::Escape($SectionName) + '\]\s*$')) {
            $insideSection = $true
        } elseif ($insideSection -and $canonicalLine -match '^\s*\[') {
            break
        }

        if ($insideSection) {
            $sectionLines.Add($canonicalLine) | Out-Null
        }
    }

    if ($sectionLines.Count -eq 0) {
        throw "failed to find canonical section [$SectionName] in $CanonicalConfigPath"
    }

    $mutableLines = New-Object System.Collections.Generic.List[string]
    foreach ($line in $configLines) {
        $mutableLines.Add($line) | Out-Null
    }
    if ($mutableLines.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace($mutableLines[$mutableLines.Count - 1])) {
        $mutableLines.Add("") | Out-Null
    }
    foreach ($sectionLine in $sectionLines) {
        $mutableLines.Add($sectionLine) | Out-Null
    }

    [System.IO.File]::WriteAllLines($ConfigPath, $mutableLines, (New-Object System.Text.UTF8Encoding($true)))
}

function Sync-ConfigCommentBeforeKey {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$CanonicalConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$Key
    )

    $lines = [System.IO.File]::ReadAllLines($ConfigPath, [System.Text.Encoding]::UTF8)
    $mutableLines = New-Object System.Collections.Generic.List[string]
    foreach ($line in $lines) {
        $mutableLines.Add($line) | Out-Null
    }
    $canonicalLines = [System.IO.File]::ReadAllLines($CanonicalConfigPath, [System.Text.Encoding]::UTF8)
    $keyPattern = '^\s*' + [regex]::Escape($Key) + '\s*='
    $canonicalKeyIndex = -1
    for ($i = 0; $i -lt $canonicalLines.Length; $i++) {
        if ($canonicalLines[$i] -match $keyPattern) {
            $canonicalKeyIndex = $i
            break
        }
    }

    if ($canonicalKeyIndex -le 0) {
        throw "failed to find canonical key [$Key] in $CanonicalConfigPath"
    }

    $canonicalCommentLine = $canonicalLines[$canonicalKeyIndex - 1]
    if ([string]::IsNullOrWhiteSpace($canonicalCommentLine) -or $canonicalCommentLine -notmatch '^\s*#') {
        throw "failed to find canonical comment before [$Key] in $CanonicalConfigPath"
    }

    for ($i = 1; $i -lt $mutableLines.Count; $i++) {
        if ($mutableLines[$i] -match $keyPattern) {
            if ($mutableLines[$i - 1] -match '^\s*#') {
                $mutableLines[$i - 1] = $canonicalCommentLine
            } else {
                $mutableLines.Insert($i, $canonicalCommentLine)
            }
            [System.IO.File]::WriteAllLines($ConfigPath, $mutableLines, (New-Object System.Text.UTF8Encoding($true)))
            return
        }
    }
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

function Get-PreferredStreamingComponent {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ModelDir,
        [Parameter(Mandatory = $true)]
        [string]$Component
    )

    $int8 = Get-ChildItem $ModelDir -File -Filter ($Component + "*.int8.onnx") -ErrorAction SilentlyContinue |
        Sort-Object Name |
        Select-Object -First 1
    if ($int8) {
        return $int8.FullName
    }

    $fallback = Get-ChildItem $ModelDir -File -Filter ($Component + "*.onnx") -ErrorAction SilentlyContinue |
        Sort-Object Name |
        Select-Object -First 1
    if ($fallback) {
        return $fallback.FullName
    }

    return $null
}

function Copy-StreamingModelRuntimeFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$TargetDir
    )

    if (!(Test-Path $SourceDir)) {
        throw "missing streaming model directory: $SourceDir"
    }

    $tokensFile = Join-Path $SourceDir "tokens.txt"
    if (!(Test-Path $tokensFile)) {
        throw "missing streaming tokens file: $tokensFile"
    }

    $components = @("encoder", "decoder", "joiner")
    $copied = New-Object System.Collections.Generic.HashSet[string]

    Copy-Item $tokensFile (Join-Path $TargetDir "tokens.txt") -Force
    foreach ($component in $components) {
        $sourceFile = Get-PreferredStreamingComponent -ModelDir $SourceDir -Component $component
        if ([string]::IsNullOrWhiteSpace($sourceFile)) {
            if ($component -eq "joiner") {
                continue
            }
            throw "missing streaming $component model in $SourceDir"
        }

        $fileName = [System.IO.Path]::GetFileName($sourceFile)
        if ($copied.Add($fileName)) {
            Copy-Item $sourceFile (Join-Path $TargetDir $fileName) -Force
        }
    }
}

Write-Host "Building release binary before packaging..."
Push-Location $repoRoot
try {
    & cargo build -p ainput-desktop --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

if (!(Test-Path $releaseExe)) {
    throw "missing release binary after build: $releaseExe"
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

$rawCaptureBackupDir = $null
$existingRawCaptureDir = Join-Path $packageDir "logs\streaming-raw-captures"
if (Test-Path $existingRawCaptureDir) {
    $rawCaptureBackupDir = Join-Path ([System.IO.Path]::GetTempPath()) ("ainput-streaming-raw-captures-" + [Guid]::NewGuid().ToString("N"))
    Copy-Item $existingRawCaptureDir $rawCaptureBackupDir -Recurse -Force
}

if (Test-Path $packageDir) {
    Remove-ItemWithRetry -Path $packageDir
}

if (Test-Path $zipPath) {
    try {
        Remove-ItemWithRetry -Path $zipPath
    } catch {
        $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $zipPath = Join-Path $distRoot "$packageName-$timestamp.zip"
        Write-Warning "existing zip is locked; writing archive to $zipPath instead"
    }
}

$mainConfigSource = Join-Path $repoRoot "config\ainput.toml"
$hudConfigSource = Get-PreferredConfigSource `
    -FileName "hud-overlay.toml" `
    -FallbackPath (Join-Path $repoRoot "config\hud-overlay.toml")

New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "config") | Out-Null
New-Item -ItemType Directory -Force -Path $modelTarget | Out-Null
New-Item -ItemType Directory -Force -Path $streamingModelTarget | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "models\punctuation") | Out-Null
New-Item -ItemType Directory -Force -Path $streamingPunctuationModelTarget | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "logs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "assets") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "data\terms") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "scripts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "fixtures\streaming-hud-e2e") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $packageDir "fixtures\streaming-selftest") | Out-Null

Copy-Item $releaseExe (Join-Path $packageDir "ainput-desktop.exe") -Force
Copy-Item $mainConfigSource (Join-Path $packageDir "config\ainput.toml") -Force
Ensure-MainConfigSection `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.endpoint"
Ensure-MainConfigSection `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.stability"
Ensure-MainConfigSection `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.finalize"
Ensure-MainConfigSection `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.performance"
Ensure-MainConfigSection `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.commit"
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming" `
    -Keys @("model_dir", "rewrite_enabled", "punctuation_model_dir", "punctuation_num_threads", "chunk_ms") `
    -ForcedValues @{
        model_dir = ('model_dir = "models/' + $streamingModelName + '"')
        punctuation_model_dir = ('punctuation_model_dir = "models/punctuation/' + $streamingPunctuationModelName + '"')
    }
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.endpoint" `
    -Keys @("enabled", "pause_ms", "soft_flush_ms", "min_segment_ms", "max_segment_ms", "tail_padding_ms", "preroll_ms")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.stability" `
    -Keys @("min_agreement", "max_rollback_chars")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.finalize" `
    -Keys @("release_drain_min_ms", "release_drain_idle_settle_ms", "release_drain_max_ms", "final_decode_timeout_ms", "release_to_commit_hard_ms", "allow_display_fallback_on_timeout")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.performance" `
    -Keys @("asr_num_threads", "punctuation_num_threads", "final_num_threads", "background_writer_threads", "gpu_enabled")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.commit" `
    -Keys @("single_commit_envelope", "reject_post_hud_flush_mutations", "require_hud_flush_before_commit")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "voice.streaming.ai_rewrite" `
    -Keys @("enabled", "endpoint_url", "model", "api_key_env", "timeout_ms", "debounce_ms", "min_visible_chars", "max_context_chars", "max_output_chars")
Sync-MainConfigSectionKeys `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -SectionName "asr" `
    -Keys @("model_dir", "provider", "sample_rate_hz", "language", "use_itn", "num_threads")
Sync-ConfigCommentBeforeKey `
    -ConfigPath (Join-Path $packageDir "config\ainput.toml") `
    -CanonicalConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -Key "fps"
Copy-HudOverlayTemplateWithValues `
    -TemplatePath (Join-Path $repoRoot "config\hud-overlay.toml") `
    -SourcePath $hudConfigSource `
    -DestinationPath (Join-Path $packageDir "config\hud-overlay.toml")
Copy-Item (Join-Path $repoRoot "README.md") (Join-Path $packageDir "README.md") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon.ico") (Join-Path $packageDir "assets\app-icon.ico") -Force
Copy-Item (Join-Path $repoRoot "assets\app-icon-256.png") (Join-Path $packageDir "assets\app-icon-256.png") -Force
Copy-Item (Join-Path $repoRoot "data\terms\base_terms.json") (Join-Path $packageDir "data\terms\base_terms.json") -Force
Copy-Item (Join-Path $repoRoot "scripts\run-streaming-live-e2e.ps1") (Join-Path $packageDir "scripts\run-streaming-live-e2e.ps1") -Force
Copy-Item (Join-Path $repoRoot "scripts\run-streaming-raw-corpus.ps1") (Join-Path $packageDir "scripts\run-streaming-raw-corpus.ps1") -Force
Copy-Item (Join-Path $repoRoot "scripts\run-startup-idle-acceptance.ps1") (Join-Path $packageDir "scripts\run-startup-idle-acceptance.ps1") -Force
Copy-Item (Join-Path $repoRoot "fixtures\streaming-hud-e2e\manifest.json") (Join-Path $packageDir "fixtures\streaming-hud-e2e\manifest.json") -Force
Copy-Item (Join-Path $repoRoot "fixtures\streaming-selftest\*") (Join-Path $packageDir "fixtures\streaming-selftest") -Recurse -Force
Copy-Item $modelSource $modelTarget -Recurse -Force
if (!(Test-Path $streamingPunctuationModelSource)) {
    throw "missing streaming punctuation model directory: $streamingPunctuationModelSource"
}
Copy-StreamingModelRuntimeFiles -SourceDir $streamingModelSource -TargetDir $streamingModelTarget
Copy-Item (Join-Path $streamingPunctuationModelSource "*") $streamingPunctuationModelTarget -Recurse -Force
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
    "3. Fast voice mode uses your configured hotkey; streaming voice mode uses hold Ctrl to talk and submits on release; press Alt+X to capture; press F1/F2 to record video; press F8/F9/F10 for automation, F7 to pause or resume playback, Esc to stop the current automation flow",
    "",
    "Files:",
    "- ainput-desktop.exe: main app",
    "- run-ainput.bat: launcher",
    "- README.md: full guide",
    "- config\ainput.toml: main config",
    "- config\hud-overlay.toml: HUD parameter document",
    "- models\\sense-voice\\: fast voice recognition model",
    ("- models\\" + $streamingModelName + "\\: streaming voice recognition model"),
    ("- models\\punctuation\\" + $streamingPunctuationModelName + "\\: streaming punctuation model"),
    "- assets\app-icon.ico: tray icon resource",
    "- data\terms\base_terms.json: built-in AI terms",
    "- scripts\run-streaming-live-e2e.ps1: streaming HUD/readback acceptance script",
    "- scripts\run-streaming-raw-corpus.ps1: raw capture replay acceptance script",
    "- scripts\run-startup-idle-acceptance.ps1: startup idle no-auto-recording acceptance script",
    "- fixtures\streaming-hud-e2e\: synthetic HUD/readback acceptance fixtures",
    "- fixtures\streaming-selftest\: fixed wav streaming acceptance fixtures",
    "- logs\: runtime logs",
    "",
    "Notes:",
    "- Launch at login is enabled by default and can be toggled from the tray menu",
    "- Release build does not show a console window",
    "- data\terms\user_terms.json and learned_terms.json will be created on first use",
    "- Streaming voice now uses hold Ctrl to trigger the local streaming paraformer model, and submits the finalized text after release",
    "- You can open config\hud-overlay.toml directly from the tray menu to adjust font size, color, width, and position",
    "- Saving config\hud-overlay.toml hot-reloads the HUD immediately",
    "- New preview packages reuse the latest dist HUD config when available so HUD settings are kept",
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

if (![string]::IsNullOrWhiteSpace($rawCaptureBackupDir) -and (Test-Path $rawCaptureBackupDir)) {
    $restoredRawCaptureDir = Join-Path $packageDir "logs\streaming-raw-captures"
    New-Item -ItemType Directory -Force -Path $restoredRawCaptureDir | Out-Null
    Copy-Item (Join-Path $rawCaptureBackupDir "*") $restoredRawCaptureDir -Recurse -Force
    Remove-Item $rawCaptureBackupDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output $packageDir
Write-Output $zipPath

