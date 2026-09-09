#Requires -Version 5.1
# Start the Whisper-turbo streaming sidecar for ainput's whisper-turbo engine.
# GPU (faster-whisper) must work first; model downloads once (~1.6GB).
# First run warms up (~1 min model load), then partials stream per second.
# Keep this window open while using the Whisper-Turbo tray engine.
# Health: http://127.0.0.1:8766/healthz
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
& C:\Python314\python.exe -u sidecar/whisper_turbo_server.py
