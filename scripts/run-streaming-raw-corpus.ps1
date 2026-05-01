param(
    [string]$ExePath = "",
    [string]$RawDir = "",
    [string]$ReportDir = "",
    [int]$ShortCount = 2,
    [int]$LongCount = 2,
    [int]$ShortMinBytes = 200000,
    [int]$TailToleranceChars = 1,
    [int]$PunctuationMinDurationMs = 1200
)

$ErrorActionPreference = "Stop"

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

if ([string]::IsNullOrWhiteSpace($RawDir)) {
    $RawDir = Join-Path $repoRoot "logs\streaming-raw-captures"
}
if (!(Test-Path $RawDir)) {
    throw "missing raw capture dir: $RawDir"
}

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\streaming-raw-corpus\" + $stamp)
}
New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null

function Get-ContentCharCount([string]$Text) {
    if ([string]::IsNullOrWhiteSpace($Text)) {
        return 0
    }
    $punctuation = @(
        [char]0xFF0C, # fullwidth comma
        [char]0x3002, # ideographic full stop
        [char]0xFF01, # fullwidth exclamation
        [char]0xFF1F, # fullwidth question
        [char]0xFF1B, # fullwidth semicolon
        [char]0x002C,
        [char]0x002E,
        [char]0x0021,
        [char]0x003F,
        [char]0x003B
    )
    $count = 0
    $indexes = [System.Globalization.StringInfo]::ParseCombiningCharacters($Text)
    foreach ($index in $indexes) {
        $element = [System.Globalization.StringInfo]::GetNextTextElement($Text, $index)
        if (![string]::IsNullOrWhiteSpace($element) -and !($punctuation -contains [char]$element[0])) {
            $count += 1
        }
    }
    return $count
}

function Has-SentencePunctuation([string]$Text) {
    if ([string]::IsNullOrEmpty($Text)) {
        return $false
    }
    $punctuation = [char[]]@(
        [char]0xFF0C,
        [char]0x3002,
        [char]0xFF01,
        [char]0xFF1F,
        [char]0xFF1B,
        [char]0x002C,
        [char]0x002E,
        [char]0x0021,
        [char]0x003F,
        [char]0x003B
    )
    return $Text.IndexOfAny($punctuation) -ge 0
}

function Get-ContentText([string]$Text) {
    if ([string]::IsNullOrWhiteSpace($Text)) {
        return ""
    }
    $punctuation = @(
        [char]0xFF0C,
        [char]0x3002,
        [char]0xFF01,
        [char]0xFF1F,
        [char]0xFF1B,
        [char]0x3001,
        [char]0x002C,
        [char]0x002E,
        [char]0x0021,
        [char]0x003F,
        [char]0x003B,
        [char]0x003A
    )
    $builder = [System.Text.StringBuilder]::new()
    $indexes = [System.Globalization.StringInfo]::ParseCombiningCharacters($Text)
    foreach ($index in $indexes) {
        $element = [System.Globalization.StringInfo]::GetNextTextElement($Text, $index)
        if (![string]::IsNullOrWhiteSpace($element) -and !($punctuation -contains [char]$element[0])) {
            [void]$builder.Append($element)
        }
    }
    return $builder.ToString()
}

function Has-TerminalSentencePunctuation([string]$Text) {
    if ([string]::IsNullOrWhiteSpace($Text)) {
        return $false
    }
    $trimmed = $Text.Trim()
    $last = $trimmed[$trimmed.Length - 1]
    return @(
        [char]0x3002,
        [char]0xFF01,
        [char]0xFF1F,
        [char]0xFF1B,
        [char]0x002E,
        [char]0x0021,
        [char]0x003F,
        [char]0x003B
    ) -contains [char]$last
}

function Has-DuplicateOrConflictingPunctuation([string]$Text) {
    if ([string]::IsNullOrWhiteSpace($Text)) {
        return $false
    }
    return [regex]::IsMatch(
        $Text,
        '[，,]{2,}|[。\.]{2,}|[！!]{2,}|[？\?]{2,}|[；;]{2,}|[，,][。\.！？\?!；;]|[。\.！？\?!；;][，,]|[。\.！？\?!；;]{2,}'
    )
}

function Get-TrailingContentChar([string]$Text) {
    $content = Get-ContentText $Text
    if ([string]::IsNullOrEmpty($content)) {
        return ""
    }
    return [string]$content[$content.Length - 1]
}

function Is-TailParticle([string]$TextElement) {
    if ([string]::IsNullOrEmpty($TextElement)) {
        return $false
    }
    $tailParticles = [char[]]@(
        [char]0x4E86,
        [char]0x554A,
        [char]0x5462,
        [char]0x5427,
        [char]0x5417,
        [char]0x5440,
        [char]0x561B,
        [char]0x54E6,
        [char]0x5662,
        [char]0x8BF6
    )
    return $tailParticles -contains [char]$TextElement[0]
}

$allWavs = Get-ChildItem $RawDir -Filter "streaming-raw-*.wav" |
    Where-Object { $_.Length -ge $ShortMinBytes } |
    Sort-Object Length
if ($allWavs.Count -eq 0) {
    throw "no raw capture wav files large enough for replay found in $RawDir"
}

$shortCandidates = @($allWavs | Where-Object { $_.Length -ge $ShortMinBytes } | Select-Object -First $ShortCount)
if ($shortCandidates.Count -lt $ShortCount) {
    $shortCandidates = @($allWavs | Select-Object -First $ShortCount)
}
$shortPaths = @{}
foreach ($wav in $shortCandidates) {
    if ($null -ne $wav) {
        $shortPaths[$wav.FullName] = $true
    }
}
$longCandidates = @(
    $allWavs |
        Sort-Object Length -Descending |
        Where-Object { !$shortPaths.ContainsKey($_.FullName) } |
        Select-Object -First $LongCount
)
if ($longCandidates.Count -lt $LongCount) {
    $longCandidatePaths = @{}
    foreach ($wav in $longCandidates) {
        if ($null -ne $wav) {
            $longCandidatePaths[$wav.FullName] = $true
        }
    }
    $longCandidates = @(
        $longCandidates +
            ($allWavs |
                Sort-Object Length -Descending |
                Where-Object { !$longCandidatePaths.ContainsKey($_.FullName) } |
                Select-Object -First ($LongCount - $longCandidates.Count))
    )
}
$selected = @{}
$selectedRoles = @{}
foreach ($wav in $shortCandidates) {
    if ($null -ne $wav) {
        $selected[$wav.FullName] = $wav
        $selectedRoles[$wav.FullName] = "short"
    }
}
foreach ($wav in $longCandidates) {
    if ($null -ne $wav) {
        $selected[$wav.FullName] = $wav
        if ($selectedRoles.ContainsKey($wav.FullName)) {
            $selectedRoles[$wav.FullName] = $selectedRoles[$wav.FullName] + ",long"
        } else {
            $selectedRoles[$wav.FullName] = "long"
        }
    }
}
foreach ($wav in @($shortCandidates + $longCandidates)) {
    if ($null -ne $wav) {
        $selected[$wav.FullName] = $wav
    }
}
$cases = @($selected.Values | Sort-Object Length)

$rows = @()
$failures = @()
$selectedShortCount = @($selectedRoles.GetEnumerator() | Where-Object { $_.Value -like "*short*" }).Count
$selectedLongCount = @($selectedRoles.GetEnumerator() | Where-Object { $_.Value -like "*long*" }).Count
$minimumDistinctCases = [Math]::Min($allWavs.Count, [Math]::Max(1, $ShortCount) + [Math]::Max(1, $LongCount))
if ($selectedShortCount -lt 1) {
    $failures += [pscustomobject]@{
        case_id = "selection"
        category = "raw_short_sample_missing"
        message = "raw corpus selection did not include any short sample"
    }
}
if ($selectedLongCount -lt 1) {
    $failures += [pscustomobject]@{
        case_id = "selection"
        category = "raw_long_sample_missing"
        message = "raw corpus selection did not include any long sample"
    }
}
if ($cases.Count -lt $minimumDistinctCases) {
    $failures += [pscustomobject]@{
        case_id = "selection"
        category = "raw_distinct_sample_missing"
        message = "selected $($cases.Count) distinct raw samples, expected at least $minimumDistinctCases"
    }
}
$index = 0
foreach ($wav in $cases) {
    $index += 1
    $caseId = "{0:D2}-{1}" -f $index, [System.IO.Path]::GetFileNameWithoutExtension($wav.Name)
    $jsonPath = Join-Path $ReportDir ($caseId + ".json")
    $stdoutPath = Join-Path $ReportDir ($caseId + ".stdout.txt")
    $stderrPath = Join-Path $ReportDir ($caseId + ".stderr.txt")
    Remove-Item $jsonPath, $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue

    $previousJsonOutput = $env:AINPUT_JSON_OUTPUT_PATH
    $env:AINPUT_JSON_OUTPUT_PATH = $jsonPath
    try {
        $process = Start-Process -FilePath $ExePath `
            -ArgumentList @("replay-streaming-wav", $wav.FullName) `
            -WorkingDirectory $repoRoot `
            -Wait `
            -PassThru `
            -NoNewWindow `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath
        $exitCode = [int]$process.ExitCode
    } finally {
        $env:AINPUT_JSON_OUTPUT_PATH = $previousJsonOutput
    }

    if ($exitCode -ne 0) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_replay_failed"
            message = "replay-streaming-wav exited with $exitCode"
        }
        continue
    }
    if (!(Test-Path $jsonPath)) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_report_missing"
            message = "missing report $jsonPath"
        }
        continue
    }

    $reportText = [System.IO.File]::ReadAllText($jsonPath, [System.Text.Encoding]::UTF8)
    $report = $reportText | ConvertFrom-Json
    $partials = @($report.partial_timeline)
    $lastPartial = if ($partials.Count -gt 0) { $partials[-1].prepared_text } else { "" }
    $lastPartialChars = Get-ContentCharCount $lastPartial
    $lastPartialContent = Get-ContentText $lastPartial
    $finalChars = Get-ContentCharCount $report.final_text
    $finalContent = Get-ContentText $report.final_text
    $finalExtraChars = [Math]::Max(0, $finalChars - $lastPartialChars)
    $partialHasPunctuation = $false
    $previousPartialText = ""
    foreach ($partial in $partials) {
        if (Has-SentencePunctuation $partial.prepared_text) {
            $partialHasPunctuation = $true
        }
        if (Has-DuplicateOrConflictingPunctuation $partial.prepared_text) {
            $failures += [pscustomobject]@{
                case_id = $caseId
                category = "raw_duplicate_punctuation"
                message = "partial contains duplicate or conflicting punctuation: $($partial.prepared_text)"
            }
        }
        if ($partial.source -eq "endpoint_rollover" -and
            (Has-TerminalSentencePunctuation $partial.prepared_text) -and
            !(Has-TerminalSentencePunctuation $previousPartialText)) {
            $failures += [pscustomobject]@{
                case_id = $caseId
                category = "raw_punctuation_forced_by_pause"
                message = "endpoint rollover added terminal punctuation after an unterminated partial"
            }
        }
        $previousPartialText = $partial.prepared_text
    }
    $finalHasPunctuation = Has-SentencePunctuation $report.final_text
    if (Has-DuplicateOrConflictingPunctuation $report.final_text) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_duplicate_punctuation"
            message = "final contains duplicate or conflicting punctuation: $($report.final_text)"
        }
    }

    if ($report.partial_updates -lt 1) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_no_partial"
            message = "no HUD partial was produced"
        }
    }
    if ($finalExtraChars -gt $TailToleranceChars) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_tail_late"
            message = "final has $finalExtraChars more content chars than last HUD partial"
        }
    }
    if ($finalChars -lt $lastPartialChars) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_final_tail_dropped"
            message = "final has fewer content chars than last HUD partial: final=$finalChars last_partial=$lastPartialChars"
        }
    }
    $lastTail = Get-TrailingContentChar $lastPartial
    if ((Is-TailParticle $lastTail) -and
        ![string]::IsNullOrEmpty($lastPartialContent) -and
        !$finalContent.Contains($lastPartialContent)) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_final_tail_dropped"
            message = "final dropped tail particle '$lastTail' from last HUD partial"
        }
    }
    if ($report.input_duration_ms -ge $PunctuationMinDurationMs -and
        $finalHasPunctuation -and
        !$partialHasPunctuation -and
        $finalExtraChars -eq 0) {
        $failures += [pscustomobject]@{
            case_id = $caseId
            category = "raw_punctuation_late"
            message = "final has punctuation but no partial showed punctuation"
        }
    }

    $rows += [pscustomobject]@{
        case_id = $caseId
        bytes = $wav.Length
        duration_ms = $report.input_duration_ms
        partials = $report.partial_updates
        first_partial_ms = $report.first_partial_ms
        last_partial_gap_ms = $report.last_partial_to_final_gap_ms
        final_extra_chars = $finalExtraChars
        final_missing_chars = [Math]::Max(0, $lastPartialChars - $finalChars)
        report_final_extra_chars = $report.final_extra_content_chars
        report_final_missing_chars = $report.final_missing_content_chars
        partial_punct = $partialHasPunctuation
        final_punct = $finalHasPunctuation
        role = if ($selectedRoles.ContainsKey($wav.FullName)) { $selectedRoles[$wav.FullName] } else { "" }
        status = "checked"
        report = $jsonPath
    }
}

$summary = [pscustomobject]@{
    overall_status = if ($failures.Count -eq 0) { "pass" } else { "fail" }
    raw_dir = $RawDir
    report_dir = $ReportDir
    cases_total = $cases.Count
    failures = $failures
    cases = $rows
}
$summaryPath = Join-Path $ReportDir "summary.json"
$summary | ConvertTo-Json -Depth 8 | Set-Content -Path $summaryPath -Encoding UTF8

Write-Host ""
Write-Host "streaming raw corpus replay"
Write-Host ("overall_status={0}" -f $summary.overall_status)
Write-Host ("cases_total={0}" -f $summary.cases_total)
Write-Host ("report_dir={0}" -f $ReportDir)
Write-Host ""
$rows | Format-Table -AutoSize

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "failures"
    foreach ($failure in $failures) {
        Write-Host ("[{0}] {1}: {2}" -f $failure.case_id, $failure.category, $failure.message)
    }
    exit 1
}
