param(
    [string]$ExePath = "",
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $ExePath = Join-Path $repoRoot "target\release\ainput-desktop.exe"
    } else {
        $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
    }
}

if (!(Test-Path $ExePath)) {
    throw "找不到可执行文件：$ExePath"
}

$runtimeRootCandidate = Split-Path -Parent $ExePath
$runtimeRoot = if (Test-Path (Join-Path $runtimeRootCandidate "config\ainput.toml")) {
    $runtimeRootCandidate
} else {
    $repoRoot
}

function Get-StreamingModelDirFromConfig {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$RepoRoot
    )

    $insideSection = $false
    foreach ($line in Get-Content $ConfigPath -Encoding UTF8) {
        if ($line -match '^\s*\[voice\.streaming\]\s*$') {
            $insideSection = $true
            continue
        }
        if ($insideSection -and $line -match '^\s*\[') {
            break
        }
        if ($insideSection -and $line -match '^\s*model_dir\s*=\s*"([^"]+)"') {
            $relativePath = $Matches[1] -replace '/', '\'
            return Join-Path $RepoRoot $relativePath
        }
    }

    throw "找不到 [voice.streaming].model_dir：$ConfigPath"
}

$streamingModelDir = Get-StreamingModelDirFromConfig `
    -ConfigPath (Join-Path $repoRoot "config\ainput.toml") `
    -RepoRoot $repoRoot
$testWavDir = Join-Path $streamingModelDir "test_wavs"

if (!(Test-Path $testWavDir)) {
    throw "找不到当前 streaming 模型自带的 test_wavs 目录：$testWavDir"
}

$thresholdMap = @{
    "0.wav" = 18
    "1.wav" = 10
    "2.wav" = 12
    "3.wav" = 16
    "4.wav" = 10
    "46.wav" = 8
    "8k.wav" = 4
}
$preferredCaseOrder = @("0.wav", "1.wav", "2.wav", "3.wav", "4.wav", "46.wav", "8k.wav")
$cases = New-Object System.Collections.Generic.List[object]

foreach ($sampleName in $preferredCaseOrder) {
    $samplePath = Join-Path $testWavDir $sampleName
    if (!(Test-Path $samplePath)) {
        continue
    }
    $cases.Add([pscustomobject]@{
        Name = $sampleName
        MinChars = $thresholdMap[$sampleName]
    }) | Out-Null
}

if ($cases.Count -eq 0) {
    throw "当前 streaming 模型没有可用回归 wav：$testWavDir"
}

$concatSampleNames = @("0.wav", "1.wav", "2.wav", "3.wav", "4.wav", "46.wav") |
    Where-Object { Test-Path (Join-Path $testWavDir $_) }
$concatMinChars = 0
if ($concatSampleNames.Count -ge 3) {
    $concatMinChars = [Math]::Max(
        22,
        [int][Math]::Floor(
            (($cases | Where-Object { $_.Name -in $concatSampleNames } | Measure-Object -Property MinChars -Sum).Sum) * 0.7
        )
    )
}

$pythonScript = @'
import struct
import wave
from pathlib import Path

repo_root = Path(r"__REPO_ROOT__")
test_dir = Path(r"__TEST_WAV_DIR__")
out_path = repo_root / "tmp" / "streaming-regression-concat-long-16k.wav"
out_path.parent.mkdir(parents=True, exist_ok=True)
files = [test_dir / name for name in [__LONG_SAMPLE_NAMES__]]

def read_wav(path):
    with wave.open(str(path), "rb") as wf:
        rate = wf.getframerate()
        frames = wf.readframes(wf.getnframes())
        data = struct.unpack("<" + "h" * (len(frames) // 2), frames)
        return rate, list(data)

rate, _ = read_wav(files[0])
silence = [0] * int(rate * 0.25)
combo = []
for index, wav_path in enumerate(files):
    current_rate, data = read_wav(wav_path)
    if current_rate != rate:
        raise RuntimeError(f"unexpected sample rate: {wav_path} -> {current_rate}")
    combo.extend(data)
    if index != len(files) - 1:
        combo.extend(silence)

with wave.open(str(out_path), "wb") as wf:
    wf.setnchannels(1)
    wf.setsampwidth(2)
    wf.setframerate(rate)
    wf.writeframes(struct.pack("<" + "h" * len(combo), *combo))

print(out_path)
'@

$pythonScript = $pythonScript.Replace("__REPO_ROOT__", $repoRoot.Replace('\', '\\'))
$pythonScript = $pythonScript.Replace("__TEST_WAV_DIR__", $testWavDir.Replace('\', '\\'))
$pythonScript = $pythonScript.Replace(
    "__LONG_SAMPLE_NAMES__",
    (($concatSampleNames | ForEach-Object { '"' + $_ + '"' }) -join ", ")
)
$concatWavPath = ""

if ($concatSampleNames.Count -ge 3) {
    $pythonTempPath = Join-Path $repoRoot "tmp\streaming-regression-build-long.py"
    Set-Content -Path $pythonTempPath -Value $pythonScript -Encoding UTF8
    $concatOutput = python $pythonTempPath
    if ($LASTEXITCODE -ne 0) {
        throw "生成长句回归 wav 失败"
    }

    $concatWavPath = ($concatOutput | Select-Object -Last 1).Trim()
    if (!(Test-Path $concatWavPath)) {
        throw "长句回归 wav 不存在：$concatWavPath"
    }

    $cases.Add([pscustomobject]@{
        Name = "__concat_long__"
        MinChars = $concatMinChars
    }) | Out-Null
}

function Get-VisibleCharCount {
    param([string]$Text)

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return 0
    }

    $trimmed = $Text.Trim()
    $trimmed = $trimmed -replace '^[\s\p{P}\p{S}]+', ''
    $trimmed = $trimmed -replace '[\s\p{P}\p{S}]+$', ''
    return $trimmed.Length
}

$results = @()
$failures = @()

foreach ($case in $cases) {
    if ($case.Name -eq "__concat_long__") {
        $wavPath = $concatWavPath
    } else {
        $wavPath = Join-Path $testWavDir $case.Name
    }
    if (!(Test-Path $wavPath)) {
        $failures += "缺少样本：$($case.Name)"
        continue
    }

    $lastResultPath = Join-Path $runtimeRoot "logs\last_result.txt"
    Remove-Item $lastResultPath -ErrorAction SilentlyContinue
    Push-Location $runtimeRoot
    $previousRuntimeRoot = $env:AINPUT_ROOT
    $env:AINPUT_ROOT = $runtimeRoot
    try {
        & $ExePath transcribe-streaming-wav $wavPath | Out-Null
    } finally {
        if ($null -eq $previousRuntimeRoot) {
            Remove-Item Env:AINPUT_ROOT -ErrorAction SilentlyContinue
        } else {
            $env:AINPUT_ROOT = $previousRuntimeRoot
        }
        Pop-Location
    }
    if ($LASTEXITCODE -ne 0) {
        $failures += "执行失败：$($case.Name)"
        continue
    }

    for ($i = 0; $i -lt 15 -and !(Test-Path $lastResultPath); $i++) {
        Start-Sleep -Milliseconds 200
    }
    $text = if (Test-Path $lastResultPath) {
        (Get-Content $lastResultPath -Raw).Trim()
    } else {
        ""
    }
    $visibleChars = Get-VisibleCharCount -Text $text
    $passed = $visibleChars -ge $case.MinChars

    $results += [pscustomobject]@{
        wav           = if ($case.Name -eq "__concat_long__") { "concat-long-16k.wav" } else { $case.Name }
        visible_chars = $visibleChars
        min_chars     = $case.MinChars
        passed        = $passed
        text          = $text
    }

    if (-not $passed) {
        $failures += "$($case.Name) 只得到 $visibleChars 个可见字符，低于阈值 $($case.MinChars)"
    }
}

$reportPath = Join-Path $repoRoot "tmp\streaming-regression-latest.txt"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $reportPath) | Out-Null

$reportLines = @(
    "ainput streaming regression",
    "exe=$ExePath",
    "runtime_root=$runtimeRoot",
    "model_dir=$streamingModelDir",
    "wav_dir=$testWavDir",
    "gate=fixed-wav-only",
    "note=This report does not replace live hotkey verification.",
    ""
)

foreach ($result in $results) {
    $reportLines += "[{0}] chars={1}/{2} text={3}" -f $result.wav, $result.visible_chars, $result.min_chars, $result.text
}

if ($failures.Count -gt 0) {
    $reportLines += ""
    $reportLines += "FAILURES:"
    $reportLines += $failures
}

Set-Content -Path $reportPath -Value $reportLines -Encoding UTF8
$results | Format-Table -AutoSize
Write-Output "报告已写入：$reportPath"

if ($failures.Count -gt 0) {
    throw ($failures -join "; ")
}
