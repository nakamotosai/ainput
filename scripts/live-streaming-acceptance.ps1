param(
    [string]$ExePath = "",
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        throw "必须提供 -ExePath 或 -Version，例如：.\scripts\live-streaming-acceptance.ps1 -Version 1.0.0-preview.24"
    }
    $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
}

if (!(Test-Path $ExePath)) {
    throw "找不到可执行文件：$ExePath"
}

$runtimeRoot = Split-Path -Parent $ExePath
$logPath = Join-Path $runtimeRoot "logs\ainput.log"
$historyPath = Join-Path $runtimeRoot "logs\voice-history.log"
$reportPath = Join-Path $runtimeRoot "tmp\live-streaming-acceptance-latest.txt"
$sentences = @(
    "喂喂喂，你好你好，怎么回事啊？",
    "我的名字叫老蔡，现在这个吐字不够丝滑。",
    "然后不管我说多少个字，它永远只能显示出来两个字。",
    "应该是我不断的说话之后，它能不断的出现文字。",
    "明明这个 HUD 上面已经把正确的文案显示出来了，但是它有时候上屏还是慢。"
)

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $reportPath) | Out-Null
if (!(Test-Path $logPath)) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $logPath) | Out-Null
    Set-Content -Path $logPath -Value "" -Encoding UTF8
}
if (!(Test-Path $historyPath)) {
    Set-Content -Path $historyPath -Value "" -Encoding UTF8
}

Write-Host ""
Write-Host "live streaming acceptance"
Write-Host "exe: $ExePath"
Write-Host "runtime root: $runtimeRoot"
Write-Host ""
Write-Host "请先启动这个 preview 包，然后按下面句子逐条验收。"
Write-Host "每一轮都按住 Ctrl 说完整句，松手确认已上屏，再回来按回车继续。"
Write-Host ""

$reportLines = @(
    "ainput live streaming acceptance",
    "exe=$ExePath",
    "runtime_root=$runtimeRoot",
    "gate=real-hotkey-live",
    "note=This report complements fixed-wav regression and must be read together with logs\\ainput.log.",
    ""
)

$baselineLogCount = (Get-Content $logPath -ErrorAction SilentlyContinue).Count
$baselineHistoryCount = (Get-Content $historyPath -ErrorAction SilentlyContinue).Count

for ($index = 0; $index -lt $sentences.Count; $index++) {
    $sentence = $sentences[$index]
    Write-Host ""
    Write-Host ("[{0}/{1}] {2}" -f ($index + 1), $sentences.Count, $sentence)
    [void](Read-Host "按回车后开始说")
    [void](Read-Host "说完并确认已上屏后，再按回车采集结果")

    $historyTail = Get-Content $historyPath -ErrorAction SilentlyContinue |
        Select-Object -Skip $baselineHistoryCount |
        Select-Object -Last 3
    $logTail = Select-String -Path $logPath -Pattern "streaming partial updated|streaming final transcription ready|streaming transcription delivered|selected_commit_source" -ErrorAction SilentlyContinue |
        Select-Object -Skip $baselineLogCount |
        Select-Object -Last 10 |
        ForEach-Object { $_.Line }

    $hudContinuous = Read-Host "按住期间 HUD 是否持续出字并出现最近一句修正？(y/n)"
    $missingWords = Read-Host "本轮是否出现只出两三个字或明显丢字？(y/n)"
    $notes = Read-Host "补充备注（可留空）"

    $reportLines += ("[sentence {0}] target={1}" -f ($index + 1), $sentence)
    $reportLines += ("hud_continuous={0}" -f $hudContinuous)
    $reportLines += ("short_or_missing={0}" -f $missingWords)
    if ($notes) {
        $reportLines += ("notes={0}" -f $notes)
    }
    $reportLines += "history_tail:"
    if ($historyTail) {
        $reportLines += $historyTail
    } else {
        $reportLines += "(no new history lines)"
    }
    $reportLines += "log_tail:"
    if ($logTail) {
        $reportLines += $logTail
    } else {
        $reportLines += "(no new streaming log lines)"
    }
    $reportLines += ""

    $baselineLogCount = (Get-Content $logPath -ErrorAction SilentlyContinue).Count
    $baselineHistoryCount = (Get-Content $historyPath -ErrorAction SilentlyContinue).Count
}

Set-Content -Path $reportPath -Value $reportLines -Encoding UTF8
Write-Host ""
Write-Host "报告已写入：$reportPath"
