param(
    [string]$OutputDir = "",
    [string]$VoiceName = "Microsoft Huihui Desktop"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $repoRoot "fixtures\streaming-selftest"
}

$ffmpeg = (Get-Command ffmpeg -ErrorAction SilentlyContinue).Source
if ([string]::IsNullOrWhiteSpace($ffmpeg)) {
    $ffmpeg = "C:\Users\sai\ffmpeg\bin\ffmpeg.exe"
}
if (!(Test-Path $ffmpeg)) {
    throw "找不到 ffmpeg：$ffmpeg"
}

Add-Type -AssemblyName System.Speech
$speaker = New-Object System.Speech.Synthesis.SpeechSynthesizer
$availableVoices = $speaker.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name }
if ($availableVoices -contains $VoiceName) {
    $speaker.SelectVoice($VoiceName)
} elseif ($availableVoices.Count -gt 0) {
    $speaker.SelectVoice($availableVoices[0])
}
$speaker.Rate = -1
$speaker.Volume = 100

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$rawDir = Join-Path $OutputDir "raw"
New-Item -ItemType Directory -Force -Path $rawDir | Out-Null

function Decode-Utf8Base64([string]$value) {
    $bytes = [Convert]::FromBase64String($value)
    return [System.Text.Encoding]::UTF8.GetString($bytes)
}

$cases = @(
    @{
        id = "sentence_01"
        text_b64 = "5ZaC5ZaC5ZaC77yM5L2g5aW95L2g5aW977yM5oCO5LmI5Zue5LqL5ZWK77yf"
        wav = "sentence-01.wav"
        min_partial_updates = 2
        shortfall_tolerance_chars = 3
    },
    @{
        id = "sentence_02"
        text_b64 = "5oiR55qE5ZCN5a2X5Y+r6ICB6JSh77yM546w5Zyo6L+Z5Liq5ZCQ5a2X5LiN5aSf5Lid5ruR44CC"
        wav = "sentence-02.wav"
        min_partial_updates = 2
        shortfall_tolerance_chars = 3
    },
    @{
        id = "sentence_03"
        text_b64 = "54S25ZCO5LiN566h5oiR6K+05aSa5bCR5Liq5a2X77yM5a6D5rC46L+c5Y+q6IO95pi+56S65Ye65p2l5Lik5Liq5a2X44CC"
        wav = "sentence-03.wav"
        min_partial_updates = 3
        shortfall_tolerance_chars = 3
    },
    @{
        id = "sentence_04"
        text_b64 = "5bqU6K+l5piv5oiR5LiN5pat5Zyw6K+06K+d5LmL5ZCO77yM5a6D6IO95LiN5pat5Zyw5Ye6546w5paH5a2X44CC"
        wav = "sentence-04.wav"
        min_partial_updates = 3
        shortfall_tolerance_chars = 3
    },
    @{
        id = "sentence_05"
        text_b64 = "5piO5piO6L+Z5LiqSFVE5LiK6Z2i5bey57uP5oqK5q2j56Gu55qE5paH5qGI5pi+56S65Ye65p2l5LqG77yM5L2G5piv5a6D5pyJ5pe25YCZ5LiK5bGP6L+Y5piv5oWi44CC"
        wav = "sentence-05.wav"
        min_partial_updates = 4
        shortfall_tolerance_chars = 4
    },
    @{
        id = "sentence_combo_long"
        text_b64 = "54S25ZCO5LiN566h5oiR6K+05aSa5bCR5Liq5a2X77yM5a6D5rC46L+c5Y+q6IO95pi+56S65Ye65p2l5Lik5Liq5a2X44CC5bqU6K+l5piv5oiR5LiN5pat5Zyw6K+06K+d5LmL5ZCO77yM5a6D6IO95LiN5pat5Zyw5Ye6546w5paH5a2X44CC5piO5piO6L+Z5LiqSFVE5LiK6Z2i5bey57uP5oqK5q2j56Gu55qE5paH5qGI5pi+56S65Ye65p2l5LqG77yM5L2G5piv5a6D5pyJ5pe25YCZ5LiK5bGP6L+Y5piv5oWi44CC"
        wav = "sentence-combo-long.wav"
        min_partial_updates = 5
        shortfall_tolerance_chars = 5
    }
)

$manifestCases = @()

foreach ($case in $cases) {
    $text = Decode-Utf8Base64 $case.text_b64
    $rawPath = Join-Path $rawDir ($case.id + ".wav")
    $finalPath = Join-Path $OutputDir $case.wav

    $speaker.SetOutputToWaveFile($rawPath)
    $speaker.Speak($text)
    $speaker.SetOutputToNull()

    & $ffmpeg -y -loglevel error -i $rawPath -ac 1 -ar 48000 $finalPath | Out-Null

    $manifestCases += [ordered]@{
        id = $case.id
        wav_path = $case.wav
        expected_text = $text
        min_partial_updates = $case.min_partial_updates
        shortfall_tolerance_chars = $case.shortfall_tolerance_chars
    }
}

$manifest = [ordered]@{
    version = 1
    fixture_root = "."
    cases = $manifestCases
}

$manifestPath = Join-Path $OutputDir "manifest.json"
$manifest | ConvertTo-Json -Depth 6 | Set-Content -Path $manifestPath -Encoding UTF8

Write-Host "fixtures ready:"
Write-Host $OutputDir
Write-Host $manifestPath
