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

$wavRoot = Join-Path $runtimeRoot "models\sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30"
$testWavDir = Get-ChildItem $wavRoot -Recurse -Directory |
    Where-Object { $_.Name -eq "test_wavs" } |
    Select-Object -First 1

if (-not $testWavDir) {
    throw "找不到 streaming 模型自带的 test_wavs 目录：$wavRoot"
}

$pythonScript = @'
import struct
import wave
from pathlib import Path

repo_root = Path(r"__REPO_ROOT__")
test_dir = Path(r"__TEST_WAV_DIR__")
out_path = repo_root / "tmp" / "streaming-regression-concat-long-16k.wav"
out_path.parent.mkdir(parents=True, exist_ok=True)
files = [test_dir / name for name in ["0.wav", "1.wav", "2.wav", "3.wav", "4.wav", "46.wav"]]

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
$pythonScript = $pythonScript.Replace("__TEST_WAV_DIR__", $testWavDir.FullName.Replace('\', '\\'))
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

$cases = @(
    @{ Name = "0.wav"; MinChars = 8 },
    @{ Name = "1.wav"; MinChars = 6 },
    @{ Name = "2.wav"; MinChars = 4 },
    @{ Name = "3.wav"; MinChars = 8 },
    @{ Name = "4.wav"; MinChars = 5 },
    @{ Name = "46.wav"; MinChars = 4 },
    @{ Name = "__concat_long__"; MinChars = 22 }
)

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
        $wavPath = Join-Path $testWavDir.FullName $case.Name
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
    "wav_dir=$($testWavDir.FullName)",
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
