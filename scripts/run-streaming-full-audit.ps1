param(
    [string]$Version = "1.0.0-preview.45",
    [string]$ExePath = "",
    [string]$RawDir = "",
    [string]$ReportDir = "",
    [int]$LatencyRepeats = 1,
    [int]$LiveCaseLimit = 3,
    [switch]$SkipLatency
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
}
$packageDir = Split-Path -Parent $ExePath
$zipPath = Join-Path $repoRoot ("dist\ainput-" + $Version + ".zip")

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\streaming-full-audit\" + $stamp)
}
$stepsDir = Join-Path $ReportDir "steps"
New-Item -ItemType Directory -Force -Path $stepsDir | Out-Null

if ([string]::IsNullOrWhiteSpace($RawDir)) {
    $candidates = @(
        (Join-Path $repoRoot ("dist\ainput-" + $Version + "\logs\streaming-raw-captures")),
        (Join-Path $repoRoot "dist\ainput-1.0.0-preview.43\logs\streaming-raw-captures"),
        (Join-Path $repoRoot "logs\streaming-raw-captures"),
        (Join-Path $repoRoot "dist\ainput-1.0.0-preview.37\logs\streaming-raw-captures")
    )
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $usableWavCount = @(Get-ChildItem $candidate -Filter "streaming-raw-*.wav" -ErrorAction SilentlyContinue | Where-Object { $_.Length -ge 200000 }).Count
            if ($usableWavCount -gt 0) {
                $RawDir = $candidate
                break
            }
        }
    }
}

$script:steps = New-Object System.Collections.Generic.List[object]
$script:findings = New-Object System.Collections.Generic.List[object]

function Add-Finding {
    param(
        [Parameter(Mandatory = $true)][string]$Severity,
        [Parameter(Mandatory = $true)][string]$Area,
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Evidence,
        [string]$Recommendation = ""
    )
    $script:findings.Add([pscustomobject]@{
        severity = $Severity
        area = $Area
        title = $Title
        evidence = $Evidence
        recommendation = $Recommendation
    }) | Out-Null
}

function Convert-StepId {
    param([string]$Id)
    return ($Id -replace '[^A-Za-z0-9_.-]', '_')
}

function Invoke-AuditProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDir = $repoRoot,
        [string]$FailSeverity = "P1",
        [string]$FailArea = "command"
    )

    $safeId = Convert-StepId $Id
    $stepDir = Join-Path $stepsDir $safeId
    New-Item -ItemType Directory -Force -Path $stepDir | Out-Null
    $stdoutPath = Join-Path $stepDir "stdout.txt"
    $stderrPath = Join-Path $stepDir "stderr.txt"
    $metaPath = Join-Path $stepDir "step.json"
    Remove-Item $stdoutPath, $stderrPath, $metaPath -Force -ErrorAction SilentlyContinue

    $started = Get-Date
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $exitCode = $null
    $exceptionText = ""
    $oldErrorActionPreference = $ErrorActionPreference
    try {
        Push-Location $WorkingDir
        $ErrorActionPreference = "Continue"
        $global:LASTEXITCODE = 0
        & $FilePath @ArgumentList 1> $stdoutPath 2> $stderrPath
        if ($null -ne $LASTEXITCODE) {
            $exitCode = [int]$LASTEXITCODE
        } else {
            $exitCode = 0
        }
    } catch {
        $exitCode = -999
        $exceptionText = $_.Exception.Message
        Set-Content -Path $stderrPath -Encoding UTF8 -Value $exceptionText
    } finally {
        $ErrorActionPreference = $oldErrorActionPreference
        Pop-Location -ErrorAction SilentlyContinue
        $watch.Stop()
    }

    $status = if ($exitCode -eq 0) { "pass" } else { "fail" }
    $step = [pscustomobject]@{
        id = $Id
        label = $Label
        status = $status
        exit_code = $exitCode
        started_at = $started.ToString("o")
        elapsed_ms = $watch.ElapsedMilliseconds
        file = $FilePath
        args = $ArgumentList
        working_dir = $WorkingDir
        stdout = $stdoutPath
        stderr = $stderrPath
        exception = $exceptionText
    }
    $step | ConvertTo-Json -Depth 8 | Set-Content -Path $metaPath -Encoding UTF8
    $script:steps.Add($step) | Out-Null

    if ($status -ne "pass") {
        Add-Finding `
            -Severity $FailSeverity `
            -Area $FailArea `
            -Title "$Label failed" `
            -Evidence "exit=$exitCode stdout=$stdoutPath stderr=$stderrPath" `
            -Recommendation "Open the step stdout/stderr and fix before release."
    }

    return $step
}

function Read-JsonFile {
    param([string]$Path)
    if (!(Test-Path $Path)) {
        return $null
    }
    $text = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8).Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        return $null
    }
    return $text | ConvertFrom-Json
}

function ConvertTo-PowerShellLiteral {
    param([string]$Value)
    return "'" + $Value.Replace("'", "''") + "'"
}

function Get-ProcessSnapshot {
    $rows = @()
    foreach ($process in @(Get-Process ainput-desktop -ErrorAction SilentlyContinue)) {
        $path = ""
        try { $path = $process.Path } catch { $path = "" }
        $rows += [pscustomobject]@{
            id = $process.Id
            path = $path
        }
    }
    return $rows
}

function Test-PackageIntegrity {
    $required = @(
        $ExePath,
        $zipPath,
        (Join-Path $packageDir "config\ainput.toml"),
        (Join-Path $packageDir "config\hud-overlay.toml"),
        (Join-Path $packageDir "scripts\run-streaming-live-e2e.ps1"),
        (Join-Path $packageDir "scripts\run-streaming-raw-corpus.ps1"),
        (Join-Path $packageDir "scripts\run-startup-idle-acceptance.ps1"),
        (Join-Path $packageDir "fixtures\streaming-user-regression-v12\manifest.json"),
        (Join-Path $packageDir "fixtures\streaming-user-regression-v12\short-tail-da.wav"),
        (Join-Path $packageDir "fixtures\streaming-user-regression-v12\trailing-i-repeat.wav"),
        (Join-Path $packageDir "fixtures\streaming-user-regression-v12\short-tail-full.wav"),
        (Join-Path $packageDir "fixtures\streaming-user-regression-v12\punctuation-budui-i.wav"),
        (Join-Path $packageDir "models\sherpa-onnx-streaming-paraformer-bilingual-zh-en\encoder.int8.onnx"),
        (Join-Path $packageDir "models\sherpa-onnx-streaming-paraformer-bilingual-zh-en\decoder.int8.onnx"),
        (Join-Path $packageDir "models\sherpa-onnx-streaming-paraformer-bilingual-zh-en\tokens.txt")
    )
    $missing = @()
    foreach ($path in $required) {
        if (!(Test-Path $path)) {
            $missing += $path
        }
    }
    if ($missing.Count -gt 0) {
        Add-Finding -Severity "P0" -Area "package" -Title "Package is incomplete" -Evidence ($missing -join "`n") -Recommendation "Fix package-release.ps1 and rebuild a new preview."
        return "fail"
    }
    return "pass"
}

function Add-LiveReportFindings {
    param([object]$Report, [string]$Area)
    if ($null -eq $Report) {
        Add-Finding -Severity "P1" -Area $Area -Title "Missing live report" -Evidence "Report JSON was not readable." -Recommendation "Rerun the live E2E step."
        return
    }
    if ($Report.overall_status -ne "pass") {
        Add-Finding -Severity "P1" -Area $Area -Title "Live E2E failed" -Evidence ($Report.failures | ConvertTo-Json -Depth 6) -Recommendation "Fix HUD/readback/commit failures before release."
    }
    foreach ($case in @($Report.cases)) {
        if ($case.commit_request_count -ne 1) {
            Add-Finding -Severity "P0" -Area $Area -Title "Commit request count is not exactly once" -Evidence "$($case.case_id): commit_request_count=$($case.commit_request_count)" -Recommendation "Fix duplicate output commit path."
        }
        if ($case.hud_stability.alpha_drop_count -gt 0 -or $case.hud_stability.invisible_sample_count -gt 0) {
            Add-Finding -Severity "P2" -Area "hud" -Title "HUD flicker detected" -Evidence "$($case.case_id): alpha_drop=$($case.hud_stability.alpha_drop_count) invisible=$($case.hud_stability.invisible_sample_count)" -Recommendation "Inspect overlay animation and redraw path."
        }
        if ($case.hud_stability.max_center_x_delta_px -gt 3 -or $case.hud_stability.max_top_delta_px -gt 3 -or $case.hud_stability.max_height_delta_px -gt 3) {
            Add-Finding -Severity "P2" -Area "hud" -Title "HUD jitter detected" -Evidence "$($case.case_id): center/top/height=$($case.hud_stability.max_center_x_delta_px)/$($case.hud_stability.max_top_delta_px)/$($case.hud_stability.max_height_delta_px)" -Recommendation "Stabilize HUD layout metrics."
        }
    }
}

function Add-LatencyFindings {
    param([object]$Summary)
    if ($null -eq $Summary) {
        Add-Finding -Severity "P2" -Area "latency" -Title "Latency summary missing" -Evidence "No latency summary was parsed." -Recommendation "Rerun latency benchmark."
        return
    }
    $baseline = @($Summary.summary_by_variant | Where-Object { $_.variant_id -eq "paraformer_bilingual_asr6_chunk60" } | Select-Object -First 1)
    if ($baseline.Count -eq 0) {
        $baseline = @($Summary.summary_by_variant | Select-Object -First 1)
    }
    if ($baseline.Count -gt 0) {
        $b = $baseline[0]
        $firstPartialP95 = if ($b.first_partial_after_speech_p95_ms -ne $null) { $b.first_partial_after_speech_p95_ms } else { $b.first_partial_p95_ms }
        $firstPartialAvg = if ($b.first_partial_after_speech_avg_ms -ne $null) { $b.first_partial_after_speech_avg_ms } else { $b.first_partial_avg_ms }
        $firstPartialLabel = if ($b.first_partial_after_speech_p95_ms -ne $null) { "speech_to_first_partial" } else { "audio_to_first_partial" }
        if ($firstPartialP95 -ne $null -and [double]$firstPartialP95 -gt 1200) {
            Add-Finding -Severity "P2" -Area "latency" -Title "Speech-to-first-partial p95 is high" -Evidence "baseline=$($b.variant_id) metric=$firstPartialLabel p95_ms=$firstPartialP95 audio_start_p95_ms=$($b.first_partial_p95_ms)" -Recommendation "Investigate model emission cadence or a content-safe smaller bilingual streaming model."
        }
        if ($firstPartialAvg -ne $null -and [double]$firstPartialAvg -gt 800) {
            Add-Finding -Severity "P2" -Area "latency" -Title "Speech-to-first-partial average is not hand-following" -Evidence "baseline=$($b.variant_id) metric=$firstPartialLabel avg_ms=$firstPartialAvg audio_start_avg_ms=$($b.first_partial_avg_ms)" -Recommendation "Prefer the fastest passing latency variant or reduce gating before HUD display."
        }
        if ($b.processing_rtf_avg -ne $null -and [double]$b.processing_rtf_avg -gt 0.45) {
            Add-Finding -Severity "P2" -Area "latency" -Title "Processing realtime factor is high" -Evidence "baseline=$($b.variant_id) processing_rtf_avg=$($b.processing_rtf_avg)" -Recommendation "Tune thread count or model variant."
        }
    }
    $passing = @($Summary.summary_by_variant | Where-Object { [int]$_.failed_cases -eq 0 } | Sort-Object @{ Expression = { if ($_.first_partial_after_speech_avg_ms -ne $null) { [double]$_.first_partial_after_speech_avg_ms } else { [double]$_.first_partial_avg_ms } } })
    if ($passing.Count -gt 1) {
        $fastest = $passing[0]
        $baseline = @($passing | Where-Object { $_.variant_id -eq "paraformer_bilingual_asr6_chunk60" } | Select-Object -First 1)
        if ($baseline.Count -gt 0) {
            $b = $baseline[0]
            $baselineAvg = if ($b.first_partial_after_speech_avg_ms -ne $null) { $b.first_partial_after_speech_avg_ms } else { $b.first_partial_avg_ms }
            $fastestAvg = if ($fastest.first_partial_after_speech_avg_ms -ne $null) { $fastest.first_partial_after_speech_avg_ms } else { $fastest.first_partial_avg_ms }
            if ($baselineAvg -ne $null -and $fastestAvg -ne $null) {
                $gain = [double]$baselineAvg - [double]$fastestAvg
                if ($gain -gt 120) {
                    Add-Finding -Severity "P2" -Area "latency" -Title "A faster passing latency variant exists" -Evidence "baseline=$($b.variant_id) avg=$baselineAvg; fastest=$($fastest.variant_id) avg=$fastestAvg; gain_ms=$([Math]::Round($gain, 1))" -Recommendation "Consider switching config if follow-up replay/live gates still pass."
                }
            }
        }
    }
}

$auditStarted = Get-Date
$processBefore = Get-ProcessSnapshot
$packageStatus = Test-PackageIntegrity
if ($packageStatus -eq "pass") {
    $script:steps.Add([pscustomobject]@{
        id = "package_integrity"
        label = "package integrity"
        status = "pass"
        exit_code = 0
        started_at = $auditStarted.ToString("o")
        elapsed_ms = 0
        file = ""
        args = @()
        working_dir = $repoRoot
        stdout = ""
        stderr = ""
        exception = ""
    }) | Out-Null
}

$targetProcesses = @($processBefore | Where-Object { $_.path -eq $ExePath })
$wrongProcesses = @($processBefore | Where-Object { $_.path -ne "" -and $_.path -ne $ExePath })
if ($wrongProcesses.Count -gt 0) {
    Add-Finding -Severity "P1" -Area "runtime" -Title "Unexpected ainput process version is running" -Evidence ($wrongProcesses | ConvertTo-Json -Depth 4) -Recommendation "Stop old tray process before final delivery."
}

$psExe = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"

Invoke-AuditProcess -Id "cargo_fmt_check" -Label "cargo fmt --check" -FilePath "cargo" -ArgumentList @("fmt", "--check") -FailSeverity "P1" -FailArea "build" | Out-Null
Invoke-AuditProcess -Id "cargo_check_desktop" -Label "cargo check desktop" -FilePath "cargo" -ArgumentList @("check", "-p", "ainput-desktop") -FailSeverity "P1" -FailArea "build" | Out-Null
Invoke-AuditProcess -Id "cargo_test_hotkey" -Label "cargo test hotkey" -FilePath "cargo" -ArgumentList @("test", "-p", "ainput-desktop", "hotkey", "--", "--nocapture") -FailSeverity "P0" -FailArea "hotkey" | Out-Null
Invoke-AuditProcess -Id "cargo_test_streaming" -Label "cargo test streaming" -FilePath "cargo" -ArgumentList @("test", "-p", "ainput-desktop", "streaming", "--", "--nocapture") -FailSeverity "P1" -FailArea "streaming" | Out-Null
Invoke-AuditProcess -Id "cargo_test_rewrite" -Label "cargo test rewrite" -FilePath "cargo" -ArgumentList @("test", "-p", "ainput-rewrite", "--", "--nocapture") -FailSeverity "P1" -FailArea "rewrite" | Out-Null

$v12ReportPath = Join-Path $ReportDir "v12-replay-report.json"
Remove-Item $v12ReportPath -Force -ErrorAction SilentlyContinue
$v12WrapperPath = Join-Path $ReportDir "run-v12-replay-wrapper.ps1"
$v12WrapperLines = @(
    '$ErrorActionPreference = "Stop"',
    ('$env:AINPUT_JSON_OUTPUT_PATH = ' + (ConvertTo-PowerShellLiteral $v12ReportPath)),
    ('$process = Start-Process -FilePath ' + (ConvertTo-PowerShellLiteral $ExePath) + " -ArgumentList @('replay-streaming-manifest', 'fixtures\streaming-user-regression-v12\manifest.json') -WorkingDirectory " + (ConvertTo-PowerShellLiteral $packageDir) + ' -Wait -PassThru -WindowStyle Hidden'),
    '$env:AINPUT_JSON_OUTPUT_PATH = $null',
    'exit $process.ExitCode'
)
$v12WrapperLines | Set-Content -Path $v12WrapperPath -Encoding UTF8
$v12Step = Invoke-AuditProcess `
    -Id "v12_replay" `
    -Label "v12 user regression replay" `
    -FilePath $psExe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $v12WrapperPath) `
    -WorkingDir $repoRoot `
    -FailSeverity "P1" `
    -FailArea "recognition"
$v12Report = Read-JsonFile $v12ReportPath
if ($null -eq $v12Report -or $v12Report.overall_status -ne "pass") {
    Add-Finding -Severity "P1" -Area "recognition" -Title "v12 regression replay did not pass" -Evidence "report=$v12ReportPath stdout=$($v12Step.stdout)" -Recommendation "Fix tail/artifact/punctuation regression."
}

$startupReportDir = Join-Path $ReportDir "startup-idle"
Invoke-AuditProcess `
    -Id "startup_idle" `
    -Label "startup idle acceptance" `
    -FilePath $psExe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $repoRoot "scripts\run-startup-idle-acceptance.ps1"), "-Version", $Version, "-IdleSeconds", "30", "-Runs", "1", "-InteractiveTask", "-ReportDir", $startupReportDir) `
    -FailSeverity "P0" `
    -FailArea "startup" | Out-Null
$startupReport = Read-JsonFile (Join-Path $startupReportDir "startup-idle-report.json")
if ($null -ne $startupReport -and $startupReport.overall_status -ne "pass") {
    Add-Finding -Severity "P0" -Area "startup" -Title "Startup idle produced forbidden activity" -Evidence ($startupReport.results | ConvertTo-Json -Depth 6) -Recommendation "Fix self-triggering hotkey/audio path."
}

$selftestStep = Invoke-AuditProcess `
    -Id "streaming_selftest" `
    -Label "streaming selftest" `
    -FilePath $psExe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $repoRoot "scripts\run-streaming-selftest.ps1"), "-Version", $Version) `
    -FailSeverity "P1" `
    -FailArea "recognition"

$rawReportDir = Join-Path $ReportDir "raw-corpus"
if (![string]::IsNullOrWhiteSpace($RawDir) -and (Test-Path $RawDir)) {
    Invoke-AuditProcess `
        -Id "raw_corpus" `
        -Label "raw corpus replay" `
        -FilePath $psExe `
        -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $repoRoot "scripts\run-streaming-raw-corpus.ps1"), "-ExePath", $ExePath, "-RawDir", $RawDir, "-ReportDir", $rawReportDir, "-ShortCount", "2", "-LongCount", "2") `
        -FailSeverity "P1" `
        -FailArea "raw" | Out-Null
    $rawReport = Read-JsonFile (Join-Path $rawReportDir "summary.json")
    if ($null -ne $rawReport -and $rawReport.overall_status -ne "pass") {
        Add-Finding -Severity "P1" -Area "raw" -Title "Raw corpus replay failed" -Evidence ($rawReport.failures | ConvertTo-Json -Depth 6) -Recommendation "Fix raw replay regressions."
    }
} else {
    Add-Finding -Severity "P2" -Area "raw" -Title "No raw corpus directory available" -Evidence "RawDir='$RawDir'" -Recommendation "Collect new raw samples before next audit."
}

$syntheticReportDir = Join-Path $ReportDir "live-synthetic"
Invoke-AuditProcess `
    -Id "live_synthetic" `
    -Label "synthetic live E2E" `
    -FilePath $psExe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $repoRoot "scripts\run-streaming-live-e2e.ps1"), "-Version", $Version, "-Synthetic", "-InteractiveTask", "-ReportDir", $syntheticReportDir) `
    -FailSeverity "P1" `
    -FailArea "live" | Out-Null
$syntheticReport = Read-JsonFile (Join-Path $syntheticReportDir "report.json")
Add-LiveReportFindings -Report $syntheticReport -Area "live_synthetic"

$wavReportDir = Join-Path $ReportDir "live-wav"
Invoke-AuditProcess `
    -Id "live_wav" `
    -Label "wav live E2E" `
    -FilePath $psExe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $repoRoot "scripts\run-streaming-live-e2e.ps1"), "-Version", $Version, "-Wav", "-InteractiveTask", "-CaseLimit", ([string]$LiveCaseLimit), "-ReportDir", $wavReportDir) `
    -FailSeverity "P1" `
    -FailArea "live" | Out-Null
$wavReport = Read-JsonFile (Join-Path $wavReportDir "report.json")
Add-LiveReportFindings -Report $wavReport -Area "live_wav"

$latencyReportDir = Join-Path $ReportDir "latency"
if (!$SkipLatency) {
    $latencyWrapperPath = Join-Path $ReportDir "run-latency-wrapper.ps1"
    $latencyScriptPath = Join-Path $repoRoot "scripts\run-streaming-latency-benchmark.ps1"
    $latencyLines = @(
        '$ErrorActionPreference = "Stop"',
        '$benchmarkArgs = @{',
        "  ExePath = $(ConvertTo-PowerShellLiteral $ExePath)",
        "  ReportDir = $(ConvertTo-PowerShellLiteral $latencyReportDir)",
        "  Repeats = $LatencyRepeats",
        '  CaseIds = @("sentence_01", "sentence_05", "sentence_combo_long")',
        '  ModelIds = @("paraformer_bilingual")',
        '  AsrThreads = @(4, 6, 8)',
        '  ChunkMs = @(40, 60, 80)',
        '  BaselineAsrThreads = 6',
        '  BaselineChunkMs = 60',
        '  FinalThreads = 8',
        '  PunctuationThreads = 1',
        '  RawCount = 2',
        '  RawMinBytes = 200000',
        '  FailOnContentRegression = $true'
    )
    if (![string]::IsNullOrWhiteSpace($RawDir) -and (Test-Path $RawDir)) {
        $latencyLines += "  RawDir = $(ConvertTo-PowerShellLiteral $RawDir)"
    }
    $latencyLines += '}'
    $latencyLines += "try { & $(ConvertTo-PowerShellLiteral $latencyScriptPath) @benchmarkArgs; exit 0 } catch { Write-Error `$_.Exception.Message; exit 1 }"
    $latencyLines | Set-Content -Path $latencyWrapperPath -Encoding UTF8
    Invoke-AuditProcess `
        -Id "latency_benchmark" `
        -Label "latency benchmark" `
        -FilePath $psExe `
        -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $latencyWrapperPath) `
        -FailSeverity "P2" `
        -FailArea "latency" | Out-Null
    $latencySummary = Read-JsonFile (Join-Path $latencyReportDir "summary.json")
    Add-LatencyFindings -Summary $latencySummary
}

$processAfter = Get-ProcessSnapshot
$p0 = @($script:findings | Where-Object { $_.severity -eq "P0" }).Count
$p1 = @($script:findings | Where-Object { $_.severity -eq "P1" }).Count
$p2 = @($script:findings | Where-Object { $_.severity -eq "P2" }).Count
$overall = if ($p0 -gt 0 -or $p1 -gt 0) {
    "fail"
} elseif ($p2 -gt 0) {
    "pass_with_p2"
} else {
    "pass"
}

$report = [ordered]@{
    generated_at = (Get-Date).ToString("o")
    version = $Version
    exe_path = $ExePath
    package_dir = $packageDir
    zip_path = $zipPath
    raw_dir = $RawDir
    report_dir = $ReportDir
    overall_status = $overall
    counts = [ordered]@{
        p0 = $p0
        p1 = $p1
        p2 = $p2
        findings = $script:findings.Count
        steps = $script:steps.Count
    }
    processes_before = $processBefore
    processes_after = $processAfter
    steps = $script:steps
    findings = $script:findings
    parsed_reports = [ordered]@{
        v12 = if ($null -ne $v12Report) { $v12Report.overall_status } else { $null }
        startup = if ($null -ne $startupReport) { $startupReport.overall_status } else { $null }
        raw = if ($null -ne $rawReport) { $rawReport.overall_status } else { $null }
        synthetic = if ($null -ne $syntheticReport) { $syntheticReport.overall_status } else { $null }
        wav = if ($null -ne $wavReport) { $wavReport.overall_status } else { $null }
        latency = if ($null -ne $latencySummary) { $latencySummary.overall_status } else { $null }
    }
}

$reportPath = Join-Path $ReportDir "full-audit-report.json"
$summaryPath = Join-Path $ReportDir "SUMMARY.md"
$report | ConvertTo-Json -Depth 12 | Set-Content -Path $reportPath -Encoding UTF8

$lines = @()
$lines += "# Streaming Full Audit"
$lines += ""
$lines += "- version: ``$Version``"
$lines += "- exe: ``$ExePath``"
$lines += "- raw_dir: ``$RawDir``"
$lines += "- overall_status: ``$overall``"
$lines += "- counts: P0=$p0 P1=$p1 P2=$p2 findings=$($script:findings.Count)"
$lines += ""
$lines += "## Findings"
if ($script:findings.Count -eq 0) {
    $lines += ""
    $lines += "No findings."
} else {
    $lines += ""
    $lines += "| severity | area | title | evidence | recommendation |"
    $lines += "| --- | --- | --- | --- | --- |"
    foreach ($finding in $script:findings) {
        $evidence = ([string]$finding.evidence).Replace("`r", " ").Replace("`n", "<br>")
        $recommendation = ([string]$finding.recommendation).Replace("`r", " ").Replace("`n", "<br>")
        $lines += "| $($finding.severity) | $($finding.area) | $($finding.title) | $evidence | $recommendation |"
    }
}
$lines += ""
$lines += "## Steps"
$lines += ""
$lines += "| step | status | elapsed_ms | stdout | stderr |"
$lines += "| --- | --- | ---: | --- | --- |"
foreach ($step in $script:steps) {
    $lines += "| $($step.id) | $($step.status) | $($step.elapsed_ms) | ``$($step.stdout)`` | ``$($step.stderr)`` |"
}
$lines | Set-Content -Path $summaryPath -Encoding UTF8

Write-Host ""
Write-Host "streaming full audit"
Write-Host ("overall_status={0}" -f $overall)
Write-Host ("findings=P0:{0} P1:{1} P2:{2}" -f $p0, $p1, $p2)
Write-Host ("report={0}" -f $reportPath)
Write-Host ("summary={0}" -f $summaryPath)
if ($script:findings.Count -gt 0) {
    $script:findings | Format-Table severity, area, title -AutoSize
}

if ($p0 -gt 0 -or $p1 -gt 0) {
    exit 1
}
