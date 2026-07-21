# ainput 0.1.0

Local-first Windows voice dictation. Hold **CapsLock**, speak, text pastes into the focused app. Optional AI rewrite via **your own** OpenAI-compatible API.

## Download

- **Windows x64 green zip:** `ainput-0.1.0-win64.zip` (unpack and run — no installer)
- Official site: https://input.saaaai.com/

## What's in the zip

- `ainput.exe` + `run-ainput.bat`
- Bundled **SenseVoice** model (`models/sense-voice/`)
- Config examples under `config/`
- License + third-party notices
- **No** `state/` (keys/history/logs stay on your machine after first run)

## Quick start

1. Unpack anywhere
2. Run `ainput.exe`
3. Hold **CapsLock**, speak, release
4. Optional: tray → **API / 改写设置…** (local browser) to enable rewrite
5. Tray → **听写历史…** for local history

## Notes

- Microphone audio is processed on device for ASR
- Rewrite (if enabled) sends text only to the endpoint you configure
- Do not share your `state/` folder
- Formerly known as 中日说 / CNJP Input (old installers are deprecated)

## Checksums

See release asset description / compute locally:

```powershell
Get-FileHash .\ainput-0.1.0-win64.zip -Algorithm SHA256
```
