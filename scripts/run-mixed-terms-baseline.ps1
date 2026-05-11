param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [string]$BaselineDir = ".\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101",
    [string]$OnlyTerm = "",
    [string]$OnlyCase = "",
    [int]$MinPass = 8,
    [int]$Repeat = 1,
    [string]$OutDir = ".\tmp\mixed-terms-baseline"
)

$ErrorActionPreference = "Stop"
$repoRoot = if ((Split-Path -Leaf $PSScriptRoot) -eq "scripts") {
    Split-Path -Parent $PSScriptRoot
} else {
    Get-Location
}

function Resolve-RepoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return (Resolve-Path $Path).Path
    }
    return (Resolve-Path (Join-Path $repoRoot $Path)).Path
}

function ConvertTo-CaseId {
    param(
        [Parameter(Mandatory = $true)]$Case
    )
    $termSlug = ([string]$Case.term).ToLowerInvariant() -replace '[^a-z0-9]+', '-'
    $termSlug = $termSlug.Trim('-')
    return ('{0:D2}-{1}' -f [int]$Case.index, $termSlug)
}

if ($Repeat -lt 1) {
    throw "Repeat must be >= 1"
}

$exeFull = Resolve-RepoPath $ExePath
$baselineFull = Resolve-RepoPath $BaselineDir
$manifestSourcePath = Join-Path $baselineFull "manifest.json"
if (-not (Test-Path $manifestSourcePath)) {
    throw "baseline manifest not found: $manifestSourcePath"
}

$sourceManifest = Get-Content -Raw -Encoding UTF8 $manifestSourcePath | ConvertFrom-Json
$sourceCases = @($sourceManifest.cases)
if ($OnlyTerm.Trim().Length -gt 0) {
    $sourceCases = @($sourceCases | Where-Object { $_.term -ieq $OnlyTerm })
}
if ($OnlyCase.Trim().Length -gt 0) {
    $sourceCases = @($sourceCases | Where-Object { (ConvertTo-CaseId $_) -ieq $OnlyCase -or $_.wav_file -ieq $OnlyCase })
}
if ($sourceCases.Count -eq 0) {
    throw "no baseline cases matched OnlyTerm='$OnlyTerm' OnlyCase='$OnlyCase'"
}
if (($OnlyTerm.Trim().Length -gt 0 -or $OnlyCase.Trim().Length -gt 0) -and $sourceCases.Count -ne 1) {
    throw "single-case mode expected exactly 1 case, got $($sourceCases.Count)"
}

$outRoot = if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $repoRoot $OutDir }
New-Item -ItemType Directory -Force $outRoot | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $outRoot $stamp
New-Item -ItemType Directory -Force $runDir | Out-Null

$repeatReports = @()
$bestPassedCases = 0
$totalBehaviorFailures = 0
$totalContentFailures = 0
$allRows = @()

for ($iteration = 1; $iteration -le $Repeat; $iteration++) {
    $runnerCases = @()
    foreach ($case in $sourceCases) {
        $runnerCases += [ordered]@{
            id = ConvertTo-CaseId $case
            wav_path = $case.wav_file
            expected_text = $case.expected_text
            min_partial_updates = 1
        }
    }
    $runnerManifest = [ordered]@{
        version = 1
        fixture_root = $baselineFull
        cases = $runnerCases
    }
    $runnerManifestPath = Join-Path $runDir ("iteration-{0:D2}.manifest.json" -f $iteration)
    $reportPath = Join-Path $runDir ("iteration-{0:D2}.report.json" -f $iteration)
    $runnerManifest | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 $runnerManifestPath

    $previousJsonOutput = $env:AINPUT_JSON_OUTPUT_PATH
    $env:AINPUT_JSON_OUTPUT_PATH = $reportPath
    try {
        & $exeFull replay-streaming-manifest $runnerManifestPath | Out-Host
        $exitCode = $LASTEXITCODE
    } finally {
        $env:AINPUT_JSON_OUTPUT_PATH = $previousJsonOutput
    }
    if ($exitCode -ne 0) {
        throw "replay-streaming-manifest exited with $exitCode on iteration $iteration"
    }

    $report = Get-Content -Raw -Encoding UTF8 $reportPath | ConvertFrom-Json
    $bestPassedCases = [Math]::Max($bestPassedCases, [int]$report.passed_cases)
    $totalBehaviorFailures += [int]$report.behavior_failures
    $totalContentFailures += [int]$report.content_failures
    foreach ($caseReport in @($report.cases)) {
        $allRows += [ordered]@{
            iteration = $iteration
            id = $caseReport.case_id
            content_status = $caseReport.content_status
            behavior_status = $caseReport.behavior_status
            raw = $caseReport.final_online_raw_text
            final = $caseReport.final_text
            expected = $caseReport.expected_text
            failures = $caseReport.failures
        }
    }
    $repeatReports += [ordered]@{
        iteration = $iteration
        report_path = $reportPath
        passed_cases = [int]$report.passed_cases
        total_cases = [int]$report.total_cases
        behavior_failures = [int]$report.behavior_failures
        content_failures = [int]$report.content_failures
        overall_status = $report.overall_status
    }
}

$allIterationsPass = @($repeatReports | Where-Object { $_.passed_cases -lt $MinPass -or $_.behavior_failures -ne 0 }).Count -eq 0
$summary = [ordered]@{
    status = if ($allIterationsPass) { "pass" } else { "fail" }
    exe_path = $exeFull
    baseline_dir = $baselineFull
    run_dir = $runDir
    only_term = $OnlyTerm
    only_case = $OnlyCase
    min_pass = $MinPass
    repeat = $Repeat
    best_passed_cases = $bestPassedCases
    behavior_failures_total = $totalBehaviorFailures
    content_failures_total = $totalContentFailures
    iterations = $repeatReports
    rows = $allRows
}
$summaryPath = Join-Path $runDir "summary.json"
$summary | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 $summaryPath
$summary | ConvertTo-Json -Depth 16

if (-not $allIterationsPass) {
    exit 1
}
exit 0
