param(
    [string]$ExePath = "",
    [string]$ReportDir = "",
    [int]$Repeats = 2,
    [string[]]$CaseIds = @("sentence_01", "sentence_05", "sentence_combo_long"),
    [string[]]$ModelIds = @("paraformer_bilingual", "zipformer_small_bilingual"),
    [int[]]$AsrThreads = @(2, 4, 6, 8),
    [int[]]$ChunkMs = @(40, 60, 80),
    [int]$BaselineAsrThreads = 6,
    [int]$BaselineChunkMs = 60,
    [int]$FinalThreads = 8,
    [int]$PunctuationThreads = 1,
    [bool]$IncludeRaw = $true,
    [int]$RawCount = 2,
    [int]$RawMinBytes = 200000,
    [string]$RawDir = "",
    [switch]$IncludeZhReference,
    [switch]$BuildRelease,
    [switch]$FailOnContentRegression
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ($BuildRelease) {
    cargo build -p ainput-desktop --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed"
    }
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $releaseExe = Join-Path $repoRoot "target\release\ainput-desktop.exe"
    $debugExe = Join-Path $repoRoot "target\debug\ainput-desktop.exe"
    if (Test-Path $releaseExe) {
        $ExePath = $releaseExe
    } elseif (Test-Path $debugExe) {
        $ExePath = $debugExe
        Write-Warning "target\release\ainput-desktop.exe missing; using debug exe makes wall timing less reliable"
    } else {
        throw "missing executable; run with -BuildRelease or pass -ExePath"
    }
}
if (!(Test-Path $ExePath)) {
    throw "missing executable: $ExePath"
}
$exeInfo = Get-Item $ExePath
$gitHead = ""
$gitDiffShortstat = ""
try {
    $gitHead = (git rev-parse --short HEAD 2>$null)
    $gitDiffShortstat = (git diff --shortstat 2>$null)
} catch {
    $gitHead = ""
    $gitDiffShortstat = ""
}

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\streaming-latency-benchmark\" + $stamp)
}
New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null

if ([string]::IsNullOrWhiteSpace($RawDir)) {
    $preview35RawDir = Join-Path $repoRoot "dist\ainput-1.0.0-preview.35\logs\streaming-raw-captures"
    $sourceRawDir = Join-Path $repoRoot "logs\streaming-raw-captures"
    if (Test-Path $preview35RawDir) {
        $RawDir = $preview35RawDir
    } else {
        $RawDir = $sourceRawDir
    }
}

$modelCatalog = @{
    paraformer_bilingual = @{
        model_dir = "models/sherpa-onnx-streaming-paraformer-bilingual-zh-en"
        role = "default_bilingual"
    }
    zipformer_small_bilingual = @{
        model_dir = "models/streaming-zipformer-small-bilingual-zh-en"
        role = "small_bilingual"
    }
    zh_zipformer_reference = @{
        model_dir = "models/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30"
        role = "zh_only_reference"
    }
}

if ($IncludeZhReference -and !($ModelIds -contains "zh_zipformer_reference")) {
    $ModelIds += "zh_zipformer_reference"
}

function Test-HasNumericValue([object]$Value) {
    if ($null -eq $Value) {
        return $false
    }
    if ($Value -is [string] -and [string]::IsNullOrWhiteSpace($Value)) {
        return $false
    }
    return $true
}

function Get-MedianNumber([object[]]$Values) {
    $numbers = @($Values | Where-Object { Test-HasNumericValue $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if ($numbers.Count -eq 0) {
        return $null
    }
    $middle = [int][Math]::Floor($numbers.Count / 2)
    if (($numbers.Count % 2) -eq 1) {
        return $numbers[$middle]
    }
    return ($numbers[$middle - 1] + $numbers[$middle]) / 2.0
}

function Get-PercentileNumber([object[]]$Values, [double]$Percentile) {
    $numbers = @($Values | Where-Object { Test-HasNumericValue $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if ($numbers.Count -eq 0) {
        return $null
    }
    $rank = [int][Math]::Ceiling(($Percentile / 100.0) * $numbers.Count) - 1
    $rank = [Math]::Max(0, [Math]::Min($numbers.Count - 1, $rank))
    return $numbers[$rank]
}

function Get-AverageNumber([object[]]$Values) {
    $numbers = @($Values | Where-Object { Test-HasNumericValue $_ } | ForEach-Object { [double]$_ })
    if ($numbers.Count -eq 0) {
        return $null
    }
    return ($numbers | Measure-Object -Average).Average
}

function Round-Nullable([object]$Value, [int]$Digits) {
    if (!(Test-HasNumericValue $Value)) {
        return $null
    }
    return [Math]::Round([double]$Value, $Digits)
}

function Get-OptionalProperty([object]$Object, [string]$Name) {
    if ($null -eq $Object) {
        return $null
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Set-TomlSectionValue([string]$Text, [string]$Section, [string]$Key, [string]$ValueLiteral) {
    $sectionPattern = "(?ms)(^\[$([regex]::Escape($Section))\]\r?\n)(.*?)(?=^\[|\z)"
    $match = [regex]::Match($Text, $sectionPattern)
    if (!$match.Success) {
        throw "missing TOML section [$Section]"
    }

    $body = $match.Groups[2].Value
    $keyPattern = "(?m)^(\s*$([regex]::Escape($Key))\s*=\s*).*$"
    $keyMatch = [regex]::Match($body, $keyPattern)
    if ($keyMatch.Success) {
        $newBody = $body.Substring(0, $keyMatch.Groups[1].Index) +
            $keyMatch.Groups[1].Value +
            $ValueLiteral +
            $body.Substring($keyMatch.Index + $keyMatch.Length)
    } else {
        $newBody = $body.TrimEnd() + "`r`n$Key = $ValueLiteral`r`n"
    }

    return $Text.Substring(0, $match.Groups[2].Index) +
        $newBody +
        $Text.Substring($match.Groups[2].Index + $match.Groups[2].Length)
}

function New-BenchmarkRoot([string]$VariantId, [hashtable]$Variant) {
    $root = Join-Path $ReportDir ("roots\" + $VariantId)
    if (Test-Path $root) {
        Remove-Item $root -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    Copy-Item (Join-Path $repoRoot "config") (Join-Path $root "config") -Recurse -Force
    New-Item -ItemType Directory -Force -Path (Join-Path $root "logs") | Out-Null
    New-Item -ItemType Junction -Path (Join-Path $root "models") -Target (Join-Path $repoRoot "models") | Out-Null

    $configPath = Join-Path $root "config\ainput.toml"
    $text = [System.IO.File]::ReadAllText($configPath, [System.Text.Encoding]::UTF8)
    $text = Set-TomlSectionValue $text "voice.streaming" "model_dir" ('"' + $Variant.model_dir + '"')
    $text = Set-TomlSectionValue $text "voice.streaming" "chunk_ms" ([string]$Variant.chunk_ms)
    $text = Set-TomlSectionValue $text "voice.streaming.performance" "asr_num_threads" ([string]$Variant.asr_threads)
    $text = Set-TomlSectionValue $text "voice.streaming.performance" "final_num_threads" ([string]$Variant.final_threads)
    $text = Set-TomlSectionValue $text "voice.streaming.performance" "punctuation_num_threads" ([string]$Variant.punctuation_threads)
    $text = Set-TomlSectionValue $text "voice.streaming.performance" "gpu_enabled" "false"
    $text = Set-TomlSectionValue $text "voice.streaming.ai_rewrite" "enabled" "false"
    [System.IO.File]::WriteAllText($configPath, $text, [System.Text.UTF8Encoding]::new($false))
    return $root
}

function New-ManifestFile([string]$Path, [object[]]$Cases) {
    $manifest = [ordered]@{
        version = 1
        fixture_root = ""
        cases = @(
            foreach ($case in $Cases) {
                $entry = [ordered]@{
                    id = $case.id
                    wav_path = $case.wav_path
                    min_partial_updates = $case.min_partial_updates
                    shortfall_tolerance_chars = $case.shortfall_tolerance_chars
                }
                if (![string]::IsNullOrWhiteSpace($case.expected_text)) {
                    $entry.expected_text = $case.expected_text
                }
                if ($case.keywords.Count -gt 0) {
                    $entry.keywords = @($case.keywords)
                }
                $entry
            }
        )
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -Path $Path -Encoding UTF8
}

$fixtureManifestPath = Join-Path $repoRoot "fixtures\streaming-selftest\manifest.json"
if (!(Test-Path $fixtureManifestPath)) {
    throw "missing fixture manifest: $fixtureManifestPath"
}
$fixtureManifest = [System.IO.File]::ReadAllText($fixtureManifestPath, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$fixtureRoot = Join-Path (Split-Path -Parent $fixtureManifestPath) $fixtureManifest.fixture_root
$cases = @()
foreach ($caseId in $CaseIds) {
    $fixture = @($fixtureManifest.cases | Where-Object { $_.id -eq $caseId } | Select-Object -First 1)
    if ($fixture.Count -eq 0) {
        throw "unknown fixture case id: $caseId"
    }
    $wavPath = Join-Path $fixtureRoot $fixture[0].wav_path
    if (!(Test-Path $wavPath)) {
        throw "missing fixture wav: $wavPath"
    }
    $cases += [pscustomobject]@{
        id = $fixture[0].id
        source = "fixture"
        wav_path = (Resolve-Path $wavPath).Path
        expected_text = $fixture[0].expected_text
        keywords = @($fixture[0].keywords)
        min_partial_updates = if ($null -ne $fixture[0].min_partial_updates) { [int]$fixture[0].min_partial_updates } else { 1 }
        shortfall_tolerance_chars = if ($null -ne $fixture[0].shortfall_tolerance_chars) { [int]$fixture[0].shortfall_tolerance_chars } else { 3 }
    }
}

if ($IncludeRaw -and (Test-Path $RawDir)) {
    $rawWavs = @(Get-ChildItem $RawDir -Filter "streaming-raw-*.wav" | Where-Object { $_.Length -ge $RawMinBytes } | Sort-Object Length)
    $selectedRaw = @()
    if ($rawWavs.Count -gt 0) {
        $selectedRaw += $rawWavs[0]
    }
    if ($rawWavs.Count -gt 1 -and $RawCount -gt 1) {
        $selectedRaw += $rawWavs[-1]
    }
    if ($RawCount -gt 2) {
        $selectedRaw += @($rawWavs | Sort-Object LastWriteTime -Descending | Select-Object -First ($RawCount - $selectedRaw.Count))
    }
    $selectedRaw = @($selectedRaw | Where-Object { $null -ne $_ } | Sort-Object FullName -Unique | Select-Object -First $RawCount)
    $rawIndex = 0
    foreach ($raw in $selectedRaw) {
        $rawIndex += 1
        $cases += [pscustomobject]@{
            id = ("raw_{0:D2}_{1}" -f $rawIndex, [System.IO.Path]::GetFileNameWithoutExtension($raw.Name))
            source = "raw"
            wav_path = $raw.FullName
            expected_text = ""
            keywords = @()
            min_partial_updates = 1
            shortfall_tolerance_chars = 3
        }
    }
}

if ($cases.Count -eq 0) {
    throw "no benchmark cases selected"
}

$variantsByKey = [ordered]@{}
function Add-Variant([string]$ModelId, [int]$Asr, [int]$Chunk) {
    if (!$modelCatalog.ContainsKey($ModelId)) {
        throw "unknown model id: $ModelId"
    }
    $model = $modelCatalog[$ModelId]
    $modelDirPath = Join-Path $repoRoot $model.model_dir
    if (!(Test-Path $modelDirPath)) {
        Write-Warning "skip model $ModelId because model dir is missing: $modelDirPath"
        return
    }
    $variantId = "{0}_asr{1}_chunk{2}" -f $ModelId, $Asr, $Chunk
    if ($variantsByKey.Contains($variantId)) {
        return
    }
    $variantsByKey[$variantId] = @{
        variant_id = $variantId
        model_id = $ModelId
        model_role = $model.role
        model_dir = $model.model_dir
        asr_threads = $Asr
        chunk_ms = $Chunk
        final_threads = $FinalThreads
        punctuation_threads = $PunctuationThreads
    }
}

foreach ($asr in $AsrThreads) {
    Add-Variant "paraformer_bilingual" $asr $BaselineChunkMs
}
foreach ($chunk in $ChunkMs) {
    Add-Variant "paraformer_bilingual" $BaselineAsrThreads $chunk
}
foreach ($modelId in $ModelIds) {
    Add-Variant $modelId $BaselineAsrThreads $BaselineChunkMs
}

$allRows = @()
$runFailures = @()
$variantSummaries = @()
$manifestDir = Join-Path $ReportDir "manifests"
New-Item -ItemType Directory -Force -Path $manifestDir | Out-Null

Write-Host ""
Write-Host "streaming latency benchmark"
Write-Host ("exe={0}" -f $ExePath)
Write-Host ("report_dir={0}" -f $ReportDir)
Write-Host ("cases={0} variants={1} repeats={2}" -f $cases.Count, $variantsByKey.Count, $Repeats)
Write-Host ""

foreach ($variantId in $variantsByKey.Keys) {
    $variant = $variantsByKey[$variantId]
    $root = New-BenchmarkRoot $variantId $variant
    $manifestPath = Join-Path $manifestDir ($variantId + ".json")
    New-ManifestFile $manifestPath $cases

    for ($repeat = 1; $repeat -le $Repeats; $repeat += 1) {
        $jsonPath = Join-Path $ReportDir ("{0}_repeat{1}.json" -f $variantId, $repeat)
        $stdoutPath = Join-Path $ReportDir ("{0}_repeat{1}.stdout.txt" -f $variantId, $repeat)
        $stderrPath = Join-Path $ReportDir ("{0}_repeat{1}.stderr.txt" -f $variantId, $repeat)
        Remove-Item $jsonPath, $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue

        $previousRoot = $env:AINPUT_ROOT
        $previousJson = $env:AINPUT_JSON_OUTPUT_PATH
        $env:AINPUT_ROOT = $root
        $env:AINPUT_JSON_OUTPUT_PATH = $jsonPath
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $process = Start-Process -FilePath $ExePath `
                -ArgumentList @("replay-streaming-manifest", $manifestPath) `
                -WorkingDirectory $repoRoot `
                -Wait `
                -PassThru `
                -NoNewWindow `
                -RedirectStandardOutput $stdoutPath `
                -RedirectStandardError $stderrPath
            $exitCode = [int]$process.ExitCode
        } finally {
            $watch.Stop()
            $env:AINPUT_ROOT = $previousRoot
            $env:AINPUT_JSON_OUTPUT_PATH = $previousJson
        }

        if ($exitCode -ne 0 -or !(Test-Path $jsonPath)) {
            $runFailures += [pscustomobject]@{
                variant_id = $variantId
                repeat = $repeat
                category = "replay_failed"
                message = "exit=$exitCode json_exists=$(Test-Path $jsonPath)"
                stderr = $stderrPath
            }
            continue
        }

        $report = [System.IO.File]::ReadAllText($jsonPath, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
        foreach ($caseReport in @($report.cases)) {
            $caseMeta = @($cases | Where-Object { $_.id -eq $caseReport.case_id } | Select-Object -First 1)
            $status = if ($caseReport.behavior_status -eq "pass" -and $caseReport.content_status -eq "pass") { "pass" } else { "fail" }
            $allRows += [pscustomobject]@{
                variant_id = $variantId
                repeat = $repeat
                model_id = $variant.model_id
                model_role = $variant.model_role
                model_dir = $variant.model_dir
                model_path = (Join-Path $repoRoot $variant.model_dir)
                ainput_root = $root
                ai_rewrite_enabled = $false
                asr_threads = $variant.asr_threads
                final_threads = $variant.final_threads
                punctuation_threads = $variant.punctuation_threads
                chunk_ms = $variant.chunk_ms
                case_id = $caseReport.case_id
                source = if ($caseMeta.Count -gt 0) { $caseMeta[0].source } else { "" }
                status = $status
                behavior_status = $caseReport.behavior_status
                content_status = $caseReport.content_status
                input_duration_ms = $caseReport.input_duration_ms
                speech_start_ms = Get-OptionalProperty $caseReport "speech_start_ms"
                first_partial_ms = $caseReport.first_partial_ms
                first_partial_after_speech_ms = Get-OptionalProperty $caseReport "first_partial_after_speech_ms"
                first_partial_processing_elapsed_ms = Get-OptionalProperty $caseReport "first_partial_processing_elapsed_ms"
                first_partial_processing_lag_ms = Get-OptionalProperty $caseReport "first_partial_processing_lag_ms"
                partial_updates = $caseReport.partial_updates
                last_partial_to_final_gap_ms = $caseReport.last_partial_to_final_gap_ms
                final_extra_content_chars = $caseReport.final_extra_content_chars
                final_missing_content_chars = $caseReport.final_missing_content_chars
                total_decode_steps = $caseReport.total_decode_steps
                processing_wall_elapsed_ms = $caseReport.processing_wall_elapsed_ms
                processing_realtime_factor = $caseReport.processing_realtime_factor
                final_decode_elapsed_ms = $caseReport.final_decode_elapsed_ms
                online_final_elapsed_ms = $caseReport.online_final_elapsed_ms
                offline_final_elapsed_ms = $caseReport.offline_final_elapsed_ms
                offline_final_timed_out = $caseReport.offline_final_timed_out
                punctuation_elapsed_ms = $caseReport.punctuation_elapsed_ms
                process_manifest_wall_elapsed_ms = $watch.ElapsedMilliseconds
                commit_source = $caseReport.commit_source
                final_visible_chars = $caseReport.final_visible_chars
                failures = ($caseReport.failures -join " | ")
                final_text = $caseReport.final_text
                report = $jsonPath
            }
        }
    }
}

foreach ($group in ($allRows | Group-Object variant_id)) {
    $rows = @($group.Group)
    $failed = @($rows | Where-Object { $_.status -ne "pass" })
    $variant = $variantsByKey[$group.Name]
    $variantSummaries += [pscustomobject]@{
        variant_id = $group.Name
        model_id = $variant.model_id
        model_role = $variant.model_role
        asr_threads = $variant.asr_threads
        final_threads = $variant.final_threads
        punctuation_threads = $variant.punctuation_threads
        chunk_ms = $variant.chunk_ms
        cases = $rows.Count
        failed_cases = $failed.Count
        first_partial_after_speech_avg_ms = Round-Nullable (Get-AverageNumber $rows.first_partial_after_speech_ms) 1
        first_partial_after_speech_p50_ms = Round-Nullable (Get-MedianNumber $rows.first_partial_after_speech_ms) 1
        first_partial_after_speech_p95_ms = Round-Nullable (Get-PercentileNumber $rows.first_partial_after_speech_ms 95) 1
        first_partial_avg_ms = Round-Nullable (Get-AverageNumber $rows.first_partial_ms) 1
        first_partial_p50_ms = Round-Nullable (Get-MedianNumber $rows.first_partial_ms) 1
        first_partial_p95_ms = Round-Nullable (Get-PercentileNumber $rows.first_partial_ms 95) 1
        first_partial_processing_lag_avg_ms = Round-Nullable (Get-AverageNumber $rows.first_partial_processing_lag_ms) 1
        first_partial_processing_lag_p95_ms = Round-Nullable (Get-PercentileNumber $rows.first_partial_processing_lag_ms 95) 1
        processing_wall_avg_ms = Round-Nullable (Get-AverageNumber $rows.processing_wall_elapsed_ms) 1
        processing_wall_p95_ms = Round-Nullable (Get-PercentileNumber $rows.processing_wall_elapsed_ms 95) 1
        processing_wall_max_ms = Round-Nullable (($rows.processing_wall_elapsed_ms | ForEach-Object { [double]$_ } | Measure-Object -Maximum).Maximum) 1
        processing_rtf_avg = Round-Nullable (Get-AverageNumber $rows.processing_realtime_factor) 3
        offline_final_avg_ms = Round-Nullable (Get-AverageNumber $rows.offline_final_elapsed_ms) 1
        punctuation_avg_ms = Round-Nullable (Get-AverageNumber $rows.punctuation_elapsed_ms) 1
        partial_updates_avg = Round-Nullable (Get-AverageNumber $rows.partial_updates) 1
        final_extra_chars_max = (($rows.final_extra_content_chars | ForEach-Object { [double]$_ } | Measure-Object -Maximum).Maximum)
    }
}

$casesCsv = Join-Path $ReportDir "cases.csv"
$summaryCsv = Join-Path $ReportDir "summary_by_variant.csv"
$summaryJson = Join-Path $ReportDir "summary.json"
$markdownPath = Join-Path $ReportDir "SUMMARY.md"
$allRows | Export-Csv -Path $casesCsv -NoTypeInformation -Encoding UTF8
$variantSummaries | Sort-Object failed_cases, first_partial_after_speech_avg_ms, processing_rtf_avg | Export-Csv -Path $summaryCsv -NoTypeInformation -Encoding UTF8

$summary = [ordered]@{
    overall_status = if ($runFailures.Count -eq 0 -and (!$FailOnContentRegression -or (@($allRows | Where-Object { $_.status -ne "pass" }).Count -eq 0))) { "pass" } else { "fail" }
    exe = $ExePath
    exe_last_write_time = $exeInfo.LastWriteTime.ToString("o")
    exe_length = $exeInfo.Length
    git_head = $gitHead
    git_diff_shortstat = $gitDiffShortstat
    report_dir = $ReportDir
    repeats = $Repeats
    cases = $cases
    variants = @($variantsByKey.Values)
    failures = $runFailures
    summary_by_variant = @($variantSummaries | Sort-Object failed_cases, first_partial_after_speech_avg_ms, processing_rtf_avg)
}
$summary | ConvertTo-Json -Depth 10 | Set-Content -Path $summaryJson -Encoding UTF8

$md = @()
$md += "# Streaming Latency Benchmark"
$md += ""
$md += "- exe: ``$ExePath``"
$md += "- exe_last_write_time: ``$($exeInfo.LastWriteTime.ToString("o"))``"
$md += "- git_head: ``$gitHead``"
$md += "- git_diff_shortstat: ``$gitDiffShortstat``"
$md += "- report_dir: ``$ReportDir``"
$md += "- cases: $($cases.Count)"
$md += "- variants: $($variantsByKey.Count)"
$md += "- repeats: $Repeats"
$md += ""
$md += "| variant | failed | speech->first avg | speech->first p50 | speech->first p95 | audio->first avg | audio->first p95 | proc lag avg | proc lag p95 | proc rtf | proc avg | proc p95 | offline avg | punct avg | partial avg | final extra max |"
$md += "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
foreach ($row in ($variantSummaries | Sort-Object failed_cases, first_partial_after_speech_avg_ms, processing_rtf_avg)) {
    $md += "| $($row.variant_id) | $($row.failed_cases)/$($row.cases) | $($row.first_partial_after_speech_avg_ms) | $($row.first_partial_after_speech_p50_ms) | $($row.first_partial_after_speech_p95_ms) | $($row.first_partial_avg_ms) | $($row.first_partial_p95_ms) | $($row.first_partial_processing_lag_avg_ms) | $($row.first_partial_processing_lag_p95_ms) | $($row.processing_rtf_avg) | $($row.processing_wall_avg_ms) | $($row.processing_wall_p95_ms) | $($row.offline_final_avg_ms) | $($row.punctuation_avg_ms) | $($row.partial_updates_avg) | $($row.final_extra_chars_max) |"
}
$md += ""
$md += "Notes:"
$md += "- first partial is audio-offset latency, not a UI screenshot measurement."
$md += "- processing realtime factor excludes app startup and most process/model-load cost."
$md += "- zh_zipformer_reference is a reference-only candidate and must not become default without bilingual coverage."
$md | Set-Content -Path $markdownPath -Encoding UTF8

Write-Host ""
Write-Host "summary by variant"
$variantSummaries | Sort-Object failed_cases, first_partial_avg_ms, processing_rtf_avg | Format-Table -AutoSize
Write-Host ""
Write-Host ("cases_csv={0}" -f $casesCsv)
Write-Host ("summary_csv={0}" -f $summaryCsv)
Write-Host ("summary_json={0}" -f $summaryJson)
Write-Host ("summary_md={0}" -f $markdownPath)

if ($runFailures.Count -gt 0) {
    Write-Host ""
    Write-Host "run failures"
    $runFailures | Format-Table -AutoSize
    exit 1
}

if ($FailOnContentRegression) {
    $contentFailures = @($allRows | Where-Object { $_.status -ne "pass" })
    if ($contentFailures.Count -gt 0) {
        Write-Host ""
        Write-Host "content or behavior failures"
        $contentFailures | Select-Object variant_id, repeat, case_id, behavior_status, content_status, failures | Format-Table -AutoSize
        exit 1
    }
}
