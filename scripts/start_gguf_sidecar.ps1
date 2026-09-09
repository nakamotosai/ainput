#Requires -Version 5.1
# Start the FunASR-GGUF sidecar for ainput's funasr-gguf engine.
# First run warms up (~2 min: model load + Vulkan shader compile), then
# every request is ~RTF 0.07. Keep this window open while using the
# FunASR-GGUF tray engine. Health: http://127.0.0.1:8765/healthz
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
if (-not (Test-Path "models/funasr-gguf/Fun-ASR-Nano-Decoder.q5_k.gguf")) {
    Write-Output "model not exported yet; run: python scripts/run_gguf_export.py"
    exit 1
}
& C:\Python314\python.exe -u sidecar/funasr_gguf_server.py
