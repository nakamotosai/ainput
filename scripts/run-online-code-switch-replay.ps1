param(
    [string]$ExePath = "",
    [string]$RawDir = "",
    [string]$SidecarUrl = "http://vps-jp.tail4b5213.ts.net:18765",
    [string]$ReportDir = ""
)

$ErrorActionPreference = "Stop"
$utf8 = [System.Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = $utf8
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $packagedExe = Join-Path $repoRoot "ainput-desktop.exe"
    if (Test-Path $packagedExe) {
        $ExePath = $packagedExe
    } else {
        $ExePath = Join-Path $repoRoot "target\debug\ainput-desktop.exe"
    }
}
if (!(Test-Path $ExePath)) {
    throw "missing executable: $ExePath"
}
$ExePath = (Resolve-Path $ExePath).Path

if ([string]::IsNullOrWhiteSpace($RawDir)) {
    $RawDir = Join-Path $repoRoot "dist\ainput-1.0.0-preview.78\logs\streaming-raw-captures"
}
if (!(Test-Path $RawDir)) {
    throw "missing raw capture dir: $RawDir"
}

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\online-code-switch-replay\" + $stamp)
}
New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null

function Convert-ToF32File([string]$WavPath) {
    $tmp = Join-Path $env:TEMP ("ainput-code-switch-" + [Guid]::NewGuid().ToString("N") + ".f32")
    & ffmpeg -hide_banner -loglevel error -y -i $WavPath -ac 1 -ar 16000 -f f32le $tmp
    if ($LASTEXITCODE -ne 0) {
        throw "ffmpeg failed for $WavPath"
    }
    return $tmp
}

function Repair-Utf8Mojibake([string]$Text) {
    if ([string]::IsNullOrEmpty($Text)) {
        return $Text
    }
    $chars = $Text.ToCharArray()
    foreach ($ch in $chars) {
        if ([int][char]$ch -gt 255) {
            return $Text
        }
    }
    $bytes = New-Object byte[] $chars.Length
    for ($i = 0; $i -lt $chars.Length; $i++) {
        $bytes[$i] = [byte]([int][char]$chars[$i])
    }
    try {
        return [System.Text.Encoding]::UTF8.GetString($bytes)
    } catch {
        return $Text
    }
}

function Invoke-SidecarReplay([string]$WavPath) {
    $session = Invoke-RestMethod -Method Post -Uri "$SidecarUrl/v1/sessions" -ContentType "application/json" -Body "{}"
    $sessionId = $session.session_id
    $f32Path = Convert-ToF32File $WavPath
    try {
        $bytes = [System.IO.File]::ReadAllBytes($f32Path)
        $chunkBytes = 16000 * 4 / 10
        for ($offset = 0; $offset -lt $bytes.Length; $offset += $chunkBytes) {
            $length = [Math]::Min($chunkBytes, $bytes.Length - $offset)
            $chunk = New-Object byte[] $length
            [Array]::Copy($bytes, $offset, $chunk, 0, $length)
            [void](Invoke-RestMethod -Method Post -Uri "$SidecarUrl/v1/sessions/$sessionId/chunk" -ContentType "application/octet-stream" -Body $chunk)
        }
        $finish = Invoke-RestMethod -Method Post -Uri "$SidecarUrl/v1/sessions/$sessionId/finish"
        return [string]$finish.text
    } finally {
        Remove-Item -Force -ErrorAction SilentlyContinue $f32Path
    }
}

$cases = @(
    [pscustomobject]@{
        id = "dropped_multi_after_disabled"
        wav = "streaming-raw-1778475847123.wav"
        expected_contains = "multi"
        expected_repaired = "因为我之前禁用 multi。"
    },
    [pscustomobject]@{
        id = "dropped_multi_before_model"
        wav = "streaming-raw-1778475859896.wav"
        expected_contains = "multi 模型"
        expected_repaired = "multi 模型根本就不支持中文。"
    },
    [pscustomobject]@{
        id = "pure_chinese_unchanged"
        wav = "streaming-raw-1778475841299.wav"
        expected_contains = "你这套方案"
        expected_repaired = "你这套方案是有问题的。"
    }
)

function Invoke-RepairCase([string]$CaseId, [string]$RawText) {
    $repairInPath = Join-Path $ReportDir ("raw-" + $CaseId + ".txt")
    $repairOutPath = Join-Path $ReportDir ("repair-" + $CaseId + ".txt")
    [System.IO.File]::WriteAllText($repairInPath, $RawText, $utf8)
    $repairArgs = @("repair-online-transcript", "--in", $repairInPath, "--out", $repairOutPath)
    $repair = Start-Process -FilePath $ExePath -ArgumentList $repairArgs -WindowStyle Hidden -Wait -PassThru
    if ($repair.ExitCode -ne 0) {
        throw "repair-online-transcript failed for $CaseId with exit code $($repair.ExitCode)"
    }
    if (!(Test-Path $repairOutPath)) {
        throw "repair-online-transcript did not write $repairOutPath for $CaseId"
    }
    $repairOut = [System.IO.File]::ReadAllText($repairOutPath, $utf8)
    return Repair-Utf8Mojibake ($repairOut.Trim())
}

$textCases = @(
    [pscustomobject]@{
        id = "text_new_multi_catdi"
        raw_text = "因为我之前竟用猫底。"
        expected_contains = "multi"
        expected_repaired = "因为我之前禁用 multi。"
    },
    [pscustomobject]@{
        id = "text_new_multi_mouti_model"
        raw_text = "某体模型根本就不支持中文。"
        expected_contains = "multi 模型"
        expected_repaired = "multi 模型根本就不支持中文。"
    },
    [pscustomobject]@{
        id = "text_new_codex_koudaisi"
        raw_text = "我让扣代斯重新想方案"
        expected_contains = "Codex"
        expected_repaired = "我让Codex重新想方案"
    },
    [pscustomobject]@{
        id = "text_pure_chinese_unchanged"
        raw_text = "你这套方案是有问题的。"
        expected_contains = "你这套方案"
        expected_repaired = "你这套方案是有问题的。"
    },
    [pscustomobject]@{
        id = "text_chinese_negative_catdi"
        raw_text = "猫底下有一根线。"
        expected_contains = "猫底下"
        expected_repaired = "猫底下有一根线。"
    },
    [pscustomobject]@{
        id = "text_english_terms_casing"
        raw_text = "open ai api cli git hub"
        expected_contains = "OpenAI API CLI GitHub"
        expected_repaired = "OpenAI API CLI GitHub"
    }
)

$rows = @()
$failures = @()
foreach ($case in $cases) {
    $wavPath = Join-Path $RawDir $case.wav
    if (!(Test-Path $wavPath)) {
        $failures += [pscustomobject]@{
            case_id = $case.id
            category = "missing_raw"
            message = "missing $wavPath"
        }
        continue
    }
    $rawText = Repair-Utf8Mojibake (Invoke-SidecarReplay $wavPath)
    $repairedText = Invoke-RepairCase $case.id $rawText
    $pass = $repairedText.Contains($case.expected_contains) -and $repairedText -eq $case.expected_repaired
    $row = [pscustomobject]@{
        case_id = $case.id
        source = "wav"
        wav = $case.wav
        raw_text = $rawText
        repaired_text = $repairedText
        expected_repaired = $case.expected_repaired
        pass = $pass
    }
    $rows += $row
    if (!$pass) {
        $failures += [pscustomobject]@{
            case_id = $case.id
            category = "code_switch_repair_failed"
            message = "expected '$($case.expected_repaired)', got '$repairedText' from raw '$rawText'"
        }
    }
}

foreach ($case in $textCases) {
    $rawText = Repair-Utf8Mojibake $case.raw_text
    $repairedText = Invoke-RepairCase $case.id $rawText
    $pass = $repairedText.Contains($case.expected_contains) -and $repairedText -eq $case.expected_repaired
    $row = [pscustomobject]@{
        case_id = $case.id
        source = "text"
        wav = ""
        raw_text = $rawText
        repaired_text = $repairedText
        expected_repaired = $case.expected_repaired
        pass = $pass
    }
    $rows += $row
    if (!$pass) {
        $failures += [pscustomobject]@{
            case_id = $case.id
            category = "text_repair_failed"
            message = "expected '$($case.expected_repaired)', got '$repairedText' from raw '$rawText'"
        }
    }
}

$report = [pscustomobject]@{
    sidecar_url = $SidecarUrl
    raw_dir = $RawDir
    exe_path = $ExePath
    generated_at = (Get-Date).ToString("o")
    rows = $rows
    failures = $failures
    pass = ($failures.Count -eq 0)
}
$reportPath = Join-Path $ReportDir "online-code-switch-replay-report.json"
$report | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 $reportPath
$rows | Format-Table -AutoSize
Write-Host "report=$reportPath"
if ($failures.Count -gt 0) {
    $failures | Format-List
    throw "online code-switch replay failed: $($failures.Count) failure(s)"
}
