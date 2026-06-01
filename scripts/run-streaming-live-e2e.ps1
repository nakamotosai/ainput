param(
    [string]$ExePath = "",
    [string]$Version = "",
    [string]$ManifestPath = "",
    [switch]$Synthetic,
    [switch]$Wav,
    [string]$ReportDir = "",
    [switch]$InteractiveTask,
    [int]$CaseLimit = 0,
    [int]$TimeoutSeconds = 90
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$buildDebugExe = $false
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    if (![string]::IsNullOrWhiteSpace($Version)) {
        $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
    } else {
        $packagedExe = Join-Path $repoRoot "ainput-desktop.exe"
        if (Test-Path $packagedExe) {
            $ExePath = $packagedExe
        } else {
            $ExePath = Join-Path $repoRoot "target\debug\ainput-desktop.exe"
            $buildDebugExe = $true
        }
    }
}

if ($buildDebugExe) {
    cargo build -p ainput-desktop
}

if (!(Test-Path $ExePath)) {
    throw "missing executable: $ExePath"
}

Get-Process ainput-desktop -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
$remainingAinputProcesses = @(Get-Process ainput-desktop -ErrorAction SilentlyContinue)
if ($remainingAinputProcesses.Count -gt 0) {
    $remainingDescriptions = $remainingAinputProcesses | ForEach-Object {
        $processPath = try { $_.Path } catch { "<path unavailable>" }
        "{0}:{1}" -f $_.Id, $processPath
    }
    throw ("old_tray_process_still_running: " + ($remainingDescriptions -join "; "))
}

$mode = if ($Wav) { "wav" } else { "synthetic" }
$commandName = if ($Wav) { "run-streaming-live-e2e-wav" } else { "run-streaming-live-e2e-synthetic" }

if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
    if ($Wav) {
        $ManifestPath = Join-Path $repoRoot "fixtures\streaming-selftest\manifest.json"
    } else {
        $ManifestPath = Join-Path $repoRoot "fixtures\streaming-hud-e2e\manifest.json"
    }
}
if (!(Test-Path $ManifestPath)) {
    throw "missing manifest: $ManifestPath"
}

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\streaming-live-e2e\" + $stamp)
}
New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null

$jsonOutputPath = Join-Path $ReportDir "command-report.json"
if (Test-Path $jsonOutputPath) {
    Remove-Item $jsonOutputPath -Force
}
$reportPath = Join-Path $ReportDir "report.json"

if ($InteractiveTask) {
    $taskName = "ainput-live-e2e-" + ([Guid]::NewGuid().ToString("N").Substring(0, 12))
    $taskScript = Join-Path $ReportDir "run-interactive-task.ps1"
    $taskExitPath = Join-Path $ReportDir "task-exit-code.txt"
    $taskOutputPath = Join-Path $ReportDir "task-output.txt"
    $escapedRepoRoot = $repoRoot.Replace("'", "''")
    $escapedExe = $ExePath.Replace("'", "''")
    $escapedManifest = $ManifestPath.Replace("'", "''")
    $escapedReportDir = $ReportDir.Replace("'", "''")
    $escapedJsonOutput = $jsonOutputPath.Replace("'", "''")
    $escapedTaskOutput = $taskOutputPath.Replace("'", "''")
    $escapedTaskExit = $taskExitPath.Replace("'", "''")
    $escapedCaseLimit = [string]$CaseLimit
    $escapedArgumentList = @(
        $commandName.Replace("'", "''"),
        $ManifestPath.Replace("'", "''"),
        $ReportDir.Replace("'", "''")
    )
    if ($CaseLimit -gt 0) {
        $escapedArgumentList += $escapedCaseLimit
    }
    $argumentListLiteral = "@(" + (($escapedArgumentList | ForEach-Object { "'$_'" }) -join ", ") + ")"
    Set-Content -Path $taskScript -Encoding UTF8 -Value @"
`$ErrorActionPreference = "Stop"
try {
    Set-Location '$escapedRepoRoot'
    `$env:AINPUT_ROOT = '$escapedRepoRoot'
    `$env:AINPUT_JSON_OUTPUT_PATH = '$escapedJsonOutput'
    `$arguments = $argumentListLiteral
    `$process = Start-Process -FilePath '$escapedExe' -ArgumentList `$arguments -WorkingDirectory '$escapedRepoRoot' -PassThru -Wait -WindowStyle Normal -RedirectStandardOutput '$escapedTaskOutput' -RedirectStandardError '$escapedTaskOutput.stderr'
    if (`$null -eq `$process.ExitCode) {
        `$code = if (Test-Path '$escapedJsonOutput') { 0 } else { 1 }
    } else {
        `$code = [int]`$process.ExitCode
    }
    if (Test-Path '$escapedTaskOutput.stderr') {
        Get-Content '$escapedTaskOutput.stderr' | Out-File -FilePath '$escapedTaskOutput' -Append -Encoding UTF8
        Remove-Item '$escapedTaskOutput.stderr' -Force -ErrorAction SilentlyContinue
    }
} catch {
    `$_.Exception.ToString() | Out-File -FilePath '$escapedTaskOutput' -Append -Encoding UTF8
    `$code = 1
}
Set-Content -Path '$escapedTaskExit' -Value `$code -Encoding ASCII
exit `$code
"@

    $startTime = (Get-Date).AddMinutes(1).ToString("HH:mm")
    $taskCommand = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$taskScript`""
    & schtasks.exe /Create /TN $taskName /SC ONCE /ST $startTime /TR $taskCommand /IT /F | Out-Null
    & schtasks.exe /Run /TN $taskName | Out-Null

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $taskStartedAt = Get-Date
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $taskExitPath) {
            $pendingExitContent = Get-Content $taskExitPath -Raw
            if (($null -ne $pendingExitContent) -and ![string]::IsNullOrWhiteSpace($pendingExitContent.Trim())) {
                break
            }
        }
        Start-Sleep -Milliseconds 500
    }

    & schtasks.exe /Delete /TN $taskName /F | Out-Null
    if (Test-Path $taskExitPath) {
        $exitContent = Get-Content $taskExitPath -Raw
        $rawExitCode = if ($null -eq $exitContent) { "" } else { $exitContent.Trim() }
        if ([string]::IsNullOrWhiteSpace($rawExitCode)) {
            Get-Process ainput-desktop -ErrorAction SilentlyContinue |
                Where-Object {
                    try {
                        $_.Path -eq $ExePath -and $_.StartTime -ge $taskStartedAt.AddSeconds(-5)
                    } catch {
                        $false
                    }
                } |
                Stop-Process -Force -ErrorAction SilentlyContinue
            throw "interactive live e2e task did not write a complete exit code after $TimeoutSeconds seconds; report dir: $ReportDir"
        } else {
            $exitCode = [int]$rawExitCode
        }
    } else {
        Get-Process ainput-desktop -ErrorAction SilentlyContinue |
            Where-Object {
                try {
                    $_.Path -eq $ExePath -and $_.StartTime -ge $taskStartedAt.AddSeconds(-5)
                } catch {
                    $false
                }
            } |
            Stop-Process -Force -ErrorAction SilentlyContinue
        throw "interactive live e2e task timed out after $TimeoutSeconds seconds; report dir: $ReportDir"
    }
} else {
    $env:AINPUT_JSON_OUTPUT_PATH = $jsonOutputPath
    $previousAinputRoot = $env:AINPUT_ROOT
    $env:AINPUT_ROOT = $repoRoot
    $arguments = @($commandName, $ManifestPath, $ReportDir)
    if ($CaseLimit -gt 0) {
        $arguments += [string]$CaseLimit
    }
    try {
        $process = Start-Process -FilePath $ExePath `
            -ArgumentList $arguments `
            -WorkingDirectory $repoRoot `
            -PassThru `
            -Wait `
            -WindowStyle Normal
        $exitCode = $process.ExitCode
    } finally {
        $env:AINPUT_JSON_OUTPUT_PATH = $null
        $env:AINPUT_ROOT = $previousAinputRoot
    }
}
if ($exitCode -ne 0) {
    throw "live e2e command failed with exit code $exitCode; report dir: $ReportDir"
}
if (!(Test-Path $reportPath)) {
    throw "live e2e did not produce report: $reportPath"
}

$reportJson = [System.IO.File]::ReadAllText($reportPath, [System.Text.Encoding]::UTF8)
$report = $reportJson | ConvertFrom-Json

Write-Host ""
Write-Host ("streaming live e2e ({0})" -f $mode)
if ($CaseLimit -gt 0) {
    Write-Host ("case_limit={0}" -f $CaseLimit)
}
Write-Host ("overall_status={0}" -f $report.overall_status)
Write-Host ("passed_cases={0}/{1}" -f $report.cases_passed, $report.cases_total)
Write-Host ("report_dir={0}" -f $report.report_dir)
Write-Host ""

$rows = @()
foreach ($case in $report.cases) {
    $rows += [pscustomobject]@{
        case_id = $case.case_id
        status = $case.status
        partials = $case.partial_count
        hud_center = ("{0}/{1}" -f $case.hud_stability.max_center_x_delta_px, $case.hud_stability.max_top_delta_px)
        hud_size = ("{0}/{1}" -f $case.hud_stability.max_width_delta_px, $case.hud_stability.max_height_delta_px)
        hud_flash = ("{0}/{1}" -f $case.hud_stability.alpha_drop_count, $case.hud_stability.invisible_sample_count)
        hud_panel = ("{0}/{1}/{2}" -f $case.hud_stability.white_panel_sample_count, $case.hud_stability.multiline_panel_sample_count, $case.hud_stability.short_text_wide_panel_count)
        final_text = $case.final_text
        readback = $case.target_readback
        hud = $case.hud_final_display
    }
}
$rows | Format-Table -AutoSize

if ($report.failures.Count -gt 0) {
    Write-Host ""
    Write-Host "failures"
    foreach ($failure in $report.failures) {
        Write-Host ("[{0}] {1}: {2}" -f $failure.case_id, $failure.category, $failure.message)
    }
}

if ($report.overall_status -ne "pass") {
    exit 1
}
