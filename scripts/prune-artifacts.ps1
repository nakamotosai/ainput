param(
    [string[]]$KeepVersions = @(),
    [int]$KeepNewestCount = 2,
    [int]$KeepNewestSetupCount = 1,
    [switch]$PruneTmp,
    [switch]$WhatIf
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Join-Path $repoRoot "dist"
$runBat = Join-Path $repoRoot "run-ainput.bat"

function Normalize-PackageName {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }
    if ($Value -like "ainput-*") {
        return $Value.Trim()
    }
    return ("ainput-" + $Value.Trim())
}

function Get-ItemSizeBytes {
    param([string]$Path)
    if (!(Test-Path -LiteralPath $Path)) {
        return 0L
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (!$item.PSIsContainer) {
        return [int64]$item.Length
    }
    $sum = (Get-ChildItem -LiteralPath $Path -Force -Recurse -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) {
        return 0L
    }
    return [int64]$sum
}

function Remove-ItemSafely {
    param(
        [string]$Path,
        [ref]$FreedBytes
    )
    if (!(Test-Path -LiteralPath $Path)) {
        return
    }
    $bytes = Get-ItemSizeBytes -Path $Path
    if ($WhatIf) {
        Write-Host "[whatif] remove $Path ($([math]::Round($bytes / 1GB, 2)) GB)"
        $FreedBytes.Value += $bytes
        return
    }
    Remove-Item -LiteralPath $Path -Recurse -Force
    Write-Host "removed $Path ($([math]::Round($bytes / 1GB, 2)) GB)"
    $FreedBytes.Value += $bytes
}

$keepPackages = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)

if (Test-Path -LiteralPath $runBat) {
    $runText = Get-Content -LiteralPath $runBat -Raw
    $match = [regex]::Match($runText, 'dist\\(ainput-[^\\"]+)')
    if ($match.Success) {
        [void]$keepPackages.Add($match.Groups[1].Value)
    }
}

foreach ($version in $KeepVersions) {
    $packageName = Normalize-PackageName -Value $version
    if ($packageName) {
        [void]$keepPackages.Add($packageName)
    }
}

$distDirs = @()
if (Test-Path -LiteralPath $distRoot) {
    $distDirs = Get-ChildItem -LiteralPath $distRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending
}

foreach ($dir in $distDirs | Select-Object -First ([Math]::Max($KeepNewestCount, 0))) {
    [void]$keepPackages.Add($dir.Name)
}

$freedBytes = 0L

foreach ($dir in $distDirs) {
    if (!$keepPackages.Contains($dir.Name)) {
        Remove-ItemSafely -Path $dir.FullName -FreedBytes ([ref]$freedBytes)
    }
}

$distFiles = @()
if (Test-Path -LiteralPath $distRoot) {
    $distFiles = Get-ChildItem -LiteralPath $distRoot -File -ErrorAction SilentlyContinue
}

foreach ($file in $distFiles) {
    if ($file.Extension -eq ".zip") {
        if (!$keepPackages.Contains($file.BaseName)) {
            Remove-ItemSafely -Path $file.FullName -FreedBytes ([ref]$freedBytes)
        }
        continue
    }
}

$setupFiles = @()
if (Test-Path -LiteralPath $distRoot) {
    $setupFiles = Get-ChildItem -LiteralPath $distRoot -File -Filter "ainput-setup-*.exe" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending
}

foreach ($file in $setupFiles | Select-Object -Skip ([Math]::Max($KeepNewestSetupCount, 0))) {
    Remove-ItemSafely -Path $file.FullName -FreedBytes ([ref]$freedBytes)
}

Get-ChildItem -LiteralPath $repoRoot -Directory -Filter "target*" -ErrorAction SilentlyContinue |
    ForEach-Object {
        Remove-ItemSafely -Path $_.FullName -FreedBytes ([ref]$freedBytes)
    }

foreach ($name in @("~.CAB", "~.DDF", "~.RPT", "~_LAYOUT.INF")) {
    $candidate = Join-Path $repoRoot $name
    Remove-ItemSafely -Path $candidate -FreedBytes ([ref]$freedBytes)
}

$tmpRoot = Join-Path $repoRoot "tmp"
if (Test-Path -LiteralPath $tmpRoot) {
    Get-ChildItem -LiteralPath $tmpRoot -Directory -Filter "ainput-installer-*" -ErrorAction SilentlyContinue |
        ForEach-Object {
            Remove-ItemSafely -Path $_.FullName -FreedBytes ([ref]$freedBytes)
        }
}

if ($PruneTmp) {
    if (Test-Path -LiteralPath $tmpRoot) {
        $tmpPatterns = @(
            "launch-*",
            "launch-preview*",
            "streaming-live-e2e",
            "streaming-raw-*",
            "recent-12h-precision-analysis",
            "startup-idle-acceptance",
            "send-ctrl-interactive*",
            "asr-model-sweep",
            "*.bak"
        )
        foreach ($pattern in $tmpPatterns) {
            Get-ChildItem -LiteralPath $tmpRoot -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like $pattern } |
                ForEach-Object {
                    Remove-ItemSafely -Path $_.FullName -FreedBytes ([ref]$freedBytes)
                }
        }
    }
}

$freedGb = [math]::Round($freedBytes / 1GB, 2)
Write-Host ("keep packages: " + (($keepPackages | Sort-Object) -join ", "))
Write-Host ("reclaimed_gb=" + $freedGb)
