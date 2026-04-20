param(
    [string]$ExePath = "",
    [string]$Version = "",
    [string]$ManifestPath = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$useCargoRun = $false
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    if (![string]::IsNullOrWhiteSpace($Version)) {
        $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
    } else {
        $useCargoRun = $true
    }
}

if (!$useCargoRun -and !(Test-Path $ExePath)) {
    throw "missing executable: $ExePath"
}

if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
    & (Join-Path $repoRoot "scripts\generate-streaming-fixtures.ps1")
    $ManifestPath = Join-Path $repoRoot "fixtures\streaming-selftest\manifest.json"
}

if (!(Test-Path $ManifestPath)) {
    throw "missing manifest: $ManifestPath"
}

$reportPath = Join-Path $repoRoot "tmp\streaming-selftest-latest.json"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $reportPath) | Out-Null
if (Test-Path $reportPath) {
    Remove-Item $reportPath -Force
}

$env:AINPUT_JSON_OUTPUT_PATH = $reportPath
if ($useCargoRun) {
    cargo run -p ainput-desktop -- replay-streaming-manifest $ManifestPath
    $exitCode = $LASTEXITCODE
} else {
    $process = Start-Process -FilePath $ExePath `
        -ArgumentList @("replay-streaming-manifest", $ManifestPath) `
        -PassThru `
        -Wait `
        -WindowStyle Hidden
    $exitCode = $process.ExitCode
}
$env:AINPUT_JSON_OUTPUT_PATH = $null
if ($exitCode -ne 0) {
    throw "streaming selftest command failed, see $reportPath"
}
if (!(Test-Path $reportPath)) {
    throw "streaming selftest did not produce report: $reportPath"
}

$reportJson = [System.IO.File]::ReadAllText($reportPath, [System.Text.Encoding]::UTF8)
$report = $reportJson | ConvertFrom-Json
$rows = @()
foreach ($case in $report.cases) {
    $rows += [pscustomobject]@{
        case_id = $case.case_id
        behavior = $case.behavior_status
        content = $case.content_status
        partials = $case.partial_updates
        final_chars = $case.final_visible_chars
        commit_source = $case.commit_source
        final_text = $case.final_text
    }
}

Write-Host ""
Write-Host "streaming selftest"
$rows | Format-Table -AutoSize
Write-Host ""
Write-Host ("overall_status={0}" -f $report.overall_status)
Write-Host ("passed_cases={0}/{1}" -f $report.passed_cases, $report.total_cases)
Write-Host ("report_path={0}" -f $reportPath)

if ($report.overall_status -ne "pass") {
    exit 1
}
