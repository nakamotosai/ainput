param(
    [string]$ExePath = "",
    [string]$Version = "",
    [int]$IdleSeconds = 30,
    [int]$Runs = 1,
    [string]$ReportDir = "",
    [string]$ExpectedVoiceHotkey = "",
    [switch]$InteractiveTask
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    if (![string]::IsNullOrWhiteSpace($Version)) {
        $ExePath = Join-Path $repoRoot ("dist\ainput-" + $Version + "\ainput-desktop.exe")
    } else {
        $packagedExe = Join-Path $repoRoot "ainput-desktop.exe"
        if (Test-Path $packagedExe) {
            $ExePath = $packagedExe
        } else {
            $ExePath = Join-Path $repoRoot "target\debug\ainput-desktop.exe"
            cargo build -p ainput-desktop
        }
    }
}

if (!(Test-Path $ExePath)) {
    throw "missing executable: $ExePath"
}

if ($IdleSeconds -lt 5) {
    throw "IdleSeconds must be at least 5"
}
if ($Runs -lt 1) {
    throw "Runs must be at least 1"
}

if ([string]::IsNullOrWhiteSpace($ReportDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
    $ReportDir = Join-Path $repoRoot ("tmp\startup-idle-acceptance\" + $stamp)
}
New-Item -ItemType Directory -Force -Path $ReportDir | Out-Null

$exeItem = Get-Item $ExePath
$appRoot = Split-Path -Parent $exeItem.FullName
if ($appRoot -match "\\target\\debug$" -or $appRoot -match "\\target\\release$") {
    $appRoot = $repoRoot
}
$logPath = Join-Path $appRoot "logs\ainput.log"
$rawCaptureDir = Join-Path $appRoot "logs\streaming-raw-captures"
$summaryPath = Join-Path $ReportDir "startup-idle-report.json"

if ([string]::IsNullOrWhiteSpace($ExpectedVoiceHotkey)) {
    $configPath = Join-Path $appRoot "config\ainput.toml"
    if (!(Test-Path $configPath)) {
        $configPath = Join-Path $repoRoot "config\ainput.toml"
    }
    if (Test-Path $configPath) {
        $configText = [System.IO.File]::ReadAllText($configPath, [System.Text.Encoding]::UTF8)
        $match = Select-String -Path $configPath -Pattern '^\s*voice_input\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($null -ne $match) {
            $ExpectedVoiceHotkey = $match.Matches[0].Groups[1].Value
        }
        if ($configText -match '(?m)^\s*mode\s*=\s*"streaming"\s*$') {
            $ExpectedVoiceHotkey = "Ctrl"
        }
    }
}

$forbiddenPatterns = @(
    "start microphone recording",
    "streaming microphone armed on hotkey press",
    "streaming push-to-talk recording started",
    "streaming transcription delivered",
    "output delivery timing",
    "voice hotkey matched in keyboard hook",
    "modifier-only voice hotkey matched in keyboard hook",
    "mouse middle hold voice hotkey matched"
)

function Stop-AinputProcesses {
    Get-Process ainput-desktop -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    $remaining = @(Get-Process ainput-desktop -ErrorAction SilentlyContinue)
    if ($remaining.Count -gt 0) {
        $descriptions = $remaining | ForEach-Object {
            $path = try { $_.Path } catch { "<path unavailable>" }
            "{0}:{1}" -f $_.Id, $path
        }
        throw ("old_tray_process_still_running: " + ($descriptions -join "; "))
    }
}

function Read-LogDelta([string]$Path, [long]$Offset) {
    if (!(Test-Path $Path)) {
        return ""
    }
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        if ($Offset -gt $stream.Length) {
            $Offset = 0
        }
        [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
        $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8, $true, 4096, $true)
        try {
            return $reader.ReadToEnd()
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Start-AinputIdleRun([string]$Exe, [string]$WorkingDir, [switch]$UseInteractiveTask, [string]$RunDir, [int]$Seconds) {
    if ($UseInteractiveTask) {
        $taskName = "ainput-startup-idle-" + ([Guid]::NewGuid().ToString("N").Substring(0, 12))
        $taskScript = Join-Path $RunDir "start-interactive.ps1"
        $pidPath = Join-Path $RunDir "pid.txt"
        $escapedExe = $Exe.Replace("'", "''")
        $escapedWorkingDir = $WorkingDir.Replace("'", "''")
        $escapedPidPath = $pidPath.Replace("'", "''")
        Set-Content -Path $taskScript -Encoding UTF8 -Value @"
`$ErrorActionPreference = "Stop"
`$process = Start-Process -FilePath '$escapedExe' -WorkingDirectory '$escapedWorkingDir' -PassThru -WindowStyle Normal
Set-Content -Path '$escapedPidPath' -Encoding ASCII -Value `$process.Id
"@
        $startTime = (Get-Date).AddMinutes(1).ToString("HH:mm")
        $taskCommand = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$taskScript`""
        & schtasks.exe /Create /TN $taskName /SC ONCE /ST $startTime /TR $taskCommand /IT /F | Out-Null
        & schtasks.exe /Run /TN $taskName | Out-Null

        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $deadline -and !(Test-Path $pidPath)) {
            Start-Sleep -Milliseconds 250
        }
        & schtasks.exe /Delete /TN $taskName /F | Out-Null
        if (!(Test-Path $pidPath)) {
            throw "interactive startup task did not produce pid"
        }
        $pidText = (Get-Content $pidPath -Raw).Trim()
        $process = Get-Process -Id ([int]$pidText) -ErrorAction Stop
        Start-Sleep -Seconds $Seconds
        return $process
    }

    $process = Start-Process -FilePath $Exe -WorkingDirectory $WorkingDir -PassThru -WindowStyle Normal
    Start-Sleep -Seconds $Seconds
    return $process
}

$results = @()
for ($i = 1; $i -le $Runs; $i++) {
    Stop-AinputProcesses
    $runDir = Join-Path $ReportDir ("run-" + $i)
    New-Item -ItemType Directory -Force -Path $runDir | Out-Null
    $startedAt = Get-Date
    $logOffset = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }

    $process = $null
    try {
        $process = Start-AinputIdleRun -Exe $exeItem.FullName -WorkingDir $appRoot -UseInteractiveTask:$InteractiveTask -RunDir $runDir -Seconds $IdleSeconds
    } finally {
        Get-Process ainput-desktop -ErrorAction SilentlyContinue |
            Where-Object {
                try { $_.Path -eq $exeItem.FullName } catch { $false }
            } |
            Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
    }

    $delta = Read-LogDelta -Path $logPath -Offset $logOffset
    $deltaPath = Join-Path $runDir "ainput-log-delta.txt"
    Set-Content -Path $deltaPath -Encoding UTF8 -Value $delta

    $hits = @()
    foreach ($pattern in $forbiddenPatterns) {
        if ($delta -match [regex]::Escape($pattern)) {
            $hits += $pattern
        }
    }
    if (![string]::IsNullOrWhiteSpace($ExpectedVoiceHotkey)) {
        $expectedNeedle = "voice_hotkey=$ExpectedVoiceHotkey"
        if ($delta -notmatch [regex]::Escape($expectedNeedle)) {
            $hits += "voice_hotkey_binding_mismatch"
        }
    }

    $newRawCaptures = @()
    if (Test-Path $rawCaptureDir) {
        $newRawCaptures = @(Get-ChildItem $rawCaptureDir -File -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -ge $startedAt.AddSeconds(-1) } |
            Select-Object -ExpandProperty FullName)
    }

    $status = if ($hits.Count -eq 0 -and $newRawCaptures.Count -eq 0) { "pass" } else { "fail" }
    $results += [pscustomobject]@{
        run = $i
        status = $status
        started_at = $startedAt.ToString("o")
        idle_seconds = $IdleSeconds
        log_path = $logPath
        log_delta_path = $deltaPath
        forbidden_hits = $hits
        new_raw_captures = $newRawCaptures
        expected_voice_hotkey = $ExpectedVoiceHotkey
    }
}

$overall = if (($results | Where-Object { $_.status -ne "pass" }).Count -eq 0) { "pass" } else { "fail" }
$summary = [pscustomobject]@{
    overall_status = $overall
    exe_path = $exeItem.FullName
    app_root = $appRoot
    expected_voice_hotkey = $ExpectedVoiceHotkey
    runs = $Runs
    idle_seconds = $IdleSeconds
    report_dir = $ReportDir
    results = $results
}
$summary | ConvertTo-Json -Depth 6 | Set-Content -Path $summaryPath -Encoding UTF8

Write-Host ""
Write-Host "startup idle acceptance"
Write-Host ("overall_status={0}" -f $overall)
Write-Host ("runs={0}" -f $Runs)
Write-Host ("idle_seconds={0}" -f $IdleSeconds)
Write-Host ("report_dir={0}" -f $ReportDir)
$results | Format-Table -AutoSize

if ($overall -ne "pass") {
    exit 1
}
